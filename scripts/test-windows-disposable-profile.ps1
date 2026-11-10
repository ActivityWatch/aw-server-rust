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

    $credential = New-Object System.Management.Automation.PSCredential($User, $secure)

    # Debug builds default to the `testing` profile, and automatic Python legacy
    # import only runs for `default`, so name the profile explicitly.
    # A cross-user child has no inherited console, so a Rust panic would be lost.
    # Launch through a batch wrapper that redirects both streams to a file the
    # disposable user can write, so a red run reports the panic, not just exit 101.
    $runId = [guid]::NewGuid().ToString('N')
    $logFile = Join-Path $env:Public "aw-isolation-$runId.log"
    $wrapper = Join-Path $env:Public "aw-isolation-$runId.cmd"
    # Capture only profile-related diagnostics, never the full child environment.
    # A copied profile tree alone does not prove Known Folder initialization.
    $wrapperBody = @(
        '@echo off',
        ('whoami > "{0}" 2>&1' -f $logFile),
        ('echo USERPROFILE=%USERPROFILE% >> "{0}" 2>&1' -f $logFile),
        ('echo LOCALAPPDATA=%LOCALAPPDATA% >> "{0}" 2>&1' -f $logFile),
        ('echo APPDATA=%APPDATA% >> "{0}" 2>&1' -f $logFile),
        ('reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders" >> "{0}" 2>&1' -f $logFile),
        ('"{0}" --profile default --host 127.0.0.1 --port {1} --verbose >> "{2}" 2>&1' -f $exe, $port, $logFile)
    )
    Set-Content -LiteralPath $wrapper -Value $wrapperBody -Encoding ascii

    Write-Host "Starting aw-server as disposable user '$User' on port $port (profile: default)"
    try {
        $serverProcess = Start-Process -FilePath "$env:SystemRoot\System32\cmd.exe" `
            -ArgumentList @('/d', '/s', '/c', ('""{0}""' -f $wrapper)) `
            -Credential $credential `
            -LoadUserProfile `
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
                $dbBytes = [System.IO.File]::ReadAllBytes($expectedDb)
                $databaseReady = $dbBytes.Length -ge 16 -and
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
            Remove-LocalUser -SID $createdUser.SID -ErrorAction SilentlyContinue
        }
        if ($null -ne $wrapper) {
            Remove-Item -LiteralPath $wrapper -Force -ErrorAction SilentlyContinue
        }
        if ($null -ne $logFile) {
            Remove-Item -LiteralPath $logFile -Force -ErrorAction SilentlyContinue
        }
    }
}
