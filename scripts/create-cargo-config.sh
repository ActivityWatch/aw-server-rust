#!/bin/bash

if [ -z $ANDROID_NDK_HOME ]; then
    echo '$ANDROID_NDK_HOME not set';
    exit 1;
fi

tee ~/test-cargo-config.toml <<< "
[target.aarch64-linux-android]
ar = '$NDK_HOME/arm64/bin/aarch64-linux-android-ar'
linker = '$NDK_HOME/arm64/bin/aarch64-linux-android-clang'

[target.armv7-linux-androideabi]
ar = '$NDK_HOME/arm/bin/arm-linux-androideabi-ar'
linker = '$NDK_HOME/arm/bin/arm-linux-androideabi-clang'

[target.i686-linux-android]
ar = '$NDK_HOME/x86/bin/i686-linux-android-ar'
linker = '$NDK_HOME/x86/bin/i686-linux-android-clang'
"
