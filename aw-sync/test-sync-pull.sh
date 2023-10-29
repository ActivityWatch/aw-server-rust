#!/bin/bash

# exit on fail
set -e

# get script path
SCRIPTPATH="$( cd "$(dirname "$0")" ; pwd -P )"
pushd $SCRIPTPATH

# port used for testing instance
PORT=5667

# if server already running on port 5667, don't start again
if [ "$(lsof -i:$PORT -sTCP:LISTEN -t)" ]; then
    echo "ActivityWatch server already running on port $PORT, using that."
else
    # Set up an isolated ActivityWatch instance
    ./test-server.sh $PORT &
    SERVER_PID=$!
fi


sleep 1;
SYNCROOTDIR="$HOME/ActivityWatchSync"


function sync_host() {
    host=$1
    SYNCDIR="$SYNCROOTDIR/$host"
    dbs=$(find $SYNCDIR -name "*.db")
    for db in $dbs; do
        # workaround to avoid trying to sync empty database files (size 45056)
        if [ "$(stat -c%s $db)" -lt 50000 ]; then
            continue
        fi

        AWSYNC_ARGS="--port $PORT"
        AWSYNC_ARGS_ADV="--sync-dir $SYNCDIR --sync-db $db"
        BUCKETS="aw-watcher-window_$host,aw-watcher-afk_$host"

        echo "Syncing $db to $host"
        cargo run --bin aw-sync -- $AWSYNC_ARGS sync-advanced $AWSYNC_ARGS_ADV --mode pull --buckets $BUCKETS
        # TODO: If there are no buckets from the expected host, emit a warning at the end.
        #       (push-script should not have created them to begin with)
    done
}

host=$1

# if no host given, sync all, otherwise sync only the given host
if [ -z "$host" ]; then
    echo "Syncing all hosts"
    sleep 0.5
    # For each host in the sync directory, pull the data from each database file using aw-sync
    # Use `find` to get all directories in the sync directory
    hostnames=$(find $SYNCROOTDIR -maxdepth 1 -type d -exec basename {} \;)
    # filter out "erb-m2.local"
    hostnames=$(echo $hostnames | tr ' ' '\n' | grep -v "erb-m2.local")
    # filter out folder not containing subfolders with .db files
    for host in $hostnames; do
        if [ ! "$(find $SYNCROOTDIR/$host -name "*.db")" ]; then
            hostnames=$(echo $hostnames | tr ' ' '\n' | grep -v $host)
        fi
    done
    # Sync each host, file-by-file
    for host in $hostnames; do
        sync_host $host
    done
else
    echo "Syncing host $1"
    sleep 0.5
    sync_host $host
fi

# kill aw-server-rust (if started by us)
if [ "$SERVER_PID" ]; then
    kill $SERVER_PID
fi
