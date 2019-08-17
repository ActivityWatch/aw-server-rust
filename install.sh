#!/bin/bash

# This script installs aw-server in /usr/local so it can be
# easily started by a user with systemd
#
# See aw-server-rust for more information

set -e

if [ -z "$PREFIX" ]; then
    PREFIX="/usr/local"
fi

# Install aw-server-rust.service as a user service
cp ./aw-server-rust.service ~/.config/systemd/user/
# Copy aw-server-rust binary
sudo cp ./target/release/aw-server-rust $PREFIX/bin/
# Copy over webui static assets
sudo mkdir -p $PREFIX/share/aw_server_rust/
sudo cp -r ./aw-webui/dist $PREFIX/share/aw_server_rust/static
