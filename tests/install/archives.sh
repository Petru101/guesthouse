#!/bin/sh
# Tests that a release's archives keep their links, on a copy of what's committed. run.sh runs it. All three
# archives go through the same pack, so the payload stands for them, as it's the one that builds with nothing else.
#
# Usage: tests/install/archives.sh
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

# Packs a stage holding the link it's given, with the tree's own script.
pack_with_link() { # target, the script to use
    rm -rf "$T/out"
    mkdir -p "$T/out/stage/sub"
    : >"$T/out/stage/sub/real"
    ln -s "$1" "$T/out/stage/sub/link"
    sh "$2" --out "$T/out" payload >"$T/said" 2>&1
}

mkdir -p "$T/tree"
if [ -e "$REPO/.git" ]; then
    git -C "$REPO" archive HEAD | tar -xC "$T/tree"
else
    cp -R "$REPO/." "$T/tree/"
fi
script=$T/tree/packaging/build-release.sh

pack_with_link ./real "$script"
check 'a link that starts with a dot packs' '[ $? -eq 0 ]'
check 'and keeps its target' 'tar -tvJf "$T"/out/guesthouse-*.tar.xz | grep -q "sub/link -> ./real$"'

pack_with_link ./nothing "$script"
check 'a link to nothing is refused' '[ $? -ne 0 ] && grep -q "point nowhere" "$T/said"'
check 'and named' 'grep -q "sub/link" "$T/said"'

pack_with_link /usr "$script"
check 'so is one that leaves the archive' '[ $? -ne 0 ] && grep -q "point nowhere" "$T/said"'

# What the check is for: tar with no S bends the same good link.
sed 's/,S" --owner/," --owner/' "$script" >"$T/tree/packaging/no-s.sh"
pack_with_link ./real "$T/tree/packaging/no-s.sh"
check 'a build that bends links stops' '[ $? -ne 0 ] && grep -q "point nowhere" "$T/said"'

exit "$failed"
