#!/bin/bash

set -e;

if [ -z "$ANDROID_NDK_HOME" ]; then
    echo 'ANDROID_NDK_HOME not set, downloading NDK';
    wget https://dl.google.com/android/repository/android-ndk-r20-linux-x86_64.zip -O android-ndk.zip;
    unzip -q -d NDK android-ndk.zip;
    ls NDK;
    mv NDK/*/* NDK/;
    ANDROID_NDK_HOME=`pwd`/NDK;
fi

# Needed since dependency 'ring' doesn't respect .cargo/config
NDK_TOOLCHAIN_BIN=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin
for arch in \
    'aarch64' \
    'x86_64' \
    'i686' \
; do
    sudo ln -s -f $NDK_TOOLCHAIN_BIN/$arch-linux-android26-clang $NDK_TOOLCHAIN_BIN/$arch-linux-android-clang
done

# This has a slightly different path from the ones above
sudo ln -s -f $NDK_TOOLCHAIN_BIN/armv7a-linux-androideabi26-clang $NDK_TOOLCHAIN_BIN/armv7a-linux-androideabi-clang
sudo ln -s -f $NDK_TOOLCHAIN_BIN/armv7a-linux-androideabi26-clang $NDK_TOOLCHAIN_BIN/arm-linux-androideabi-clang

# Add to Rust
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android;
