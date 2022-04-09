#!/bin/bash

# Helper script meant to be used to test aw-sync
# Example of a single-entry for cronjobs and the like

HOSTNAME="Teklas-Air.localdomain"
SYNCDIR="~/ActivityWatchSync/tekla-air-m1"
AWSYNCPARAMS="--port 5601 --sync-dir $SYNCDIR"

# TODO: Fix supplying multiple buckets in a single command
# NOTE: Only sync window and AFK buckets, for now
cargo run --bin aw-sync -- $AWSYNCPARAMS sync --buckets aw-watcher-window_$HOSTNAME
cargo run --bin aw-sync -- $AWSYNCPARAMS sync --buckets aw-watcher-afk_$HOSTNAME
