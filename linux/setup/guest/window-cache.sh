#!/bin/bash
# Lets the CPU cache the device's memory again where the firmware said not to. Its call slots and its
# window are PCI BARs, and OVMF marks the whole 64-bit PCI aperture uncacheable in the MTRRs. Linux then
# maps all of it uncached, even what the driver asks to have cached: the slots, and the GPU's system memory,
# which the CPU then reaches at about a twentieth of its speed. SeaBIOS leaves the aperture cacheable, so an
# Ubuntu guest is as it was. This takes the range out of each uncacheable MTRR, and gives the rest of that
# MTRR back as smaller uncacheable ones while there are registers free. Linux maps other devices' memory
# uncached through PAT whatever the MTRRs say, and the driver still picks its own types for video memory.
# Usage: window-cache.sh BASE SIZE, in bytes. It has to run before anything maps the device's memory.
set -uo pipefail
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/common.sh"
mtrr=$root/proc/mtrr
window_start=$(($1))
window_end=$((window_start + $2))

# An MTRR's size in bytes, from /proc/mtrr's "4096MB" or "64kB".
bytes() {
    case $1 in
    *MB) echo $((${1%MB} << 20)) ;;
    *kB) echo $((${1%kB} << 10)) ;;
    esac
}

# Prints "base size" pieces for [START, END): each a power of two, aligned to its size, as MTRRs must be.
pieces() {
    local start=$1 end=$2 size
    while [ "$start" -lt "$end" ]; do
        size=1
        while [ $((start % (size * 2))) = 0 ] && [ $((start + size * 2)) -le "$end" ]; do
            size=$((size * 2))
        done
        echo "$start $size"
        start=$((start + size))
    done
}

uncacheable_overlaps() {
    local line reg base size end
    while read -r line; do
        [[ $line == *uncachable* ]] || continue
        reg=${line%%:*}
        base=$(echo "$line" | sed -n 's/.*base=\(0x[0-9a-fA-F]*\).*/\1/p')
        size=$(bytes "$(echo "$line" | sed -n 's/.*size= *\([0-9]*[Mk]B\).*/\1/p')")
        end=$((base + size))
        [ $((base)) -lt "$window_end" ] && [ "$end" -gt "$window_start" ] && echo "${reg#reg} $((base)) $end"
    done < "$mtrr"
}

[ -w "$mtrr" ] || exit 0
# All read before any is changed, as a change renumbers nothing but rewrites the file being read.
overlaps=$(uncacheable_overlaps)
while read -r reg start end; do
    [ -n "$reg" ] || continue
    echo "The GPU's window was uncacheable in MTRR $reg, so it comes out of that range"
    echo "disable=$((10#$reg))" >> "$mtrr" || exit 1
    # The largest first, so the registers there are cover the most.
    while read -r base size; do
        printf 'base=0x%x size=0x%x type=uncachable\n' "$base" "$size" >> "$mtrr" 2>/dev/null ||
            printf 'No MTRR left for 0x%x, which stays uncached through PAT\n' "$base"
    done < <({
        [ "$start" -ge "$window_start" ] || pieces "$start" "$window_start"
        [ "$window_end" -ge "$end" ] || pieces "$window_end" "$end"
    } | sort -k2,2nr)
done <<< "$overlaps"
