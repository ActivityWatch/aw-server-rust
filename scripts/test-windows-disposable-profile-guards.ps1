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
    $loop = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.WhileStatementAst] }, $true)
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
    Write-Host 'PASS: existing profile/account preserved, refusal evidence retained, SQLite header readiness retried'
}
finally {
    foreach ($name in $originalEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name])
    }
    Remove-Item -LiteralPath $root -Recurse -Force
}
