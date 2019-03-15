#!/bin/bash

set -e

if [ -z "$ANDROID_NDK_HOME" ]; then
    # NOTE: I had some issues with this and cargo that magically resolved themselves when I made the path absolute.
    echo "Environment variable ANDROID_NDK_HOME not set, please set to location of Android NDK."
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
    NDK_ARCH_DIR="$ANDROID_NDK_HOME/$arch/bin"
    echo "Building for $arch..."

    if [ -d "$NDK_ARCH_DIR" ]; then
        export PATH="$NDK_ARCH_DIR:$ORIG_PATH"
        cargo build --target $target --lib $($RELEASE && echo '--release')
    else
        echo "Couldn't find directory for target $arch"
    fi
done
