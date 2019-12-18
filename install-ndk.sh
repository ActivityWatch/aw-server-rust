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

#export NDK_TOOLCHAIN_LOC=`pwd`/NDK;
#${ANDROID_NDK_HOME}/build/tools/make_standalone_toolchain.py --api 26 --arch arm64 --install-dir "${NDK_TOOLCHAIN_LOC}/arm64" || true;
#${ANDROID_NDK_HOME}/build/tools/make_standalone_toolchain.py --api 26 --arch arm --install-dir "${NDK_TOOLCHAIN_LOC}/arm" || true;
#${ANDROID_NDK_HOME}/build/tools/make_standalone_toolchain.py --api 26 --arch x86 --install-dir "${NDK_TOOLCHAIN_LOC}/x86" || true;
#${ANDROID_NDK_HOME}/build/tools/make_standalone_toolchain.py --api 26 --arch x86_64 --install-dir "${NDK_TOOLCHAIN_LOC}/x86_64" || true;

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
