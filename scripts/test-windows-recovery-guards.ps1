#Requires -Version 5.1
# Hermetic regressions for the recovery harness; no Windows account or server.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$mainPath = Join-Path $PSScriptRoot 'test-windows-disposable-profile.ps1'
$commonPath = Join-Path $PSScriptRoot 'windows-recovery-scenarios/common.ps1'
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($mainPath, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw ($errors | Out-String) }
$waiter = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
    $node.Name -eq 'Wait-ServerHttpReady'
}, $true)
if ($null -eq $waiter) { throw 'missing HTTP readiness waiter' }
. ([scriptblock]::Create($waiter.Extent.Text))
. $commonPath
function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}
$root = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$savedPublic = $env:Public
$savedSystemRoot = $env:SystemRoot
try {
    & {
        $script:clock = [datetime]'2026-01-01'
        $script:polls = 0
        function Get-Date { return $script:clock }
        function Start-Sleep { param($Milliseconds) $script:clock = $script:clock.AddMilliseconds($Milliseconds) }
        function Invoke-RestMethod {
            param($Uri, $TimeoutSec)
            $script:polls++
            if ($script:polls -lt 3) { throw 'HTTP not ready' }
            return @{ version = 'test' }
        }
        Wait-ServerHttpReady -Process ([pscustomobject]@{ HasExited = $false }) -Port 1234 -TimeoutSeconds 2
        Assert-True ($script:polls -eq 3) 'HTTP readiness did not retry transient startup failures'
        function Invoke-RestMethod { param($Uri, $TimeoutSec) throw 'never ready' }
        $caught = $null
        try { Wait-ServerHttpReady -Process ([pscustomobject]@{ HasExited = $false }) -Port 1234 -TimeoutSeconds 1 }
        catch { $caught = $_.Exception.Message }
        Assert-True ($null -ne $caught -and $caught -like '*HTTP*') 'HTTP timeout accepted'

        # Execute the actual scenario hook: even with a valid SQLite file,
        # a failed HTTP wait must stop it before the scenario writes fixtures.
        $hook = $ast.Find({ param($node)
            $node -is [System.Management.Automation.Language.IfStatementAst] -and
            $node.Extent.Text -like 'if ($ScenarioPath)*'
        }, $true)
        $ScenarioPath = Join-Path $root 'scenario.ps1'
        Set-Content -LiteralPath $ScenarioPath -Value '$script:scenarioRan = $true'
        $script:scenarioRan = $false
        $serverProcess = [pscustomobject]@{ HasExited = $false }
        $port = 1234
        $TimeoutSeconds = 1
        $evidence = @{}
        $caught = $null
        try { . ([scriptblock]::Create($hook.Extent.Text)) }
        catch { $caught = $_.Exception.Message }
        Assert-True ($null -ne $caught -and -not $script:scenarioRan) 'scenario ran before HTTP was ready'
    }

    & {
        $env:Public = $root
        $env:SystemRoot = $root
        $exe = 'fake-server.exe'
        $credential = $null
        $runId = 'guard'
        $scenario = @{}
        $script:killed = $false
        $script:waited = $false
        $script:clock = [datetime]'2026-01-01'
        function Get-Date { $script:clock = $script:clock.AddSeconds(121); return $script:clock }
        function Start-Process {
            param($FilePath, $ArgumentList, $Credential, [switch]$LoadUserProfile, [switch]$UseNewEnvironment, $WorkingDirectory, [switch]$PassThru)
            Set-Content -LiteralPath (Join-Path $root 'aw-timeout-guard.log') -Value 'startup diagnostic'
            $proc = [pscustomobject]@{ Id = 42; HasExited = $false }
            $proc | Add-Member ScriptMethod WaitForExit { param($Timeout) $script:waited = $true; return $true }
            return $proc
        }
        function taskkill.exe {
            Assert-True (($args -join ' ') -eq '/PID 42 /T /F') 'wrong process tree killed'
            $script:killed = $true
        }
        $caught = $null
        try { Start-ScenarioServer -Name 'timeout' -Port 1234 -ExtraArgs '' }
        catch { $caught = $_.Exception.Message }
        Assert-True ($caught -like '*startup diagnostic*') 'startup diagnostic lost'
        Assert-True ($script:killed -and $script:waited) 'timed-out process was leaked'
        Assert-True (-not (Test-Path (Join-Path $root 'aw-timeout-guard.cmd'))) 'wrapper leaked'
        Assert-True (-not (Test-Path (Join-Path $root 'aw-timeout-guard.log'))) 'log leaked'
        Assert-True ($scenario['log_1234'] -contains 'startup diagnostic') 'cleanup discarded evidence'
    }

    & {
        $scenario = @{}
        $script:clock = [datetime]'2026-01-01'
        $script:reads = 0
        function Get-Date { return $script:clock }
        function Start-Sleep { param($Seconds) $script:clock = $script:clock.AddSeconds($Seconds) }
        function Invoke-RestMethod {
            param($Uri, $TimeoutSec)
            if ($Uri -notlike '*/events*') { return [pscustomobject]@{ 'aw-guard' = 1 } }
            $script:reads++
            if ($script:reads -lt 3) { throw 'bucket not found' }
            return @([pscustomobject]@{ data = [pscustomobject]@{ sentinel = 'A-1' } })
        }
        $got = Wait-ScenarioSentinels -Port 1234 -Expected @('A-1') -Bucket 'aw-guard'
        Assert-True ((($got -join ',') -eq 'A-1') -and $script:reads -eq 3) 'sentinel polling aborted on a transient read failure'
        function Invoke-RestMethod { param($Uri, $TimeoutSec) throw 'bucket never appeared' }
        $caught = $null
        try { $null = Wait-ScenarioSentinels -Port 1234 -Expected @('A-1') -Bucket 'aw-guard' -TimeoutSeconds 5 }
        catch { $caught = $_.Exception.Message }
        Assert-True ($caught -like '*never appeared*') 'persistent sentinel read failure was reported as empty'
    }

    & {
        $scenario = @{}
        foreach ($case in @('zero', 'inserted', 'skipped', 'missing')) {
            $inserted = if ($case -eq 'inserted') { 1 } else { 0 }
            $skipped = if ($case -eq 'skipped') { 3 } else { 0 }
            $scenario['log_1234'] = if ($case -eq 'missing') { @('unrelated log') } else {
                @("INFO History recovery from source: 0 buckets created, $inserted events inserted, $skipped duplicates skipped")
            }
            $caught = $null
            try { Assert-NoRecoveryWork -Port 1234 }
            catch { $caught = $_.Exception.Message }
            Assert-True (($null -eq $caught) -eq ($case -eq 'zero')) "incorrect zero-work verdict for $case"
        }
        $scenario['log_1234'] = @('INFO History recovery from source: 0 buckets created, 0 events inserted, 3 duplicates skipped')
        Assert-NoRecoveryWork -Port 1234 -AllowSkipped
    }
    & {
        $rowPath = Join-Path $PSScriptRoot 'windows-recovery-scenarios/row1-local-older-roaming-newer.ps1'
        $rowAst = [System.Management.Automation.Language.Parser]::ParseFile($rowPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count) { throw ($errors | Out-String) }
        $preservation = $rowAst.Find({ param($node)
            $node -is [System.Management.Automation.Language.ForEachStatementAst] -and
            $node.Extent.Text -like '*pre-migration-*'
        }, $true)
        if ($null -eq $preservation) { throw 'missing original-tree assertions' }
        $disposableProfile = $root
        $localApp = Join-Path $root 'Local/aw-server-rust'
        $roamApp = Join-Path $root 'Roaming/aw-server-rust'
        $localBackup = "$localApp.pre-migration-test"
        $roamBackup = "$roamApp.migrated-test"
        foreach ($dir in @($localApp, $roamApp, $localBackup, $roamBackup)) {
            New-Item -ItemType Directory -Path $dir -Force | Out-Null
            Set-Content -LiteralPath (Join-Path $dir 'sqlite.db') -Value 'fixture'
        }
        $scenario = @{}
        . ([scriptblock]::Create($preservation.Extent.Text))
        foreach ($backup in @($localBackup, $roamBackup)) {
            $db = Join-Path $backup 'sqlite.db'
            Remove-Item -LiteralPath $db
            $caught = $null
            try { . ([scriptblock]::Create($preservation.Extent.Text)) }
            catch { $caught = $_.Exception.Message }
            Assert-True ($null -ne $caught -and $caught -like '*not retained*') 'active database masked a missing original'
            Set-Content -LiteralPath $db -Value 'fixture'
        }
    }
    Write-Host 'PASS: HTTP readiness gates fixtures, startup timeout cleans its process tree and evidence, sentinel polling retries transient reads, restart counts fail closed, both originals required'
}
finally {
    $env:Public = $savedPublic
    $env:SystemRoot = $savedSystemRoot
    Remove-Item -LiteralPath $root -Recurse -Force
}
