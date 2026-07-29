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

### Docker

A multi-stage `Dockerfile` is included. It builds the web UI and the server, and
produces a slim Debian-based image containing only the `aw-server` binary (the
web UI assets are embedded into it at compile time).

The `aw-webui` submodule must be initialized before building:

```sh
git submodule update --init --recursive
docker build -t aw-server-rust:local .
```

Run it with:

```sh
docker run -d --name aw-server-rust \
    -p 5600:5600 \
    -v aw-server-rust-data:/root/.local/share/activitywatch \
    aw-server-rust:local aw-server-rust --host 0.0.0.0
```

Notes:

 - The server binds to `127.0.0.1` by default, which is unreachable from outside
   the container. Pass `--host 0.0.0.0` (as above) to make it reachable.
 - The database lives in `/root/.local/share/activitywatch/aw-server-rust`.
   Mount a volume there to keep it across container recreations.
 - The image only ships `aw-server`. The `aw-sync` binary is deliberately not
   built, since on Linux it pulls in OpenSSL with the `vendored` feature and
   compiles it from source.
 - `docker build` produces an image for the architecture of the machine running
   it. To target a different one, use
   `docker buildx build --platform linux/amd64 ...`.

### Configuration

The server reads its configuration from `~/.config/activitywatch/aw-server-rust/config.toml` (or `config-testing.toml` in testing mode).

Available options:

```toml
# Address to listen on
#address = "127.0.0.1"

# Port to listen on (default: 5600, testing: 5666)
#port = 5600

# Additional exact CORS origins to allow (e.g. for custom web interfaces)
#cors = ["http://localhost:3000"]

# Additional regex CORS origins to allow (e.g. for sideloaded browser extensions)
#cors_regex = ["chrome-extension://yourextensionidhere"]
```

#### Custom CORS Origins

By default, the server allows requests from:
- The server's own origin (`http://127.0.0.1:<port>`, `http://localhost:<port>`)
- The official Chrome extension (`chrome-extension://nglaklhklhcoonedhgnpgddginnjdadi`)
- All Firefox extensions (`moz-extension://.*`)

To allow additional origins (e.g. a sideloaded Chrome extension), add them to your config:

```toml
# Allow a specific sideloaded Chrome extension
cors_regex = ["chrome-extension://jmdbkmbphoikckgkcnpoojbfeiaoaocl"]

# Or allow all Chrome extensions (less secure, but convenient for development)
cors_regex = ["chrome-extension://.*"]
```

### Syncing

For details about aw-sync-rust, see the [README](./aw-sync/README.md) in its subdirectory.
