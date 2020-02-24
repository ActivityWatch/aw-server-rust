#!/bin/bash

set -e

if [ -z "$ANDROID_NDK_HOME" ]; then
    # NOTE: I had some issues with this and cargo that magically resolved themselves when I made the path absolute.
    echo "Environment variable ANDROID_NDK_HOME not set, please set to location of Android NDK."
    exit 1
fi

if [ $RELEASE ]; then
    echo "Building in release mode... (slow)";
else
    echo "Building in debug mode... (fast)"
    RELEASE=false;
fi

ORIG_PATH="$PATH"

for archtargetstr in \
    'arm64 aarch64-linux-android' \
    'x86_64 x86_64-linux-android' \
    'x86 i686-linux-android' \
    'arm armv7-linux-androideabi' \
; do
    arch=$(echo $archtargetstr | cut -d " " -f 1)
    target=$(echo $archtargetstr | cut -d " " -f 2)
    NDK_ARCH_DIR="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
    echo "Building for $arch..."

    if [ -d "$NDK_ARCH_DIR" ]; then
        export PATH="$NDK_ARCH_DIR:$ORIG_PATH"
        cargo build -p aw-server --target $target --lib $($RELEASE && echo '--release')
    else
        echo "Couldn't find directory for target $arch"
    fi
done
