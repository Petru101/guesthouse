#!/bin/sh
# Runs the tests for what reaches the report. Needs gcc, and no Windows.
set -e
cd "$(dirname "$0")"
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
gcc -std=gnu11 -Wall -Wextra -O1 -g \
    -o "$out/test" test.c record.c report.c
"$out/test"
