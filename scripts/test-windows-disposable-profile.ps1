#Requires -Version 5.1
<#
.SYNOPSIS
Proves the disposable native Windows profile seam that the v0.14.2 history
recovery matrix needs, and fails loudly when that seam is unavailable.

.DESCRIPTION
aw-server resolves its Local/Roaming/Python roots through Windows Known Folder
APIs (SHGetKnownFolderPath, via the `dirs` crate). Those ignore LOCALAPPDATA,
APPDATA and USERPROFILE in a child environment, so environment-variable
redirection cannot produce disposable roots: the server would still write into
the runner user's real profile.

This script creates a disposable local Windows user, starts the real
aw-server.exe as that user, and asserts that

  * a fresh default-profile start creates the datastore under that disposable
    identity's %LOCALAPPDATA%, owned by that user, and
  * nothing is created under the runner user's own profile.

It does not cover the recovery fixture matrix (Local vs Roaming vs Python
history). It establishes and proves the isolation precondition only, which is
what has to be true before any fixture may be written.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$ServerExe,

    [ValidatePattern('^[A-Za-z][A-Za-z0-9_-]{0,19}$')]
    [string]$User = 'awrecovery',

    [int]$TimeoutSeconds = 120,

    [string]$EvidencePath = (Join-Path $env:TEMP 'windows-disposable-profile-evidence.json')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) {
        throw "ASSERTION FAILED: $Message"
    }
}

function Set-DisposableBatchLogonRight {
    param([string]$Sid, [bool]$Grant)
    # Modify one right on one owned SID through LSA, not a machine-wide secedit
    # policy import (which could overwrite unrelated runner assignments).
    if (-not ('AwIsolation.BatchLogon' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;
namespace AwIsolation {
    public static class BatchLogon {
        [StructLayout(LayoutKind.Sequential)]
        struct Attributes {
            public uint Length;
            public IntPtr RootDirectory, ObjectName;
            public uint Flags;
            public IntPtr SecurityDescriptor, SecurityQualityOfService;
        }
        [StructLayout(LayoutKind.Sequential)]
        struct UnicodeString {
            public ushort Length, MaximumLength;
            public IntPtr Buffer;
        }
        [DllImport("advapi32.dll")]
        static extern uint LsaOpenPolicy(IntPtr system, ref Attributes attributes, uint access, out IntPtr policy);
        [DllImport("advapi32.dll")]
        static extern uint LsaAddAccountRights(IntPtr policy, byte[] sid, UnicodeString[] rights, uint count);
        [DllImport("advapi32.dll")]
        static extern uint LsaRemoveAccountRights(IntPtr policy, byte[] sid,
            [MarshalAs(UnmanagedType.U1)] bool all, UnicodeString[] rights, uint count);
        [DllImport("advapi32.dll")]
        static extern uint LsaNtStatusToWinError(uint status);
        [DllImport("advapi32.dll")]
        static extern uint LsaClose(IntPtr policy);
        static void Check(uint status) {
            if (status != 0) throw new Win32Exception((int)LsaNtStatusToWinError(status));
        }
        public static void Set(string sidText, bool grant) {
            var sid = new SecurityIdentifier(sidText);
            var bytes = new byte[sid.BinaryLength];
            sid.GetBinaryForm(bytes, 0);
            var attributes = new Attributes { Length = (uint)Marshal.SizeOf(typeof(Attributes)) };
            IntPtr policy;
            // POLICY_LOOKUP_NAMES | POLICY_CREATE_ACCOUNT
            Check(LsaOpenPolicy(IntPtr.Zero, ref attributes, 0x810, out policy));
            IntPtr buffer = IntPtr.Zero;
            try {
                const string name = "SeBatchLogonRight";
                buffer = Marshal.StringToHGlobalUni(name);
                var rights = new[] { new UnicodeString {
                    Length = (ushort)(name.Length * 2),
                    MaximumLength = (ushort)((name.Length + 1) * 2), Buffer = buffer
                } };
                uint status = grant ? LsaAddAccountRights(policy, bytes, rights, 1)
                    : LsaRemoveAccountRights(policy, bytes, false, rights, 1);
                // No LSA account exists if setup failed before the grant.
                if (!grant && LsaNtStatusToWinError(status) == 2) return;
                Check(status);
            } finally {
                if (buffer != IntPtr.Zero) Marshal.FreeHGlobal(buffer);
                LsaClose(policy);
            }
        }
    }
}
'@
    }
    [AwIsolation.BatchLogon]::Set($Sid, $Grant)
}

function Wait-ProfileInitTask {
    # A measured hosted-runner cold batch logon completed ~16 seconds after
    # /Run. Allow the same bounded 120s startup budget as server readiness,
    # rather than rejecting successful profile initialization at 15s.
    param([string]$TaskName, [datetime]$PreviousRunTime, [int]$TimeoutSeconds = 120)
    $initDeadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $initDeadline) {
        $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction Stop
        $info = Get-ScheduledTaskInfo -TaskName $TaskName -ErrorAction Stop
        # Logon/action-launch HRESULTs can fail before LastRunTime advances.
        # Scheduler's informational states (0x000413xx) are not failures.
        $resultCode = [long]$info.LastTaskResult -band 0xffffffffL
        if (($resultCode -band 0x80000000L) -ne 0) {
            throw ('profile-init task failed with result 0x{0:X8} (state {1})' -f $resultCode, $task.State)
        }
        # Ready alone also describes a task that has never started. Require a
        # new execution, a terminal state, and a successful action exit code.
        if ($info.LastRunTime -gt $PreviousRunTime -and
            $task.State -notin @('Queued', 'Running') -and
            $info.LastTaskResult -ne 267009) {
            if ($info.LastTaskResult -ne 0) {
                throw "profile-init task failed with result $($info.LastTaskResult)"
            }
            return $info
        }
        Start-Sleep -Milliseconds 500
    }
    throw "profile-init task timed out after $TimeoutSeconds seconds"
}

function Get-ProfileTail {
    param([string]$Dir)
    if (-not (Test-Path -LiteralPath $Dir)) {
        return '(no profile tree was created)'
    }
    $files = @(Get-ChildItem -LiteralPath $Dir -Recurse -File -ErrorAction SilentlyContinue | Select-Object -First 10 -ExpandProperty FullName)
    return ($files -join [Environment]::NewLine)
}

$exe = (Resolve-Path -LiteralPath $ServerExe).Path
Assert-True (Test-Path -LiteralPath $exe -PathType Leaf) "server executable not found: $exe"

$runnerProfile = $env:USERPROFILE
$runnerActivityWatch = Join-Path $env:LOCALAPPDATA 'activitywatch'
$disposableProfile = Join-Path $env:SystemDrive "Users\$User"
$disposableActivityWatch = Join-Path $disposableProfile 'AppData\Local\activitywatch'
$expectedDb = Join-Path $disposableActivityWatch 'aw-server-rust\sqlite.db'

$password = 'Aw!' + ([guid]::NewGuid().ToString('N').Substring(0, 20)) + 'aA9'
$port = Get-Random -Minimum 20000 -Maximum 40000
$evidence = [ordered]@{
    server_exe          = $exe
    disposable_user     = $User
    disposable_profile  = $disposableProfile
    expected_db         = $expectedDb
    runner_profile      = $runnerProfile
    port                = $port
    started_at          = (Get-Date).ToUniversalTime().ToString('o')
}
$failure = $null
$createdUser = $null
$serverProcess = $null
$logFile = $null
$wrapper = $null
$kfProbe = $null
$initLogFile = $null

try {
    # Refuse before creating anything; finally must never remove existing users
    # or profiles, including when one of these preconditions fails.
    Assert-True (-not (Test-Path -LiteralPath $runnerActivityWatch)) `
        "refusing to run: $runnerActivityWatch already exists, so isolation cannot be proven on this runner"
    Assert-True (-not (Test-Path -LiteralPath $disposableProfile)) `
        "disposable profile $disposableProfile already exists; it must be created by this run"
    Assert-True ($null -eq (Get-LocalUser -Name $User -ErrorAction SilentlyContinue)) `
        "refusing to replace existing local user '$User'"
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $isAdmin = ([Security.Principal.WindowsPrincipal]$identity).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
    $evidence['runner_user'] = $identity.Name
    $evidence['runner_elevated'] = $isAdmin
    Write-Host "Runner identity: $($identity.Name) (elevated: $isAdmin)"
    Assert-True $isAdmin `
        'the Windows runner user is not elevated; creating a disposable local user needs administrator rights'

    $secure = ConvertTo-SecureString -String $password -AsPlainText -Force
    try {
        $createdUser = New-LocalUser -Name $User -Password $secure -PasswordNeverExpires -AccountNeverExpires `
            -Description 'temporary aw-server isolation check' -ErrorAction Stop
        Add-LocalGroupMember -Group 'Users' -Member $User -ErrorAction Stop
    }
    catch {
        throw "could not create the disposable local user '$User': $($_.Exception.Message)"
    }

    # CI's Scheduler events 101/104 reported 0x80070569: a new standard
    # user has no batch-logon right. Grant only this run's SID; revoke in finally.
    Set-DisposableBatchLogonRight -Sid $createdUser.SID.Value -Grant $true
    $evidence['batch_logon_right_granted'] = $true
    $credential = New-Object System.Management.Automation.PSCredential($User, $secure)

    # Initialize the disposable user's profile via a one-shot scheduled task BEFORE
    # launching aw-server. Start-Process -Credential -LoadUserProfile loads NTUSER.DAT
    # but does not complete the first-logon initialization: the user's token environment
    # is missing USERPROFILE, so SHGetKnownFolderPath returns E_FAIL even though the
    # profile directory was copied from the template. A Task Scheduler batch logon creates
    # a full user session (LogonUser LOGON32_LOGON_BATCH) which completes that
    # initialization, so the subsequent Start-Process launch has the correct token env.
    $runId = [guid]::NewGuid().ToString('N')
    $initTaskName = "AW-ProfileInit-$runId"
    $initLogFile = Join-Path $env:Public "aw-init-$runId.log"
    $initStartedAt = Get-Date
    $initEventLogError = $null
    $stRun = @()
    # Hosted runners may have task history disabled. Enable it before registration
    # so a rejected demand start leaves evidence; failure here is diagnostic only.
    $eventLogSetup = & wevtutil.exe sl Microsoft-Windows-TaskScheduler/Operational /e:true 2>&1
    if ($LASTEXITCODE -ne 0) { $initEventLogError = "could not enable task history: $eventLogSetup" }
    $stCreate = schtasks.exe /Create /TN $initTaskName `
        /TR "cmd.exe /d /c whoami > `"$initLogFile`" 2>&1" `
        /SC ONCE /SD "01/01/2000" /ST "00:00" `
        /RU $User /RP $password /F /RL LIMITED 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "could not create profile-init scheduled task (schtasks returned $LASTEXITCODE): $stCreate"
    }
    try {
        $previousRunTime = (Get-ScheduledTaskInfo -TaskName $initTaskName -ErrorAction Stop).LastRunTime
        $stRun = schtasks.exe /Run /TN $initTaskName 2>&1
        if ($LASTEXITCODE -ne 0) {
            throw "could not run profile-init scheduled task (schtasks returned $LASTEXITCODE): $stRun"
        }
        $initResult = Wait-ProfileInitTask -TaskName $initTaskName -PreviousRunTime $previousRunTime
        $evidence['profile_init_result'] = $initResult.LastTaskResult
        $evidence['profile_init_run_time'] = $initResult.LastRunTime.ToUniversalTime().ToString('o')
    }
    finally {
        # The task may fail before producing a log or advancing LastRunTime.
        # Snapshot only non-secret status fields before stopping/deleting it.
        try {
            $initTask = Get-ScheduledTask -TaskName $initTaskName -ErrorAction Stop
            $initInfo = Get-ScheduledTaskInfo -TaskName $initTaskName -ErrorAction Stop
            $evidence['profile_init_state'] = [string]$initTask.State
            $evidence['profile_init_last_result'] = $initInfo.LastTaskResult
            $evidence['profile_init_last_result_hex'] = '0x{0:X8}' -f ([long]$initInfo.LastTaskResult -band 0xffffffffL)
            $evidence['profile_init_last_run_time'] = $initInfo.LastRunTime.ToUniversalTime().ToString('o')
            $evidence['profile_init_previous_run_time'] = $previousRunTime.ToUniversalTime().ToString('o')
            # /Run can be accepted without an action starting. Preserve effective
            # demand-start conditions and registration warnings, not the /RP argv
            # or a full task export that could acquire sensitive fields later.
            # Native stderr is an ErrorRecord on Windows PowerShell. Persist
            # text only, not InvocationInfo/argv or nested exception metadata.
            $evidence['profile_init_create_output'] = @($stCreate | ForEach-Object { [string]$_ })
            $evidence['profile_init_run_output'] = @($stRun | ForEach-Object { [string]$_ })
            $evidence['profile_init_settings'] = $initTask.Settings | Select-Object `
                Enabled, AllowDemandStart, DisallowStartIfOnBatteries, StopIfGoingOnBatteries, `
                RunOnlyIfIdle, RunOnlyIfNetworkAvailable
            $evidence['profile_init_principal'] = $initTask.Principal | Select-Object UserId, LogonType, RunLevel
        }
        catch {
            $evidence['profile_init_diagnostic_error'] = $_.Exception.Message
        }
        try {
            if ($null -ne $initEventLogError) {
                $evidence['profile_init_event_error'] = $initEventLogError
            }
            $events = @(Get-WinEvent -FilterHashtable @{
                LogName = 'Microsoft-Windows-TaskScheduler/Operational'
                StartTime = $initStartedAt
            } -MaxEvents 200 -ErrorAction Stop)
            # Never publish free-form event messages or other tasks' data. Event
            # IDs and numeric payloads retain dispatch failures/HRESULTs safely.
            $evidence['profile_init_events'] = @($events | Where-Object {
                @($_.Properties | Where-Object { $_.Value -eq "\$initTaskName" }).Count -gt 0
            } | ForEach-Object {
                [ordered]@{
                    id = $_.Id
                    time = $_.TimeCreated.ToUniversalTime().ToString('o')
                    codes = @($_.Properties | Where-Object { $_.Value -is [ValueType] } |
                        ForEach-Object { [string]$_.Value })
                }
            })
        }
        catch {
            $evidence['profile_init_event_error'] = $_.Exception.Message
        }
        # A timed-out/failed task must not keep writing into a profile we remove.
        Stop-ScheduledTask -TaskName $initTaskName -ErrorAction SilentlyContinue
        schtasks.exe /Delete /TN $initTaskName /F 2>$null | Out-Null
    }
    Write-Host "Profile initialized for disposable user '$User' via scheduled task"

    # Debug builds default to the `testing` profile, and automatic Python legacy
    # import only runs for `default`, so name the profile explicitly.
    # A cross-user child has no inherited console, so a Rust panic would be lost.
    # Launch through a batch wrapper that redirects both streams to a file the
    # disposable user can write, so a red run reports the panic, not just exit 101.
    $logFile = Join-Path $env:Public "aw-isolation-$runId.log"
    $wrapper = Join-Path $env:Public "aw-isolation-$runId.cmd"
    $kfProbe = Join-Path $env:Public "aw-kfprobe-$runId.ps1"
    # The server panics with "Unable to get log dir" when the `dirs` crate's
    # SHGetKnownFolderPath call fails. Record the actual HRESULT and path the
    # launched user sees, so the next red run says why instead of guessing.
    $kfProbeBody = @(
        'Add-Type -TypeDefinition @"',
        'using System; using System.Runtime.InteropServices;',
        'public static class KF {',
        '  [DllImport("shell32.dll")] public static extern int SHGetKnownFolderPath([MarshalAs(UnmanagedType.LPStruct)] Guid id, uint flags, IntPtr token, out IntPtr path);',
        '}',
        '"@',
        'foreach ($f in @{ LocalAppData = "F1B32785-6FBA-4FCF-9D55-7B8E7F157091"; Profile = "5E6C858F-0E22-4760-9AFE-EA3317B67173" }.GetEnumerator()) {',
        '  $p = [IntPtr]::Zero',
        '  $hr = [KF]::SHGetKnownFolderPath([Guid]$f.Value, 0, [IntPtr]::Zero, [ref]$p)',
        '  $path = if ($hr -eq 0) { [Runtime.InteropServices.Marshal]::PtrToStringUni($p) } else { "" }',
        '  "KnownFolder {0} hr=0x{1:X8} path={2}" -f $f.Key, ([long]$hr -band 0xffffffffL), $path',
        '}'
    )
    Set-Content -LiteralPath $kfProbe -Value $kfProbeBody -Encoding ascii
    # Capture only profile-related diagnostics, never the full child environment.
    # A copied profile tree alone does not prove Known Folder initialization.
    $wrapperBody = @(
        '@echo off',
        ('whoami > "{0}" 2>&1' -f $logFile),
        ('echo USERPROFILE=%USERPROFILE% >> "{0}" 2>&1' -f $logFile),
        ('echo LOCALAPPDATA=%LOCALAPPDATA% >> "{0}" 2>&1' -f $logFile),
        ('echo APPDATA=%APPDATA% >> "{0}" 2>&1' -f $logFile),
        ('reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders" >> "{0}" 2>&1' -f $logFile),
        ('powershell.exe -NoProfile -ExecutionPolicy Bypass -File "{0}" >> "{1}" 2>&1' -f $kfProbe, $logFile),
        ('"{0}" --profile default --host 127.0.0.1 --port {1} --verbose >> "{2}" 2>&1' -f $exe, $port, $logFile)
    )
    Set-Content -LiteralPath $wrapper -Value $wrapperBody -Encoding ascii

    Write-Host "Starting aw-server as disposable user '$User' on port $port (profile: default)"
    try {
        $serverProcess = Start-Process -FilePath "$env:SystemRoot\System32\cmd.exe" `
            -ArgumentList @('/d', '/s', '/c', ('""{0}""' -f $wrapper)) `
            -Credential $credential `
            -LoadUserProfile `
            -UseNewEnvironment `
            -WorkingDirectory $env:SystemRoot `
            -PassThru
    }
    catch {
        throw "could not start aw-server as a disposable user ($($_.Exception.Message)). This is the isolation seam: starting a process as another local user needs the Secondary Logon (seclogon) service or a scheduled task."
    }

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $dbBytes = $null
    $databaseReady = $false
    while ((Get-Date) -lt $deadline) {
        # SQLite creates the file before writing its header. Retry empty files
        # and transient sharing failures instead of rejecting healthy startup.
        try {
            if (Test-Path -LiteralPath $expectedDb) {
                # The running server keeps the database open for writing, so
                # ReadAllBytes (FileShare.Read) fails with a permanent sharing
                # violation (seen in CI). Open with the same share mode SQLite uses.
                $dbStream = [System.IO.File]::Open($expectedDb, [System.IO.FileMode]::Open,
                    [System.IO.FileAccess]::Read,
                    [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete)
                try {
                    $dbBytes = New-Object byte[] 16
                    $got = $dbStream.Read($dbBytes, 0, 16)
                }
                finally { $dbStream.Dispose() }
                $databaseReady = $got -eq 16 -and
                    [System.Text.Encoding]::ASCII.GetString($dbBytes, 0, 16) -eq "SQLite format 3`0"
                if ($databaseReady) { break }
            }
        }
        catch [System.IO.IOException] {
            $evidence['last_database_read_error'] = $_.Exception.Message
        }
        if ($serverProcess.HasExited) { break }
        Start-Sleep -Milliseconds 500
    }

    $alive = $null
    $exitCode = $null
    try {
        $alive = -not $serverProcess.HasExited
        if (-not $alive) { $exitCode = $serverProcess.ExitCode }
    }
    catch {
        $alive = "unknown ($($_.Exception.Message))"
    }
    $evidence['server_process_alive'] = $alive
    $evidence['server_process_exit_code'] = $exitCode
    $evidence['server_processes'] = @(Get-Process -Name 'aw-server' -ErrorAction SilentlyContinue |
        ForEach-Object { "pid=$($_.Id) session=$($_.SessionId)" })

    if (-not $databaseReady) {
        $detail = "aw-server did not initialize a readable SQLite header at $expectedDb (pid $($serverProcess.Id), alive: $alive, exit: $exitCode)."
        foreach ($sub in @('AppData\Local', 'AppData\Roaming')) {
            $dir = Join-Path $disposableProfile $sub
            $detail += "`n$sub entries: " + ((@(Get-ChildItem -LiteralPath $dir -ErrorAction SilentlyContinue | Select-Object -First 15 -ExpandProperty Name)) -join ', ')
        }
        $detail += "`nprofile files: " + (Get-ProfileTail -Dir $disposableProfile)
        if (Test-Path -LiteralPath $logFile) {
            $detail += "`nserver log tail:" + [Environment]::NewLine +
                ((Get-Content -LiteralPath $logFile -Tail 30 -ErrorAction SilentlyContinue) -join [Environment]::NewLine)
        }
        else {
            $detail += "`nserver log: $logFile was not created"
        }
        throw $detail
    }

    $dataDir = Split-Path -Parent $expectedDb
    $owner = (Get-Acl -LiteralPath $dataDir).Owner
    Assert-True ($owner -like "*$User") `
        "datastore directory is owned by $owner, not the disposable user '$User'"

    Assert-True (-not (Test-Path -LiteralPath $runnerActivityWatch)) `
        "runner profile was written: $runnerActivityWatch exists. Known Folder isolation failed."

    $relativeEntries = @()
    foreach ($file in @(Get-ChildItem -LiteralPath $disposableActivityWatch -Recurse -File)) {
        $relativeEntries += $file.FullName.Substring($disposableProfile.Length).TrimStart('\')
    }
    $evidence['datastore_bytes'] = $dbBytes.Length
    $evidence['datastore_owner'] = $owner
    $evidence['profile_entries'] = $relativeEntries
    $evidence['conclusion'] = 'disposable Known Folder roots proven'
    Write-Host 'Disposable-profile isolation proven:'
    Write-Host "  datastore: $expectedDb ($($dbBytes.Length) bytes, owner $owner)"
    Write-Host "  runner profile untouched: $runnerActivityWatch absent"
}
catch {
    $failure = $_.Exception.Message
    $evidence['conclusion'] = "isolation NOT proven: $failure"
    throw
}
finally {
    if ($null -eq $failure) {
        $evidence['status'] = 'pass'
    }
    else {
        $evidence['status'] = 'fail'
        $evidence['failure'] = $failure
    }
    $evidence['finished_at'] = (Get-Date).ToUniversalTime().ToString('o')

    if ($null -ne $logFile -and (Test-Path -LiteralPath $logFile)) {
        $evidence['server_log'] = @(Get-Content -LiteralPath $logFile -ErrorAction SilentlyContinue)
    }
    if ($null -ne $initLogFile -and (Test-Path -LiteralPath $initLogFile)) {
        $evidence['profile_init_log'] = @(Get-Content -LiteralPath $initLogFile -ErrorAction SilentlyContinue)
    }
    # Persist before cleanup so an unavailable CIM/cleanup primitive cannot
    # discard the original startup failure and its child-session diagnostics.
    try {
        $evidence | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $EvidencePath -Encoding utf8
        Write-Host "Evidence written to $EvidencePath"
    }
    finally {
        # Stop only this run's wrapper and its descendants, never every aw-server.
        if ($null -ne $serverProcess -and -not $serverProcess.HasExited) {
            & taskkill.exe /PID $serverProcess.Id /T /F 2>$null | Out-Null
        }
        if ($null -ne $createdUser) {
            # The new SID is the ownership token. Pre-existing accounts/profiles can
            # never reach this cleanup, even if account creation or a guard fails.
            Start-Sleep -Seconds 2
            $profile = Get-CimInstance Win32_UserProfile -Filter "SID='$($createdUser.SID.Value)'" -ErrorAction SilentlyContinue
            if ($null -ne $profile) {
                $profile | Remove-CimInstance -ErrorAction SilentlyContinue
            }
            try {
                Set-DisposableBatchLogonRight -Sid $createdUser.SID.Value -Grant $false
            } catch {
                Write-Warning "Failed to revoke batch logon right for $($createdUser.SID.Value): $_"
            } finally {
                Remove-LocalUser -SID $createdUser.SID -ErrorAction SilentlyContinue
            }
        }
        if ($null -ne $kfProbe) {
            Remove-Item -LiteralPath $kfProbe -Force -ErrorAction SilentlyContinue
        }
        if ($null -ne $wrapper) {
            Remove-Item -LiteralPath $wrapper -Force -ErrorAction SilentlyContinue
        }
        if ($null -ne $logFile) {
            Remove-Item -LiteralPath $logFile -Force -ErrorAction SilentlyContinue
        }
        if ($null -ne $initLogFile -and (Test-Path -LiteralPath $initLogFile -ErrorAction SilentlyContinue)) {
            Remove-Item -LiteralPath $initLogFile -Force -ErrorAction SilentlyContinue
        }
    }
}
