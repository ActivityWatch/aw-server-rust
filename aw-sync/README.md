aw-sync-rust
============

Synchronization for ActivityWatch.

Works by syncing local buckets with a special folder, which in turn should be synchronized by rsync/Syncthing/Dropbox/GDrive/etc.

Android is supported: the official app drives this same crate over JNI.

The latest beta versions of ActivityWatch ship with the `aw-sync` binary, but it's not enabled by default. You can start it from aw-qt or the command line, but due to the early state of development might not have the best UX. Please report issues and submit PRs!

Was originally prototyped as a PR to aw-server: https://github.com/ActivityWatch/aw-server/pull/50


## Usage

The command that actually pulls and pushes today is a one-shot pass. Share the sync directory first (see below), then:

```sh
# Pull every 3-level peer and push this device, then exit
aw-sync sync

# Doctor: why is pull empty / which peers exist in the folder?
# Also prints the last persisted pass (peers, events, skips).
aw-sync status

# One-off cleanup: collapse exact-duplicate events that accumulated in
# `-synced-from-` buckets from pull passes before #713. Never touches a
# host's own first-hand buckets.
aw-sync dedupe --dry-run   # report only, across every -synced-from- bucket
aw-sync dedupe --bucket aw-watcher-window-synced-from-host  # delete the extras in one bucket (lowest-id copy is kept)
# Deleting from every -synced-from- bucket at once (no --bucket) requires --dry-run
# review first: -synced-from- is an ID convention, not a reserved bucket type, so
# `--bucket` makes the buckets you're deleting from an explicit, reviewed list.
#
# Caveat: deduplication compares (timestamp, duration, data) for exact equality.
# Two genuinely distinct events that happen to share all three fields would be
# collapsed — one copy deleted. This is practically impossible for real activity
# data, but note it before running on a live instance.
```

The daemon is push-only by default. To enable pulling, set `pull = true` under the `[daemon]` section in aw-sync's `config.toml`, or pass `--mode both` to `aw-sync daemon`. When pulling from the sync-folder root, the daemon discovers both the 3-level `{hostname}/{device_id}/` layout used by Android and bare `aw-sync sync`, and the legacy 2-level `{device_id}/` layout. The daemon still writes its staging database in the 2-level layout; bare `aw-sync sync` and Android write in the 3-level layout.

```sh
# Daemon — push-only unless pulling is enabled in config or on the command line
aw-sync
aw-sync daemon --mode both
```

For more options, see `aw-sync --help`. Some notable options:
- `--config`: Read the local server port and API key from a custom aw-server config file. Pass the same path to both aw-server and aw-sync when using aw-server's `--config` override.
- `--buckets`: Specify which buckets to sync (comma-separated). By default, all buckets are synced.
  - Use `--buckets "bucket1,bucket2"` to sync specific buckets
  - Not specifying this option syncs all buckets by default
- `--start-date`: Only sync events after this date (YYYY-MM-DD)
- `--sync-db`: Specify a specific database file in the sync directory
- `--mode`: Choose sync mode: "push", "pull", or "both" (default: "both"). For the daemon, this overrides its push-only-by-default config. `aw-sync sync` with advanced options (`--mode`, `--buckets`, `--start-date`, or `--sync-db`) uses `sync_run`; without `--sync-db`, it discovers both 3-level and legacy 2-level peers, but writes to the 2-level layout. The simple `aw-sync sync` path (including `--host` without advanced options) uses the host-based wrapper and reads/writes the 3-level layout while doing both a pull and a push.

### Setting up sync

Share the sync directory with Syncthing, Dropbox, Drive, or rsync **before** running aw-sync. aw-sync only reads and writes files in that folder; it does not transport them between devices.

Default directory: `~/ActivityWatchSync` (`--sync-dir` or `AW_SYNC_DIR`).

Working paths (bare `aw-sync sync`, Android) write:

```txt
~/ActivityWatchSync/{hostname}/{device_id}/sync.db
```

The default daemon and advanced `sync_run` path write `~/ActivityWatchSync/{device_id}/sync.db` (two levels). Existing `test.db` staging databases and their SQLite sidecars are renamed to `sync.db` before opening — this applies to both the two-level layout (`{device_id}/test.db`) and the three-level layout (`{hostname}/{device_id}/test.db`) used by bare `aw-sync sync` and Android. If both names exist in the same folder, `sync.db` wins and `test.db` and its SQLite sidecars are left untouched for manual inspection.

#### Experimental v2 folder format (`sync-v2` feature)

Building with `--features sync-v2` compiles a writer for the immutable-segment format (`devices/{device_id}/manifest.json` plus `{slug}.{generation}.jsonl.zst` segments). It is not wired into any command yet.

The writer stages every file as `*.tmp` next to its final name and renames it into place, so a file syncer can ship a half-written temp file. Readers ignore unknown names, but tell your syncer to skip them too. For Syncthing, add this to the sync folder's `.stignore`:

```txt
(?d)*.tmp
```

### Running from source

If you want to run it from source, in the root of the repository run:

```sh
cargo run --bin aw-sync -- sync
```
For more options, see `cargo run --bin aw-sync -- --help`. Bare `cargo run --bin aw-sync` starts the daemon, which is push-only by default; enable pulls by setting `pull = true` under `[daemon]` in aw-sync's `config.toml`, or by passing `--mode both`.

## FAQ

### When is it ready?

It works today, but there is still testing and polishing to be done before it's "click and play".

### Why do it this way?

By essentially only offering a "sync with folder" feature, we give the user a lot of freedom to choose how to store and sync their data.

We also avoid having to implement complex features such as conflict resolution, by enforcing that each device only writes to files in the sync folder they "own", and other devices may not modify them.

### What are the limitations?

- The daemon is push-only by default; when pulls are enabled, its root-level scan discovers both 3-level and legacy 2-level peers. The daemon and advanced `aw-sync sync` write the 2-level layout; bare `aw-sync sync` and Android write the 3-level layout.
- By default all buckets are synced. `--buckets` is opt-in filtering, not a window/afk-only default.
- It doesn't sync settings.
- It mirrors events to all devices,
  - If you have a lot of devices you'll get a lot of duplicates, taking up a lot of space and potentially impacting performance.
- Event deletion is not synced. Owner-originated edits (same timestamp, new data) are reconciled within 7 days of the destination resume cursor; older edits are skipped to keep peak memory bounded.

---

## Advanced usage

### Syncing real data to a testing instance

If you want to try sync, you can do so by following these steps.

We will use a separate testing instance of aw-server(-rust) to store and view the synced data from the sync directory. This is to avoid testing against & potentially pollute production instances in write-scenarios. We will sync all devices with the sync folder, and then sync the sync folder into our testing instance to view.

To test syncing real events to a sync folder which can then be pulled from.
We will use some helper scripts to do the following:

1. `./test-sync-push.sh`
    - Creates a sync directory **for you to set up sync** with Syncthing/Dropbox/Gdrive/rclone/whatever
      - By default `~/ActivityWatchSync`
    - Creates a datastore for the current host in the sync folder
    - Sync all local buckets of interest (window & afk buckets, by default) to the sync dir

2. `./test-server.sh`
    - Starts a testing server **on port 5667** using a temporary directory as datastore (`/tmp/...`)

3. `./test-sync-pull.sh`
    - Imports all the events from sync folder into the testing server on port 5667

4. You should now have all events synced to a local testing instance!
    - You can browse [127.0.0.1:5667](http://127.0.0.1:5667) to view testing instance, where you'll see events from synced all hosts.
    - You can now set up syncing for `~/ActivityWatchSync` on more devices, and on each one use the script `./test-sync.sh` to push their events into the sync folder, then run `./test-import-sync.sh` on the device where you have the testing instance to update the data there.

5. To view data from all devices at once, go into [127.0.0.1:5667/#/settings](127.0.0.1:5667/#/settings) and check the "Use multidevice query" checkbox (near the bottom, under "developer settings").
    - You can now navigate back to the activity view for any device, where you should see data from multiple devices being included in (most of) the visualizations.

In the end, You should get something like this: https://twitter.com/ErikBjare/status/1519399784234246147

### Testing with fake data

#### Pushing to the sync directory

First start the sending aw-server instance. For example:

```sh
PORT=5667
cargo run --bin aw-server -- --testing --port $PORT --dbpath test-$PORT.sqlite --device-id $PORT --no-legacy-import
```

You can create some test data by opening `http://localhost:5667/#/stopwatch` and creating a few events.

Then run `cargo run --bin aw-sync-rust -- --port 5667` to sync your instance's buckets with the target directory.

#### Pulling from the sync directory

Now to sync things back from the sync directory into another instance.

First, lets start another instance:

```sh
PORT=5668
cargo run --bin aw-server -- --testing --port $PORT --dbpath test-$PORT.sqlite --device-id $PORT --no-legacy-import
```

Now run `cargo run --bin aw-sync -- --port 5668` to pull buckets from the sync dir (retrieving events from the 5667 instance) and push buckets from the 5668 instance to the sync dir.
