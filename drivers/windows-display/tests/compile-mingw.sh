#!/bin/sh
# Compiles what can be compiled without the WDK against mingw's D3D11 headers, in the project's build container.
# It proves the D3D11 and Win32 calls are spelled right. It doesn't prove anything about iddcx.h, and the shader isn't compiled.
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$HERE/../../.." && pwd)
GUESTHOUSE=$HERE/../ZakoVDD/Guesthouse
OUT=$HERE/build/mingw
mkdir -p "$OUT"
# The sources use Windows path separators for zako's folders, which MSVC accepts and Linux doesn't.
mkdir -p "$OUT/shim"
for file in FramePass KernelEscapeChannel HostExporter CursorLink; do
    sed 's#"\.\.\\Logging\\Logger\.h"#"../../../ZakoVDD/Logging/Logger.h"#; s#"Shaders/FramePass.generated.h"#"FramePass.generated.h"#' \
        "$GUESTHOUSE/$file.cpp" > "$OUT/shim/$file.cpp"
    "$ROOT/scripts/inbuilder.sh" "$ROOT" x86_64-w64-mingw32-g++ -std=c++17 -Wall -Wextra -Werror -c \
        -I"$GUESTHOUSE" -I"$HERE/../Common/Include" -I"$HERE/stubs/Shaders" -I"$OUT/shim" \
        "$OUT/shim/$file.cpp" -o "$OUT/$file.o" || exit 1
    echo "compiled $file"
done
