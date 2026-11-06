#!/bin/sh
# forward.c and window.c need all of QEMU, so this only reads them: the wait for the call rate has to
# come before the sensor code in each. tests/sensorrate.c is the same check on a real guest.
# Usage: throttle_first.sh DEVICE_FOLDER
line_of() {
    grep -n "$2" "$1" | head -n 1 | cut -d: -f1
}

check() {
    wait_at=$(line_of "$1" 'throttle(s);')
    sensor_at=$(line_of "$1" "$2")
    if [ -z "$wait_at" ] || [ -z "$sensor_at" ] || [ "$wait_at" -ge "$sensor_at" ]; then
        echo "$1: throttle(s) has to come before $2"
        exit 1
    fi
}

check "$1/forward.c" 'sensors_answer('
check "$1/window.c" 'sensors_take_mapping('
echo ok
