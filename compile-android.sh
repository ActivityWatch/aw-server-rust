#!/bin/bash

set -e

# Ring won't build in the below target for whatever reason
# 'arm armv7-linux-androideabi' \

for archtargetstr in \
    'x86 i686-linux-android' \
    'arm64 aarch64-linux-android' \
; do
    arch=$(echo $archtargetstr | cut -d " " -f 1)
    target=$(echo $archtargetstr | cut -d " " -f 2)
    env PATH="$PATH:/home/erb/Programming/activitywatch/other/aw-android/NDK/$arch/bin" \
        cargo build --target $target --release --lib
done
