#!/bin/bash
# In a guest someone installed by hand, once its display manager has started on the GPU: if no screen
# on the GPU lights up in time, the desktop goes back to the plain screen, so there's always one.
# A later boot with the same driver and kernel then keeps it plain from the start.
set -uo pipefail
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/common.sh"
. "$here/own-desktop.sh"
light_seconds=${NVFWD_LIGHT_SECONDS:-90}
poll_seconds=2

[ "$(value_of "$state_file" desktop 2>/dev/null)" = gpu ] || exit 0
for ((waited = 0; waited < light_seconds; waited += poll_seconds)); do
    if gpu_screen_lit; then
        echo "The desktop is up on the GPU"
        exit 0
    fi
    sleep "$poll_seconds"
done
reason="the login screen didn't come up on the GPU in $light_seconds s"
echo "Back to the plain screen: $reason"
back_to_plain "$reason" "$(value_of "$state_file" built_for)"
