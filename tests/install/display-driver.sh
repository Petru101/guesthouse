#!/bin/sh
# Tests what the packaging does with the display driver, on a copy of what's committed. run.sh runs it. It needs no
# Windows and no real driver: a stand-in build is enough for this, and nothing here is compiled.
#
# Usage: tests/install/display-driver.sh
set -u

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$HERE/../.." && pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
failed=0

check() {
    if eval "$2"; then
        printf 'ok: %s\n' "$1"
    else
        printf 'FAILED: %s\n' "$1"
        failed=1
    fi
}

build() {
    sh "$T/tree/packaging/build-release.sh" --out "$T/out" "$@" >"$T/said" 2>&1
}

mkdir -p "$T/tree" "$T/good" "$T/source-inf"
if [ -e "$REPO/.git" ]; then
    git -C "$REPO" archive HEAD | tar -xC "$T/tree"
else
    cp -R "$REPO/." "$T/tree/"
fi
printf 'MZ a stand-in\n' >"$T/good/GuesthouseDisplay.dll"
printf 'UmdfLibraryVersion=2.25.0\n' >"$T/good/GuesthouseDisplay.inf"
cp "$T/good/GuesthouseDisplay.dll" "$T/source-inf/"
cp "$T/tree/drivers/windows-display/GuesthouseDisplay/GuesthouseDisplay.inf" "$T/source-inf/"

build payload
check 'a build with no driver still works' '[ $? -eq 0 ]'
check 'and says what it means' 'grep -q "stay on the basic display" "$T/said"'
check 'and has no driver folder' '[ ! -d "$T/out/stage/display-driver" ]'
check 'and does not claim one' '! grep -q "^parts=.*display" "$T/out/stage/RELEASE"'

build source
check 'a release with no driver is refused' '[ $? -ne 0 ] && grep -q "without the display driver" "$T/said"'

build --display-driver "$T/source-inf" payload
check 'the INF from the source is refused' '[ $? -ne 0 ] && grep -q "not the build" "$T/said"'

sh "$T/tree/packaging/build-in-images.sh" --out "$T/images" source >"$T/said" 2>&1
check 'the images refuse a release with no driver at the start' '[ $? -ne 0 ] && grep -q "without the display driver" "$T/said"'
sh "$T/tree/packaging/build-in-images.sh" --out "$T/images" --display-driver "$T/source-inf" source >"$T/said" 2>&1
check 'and a folder that has no real build' '[ $? -ne 0 ] && grep -q "not the build" "$T/said"'
sh "$T/tree/packaging/build-in-images.sh" --out "$T/images" --display-driver "$T/nothing" source >"$T/said" 2>&1
check 'and a folder that is not there' '[ $? -ne 0 ] && grep -q "no folder" "$T/said"'

build --display-driver "$T/good" payload
check 'a build with the driver works' '[ $? -eq 0 ]'
for name in GuesthouseDisplay.dll GuesthouseDisplay.inf vdd_settings.xml LICENSE LICENSE-MS-PL \
    sign-and-install.ps1 install-take-over.ps1 take-over-display.ps1 package.txt; do
    check "the payload has $name" '[ -s "$T/out/stage/display-driver/$name" ]'
    check "and it's in the archive" 'tar -tJf "$T"/out/guesthouse-*-x86_64.tar.xz | grep -q "display-driver/$name$"'
done
check 'the licences are the repo'\''s' 'cmp -s "$T/tree/drivers/windows-display/LICENSE-MS-PL" "$T/out/stage/display-driver/LICENSE-MS-PL"'
check 'the release says it has one' 'grep -q "^parts=.*display" "$T/out/stage/RELEASE"'

exit "$failed"
