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
    for db in $(ls $SYNCDIR/*/*.db); do
        AWSYNCPARAMS="--port $PORT --sync-dir $SYNCDIR"
        BUCKETS="aw-watcher-window_$host,aw-watcher-afk_$host"

        echo "Syncing $db to $host"
        cargo run --bin aw-sync -- $AWSYNCPARAMS sync --mode pull --buckets $BUCKETS
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
    for host in $(ls $SYNCROOTDIR); do
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
