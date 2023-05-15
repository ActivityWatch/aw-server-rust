#!/bin/bash

# port and db used for testing instance

# If port already set, use that, otherwise, use 5667
PORT=${PORT:-5667}

DBPATH=/tmp/aw-server-rust-sync-testing/
mkdir -p $DBPATH

# Set up an isolated ActivityWatch instance
pushd ..
cargo run --bin aw-server -- --testing --port $PORT --dbpath $DBPATH/data.db --no-legacy-import --verbose
