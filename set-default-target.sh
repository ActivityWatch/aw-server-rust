#!/bin/bash

set -e

if [ -z "$TARGET" ]; then
    echo "\$TARGET not set"
    exit 1
fi

if [ -z "$BUILD_ANDROID" ]; then
    mkdir -p ".cargo"
    echo -e "[build]\ntarget = \"$TARGET\"" > .cargo/config
fi
