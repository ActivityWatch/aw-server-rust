#!/bin/bash

set -x

export PATH="$PATH:/home/erb/Programming/activitywatch/other/aw-android/NDK/arm64/bin"

cargo build --target aarch64-linux-android --release --lib
