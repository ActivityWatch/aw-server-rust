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
    $wrapper = $null
    $serverProcess = $null
    $createdUser = [pscustomobject]@{ SID = [pscustomobject]@{ Value = 'owned-test-sid' } }
    $EvidencePath = Join-Path $root 'missing-directory/evidence.json'
    $script:deletedOwnedUser = $false
    function Get-CimInstance { return $null }
    function Remove-LocalUser {
        param($SID)
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
    Write-Host 'PASS: existing resources preserved, SQLite readiness retried, cleanup survives evidence and CIM failures'
}
finally {
    foreach ($name in $originalEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name])
    }
    Remove-Item -LiteralPath $root -Recurse -Force
}
