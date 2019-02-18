#!/bin/bash

set -e

if [ -z "$NDK_HOME" ]; then
    echo "Environment variable NDK_HOME not set, please set to location of Android NDK."
    exit 1
fi

# Ring won't build in the below target for whatever reason
# 'arm armv7-linux-androideabi' \

ORIG_PATH="$PATH"

for archtargetstr in \
    'x86 i686-linux-android' \
    'arm64 aarch64-linux-android' \
; do
    arch=$(echo $archtargetstr | cut -d " " -f 1)
    target=$(echo $archtargetstr | cut -d " " -f 2)
    NDK_ARCH_DIR="$NDK_HOME/$arch/bin"
    echo "Building for $arch..."

    if [ -d "$NDK_ARCH_DIR" ]; then
        env PATH="$ORIG_PATH:$NDK_ARCH_DIR" \
            cargo build --target $target --release --lib
    else
        echo "Couldn't find directory for target $arch"
    fi
done
