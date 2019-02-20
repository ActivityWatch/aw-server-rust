#!/bin/bash

wget https://dl.google.com/android/repository/android-ndk-r19b-linux-x86_64.zip -O android-ndk.zip
unzip -q -d NDK android-ndk.zip
mv NDK/*/* NDK/
