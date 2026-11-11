# Recovery matrix row 2: stale Local history that also holds a saved
# `testing`-profile database (sqlite-testing.db), newer v0.14.0 Roaming history.
# Dot-sourced by test-windows-disposable-profile.ps1 after isolation is proven.
# Context: $exe $User $credential $disposableProfile $serverProcess $port
#          $runId $evidence. Helpers live in common.ps1.
#
# Expected: the default profile selects Roaming (newer events) and unions the
# stale Local default history; the Local testing database must not be lost, so a
# `--profile testing` start recovers it from the preserved Local tree.

$bucketId = 'aw-recovery-row2'
$localApp = Join-Path $disposableProfile 'AppData\Local\activitywatch\aw-server-rust'
$roamApp = Join-Path $disposableProfile 'AppData\Roaming\activitywatch\aw-server-rust'
$scenario = [ordered]@{ row = 'stale-local-testing-newer-roaming' }
$evidence['scenario'] = $scenario
. (Join-Path $PSScriptRoot 'common.ps1')

$expected = @('LOCAL-1', 'LOCAL-2', 'LOCAL-3', 'ROAM-1', 'ROAM-2', 'ROAM-3')
$expectedTesting = @('TEST-1', 'TEST-2', 'TEST-3')
$server = $null
try {
    # 1. Stale Local default history through the isolation proof's server.
    Add-FixtureEvents -Port $port -Tag 'LOCAL' -Day '2026-01-01'
    Wait-FixtureCommitted -Port $port -Dir $localApp -Name 'local'
    Stop-FixtureProcess $serverProcess

    # 2. Saved testing-profile history in the same Local directory (legacy
    #    shared layout: sqlite-testing.db beside sqlite.db). Fixture only.
    $server = Start-ScenarioServer -Name 'testing-fixture' -Port ($port + 1) `
        -ExtraArgs ('--profile default --dbpath "{0}"' -f (Join-Path $localApp 'sqlite-testing.db'))
    Add-FixtureEvents -Port ($port + 1) -Tag 'TEST' -Day '2026-02-01'
    Wait-FixtureCommitted -Port ($port + 1) -Dir $localApp -Name 'testing' -Db 'sqlite-testing.db'
    Stop-ScenarioServer $server
    $server = $null
    Assert-True (Test-Path -LiteralPath (Join-Path $localApp 'sqlite-testing.db')) 'Local testing fixture was not created'
    $scenario['local_dir_after_fixture'] = @(Get-ChildItem -LiteralPath $localApp -Name -ErrorAction SilentlyContinue)

    # 3. Newer Roaming default history, colliding ids, WAL-only.
    New-Item -ItemType Directory -Path $roamApp -Force | Out-Null
    $server = Start-ScenarioServer -Name 'roam-fixture' -Port ($port + 2) `
        -ExtraArgs ('--profile default --dbpath "{0}"' -f (Join-Path $roamApp 'sqlite.db'))
    Add-FixtureEvents -Port ($port + 2) -Tag 'ROAM' -Day '2026-06-01'
    Wait-FixtureCommitted -Port ($port + 2) -Dir $roamApp -Name 'roaming'
    Stop-ScenarioServer $server
    $server = $null

    # 4. Default start: Roaming selected, stale Local unioned. Debug builds
    #    default to the `testing` profile, which would have created the
    #    new-layout `activitywatch-testing` root during fixture creation; that
    #    root takes precedence over legacy files and would hide the scenario
    #    (CI run 42962833628). The fixture servers therefore run as `default`;
    #    prove they left no such root before asserting.
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $disposableProfile 'AppData\Local\activitywatch-testing'))) `
        'fixture creation made the new-layout testing root; the legacy testing scenario would be hidden'
    $server = Start-ScenarioServer -Name 'assert-default1' -Port ($port + 3) -ExtraArgs '--profile default'
    $got = Wait-ScenarioSentinels -Port ($port + 3) -Expected $expected
    $scenario['sentinels_default_first'] = $got
    Assert-True (($got -join ',') -eq ($expected -join ',')) `
        "default first start: expected $($expected -join ',') but found $($got -join ',')"
    $scenario['local_tree_after_recovery'] = @(Get-ChildItem -LiteralPath (Split-Path -Parent $localApp) -Name -ErrorAction SilentlyContinue)
    Stop-ScenarioServer $server
    $server = $null

    # 5. Saved testing history must be recovered by a fresh testing-profile
    #    process, then stay stable across a second one.
    foreach ($n in 1, 2) {
        $server = Start-ScenarioServer -Name "assert-testing$n" -Port ($port + 3 + $n) -ExtraArgs '--profile testing'
        $got = Wait-ScenarioSentinels -Port ($port + 3 + $n) -Expected $expectedTesting
        $scenario["sentinels_testing_$n"] = $got
        Assert-True (($got -join ',') -eq ($expectedTesting -join ',')) `
            "testing start ${n}: expected $($expectedTesting -join ',') but found $($got -join ',')"
        Stop-ScenarioServer $server
        $server = $null
    }

    # 6. Default history unchanged after the testing recovery.
    $server = Start-ScenarioServer -Name 'assert-default2' -Port ($port + 6) -ExtraArgs '--profile default'
    $got = Get-FixtureSentinels -Port ($port + 6)
    $scenario['sentinels_default_restart'] = $got
    Assert-True (($got -join ',') -eq ($expected -join ',')) `
        "default restart: expected $($expected -join ',') but found $($got -join ',')"
    Stop-ScenarioServer $server
    $server = $null

    # 7. Original trees retained.
    $preserved = @(Get-ChildItem -LiteralPath (Join-Path $disposableProfile 'AppData') -Recurse -File `
        -Filter 'sqlite*.db' -ErrorAction SilentlyContinue)
    $scenario['preserved_databases'] = @($preserved | ForEach-Object { $_.FullName.Substring($disposableProfile.Length) })
    Assert-True (@($preserved | Where-Object { $_.FullName -like '*\Roaming\*' }).Count -ge 1) `
        'original Roaming database is not retained under Roaming'
    $scenario['result'] = 'pass'
}
catch {
    $scenario['result'] = "fail: $($_.Exception.Message)"
    throw
}
finally {
    if ($null -ne $server) { Stop-ScenarioServer $server }
}
