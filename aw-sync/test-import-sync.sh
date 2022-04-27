#!/bin/bash

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
fi


sleep 1;
SYNCROOTDIR="$HOME/ActivityWatchSync"

# For each host in the sync directory, pull the data from each database file using aw-sync
for host in $(ls $SYNCROOTDIR); do
    SYNCDIR="$SYNCROOTDIR/$host"
    for db in $(ls $SYNCDIR/*/*.db); do
        AWSYNCPARAMS="--port $PORT --sync-dir $SYNCDIR"
        BUCKETS="aw-watcher-window_$host,aw-watcher-afk_$host"

        echo "Syncing $db to $host"
        cargo run --bin aw-sync -- $AWSYNCPARAMS sync --mode pull --buckets $BUCKETS
    done
done

# kill aw-server-rust
#kill %1
fg
