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

# Allow official ActivityWatch Chrome extension? (default: true)
#cors_allow_aw_chrome_extension = true

# Allow all Firefox extensions? (default: false, DANGEROUS)
#cors_allow_all_mozilla_extension = false
```

#### Persistence and Settings UI

The CORS-related settings (`cors`, `cors_regex`, `cors_allow_aw_chrome_extension`, and `cors_allow_all_mozilla_extension`) follow a precedence logic between the configuration file and the database:

- **TOML Precedence**: If a field is explicitly defined in your `config.toml`, it takes absolute precedence. The server will use the value from the file, and that setting will be **read-only** in the Web UI (marked as "Fixed in config file").
- **Database Fallback**: If a field is **missing** or commented out in the `config.toml`, the server will look for it in the database. These can be managed and edited via the **Security & CORS** modal in the Settings page.
- **Initial Setup**: On the first start, a default `config.toml` is created with all settings commented out, allowing the Web UI to take control of the configuration by default while providing a template for manual overrides.

> [!IMPORTANT]
> **Server Restart Required**: Changing any CORS-related settings (whether via `config.toml` or the Web UI) requires stopping and restarting the server for the changes to take effect. These settings are loaded into memory once during the server's initialization and are not hot-reloadable.

#### Custom CORS Origins

By default, the server allows requests from:
- The server's own origin (`http://127.0.0.1:<port>`, `http://localhost:<port>`)
- The official Chrome extension (`chrome-extension://nglaklhklhcoonedhgnpgddginnjdadi`) if `cors_allow_aw_chrome_extension` is true (default).
- All Firefox extensions (`moz-extension://.*`) ONLY IF `cors_allow_all_mozilla_extension` is set to true.

To allow additional origins (e.g. a sideloaded Chrome extension), add them to your `cors` or `cors_regex` config:

```toml
# Allow a specific sideloaded Chrome extension
cors_regex = ["chrome-extension://jmdbkmbphoikckgkcnpoojbfeiaoaocl"]

# Or allow all Chrome extensions (less secure, but convenient for development)
cors_regex = ["chrome-extension://.*"]
```

### Syncing

For details about aw-sync-rust, see the [README](./aw-sync/README.md) in its subdirectory.
