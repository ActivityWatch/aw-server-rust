# Recovery matrix row 1: existing Local history, newer v0.14.0 Roaming history.
# Dot-sourced by test-windows-disposable-profile.ps1 after isolation is proven.
# Context: $exe $User $credential $disposableProfile $serverProcess $port
#          $runId $evidence.
# Fixtures are created by the real server (schema stays migration-owned);
# --dbpath is used for fixture creation only, never for the assertion runs.

$bucketId = 'aw-recovery-row1'
$localApp = Join-Path $disposableProfile 'AppData\Local\activitywatch\aw-server-rust'
$roamApp = Join-Path $disposableProfile 'AppData\Roaming\activitywatch\aw-server-rust'
$scenario = [ordered]@{ row = 'local-older-roaming-newer' }
$evidence['scenario'] = $scenario

function Start-ScenarioServer {
    param([string]$Name, [int]$Port, [string]$ExtraArgs)
    $log = Join-Path $env:Public "aw-$Name-$runId.log"
    $cmd = Join-Path $env:Public "aw-$Name-$runId.cmd"
    $line = '"{0}" --host 127.0.0.1 --port {1} {2} --verbose >> "{3}" 2>&1' -f $exe, $Port, $ExtraArgs, $log
    Set-Content -LiteralPath $cmd -Value @('@echo off', $line) -Encoding ascii
    $proc = Start-Process -FilePath "$env:SystemRoot\System32\cmd.exe" `
        -ArgumentList @('/d', '/s', '/c', ('""{0}""' -f $cmd)) `
        -Credential $credential -LoadUserProfile -UseNewEnvironment `
        -WorkingDirectory $env:SystemRoot -PassThru
    $deadline = (Get-Date).AddSeconds(120)
    while ((Get-Date) -lt $deadline) {
        if ($proc.HasExited) { break }
        try {
            $null = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/0/info" -TimeoutSec 3
            return [pscustomobject]@{ Process = $proc; Log = $log; Cmd = $cmd; Port = $Port }
        }
        catch { Start-Sleep -Milliseconds 500 }
    }
    $tail = if (Test-Path -LiteralPath $log) { (Get-Content -LiteralPath $log -Tail 30) -join "`n" } else { '(no log)' }
    throw "scenario server '$Name' did not become ready on port $Port (exited: $($proc.HasExited)).`n$tail"
}

function Stop-ScenarioServer {
    param($Server)
    # taskkill /F is deliberate: committed events stay WAL-only, unflushed.
    & taskkill.exe /PID $Server.Process.Id /T /F 2>$null | Out-Null
    $null = $Server.Process.WaitForExit(15000)
    $scenario["log_$($Server.Port)"] = @(Get-Content -LiteralPath $Server.Log -ErrorAction SilentlyContinue)
    Remove-Item -LiteralPath $Server.Log, $Server.Cmd -Force -ErrorAction SilentlyContinue
}

function Add-FixtureEvents {
    param([int]$Port, [string]$Tag, [string]$Day)
    $base = "http://127.0.0.1:$Port/api/0/buckets/$bucketId"
    $body = ConvertTo-Json -Compress -InputObject @{ client = 'recovery-test'; type = 'test'; hostname = 'recovery-host' }
    try { Invoke-RestMethod -Method Post -Uri $base -ContentType 'application/json' -Body $body | Out-Null }
    catch { if ($_.Exception.Response.StatusCode.value__ -ne 304) { throw } }
    $events = @(1..3 | ForEach-Object {
        @{ timestamp = "$Day`T0$_`:00:00+00:00"; duration = 1; data = @{ sentinel = "$Tag-$_" } }
    })
    Invoke-RestMethod -Method Post -Uri "$base/events" -ContentType 'application/json' `
        -Body (ConvertTo-Json -Compress -Depth 5 -InputObject $events) | Out-Null
}

function Wait-FixtureCommitted {
    # The datastore worker blocks on its request queue and only evaluates the
    # 15 s batch window after handling a request (worker.rs), so idle waiting
    # never commits queued inserts. Creating a bucket is a committing command:
    # it flushes everything queued before it. Require a non-empty WAL as
    # evidence of committed-but-uncheckpointed state before the kill.
    param([int]$Port, [string]$Dir, [string]$Name)
    $body = ConvertTo-Json -Compress -InputObject @{ client = 'recovery-test'; type = 'test'; hostname = 'recovery-host' }
    Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/api/0/buckets/aw-recovery-flush-$Name" `
        -ContentType 'application/json' -Body $body | Out-Null
    $wal = Join-Path $Dir 'sqlite.db-wal'
    $len = if (Test-Path -LiteralPath $wal) { (Get-Item -LiteralPath $wal).Length } else { 0 }
    $scenario["wal_bytes_$Name"] = $len
    Assert-True ($len -gt 0) "$Name fixture has no committed WAL content to recover from"
}

function Get-FixtureSentinels {
    param([int]$Port)
    $events = @(Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/0/buckets/$bucketId/events?limit=1000" -TimeoutSec 15)
    # PS 5.1 turns an empty JSON array into $null; drop it before StrictMode
    # property access, and keep a bucket inventory for the evidence file.
    $buckets = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/0/buckets/" -TimeoutSec 15
    $scenario["buckets_$Port"] = @($buckets.PSObject.Properties.Name)
    return @($events | Where-Object { $null -ne $_ } | ForEach-Object { $_.data.sentinel } | Sort-Object)
}

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
