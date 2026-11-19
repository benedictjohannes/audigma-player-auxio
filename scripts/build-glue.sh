#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 Audigma Player Project
# Cross-builds engine/native/audigma-glue (libaudigma_glue.so) for arm64-v8a.
# Usage: build-glue.sh [output dir for arm64-v8a/libaudigma_glue.so; default: engine/build/rustJniLibs]
# Needs: rustup target aarch64-linux-android, NDK from build.gradle's ndk_version.
set -e
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$root/engine/build/rustJniLibs}
: "${ANDROID_HOME:=$HOME/.android/sdk}"
ndk=$(sed -n 's/.*ndk_version *= *"\([^"]*\)".*/\1/p' "$root/build.gradle")
bin=$ANDROID_HOME/ndk/$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin
[ -x "$bin/aarch64-linux-android30-clang" ] || { echo "missing NDK $ndk in $ANDROID_HOME" >&2; exit 1; }
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$bin/aarch64-linux-android30-clang
export CC_aarch64_linux_android=$bin/aarch64-linux-android30-clang
export AR_aarch64_linux_android=$bin/llvm-ar
cd "$root/engine/native/audigma-glue"
cargo build --release --target aarch64-linux-android
mkdir -p "$out/arm64-v8a"
cp target/aarch64-linux-android/release/libaudigma_glue.so "$out/arm64-v8a/"
