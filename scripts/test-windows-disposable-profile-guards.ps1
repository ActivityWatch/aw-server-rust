#Requires -Version 5.1
# Runs without administrator rights or Windows APIs. Exercise the actual script's
# refusal paths and polling block; native logon remains the Windows CI proof.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$scriptPath = Join-Path $PSScriptRoot 'test-windows-disposable-profile.ps1'
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw ($errors | Out-String) }

$root = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$originalEnvironment = @{}
foreach ($name in @('USERPROFILE', 'LOCALAPPDATA', 'SystemDrive')) {
    $originalEnvironment[$name] = [Environment]::GetEnvironmentVariable($name)
}
try {
    $env:USERPROFILE = Join-Path $root 'runner'
    $env:LOCALAPPDATA = Join-Path $root 'runner-local'
    $env:SystemDrive = $root
    $exe = Join-Path $root 'fake-server.exe'
    Set-Content -LiteralPath $exe -Value 'not executed'
    $profile = Join-Path $root 'Users/awrecovery'
    New-Item -ItemType Directory -Path $profile -Force | Out-Null
    $sentinel = Join-Path $profile 'keep.txt'
    Set-Content -LiteralPath $sentinel -Value 'pre-existing profile'

    # Destructive commands must not run on either refusal path.
    function Remove-LocalUser { throw 'unexpected account deletion' }
    function Remove-CimInstance { throw 'unexpected profile deletion' }
    function Stop-Process { throw 'unexpected process termination' }
    function Get-LocalUser { return [pscustomobject]@{ Name = 'awrecovery' } }
    foreach ($case in @('existing-profile', 'existing-account')) {
        if ($case -eq 'existing-account') {
            Remove-Item -LiteralPath $profile -Recurse -Force
        }
        $evidencePath = Join-Path $root "$case.json"
        $caught = $null
        try { & $scriptPath -ServerExe $exe -EvidencePath $evidencePath }
        catch { $caught = $_.Exception.Message }
        $expected = if ($case -eq 'existing-profile') { 'already exists' } else { 'refusing to replace existing local user' }
        if ($null -eq $caught -or -not $caught.Contains($expected)) {
            throw "$case did not refuse safely: $caught"
        }
        if ($case -eq 'existing-profile' -and (Get-Content -LiteralPath $sentinel) -ne 'pre-existing profile') {
            throw 'pre-existing profile was modified'
        }
        $receipt = Get-Content -LiteralPath $evidencePath -Raw | ConvertFrom-Json
        if ($receipt.status -ne 'fail') { throw "$case lost its failure evidence" }
    }

    # Only the newly created account may receive batch logon; grant it before
    # task registration, and revoke it before deleting that account. Never
    # replace the machine's existing user-right assignments.
    $grant = @($ast.FindAll({ param($node)
        $node -is [System.Management.Automation.Language.CommandAst] -and
        $node.GetCommandName() -eq 'Set-DisposableBatchLogonRight'
    }, $true))
    if ($grant.Count -ne 2 -or
        $grant[0].Extent.Text -notlike '*-Sid $createdUser.SID.Value -Grant $true*' -or
        $grant[1].Extent.Text -notlike '*-Sid $createdUser.SID.Value -Grant $false*') {
        throw 'batch-logon right must be granted and revoked only for the owned SID'
    }
    $registration = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.CommandAst] -and
        $node.Extent.Text -like 'schtasks.exe /Create*'
    }, $true)
    if ($grant[0].Extent.StartOffset -ge $registration.Extent.StartOffset) {
        throw 'batch-logon right was not granted before task registration'
    }

    # Exercise the production task waiter without Windows APIs. A newly created
    # task can be Ready before its first run, and Queued is not completion.
    $waiter = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Wait-ProfileInitTask'
    }, $true)
    if ($null -eq $waiter) { throw 'missing profile-init task waiter' }
    . ([scriptblock]::Create($waiter.Extent.Text))
    $baseline = [datetime]'2000-01-01'
    $script:taskPoll = 0
    function Get-ScheduledTask {
        param($TaskName)
        $state = @('Ready', 'Queued', 'Running', 'Ready')[$script:taskPoll]
        return [pscustomobject]@{ State = $state }
    }
    function Get-ScheduledTaskInfo {
        param($TaskName)
        return [pscustomobject]@{
            LastRunTime = $baseline.AddSeconds([int]($script:taskPoll -ge 2))
            LastTaskResult = if ($script:taskPoll -lt 2) { 267011 } elseif ($script:taskPoll -eq 2) { 267009 } else { 0 }
        }
    }
    function Start-Sleep { param($Milliseconds) $script:taskPoll++ }
    $result = Wait-ProfileInitTask -TaskName 'fake-task' -PreviousRunTime $baseline
    if ($script:taskPoll -ne 3 -or $result.LastTaskResult -ne 0) {
        throw 'task waiter accepted an unstarted, queued, or running task'
    }
    function Get-ScheduledTaskInfo {
        param($TaskName)
        return [pscustomobject]@{ LastRunTime = $baseline.AddSeconds(1); LastTaskResult = 1 }
    }
    $caught = $null
    try { Wait-ProfileInitTask -TaskName 'fake-task' -PreviousRunTime $baseline }
    catch { $caught = $_.Exception.Message }
    if ($null -eq $caught -or $caught -notlike '*failed with result 1*') {
        throw "task failure was not rejected: $caught"
    }
    # Scheduler can reject logon/action launch without advancing LastRunTime.
    function Get-ScheduledTask {
        param($TaskName)
        return [pscustomobject]@{ State = 'Ready' }
    }
    function Get-ScheduledTaskInfo {
        param($TaskName)
        return [pscustomobject]@{ LastRunTime = $baseline; LastTaskResult = 2147943785 }
    }
    $caught = $null
    try { Wait-ProfileInitTask -TaskName 'fake-task' -PreviousRunTime $baseline }
    catch { $caught = $_.Exception.Message }
    if ($null -eq $caught -or $caught -notlike '*0x80070569*') {
        throw "task launch failure without a new timestamp was not reported: $caught"
    }
    $caught = $null
    try { Wait-ProfileInitTask -TaskName 'fake-task' -PreviousRunTime $baseline -TimeoutSeconds 0 }
    catch { $caught = $_.Exception.Message }
    if ($null -eq $caught -or $caught -notlike '*timed out*') {
        throw "task timeout was not rejected: $caught"
    }

    # CI's first batch logon took ~16 seconds after /Run. Exercise a 20-second
    # cold start with a virtual clock, not a slow test or an assumed instant logon.
    & {
        $script:waitClock = [datetime]'2026-01-01'
        $script:waitStart = $script:waitClock
        $script:neverComplete = $false
        function Get-Date { return $script:waitClock }
        function Start-Sleep {
            param($Milliseconds)
            $script:waitClock = $script:waitClock.AddMilliseconds($Milliseconds)
        }
        function Get-ScheduledTask {
            param($TaskName)
            return [pscustomobject]@{ State = 'Ready' }
        }
        function Get-ScheduledTaskInfo {
            param($TaskName)
            $complete = -not $script:neverComplete -and
                ($script:waitClock - $script:waitStart).TotalSeconds -ge 20
            return [pscustomobject]@{
                LastRunTime = if ($complete) { $baseline.AddSeconds(1) } else { $baseline }
                LastTaskResult = if ($complete) { 0 } else { 267011 }
            }
        }
        $result = Wait-ProfileInitTask -TaskName 'cold-task' -PreviousRunTime $baseline
        if ($result.LastTaskResult -ne 0 -or
            ($script:waitClock - $script:waitStart).TotalSeconds -ne 20) {
            throw 'cold profile initialization was not awaited to confirmed completion'
        }
        # An explicit shorter budget and a permanently unstarted task still fail.
        foreach ($case in @('short-budget', 'never-started')) {
            $script:waitClock = $script:waitStart
            $script:neverComplete = $case -eq 'never-started'
            $caught = $null
            try {
                if ($case -eq 'short-budget') {
                    Wait-ProfileInitTask -TaskName 'cold-task' -PreviousRunTime $baseline -TimeoutSeconds 15
                }
                else {
                    Wait-ProfileInitTask -TaskName 'cold-task' -PreviousRunTime $baseline
                }
            }
            catch { $caught = $_.Exception.Message }
            if ($null -eq $caught -or $caught -notlike '*timed out*') {
                throw "$case did not fail closed: $caught"
            }
            $expectedSeconds = if ($case -eq 'short-budget') { 15 } else { 120 }
            if (($script:waitClock - $script:waitStart).TotalSeconds -ne $expectedSeconds) {
                throw "$case did not respect its bounded initialization budget"
            }
        }
    }

    # Failure diagnostics must be collected before the task is deleted, and a
    # failed diagnostic query must not prevent owned-task cleanup.
    $taskCleanup = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.TryStatementAst] -and
        $null -ne $node.Finally -and
        $node.Finally.Extent.Text -like '*Stop-ScheduledTask*' -and
        $node.Body.Extent.Text -like '*$previousRunTime*'
    }, $true)
    if ($null -eq $taskCleanup) { throw 'missing profile-init task cleanup block' }
    $initTaskName = 'fake-task'
    $previousRunTime = $baseline
    $evidence = @{}
    $script:taskDeleted = $false
    function Get-ScheduledTask {
        param($TaskName)
        if ($script:taskDeleted) { throw 'diagnostics queried after deletion' }
        return [pscustomobject]@{
            State = 'Ready'
            Settings = [pscustomobject]@{
                Enabled = $true; AllowDemandStart = $false
                DisallowStartIfOnBatteries = $true; StopIfGoingOnBatteries = $true
                RunOnlyIfIdle = $false; RunOnlyIfNetworkAvailable = $false
            }
            Principal = [pscustomobject]@{ UserId = 'disposable'; LogonType = 'Password'; RunLevel = 'Limited' }
        }
    }
    $nativeError = [System.Management.Automation.ErrorRecord]::new(
        [Exception]::new('WARNING: batch logon privilege missing'), 'NativeCommandError',
        [System.Management.Automation.ErrorCategory]::NotSpecified, $null)
    $stCreate = @('registered', $nativeError)
    $stRun = @('run request accepted')
    $initEventLogError = $null
    $initStartedAt = Get-Date
    function Get-WinEvent {
        param($FilterHashtable, $MaxEvents)
        return @(
            [pscustomobject]@{ Id = 101; TimeCreated = $initStartedAt; Properties = @(
                [pscustomobject]@{ Value = '\fake-task' }
            ) },
            [pscustomobject]@{ Id = 999; TimeCreated = $initStartedAt; Properties = @(
                [pscustomobject]@{ Value = '\unrelated-task' }
            ) }
        )
    }
    function Stop-ScheduledTask { param($TaskName) }
    function schtasks.exe { $script:taskDeleted = $true }
    . ([scriptblock]::Create($taskCleanup.Finally.Extent.Text.TrimStart('{').TrimEnd('}')))
    if (-not $script:taskDeleted -or $evidence['profile_init_last_result_hex'] -ne '0x80070569' -or
        $evidence['profile_init_state'] -ne 'Ready') {
        throw 'pre-cleanup Scheduler failure diagnostics were lost'
    }
    if ($evidence['profile_init_settings'].AllowDemandStart -ne $false -or
        $evidence['profile_init_principal'].LogonType -ne 'Password' -or
        $evidence['profile_init_create_output'] -notcontains 'WARNING: batch logon privilege missing' -or
        $evidence['profile_init_run_output'] -notcontains 'run request accepted') {
        throw 'unstarted-task conditions, principal or registration warnings were lost'
    }
    if (@($evidence['profile_init_events']).Count -ne 1 -or $evidence['profile_init_events'][0].id -ne 101) {
        throw 'task-specific Scheduler event IDs were not retained or unrelated events leaked'
    }
    function Get-WinEvent { param($FilterHashtable, $MaxEvents) throw 'event channel unavailable' }
    function Get-ScheduledTask { param($TaskName) throw 'diagnostic query unavailable' }
    $script:taskDeleted = $false
    $evidence = @{}
    . ([scriptblock]::Create($taskCleanup.Finally.Extent.Text.TrimStart('{').TrimEnd('}')))
    if (-not $script:taskDeleted -or $evidence['profile_init_diagnostic_error'] -ne 'diagnostic query unavailable' -or
        $evidence['profile_init_event_error'] -ne 'event channel unavailable') {
        throw 'diagnostic query failure prevented task cleanup or lost its evidence'
    }

    # Run the production polling loop against an empty, then initialized file.
    # Use FindAll and select the SQLite readiness loop specifically (identified by
    # $databaseReady), since the script now has a second while loop for schtasks polling.
    $loop = @($ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.WhileStatementAst] }, $true)) |
        Where-Object { $_.Extent.Text -clike '*$databaseReady*' } |
        Select-Object -First 1
    if ($null -eq $loop) { throw 'could not locate the SQLite readiness polling loop in the production script' }
    $expectedDb = Join-Path $root 'sqlite.db'
    [IO.File]::WriteAllBytes($expectedDb, [byte[]]@())
    $serverProcess = [pscustomobject]@{ HasExited = $false }
    $evidence = @{}
    $deadline = (Get-Date).AddSeconds(10)
    $dbBytes = $null
    $databaseReady = $false
    $script:sleeps = 0
    function Start-Sleep {
        param($Milliseconds)
        $script:sleeps++
        [IO.File]::WriteAllBytes($expectedDb, [Text.Encoding]::ASCII.GetBytes("SQLite format 3`0"))
    }
    . ([scriptblock]::Create($loop.Extent.Text))
    if (-not $databaseReady -or $script:sleeps -ne 1) {
        throw 'polling did not retry the empty SQLite file'
    }
    # A live server holds the database open for writing; readiness must still
    # be detected (CI: ReadAllBytes hit a permanent sharing violation).
    $holder = [IO.File]::Open($expectedDb, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite,
        [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
    try {
        $databaseReady = $false
        $script:sleeps = 0
        . ([scriptblock]::Create($loop.Extent.Text))
        if (-not $databaseReady -or $script:sleeps -ne 0) {
            throw 'readiness was not detected while another process held the database open'
        }
    }
    finally { $holder.Dispose() }
    [IO.File]::WriteAllBytes($expectedDb, [Text.Encoding]::ASCII.GetBytes('not a SQLite db!'))
    $serverProcess.HasExited = $true
    $databaseReady = $false
    . ([scriptblock]::Create($loop.Extent.Text))
    if ($databaseReady) { throw 'invalid database accepted' }
    # Evidence persistence may fail; owned cleanup must still execute. Run the
    # actual outer finally block with fake owned resources and no Windows APIs.
    $outerTry = $ast.EndBlock.Statements | Where-Object { $_ -is [System.Management.Automation.Language.TryStatementAst] }
    $failure = 'original startup failure'
    $evidence = @{}
    $logFile = $null
    $initLogFile = $null
    $wrapper = $null
    $kfProbe = $null
    $serverProcess = $null
    $createdUser = [pscustomobject]@{ SID = [pscustomobject]@{ Value = 'owned-test-sid' } }
    $EvidencePath = Join-Path $root 'missing-directory/evidence.json'
    $script:deletedOwnedUser = $false
    function Get-CimInstance { return $null }
    $script:revokedBatchLogon = $false
    function Set-DisposableBatchLogonRight {
        param($Sid, $Grant)
        if ($Sid -ne 'owned-test-sid' -or $Grant) { throw 'wrong right-cleanup identity' }
        $script:revokedBatchLogon = $true
    }
    function Remove-LocalUser {
        param($SID)
        if (-not $script:revokedBatchLogon) { throw 'account removed before batch right cleanup' }
        if ($SID.Value -ne 'owned-test-sid') { throw 'wrong cleanup identity' }
        $script:deletedOwnedUser = $true
    }
    function Start-Sleep { param($Seconds) }
    $writeFailed = $false
    try { . ([scriptblock]::Create($outerTry.Finally.Extent.Text.TrimStart('{').TrimEnd('}'))) }
    catch { $writeFailed = $true }
    if (-not $writeFailed -or -not $script:deletedOwnedUser) {
        throw 'evidence write failure skipped owned-user cleanup'
    }
    $EvidencePath = Join-Path $root 'cim-failure.json'
    $script:deletedOwnedUser = $false
    function Get-CimInstance {
        [CmdletBinding()]
        param($ClassName, $Filter)
        Write-Error 'simulated unavailable CIM provider'
    }
    . ([scriptblock]::Create($outerTry.Finally.Extent.Text.TrimStart('{').TrimEnd('}')))
    if (-not $script:deletedOwnedUser) { throw 'CIM query failure skipped account cleanup' }
    $initLogFile = Join-Path $root 'profile-init.log'
    Set-Content -LiteralPath $initLogFile -Value 'disposable-user task diagnostics'
    $EvidencePath = Join-Path $root 'init-log.json'
    . ([scriptblock]::Create($outerTry.Finally.Extent.Text.TrimStart('{').TrimEnd('}')))
    $receipt = Get-Content -LiteralPath $EvidencePath -Raw | ConvertFrom-Json
    if ($receipt.profile_init_log -notcontains 'disposable-user task diagnostics') {
        throw 'profile-init diagnostics were not retained'
    }
    if (Test-Path -LiteralPath $initLogFile) { throw 'profile-init log was not cleaned up' }
    Write-Host 'PASS: task completion/failure/timeout checked, init log retained, existing resources preserved, SQLite readiness retried, cleanup survives evidence and CIM failures'
}
finally {
    foreach ($name in $originalEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name])
    }
    Remove-Item -LiteralPath $root -Recurse -Force
}
