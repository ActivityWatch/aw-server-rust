#!/bin/bash
# Based on https://mozilla.github.io/firefox-browser-architecture/experiments/2017-09-21-rust-on-android.html
# Depended on by aw-android/scripts/setup-rust-with-ndk.sh

set -e;

NDK_VERSION=r25c

script_dir="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
project_path="$(readlink -f "$script_dir/.")"

platform="$(uname -s | tr '[:upper:]' '[:lower:]')"

if [ -z "$ANDROID_NDK_HOME" ]; then
    if [ -d `pwd`/"NDK" ]; then
        echo "Found NDK folder in root, using."
    else
        echo 'ANDROID_NDK_HOME not set, downloading NDK...';
        # Download Linux NDK or macOS NDK, depending on OS
        wget --no-verbose -O android-ndk.zip https://dl.google.com/android/repository/android-ndk-$NDK_VERSION-$platform.zip;
        unzip -q -d NDK android-ndk.zip;
        ls NDK;
        mv NDK/*/* NDK/;
    fi
    ANDROID_NDK_HOME=`pwd`/NDK;
fi

# Needed since dependency 'ring' doesn't respect .cargo/config
echo "Setting up toolchain binary symlinks..."
NDK_TOOLCHAIN_BIN=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$platform-x86_64/bin
for arch in \
    'aarch64' \
    'x86_64' \
    'i686' \
; do
    ln -s -f $NDK_TOOLCHAIN_BIN/$arch-linux-android26-clang $NDK_TOOLCHAIN_BIN/$arch-linux-android-clang
done

# This has a slightly different path from the ones above
ln -s -f $NDK_TOOLCHAIN_BIN/armv7a-linux-androideabi26-clang $NDK_TOOLCHAIN_BIN/armv7a-linux-androideabi-clang
ln -s -f $NDK_TOOLCHAIN_BIN/armv7a-linux-androideabi26-clang $NDK_TOOLCHAIN_BIN/arm-linux-androideabi-clang

# Add to Rust
echo "Setting up Rust toolchains..."
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android;

# Creates cargo config
echo "Creating cargo config..."
mkdir -p $project_path/.cargo
echo "
[target.aarch64-linux-android]
ar = '$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$platform-x86_64/bin/llvm-ar'
linker = '$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$platform-x86_64/bin/aarch64-linux-android26-clang'

[target.armv7-linux-androideabi]
ar = '$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$platform-x86_64/bin/llvm-ar'
linker = '$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$platform-x86_64/bin/armv7a-linux-androideabi-clang'

[target.i686-linux-android]
ar = '$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$platform-x86_64/bin/llvm-ar'
linker = '$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$platform-x86_64/bin/i686-linux-android26-clang'

[target.x86_64-linux-android]
ar = '$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$platform-x86_64/bin/llvm-ar'
linker = '$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$platform-x86_64/bin/x86_64-linux-android26-clang'
" > $project_path/.cargo/config
