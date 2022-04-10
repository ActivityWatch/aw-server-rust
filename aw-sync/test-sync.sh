#!/bin/bash
# Helper script meant to be used to test aw-sync
# Example of a single-entry for cronjobs and the like

HOSTNAME=$(hostnamectl --static)
# TODO: Fetch in a cross-platform way (from aw-client command output?)
AWSERVERCONF=~/.config/activitywatch/aw-server/aw-server.toml

# trim everything in file AWSERVERCONF before '[server-testing]' section
# grep for the aw-server port in aw-server.toml
PORT=$(sed '/\[server-testing\]/,/\[.*\]/{//!d}' $AWSERVERCONF | grep -oP 'port = "\K[0-9]+')

SYNCDIR="$HOME/ActivityWatchSync/$HOSTNAME"
AWSYNCPARAMS="--port $PORT --sync-dir $SYNCDIR"

# TODO: Fix supplying multiple buckets in a single command
# NOTE: Only sync window and AFK buckets, for now
cargo run --bin aw-sync -- $AWSYNCPARAMS sync --mode push --buckets aw-watcher-window_$HOSTNAME,aw-watcher-afk_$HOSTNAME
