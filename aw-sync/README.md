aw-sync-rust
============

Synchronization for ActivityWatch.

Works by syncing local buckets with a special folder, which in turn should be synchronized by rsync/Syncthing/Dropbox/whatever.


## Usage

**Note:** this documents usage for testing, it is not yet ready for production usage.

### Pushing to the sync directory

First start your aw-server instance. 

For example: 

```sh
PORT=5667
cargo run --bin aw-server -- --testing --port $PORT --dbpath test-$PORT.sqlite --device-id $PORT --no-legacy-import
```

You can create some test data by opening `http://localhost:5667/#/stopwatch` and creating a few events.

Then run `cargo run --bin aw-sync-rust` to sync your instance's buckets with the target directory.

### Pulling from the sync directory

Now to sync things back from the sync directory onto another instance. First, lets start another instance:

```
PORT=5668
cargo run --bin aw-server -- --testing --port $PORT --dbpath test-$PORT.sqlite --device-id $PORT --no-legacy-import
```

Now run `aw-sync-rust` again.
