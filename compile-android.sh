#!/bin/bash

set -e
platform="$(uname -s | tr '[:upper:]' '[:lower:]')"

if [ -z "$ANDROID_NDK_HOME" ]; then
    if [ -d `pwd`/"NDK" ]; then
        echo "Found NDK folder in root, using."
        ANDROID_NDK_HOME=`pwd`/NDK
    else
        # NOTE: I had some issues with this and cargo that magically resolved themselves when I made the path absolute.
        echo "Environment variable ANDROID_NDK_HOME not set, please set to location of Android NDK."
        exit 1
    fi
fi
export ANDROID_NDK_HOME

if [ "$RELEASE" = "true" ]; then
    echo "Building in release mode... (slow)";
else
    echo "Building in debug mode... (fast)"
    RELEASE=false;
fi

ORIG_PATH="$PATH"

# Workaround for "error: unable to find library -lgcc"
# See: https://stackoverflow.com/questions/68873570/how-do-i-fix-ld-error-unable-to-find-library-lgcc-when-cross-compiling-rust
find "${ANDROID_NDK_HOME}" -name "libunwind.a" -execdir bash -c 'echo "INPUT(-lunwind)" > libgcc.a' \;

for archtargetstr in \
    'arm64 aarch64-linux-android' \
    'x86_64 x86_64-linux-android' \
    'x86 i686-linux-android' \
    'arm armv7-linux-androideabi' \
; do
    arch=$(echo $archtargetstr | cut -d " " -f 1)
    target=$(echo $archtargetstr | cut -d " " -f 2)
    target_underscore=$(echo $target | sed 's/-/_/g')


    echo ARCH $arch
    echo TARGET $target

    NDK_ARCH_DIR="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$platform-x86_64/bin"
    echo "Building for $arch..."

    if [ -d "$NDK_ARCH_DIR" ]; then
        export PATH="$NDK_ARCH_DIR:$ORIG_PATH"
        # Need to set AR for target since NDK 21+:
        #   https://github.com/rust-lang/cc-rs/issues/636#issuecomment-1075352495
        declare -x "AR_${target_underscore}"="$NDK_ARCH_DIR/llvm-ar"
        declare -x "CC_${target_underscore}"="$NDK_ARCH_DIR/${target}-clang"
        declare -x "RANLIB_${target_underscore}"="$NDK_ARCH_DIR/llvm-ranlib"

        # fix armv7 -> arm
        if [ "$arch" = "arm" ]; then
            declare -x "CC_${target_underscore}"="$NDK_ARCH_DIR/arm-linux-androideabi-clang"
        fi

        # check that they exist
        for var in AR_${target_underscore} CC_${target_underscore} RANLIB_${target_underscore}; do
            if [ ! -f "${!var}" ]; then
                echo "Couldn't find ${!var} set for variable $var"
                exit 1
            fi
        done

        # People suggest to use this, but ime it needs all the same workarounds anyway :shrug:
        #cargo ndk build -p aw-server --target $target --lib $($RELEASE && echo '--release')
        cargo build -p aw-server --target $target --lib $($RELEASE && echo '--release')
    else
        echo "Couldn't find directory $NDK_ARCH_DIR"
        exit 1
    fi
done
