#!/bin/bash
# Helper script meant to be used to test aw-sync
# Example of a single-entry for cronjobs and the like

# exit on fail
set -e

# on Linux, use `hostnamectl`, on macOS, use `hostname`
if [ -x "$(command -v hostnamectl)" ]; then
    HOSTNAME=$(hostnamectl --static)
else
    HOSTNAME=$(hostname)
fi

# TODO: Fetch in a cross-platform way (from aw-client command output?)
AWSERVERCONF=~/.config/activitywatch/aw-server/aw-server.toml

# trim everything in file AWSERVERCONF before '[server-testing]' section
# grep for the aw-server port in aw-server.toml
# if config doesn't exist, assume 5600
if [ -f "$AWSERVERCONF" ]; then
    PORT=$(sed '/\[server-testing\]/,/\[.*\]/{//!d}' $AWSERVERCONF | grep -oP 'port = "\K[0-9]+')
else
    PORT=5600
fi

SYNCDIR="$HOME/ActivityWatchSync/$HOSTNAME"
AWSYNC_ARGS="--port $PORT"
AWSYNC_ARGS_ADV="--sync-dir $SYNCDIR"

# NOTE: Only sync window and AFK buckets, for now
cargo run --bin aw-sync --release -- $AWSYNC_ARGS sync-advanced $AWSYNC_ARGS_ADV --mode push --buckets aw-watcher-window_$HOSTNAME,aw-watcher-afk_$HOSTNAME
