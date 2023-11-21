aw-sync-rust
============

Synchronization for ActivityWatch.

Works by syncing local buckets with a special folder, which in turn should be synchronized by rsync/Syncthing/Dropbox/GDrive/whatever.

Was originally prototyped as a PR to aw-server: https://github.com/ActivityWatch/aw-server/pull/50


## Usage

NOTE: Basic usage not quite ready yet, see the below testing sections for MVP usage.

```
cargo run --bin aw-sync -- sync
```

## Running with real data on a testing instance

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


## Testing with fake data

**Note:** this documents usage for testing, it is not yet ready for production usage.

It assumes you're running a temporary aw-server instances.

### Pushing to the sync directory

First start the sending aw-server instance. For example: 

```sh
PORT=5667
cargo run --bin aw-server -- --testing --port $PORT --dbpath test-$PORT.sqlite --device-id $PORT --no-legacy-import
```

You can create some test data by opening `http://localhost:5667/#/stopwatch` and creating a few events.

Then run `cargo run --bin aw-sync-rust -- --port 5667` to sync your instance's buckets with the target directory.

### Pulling from the sync directory

Now to sync things back from the sync directory into another instance. 

First, lets start another instance:

```sh
PORT=5668
cargo run --bin aw-server -- --testing --port $PORT --dbpath test-$PORT.sqlite --device-id $PORT --no-legacy-import
```

Now run `cargo run --bin aw-sync-rust -- --port 5668` to pull buckets from the sync dir (retrieving events from the 5667 instance) and push buckets from the 5668 instance to the sync dir.
