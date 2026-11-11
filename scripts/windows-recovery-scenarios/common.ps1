# Shared helpers for the Windows recovery scenarios. Dot-sourced by each
# scenario after it defines $bucketId and $scenario. Context from the isolation
# script: $exe $credential $disposableProfile $serverProcess $port $runId.
# Fixtures are created by the real server (schema stays migration-owned);
# --dbpath is used for fixture creation only, never for the assertion runs.

function Wait-ScenarioSentinels {
    # Poll until the bucket holds exactly the expected sentinels (recovery and
    # import can finish after the HTTP API is already answering).
    param([int]$Port, [string[]]$Expected, [string]$Bucket = $bucketId, [int]$TimeoutSeconds = 60)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $got = Get-FixtureSentinels -Port $Port -Bucket $Bucket
        if (($got -join ',') -eq ($Expected -join ',')) { break }
        Start-Sleep -Seconds 2
    } while ((Get-Date) -lt $deadline)
    return $got
}

function Stop-FixtureProcess {
    # Kill the isolation script's default-profile server (fixture phase).
    param($Process)
    & taskkill.exe /PID $Process.Id /T /F 2>$null | Out-Null
    $null = $Process.WaitForExit(15000)
}

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
    param([int]$Port, [string]$Tag, [string]$Day, [string]$Bucket = $bucketId)
    $base = "http://127.0.0.1:$Port/api/0/buckets/$Bucket"
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
    param([int]$Port, [string]$Dir, [string]$Name, [string]$Db = 'sqlite.db')
    $body = ConvertTo-Json -Compress -InputObject @{ client = 'recovery-test'; type = 'test'; hostname = 'recovery-host' }
    Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/api/0/buckets/aw-recovery-flush-$Name" `
        -ContentType 'application/json' -Body $body | Out-Null
    $wal = Join-Path $Dir "$Db-wal"
    $len = if (Test-Path -LiteralPath $wal) { (Get-Item -LiteralPath $wal).Length } else { 0 }
    $scenario["wal_bytes_$Name"] = $len
    Assert-True ($len -gt 0) "$Name fixture has no committed WAL content to recover from"
}

function Get-FixtureSentinels {
    param([int]$Port, [string]$Bucket = $bucketId)
    $events = @(Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/0/buckets/$Bucket/events?limit=1000" -TimeoutSec 15)
    # PS 5.1 turns an empty JSON array into $null; drop it before StrictMode
    # property access, and keep a bucket inventory for the evidence file.
    $buckets = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/0/buckets/" -TimeoutSec 15
    $scenario["buckets_$Port"] = @($buckets.PSObject.Properties.Name)
    return @($events | Where-Object { $null -ne $_ } | ForEach-Object { $_.data.sentinel } | Sort-Object)
}
