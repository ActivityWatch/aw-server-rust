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

# A pre-existing tree would make the "nothing under the runner profile" half of
# this proof meaningless, so refuse instead of guessing.
Assert-True (-not (Test-Path -LiteralPath $runnerActivityWatch)) `
    "refusing to run: $runnerActivityWatch already exists, so isolation cannot be proven on this runner"

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

try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $isAdmin = ([Security.Principal.WindowsPrincipal]$identity).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
    $evidence['runner_user'] = $identity.Name
    $evidence['runner_elevated'] = $isAdmin
    Write-Host "Runner identity: $($identity.Name) (elevated: $isAdmin)"
    Assert-True $isAdmin `
        'the Windows runner user is not elevated; creating a disposable local user needs administrator rights'

    Remove-LocalUser -Name $User -ErrorAction SilentlyContinue
    $secure = ConvertTo-SecureString -String $password -AsPlainText -Force
    try {
        New-LocalUser -Name $User -Password $secure -PasswordNeverExpires -AccountNeverExpires `
            -Description 'temporary aw-server isolation check' -ErrorAction Stop | Out-Null
        Add-LocalGroupMember -Group 'Users' -Member $User -ErrorAction Stop
    }
    catch {
        throw "could not create the disposable local user '$User': $($_.Exception.Message)"
    }

    Assert-True (-not (Test-Path -LiteralPath $disposableProfile)) `
        "disposable profile $disposableProfile already exists; it must be created by this run"

    $credential = New-Object System.Management.Automation.PSCredential($User, $secure)

    # Debug builds default to the `testing` profile, and automatic Python legacy
    # import only runs for `default`, so name the profile explicitly.
    # A cross-user child has no inherited console, so a Rust panic would be lost.
    # Launch through a batch wrapper that redirects both streams to a file the
    # disposable user can write, so a red run reports the panic, not just exit 101.
    $logFile = Join-Path $env:Public 'aw-isolation-server.log'
    $wrapper = Join-Path $env:Public 'aw-isolation-run.cmd'
    Remove-Item -LiteralPath $logFile, $wrapper -Force -ErrorAction SilentlyContinue
    $wrapperBody = @(
        '@echo off',
        ('"{0}" --profile default --host 127.0.0.1 --port {1} --verbose > "{2}" 2>&1' -f $exe, $port, $logFile)
    )
    Set-Content -LiteralPath $wrapper -Value $wrapperBody -Encoding ascii

    Write-Host "Starting aw-server as disposable user '$User' on port $port (profile: default)"
    try {
        $serverProcess = Start-Process -FilePath $wrapper `
            -Credential $credential `
            -LoadUserProfile `
            -WorkingDirectory $env:SystemRoot `
            -PassThru
    }
    catch {
        throw "could not start aw-server as a disposable user ($($_.Exception.Message)). This is the isolation seam: starting a process as another local user needs the Secondary Logon (seclogon) service or a scheduled task."
    }

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path -LiteralPath $expectedDb) { break }
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

    if (-not (Test-Path -LiteralPath $expectedDb)) {
        $detail = "aw-server never created $expectedDb (pid $($serverProcess.Id), alive: $alive, exit: $exitCode)."
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

    $dbBytes = [System.IO.File]::ReadAllBytes($expectedDb)
    Assert-True ($dbBytes.Length -ge 16) "datastore at $expectedDb is empty"
    $header = [System.Text.Encoding]::ASCII.GetString($dbBytes, 0, 15)
    Assert-True ($header -eq 'SQLite format 3') "datastore at $expectedDb is not a SQLite database (header: '$header')"

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

    Get-Process -Name 'aw-server' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
    Remove-LocalUser -Name $User -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $disposableProfile) {
        Remove-Item -LiteralPath $disposableProfile -Recurse -Force -ErrorAction SilentlyContinue
    }
    $evidence | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $EvidencePath -Encoding utf8
    Write-Host "Evidence written to $EvidencePath"
}
