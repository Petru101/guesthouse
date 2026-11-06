#!/bin/sh
# Builds and runs the driver's tests on Linux. They cover what doesn't need Windows: the protocol, the ring's rules and the colour maths.
set -e
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BUILD=$HERE/build
mkdir -p "$BUILD"
FLAGS="-std=c++17 -Wall -Wextra -Werror -O1 -g -fsanitize=address,undefined -I$HERE/../Common/Include"
GUESTHOUSE=$HERE/../ZakoVDD/Guesthouse

clang++ $FLAGS "$HERE/host_link_test.cpp" "$GUESTHOUSE/HostLink.cpp" "$GUESTHOUSE/CursorLink.cpp" -o "$BUILD/host_link_test"
"$BUILD/host_link_test"
for test in "$HERE"/*_test.cpp; do
    name=$(basename "$test" .cpp)
    [ "$name" = host_link_test ] && continue
    clang++ $FLAGS "$test" "$GUESTHOUSE/ColourTransform.cpp" -o "$BUILD/$name"
    "$BUILD/$name"
done
