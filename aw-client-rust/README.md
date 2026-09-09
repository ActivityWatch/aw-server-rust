aw-client-rust
==============

WIP: aw-client implementation in Rust

`AwClient::wait_for_start()` is asynchronous: call
`client.wait_for_start().await?` inside a Tokio runtime with networking and timers
enabled. It waits for TCP connectivity, with a ten-second total deadline covering
DNS resolution, connection attempts, and retry delays. It does not check HTTP readiness.
`blocking::AwClient::wait_for_start()?` remains synchronous.
