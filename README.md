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

### Device-addressed API (draft)

Device-scoped bucket/event/query operations live under
`/api/0/devices/{device_id}/...`. `local` and this server's `/info.device_id`
select the same local history. Legacy name-only routes remain supported.

- `GET /api/0/devices` lists the local UUID even when empty, plus stored peer
  identities. A peer hostname is a deterministic representative of its bucket
  metadata, not a latest-hostname registry.
- `GET /api/0/devices/buckets` returns metadata as
  `{"devices": {"<source-device-id>": {"buckets": {"<bucket-id>": {}}}}}`.
- `GET /api/0/devices/export` uses the same grouping with event arrays. The local
  sentinel is exported as the source server UUID. Events stream to a private
  tempfile in one datastore transaction before a successful response begins.
- `POST /api/0/devices/import` accepts that grouped JSON. Every bucket ID and
  ownership field must match its enclosing keys. Empty and `local` source IDs
  are rejected before writes. Runtime errors may leave earlier groups imported;
  retry uses existing event deduplication. Importing another server's export does
  not turn its local history into this server's local history.

Explicitly scoped creation supports equal bucket names on different devices;
reload, rename and legacy Android-name migration retain their separate histories.
Legacy name-only lookup, flat listing and export fail with a conflict when names
are ambiguous. Globally unique compatibility aliases still resolve their device;
creating or renaming an exact name cannot take over an active peer alias. Legacy
unscoped creation retains its cross-device name guard. These endpoints do not
enable watcher rollout or V2 segment import. All routes retain the normal host,
CORS and configured API-key checks.

### Docker

A multi-stage `scripts/Dockerfile` is included. It builds the web UI and the
server, and produces a slim Debian-based image containing only the `aw-server`
binary (the web UI assets are embedded into it at compile time).

The `aw-webui` submodule must be initialized before building:

```sh
git submodule update --init --recursive
docker build -f scripts/Dockerfile -t aw-server-rust:local .
```

Run it with:

```sh
docker run -d --name aw-server-rust \
    -p 5600:5600 \
    -v aw-server-rust-data:/home/aw/.local/share/activitywatch \
    aw-server-rust:local aw-server-rust --host 0.0.0.0
```

Notes:

 - The server binds to `127.0.0.1` by default, which is unreachable from outside
   the container. Pass `--host 0.0.0.0` (as above) to make it reachable.
 - The server runs as the unprivileged user `aw` (uid/gid `10001`). When using a
   bind mount instead of a named volume, the host directory must be writable by
   that uid (`sudo chown -R 10001:10001 <dir>`).
 - The database lives in `/home/aw/.local/share/activitywatch/aw-server-rust`.
   Mount a volume there to keep it across container recreations. Images built
   before the switch to a non-root user stored it under `/root/.local/share`;
   to keep existing data, copy it over and fix its ownership, e.g.
   `docker run --rm -v aw-server-rust-data:/data alpine sh -c 'chown -R 10001:10001 /data'`.
 - The image only ships `aw-server`. The `aw-sync` binary is deliberately not
   built, since on Linux it pulls in OpenSSL with the `vendored` feature and
   compiles it from source.
 - `docker build` produces an image for the architecture of the machine running
   it. To target a different one, use
   `docker buildx build --platform linux/amd64 -f scripts/Dockerfile ...`.

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
