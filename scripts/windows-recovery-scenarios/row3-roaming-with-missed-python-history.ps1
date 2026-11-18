# Recovery matrix row 3: v0.14.0 Roaming history plus a Python aw-server
# database (%LOCALAPPDATA%\activitywatch\activitywatch\aw-server\
# peewee-sqlite.v2.db) that v0.14.0's first-start import never read.
# Dot-sourced by test-windows-disposable-profile.ps1 after isolation is proven.
# Context: $exe $User $credential $disposableProfile $serverProcess $port
#          $runId $evidence. Helpers live in common.ps1.
#
# Expected: Roaming is migrated to Local, the missed Python import runs once on
# that first default-profile start, and a fresh-process restart neither re-runs
# it nor duplicates events.
#
# Limit: the Python database is a peewee schema (bucketmodel/eventmodel, the
# tables aw-datastore's importer reads) created with the runner's Python
# sqlite3, not by a Python aw-server. Its journal mode is the default rollback
# journal, so no WAL-only Python events are tested.

$bucketId = 'aw-recovery-row3'
$pythonBucket = 'aw-recovery-row3-python'
$localApp = Join-Path $disposableProfile 'AppData\Local\activitywatch\aw-server-rust'
$roamApp = Join-Path $disposableProfile 'AppData\Roaming\activitywatch\aw-server-rust'
$pythonDir = Join-Path $disposableProfile 'AppData\Local\activitywatch\activitywatch\aw-server'
$pythonDb = Join-Path $pythonDir 'peewee-sqlite.v2.db'
$scenario = [ordered]@{ row = 'roaming-with-missed-python-history' }
$evidence['scenario'] = $scenario
. (Join-Path $PSScriptRoot 'common.ps1')

$expectedRoam = @('ROAM-1', 'ROAM-2', 'ROAM-3')
$expectedPython = @('PY-1', 'PY-2', 'PY-3')
$importMarker = 'Database was migrated from the v0.14.0 location'
$server = $null
try {
    # 1. Only Roaming holds a Rust database, as for a user who ran v0.14.0: drop
    #    the empty Local datastore the isolation proof's server created.
    Stop-FixtureProcess $serverProcess
    # The killed server's file handles can linger briefly on Windows.
    for ($attempt = 1; $attempt -le 10 -and (Test-Path -LiteralPath $localApp); $attempt++) {
        Remove-Item -LiteralPath $localApp -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $localApp) { Start-Sleep -Seconds 1 }
    }
    Assert-True (-not (Test-Path -LiteralPath $localApp)) 'could not clear the Local datastore before the Roaming fixture'

    # 2. Roaming history, created by the real server, WAL-only.
    New-Item -ItemType Directory -Path $roamApp -Force | Out-Null
    $server = Start-ScenarioServer -Name 'roam-fixture' -Port ($port + 1) `
        -ExtraArgs ('--dbpath "{0}"' -f (Join-Path $roamApp 'sqlite.db'))
    Add-FixtureEvents -Port ($port + 1) -Tag 'ROAM' -Day '2026-06-01'
    Wait-FixtureCommitted -Port ($port + 1) -Dir $roamApp -Name 'roaming'
    Stop-ScenarioServer $server
    $server = $null

    # 3. The Python database v0.14.0 missed (colliding integer ids 1..3).
    $python = @('python', 'py') | ForEach-Object { Get-Command $_ -ErrorAction SilentlyContinue } |
        Select-Object -First 1
    Assert-True ($null -ne $python) 'no python on PATH to create the Python aw-server fixture database'
    New-Item -ItemType Directory -Path $pythonDir -Force | Out-Null
    $builder = Join-Path $env:Public "aw-pyfixture-$runId.py"
    Set-Content -LiteralPath $builder -Encoding ascii -Value @'
import json, sqlite3, sys
path, bucket = sys.argv[1], sys.argv[2]
con = sqlite3.connect(path)
con.execute("CREATE TABLE bucketmodel (key INTEGER PRIMARY KEY, id TEXT, created TEXT, name TEXT, type TEXT, client TEXT, hostname TEXT)")
con.execute("CREATE TABLE eventmodel (id INTEGER PRIMARY KEY, bucket_id INTEGER, timestamp TEXT, duration REAL, datastr TEXT)")
con.execute("INSERT INTO bucketmodel (key, id, created, name, type, client, hostname) VALUES (1, ?, '2025-12-01T00:00:00+00:00', NULL, 'test', 'recovery-test', 'recovery-host')", (bucket,))
for i in (1, 2, 3):
    con.execute("INSERT INTO eventmodel (id, bucket_id, timestamp, duration, datastr) VALUES (?, 1, ?, 1.0, ?)",
                (i, "2025-12-01 0%d:00:00+00:00" % i, json.dumps({"sentinel": "PY-%d" % i})))
con.commit()
con.close()
'@
    & $python.Source $builder $pythonDb $pythonBucket
    Assert-True ($LASTEXITCODE -eq 0) 'Python fixture builder failed'
    Remove-Item -LiteralPath $builder -Force -ErrorAction SilentlyContinue
    Assert-True (Test-Path -LiteralPath $pythonDb) 'Python fixture database was not created'
    $scenario['python_fixture_bytes'] = (Get-Item -LiteralPath $pythonDb).Length

    # 4. First real default-profile start: migrate Roaming, import Python once.
    $server = Start-ScenarioServer -Name 'assert1' -Port ($port + 2) -ExtraArgs '--profile default'
    $gotRoam = Wait-ScenarioSentinels -Port ($port + 2) -Expected $expectedRoam
    $gotPy = Wait-ScenarioSentinels -Port ($port + 2) -Expected $expectedPython -Bucket $pythonBucket
    $scenario['sentinels_first_roaming'] = $gotRoam
    $scenario['sentinels_first_python'] = $gotPy
    Assert-True (($gotRoam -join ',') -eq ($expectedRoam -join ',')) `
        "first start: expected Roaming $($expectedRoam -join ',') but found $($gotRoam -join ',')"
    Assert-True (($gotPy -join ',') -eq ($expectedPython -join ',')) `
        "first start: expected missed Python history $($expectedPython -join ',') but found $($gotPy -join ',')"
    Stop-ScenarioServer $server
    $server = $null
    $firstLog = @($scenario["log_$($port + 2)"])
    Assert-True (@($firstLog | Where-Object { $_ -like "*$importMarker*" }).Count -ge 1) `
        'first start did not log the post-migration Python import'

    # 5. Fresh-process restart: same content, no duplicates, import not repeated.
    $server = Start-ScenarioServer -Name 'assert2' -Port ($port + 3) -ExtraArgs '--profile default'
    $gotRoam = Get-FixtureSentinels -Port ($port + 3)
    $gotPy = Get-FixtureSentinels -Port ($port + 3) -Bucket $pythonBucket
    $scenario['sentinels_restart_roaming'] = $gotRoam
    $scenario['sentinels_restart_python'] = $gotPy
    Assert-True (($gotRoam -join ',') -eq ($expectedRoam -join ',')) `
        "restart: expected Roaming $($expectedRoam -join ',') but found $($gotRoam -join ',')"
    Assert-True (($gotPy -join ',') -eq ($expectedPython -join ',')) `
        "restart: expected Python $($expectedPython -join ',') but found $($gotPy -join ',')"
    Stop-ScenarioServer $server
    $server = $null
    $restartLog = @($scenario["log_$($port + 3)"])
    Assert-True (@($restartLog | Where-Object { $_ -like "*$importMarker*" }).Count -eq 0) `
        'restart repeated the one-shot post-migration Python import'

    # 6. Originals retained: Roaming tree and the Python database itself.
    $preserved = @(Get-ChildItem -LiteralPath (Join-Path $disposableProfile 'AppData\Roaming\activitywatch') `
        -Recurse -File -Filter 'sqlite*.db' -ErrorAction SilentlyContinue)
    $scenario['preserved_roaming_databases'] = @($preserved | ForEach-Object { $_.FullName.Substring($disposableProfile.Length) })
    Assert-True ($preserved.Count -ge 1) 'original Roaming database is not retained under Roaming'
    Assert-True (Test-Path -LiteralPath $pythonDb) 'the Python database must be left in place'
    $scenario['result'] = 'pass'
}
catch {
    $scenario['result'] = "fail: $($_.Exception.Message)"
    throw
}
finally {
    if ($null -ne $server) { Stop-ScenarioServer $server }
}
