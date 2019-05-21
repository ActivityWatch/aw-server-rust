#!/bin/bash

# This script installs aw-server in /usr/local so it can be
# easily started by a user with systemd
#
# See aw-server-rust for more information

set -e
sudo cp target/release/aw-server-rust /usr/local/bin
sudo mkdir -p /usr/local/share/aw-server-rust/aw-webui/dist
sudo cp -r aw-webui/dist/* /usr/local/share/aw-server-rust/aw-webui/dist
