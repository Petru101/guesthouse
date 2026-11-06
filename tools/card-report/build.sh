#!/bin/sh
# Cross-compiles card-report.exe. Uses x86_64-w64-mingw32-gcc if it's here, or podman if it isn't.
set -e
cd "$(dirname "$0")"
BUILD='x86_64-w64-mingw32-gcc -std=gnu11 -O2 -g0 -Wall -Wextra -Wno-cast-function-type -D__USE_MINGW_ANSI_STDIO=1 -static -static-libgcc -s -o card-report.exe main.c record.c report.c search.c'
if command -v x86_64-w64-mingw32-gcc >/dev/null; then
    sh -c "$BUILD"
else
    podman run --rm --security-opt label=disable -v "$PWD:/src" -w /src registry.fedoraproject.org/fedora:44 \
        sh -c "dnf -y -q install mingw64-gcc >/dev/null && $BUILD"
fi
