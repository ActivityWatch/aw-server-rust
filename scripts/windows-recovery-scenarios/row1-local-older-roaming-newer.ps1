# Recovery matrix row 1: existing Local history, newer v0.14.0 Roaming history.
# Dot-sourced by test-windows-disposable-profile.ps1 after isolation is proven.
# Context: $exe $User $credential $disposableProfile $serverProcess $port
#          $runId $evidence.
# Helpers live in common.ps1.

$bucketId = 'aw-recovery-row1'
$localApp = Join-Path $disposableProfile 'AppData\Local\activitywatch\aw-server-rust'
$roamApp = Join-Path $disposableProfile 'AppData\Roaming\activitywatch\aw-server-rust'
$scenario = [ordered]@{ row = 'local-older-roaming-newer' }
$evidence['scenario'] = $scenario

. (Join-Path $PSScriptRoot 'common.ps1')

$expected = @('LOCAL-1', 'LOCAL-2', 'LOCAL-3', 'ROAM-1', 'ROAM-2', 'ROAM-3')
$server = $null
try {
    # 1. Local history (older): the isolation proof's default-profile server
    #    already created the Local datastore. Write through it, then kill it.
    Add-FixtureEvents -Port $port -Tag 'LOCAL' -Day '2026-01-01'
    Wait-FixtureCommitted -Port $port -Dir $localApp -Name 'local'
    & taskkill.exe /PID $serverProcess.Id /T /F 2>$null | Out-Null
    $null = $serverProcess.WaitForExit(15000)
    $scenario['local_dir_after_fixture'] = @(Get-ChildItem -LiteralPath $localApp -Name -ErrorAction SilentlyContinue)

    # 2. Roaming history (newer), colliding autoincrement ids, WAL-only.
    New-Item -ItemType Directory -Path $roamApp -Force | Out-Null
    $roamPort = $port + 1
    $server = Start-ScenarioServer -Name 'roam-fixture' -Port $roamPort -ExtraArgs ('--dbpath "{0}"' -f (Join-Path $roamApp 'sqlite.db'))
    Add-FixtureEvents -Port $roamPort -Tag 'ROAM' -Day '2026-06-01'
    Wait-FixtureCommitted -Port $roamPort -Dir $roamApp -Name 'roaming'
    Stop-ScenarioServer $server
    $server = $null
    Assert-True (Test-Path -LiteralPath (Join-Path $roamApp 'sqlite.db')) 'Roaming fixture database was not created'
    $scenario['roaming_dir_after_fixture'] = @(Get-ChildItem -LiteralPath $roamApp -Name)

    # 3. Real default-profile start must pick Roaming (newest event), migrate
    #    it and union the Local history into the selected database.
    $server = Start-ScenarioServer -Name 'assert1' -Port ($port + 2) -ExtraArgs '--profile default'
    $deadline = (Get-Date).AddSeconds(60)
    do {
        $got = Get-FixtureSentinels -Port ($port + 2)
        if (($got -join ',') -eq ($expected -join ',')) { break }
        Start-Sleep -Seconds 2
    } while ((Get-Date) -lt $deadline)
    $scenario['sentinels_first_start'] = $got
    Assert-True (($got -join ',') -eq ($expected -join ',')) `
        "first start: expected union $($expected -join ',') but found $($got -join ',')"
    $scenario['roaming_tree_after_recovery'] = @(Get-ChildItem -LiteralPath (Split-Path -Parent $roamApp) -Name -ErrorAction SilentlyContinue)
    $scenario['local_tree_after_recovery'] = @(Get-ChildItem -LiteralPath (Split-Path -Parent $localApp) -Name -ErrorAction SilentlyContinue)
    Stop-ScenarioServer $server
    $server = $null

    # 4. Fresh process restart: same content, no duplicates, no re-union.
    $server = Start-ScenarioServer -Name 'assert2' -Port ($port + 3) -ExtraArgs '--profile default'
    $got = Get-FixtureSentinels -Port ($port + 3)
    $scenario['sentinels_restart'] = $got
    Assert-True (($got -join ',') -eq ($expected -join ',')) `
        "restart: expected $($expected -join ',') but found $($got -join ',')"
    Stop-ScenarioServer $server
    $server = $null

    # 5. Original Roaming history must still exist somewhere under Roaming.
    $preserved = @(Get-ChildItem -LiteralPath (Join-Path $disposableProfile 'AppData\Roaming\activitywatch') `
        -Recurse -File -Filter 'sqlite*.db' -ErrorAction SilentlyContinue)
    $scenario['preserved_roaming_databases'] = @($preserved | ForEach-Object { $_.FullName.Substring($disposableProfile.Length) })
    Assert-True ($preserved.Count -ge 1) 'original Roaming database is not retained under Roaming'
    $scenario['result'] = 'pass'
}
catch {
    $scenario['result'] = "fail: $($_.Exception.Message)"
    throw
}
finally {
    if ($null -ne $server) { Stop-ScenarioServer $server }
}
