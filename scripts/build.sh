#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 Audigma Player Project
# Build the debug APK from a fresh clone. Extra arguments go to Gradle.
# Needs: JDK 21, cmake and ninja on PATH, Android SDK in $ANDROID_HOME (default ~/.android/sdk) with
#   platforms;android-37.0   ndk;28.2.13676358   cmake;3.22.1   (sdkmanager, licenses accepted).
set -e
cd "$(dirname "$0")/.."

: "${ANDROID_HOME:=$HOME/.android/sdk}"
export ANDROID_HOME   # the media submodule ignores the root local.properties
if [ -z "$JAVA_HOME" ] && [ -d /usr/lib/jvm/java-21-openjdk ]; then JAVA_HOME=/usr/lib/jvm/java-21-openjdk; fi
export JAVA_HOME

ndk=$(sed -n 's/.*ndk_version *= *"\([^"]*\)".*/\1/p' build.gradle)
[ -d "$ANDROID_HOME/ndk/$ndk" ] || { echo "missing NDK $ndk in $ANDROID_HOME (sdkmanager \"ndk;$ndk\")" >&2; exit 1; }
command -v cmake >/dev/null && command -v ninja >/dev/null || { echo "cmake and ninja must be on PATH" >&2; exit 1; }

git submodule update --init --recursive

# AGP 9 fails when a library's consumer proguard file is missing; the Media3 fork lacks it in many
# modules. Empty stubs are enough. They and the TagLib build output are hidden from submodule status.
for d in media/libraries/*/; do
  if [ -f "${d}build.gradle" ] && [ ! -f "${d}proguard-rules.txt" ]; then : > "${d}proguard-rules.txt"; fi
done
hide() { # hide <submodule> <pattern>
  ex=$(cd "$1" && git rev-parse --git-path info/exclude)
  case $ex in /*) ;; *) ex="$1/$ex" ;; esac
  mkdir -p "$(dirname "$ex")"
  grep -qxF "$2" "$ex" 2>/dev/null || echo "$2" >> "$ex"
}
hide media 'libraries/*/proguard-rules.txt'
hide musikr/src/main/cpp/taglib 'pkg/'
hide musikr/src/main/cpp/taglib 'build/'

exec ./gradlew --no-daemon assembleDebug "$@"
