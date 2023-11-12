aw-server-rust
==============

[![Build Status](https://github.com/ActivityWatch/aw-server-rust/workflows/Build/badge.svg?branch=master)](https://github.com/ActivityWatch/aw-server-rust/actions?query=workflow%3ABuild+branch%3Amaster)
[![Coverage Status](https://codecov.io/gh/ActivityWatch/aw-server-rust/branch/master/graph/badge.svg)](https://codecov.io/gh/ActivityWatch/aw-server-rust)
[![Dependency Status](https://deps.rs/repo/github/activitywatch/aw-server-rust/status.svg)](https://deps.rs/repo/github/activitywatch/aw-server-rust)

A reimplementation of aw-server in Rust.

Features missing compared to the Python implementation of aw-server:

 - API explorer (Swagger/OpenAPI)

### How to compile

Build with `cargo`:

```sh
cargo build --release
```

You can also build with make, which will build the web assets as well:

```
make build
```

Your built executable will be located in `./target/release/aw-server-rust`. If you want to use it with a development version of `aw-qt` you'll want to copy this binary into your `venv`:

```shell
cp target/release/aw-server ../venv/bin/aw-server-rust
```


### How to run

If you want to quick-compile for debugging, run cargo run from the project root:

```sh
cargo run --bin aw-server
```

*NOTE:* This will start aw-server-rust in testing mode (on port 5666 instead of port 5600).

### Syncing

For details about aw-sync-rust, see the [README](./aw-sync/README.md) in its subdirectory.
