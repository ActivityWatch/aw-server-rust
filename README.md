aw-server-rust
==============

[![Build Status](https://github.com/ActivityWatch/aw-server-rust/workflows/Build/badge.svg?branch=master)](https://github.com/ActivityWatch/aw-server-rust/actions?query=workflow%3ABuild+branch%3Amaster)
[![Coverage Status](https://codecov.io/gh/ActivityWatch/aw-server-rust/branch/master/graph/badge.svg)](https://codecov.io/gh/ActivityWatch/aw-server-rust)
[![Dependency Status](https://deps.rs/repo/github/activitywatch/aw-server-rust/status.svg)](https://deps.rs/repo/github/activitywatch/aw-server-rust)

A reimplementation of aw-server in Rust.

Caveats:

 - Lots of TODO and FIXME comments

Features missing compared to the Python implementation of aw-server:

 - API explorer (Swagger/OpenAPI)

### How to compile

Install rust nightly with `rustup`:

```
rustup default nightly
```

Build with `cargo`:

```
cargo build --release 
```

Your built executable will be located in `./target/release/aw-server-rust`


### How to run

If you want to quick-compile for debugging, run cargo run from the project root:

```
cargo run --bin aw-server
```

*NOTE:* This will start aw-server-rust in testing mode (on port 5666 instead of port 5600).
