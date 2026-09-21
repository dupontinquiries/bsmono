#!/bin/sh
# fp5-touch-diag.sh - find which layer drops touch under cosmic on the FP5.
# Run over SSH as the phone user while the cosmic session is logged in:
#   sh fp5-touch-diag.sh            # static checks
#   sh fp5-touch-diag.sh live       # + 15s live capture: tap the screen a few times
#
# Layers, bottom to top:
#   1 kernel evdev   2 udev tags/seat   3 libinput   4 cosmic-comp -> wl_touch   5 client
set -u
UID_N=$(id -u)
RT=${XDG_RUNTIME_DIR:-/run/user/$UID_N}
CFG=${XDG_CONFIG_HOME:-$HOME/.config}/cosmic/com.system76.CosmicComp/v1

cnt() { [ -f "$2" ] && grep -c "$1" "$2" || true; [ -f "$2" ] || echo n/a; }
hr() { printf '\n== %s\n' "$*"; }

hr "1. kernel input devices (PROP=2 means INPUT_PROP_DIRECT = touchscreen)"
awk '/^N:/{n=$0} /^H:/{h=$0} /^B: PROP=/{p=$0} /^$/{ if (p ~ /PROP=[0-9a-f]*[2367abef]$/ || n ~ /[Tt]ouch/) print n "\n  " h "\n  " p; n=h=p="" }' /proc/bus/input/devices

hr "2. udev properties of touch-capable event nodes"
for e in /dev/input/event*; do
    props=$(udevadm info -q property -n "$e" 2>/dev/null)
    echo "$props" | grep -q 'ID_INPUT_TOUCHSCREEN=1' || continue
    echo "$e"
    echo "$props" | grep -E '^(NAME|ID_INPUT_TOUCHSCREEN|ID_SEAT|LIBINPUT_[A-Z_]+|ID_INPUT_[A-Z]+)=' | sed 's/^/  /'
    ls -l "$e" | sed 's/^/  /'
done

hr "3. libinput's view"
if command -v libinput >/dev/null 2>&1; then
    sudo libinput list-devices 2>/dev/null | awk 'BEGIN{RS=""} /touch/'
else
    echo "libinput CLI missing (try: sudo apk add libinput-tools; package name unverified)"
fi

hr "4a. cosmic-comp input config (touchscreens use input_default + input_devices)"
for f in input_default input_devices; do
    printf -- '-- %s: ' "$f"; [ -f "$CFG/$f" ] && { echo; sed 's/^/  /' "$CFG/$f"; } || echo "(unset, defaults)"
done

hr "4b. cosmic-comp log lines about input/outputs (this boot)"
{ journalctl --user -b --no-pager 2>/dev/null; sudo journalctl -b --no-pager 2>/dev/null; } \
    | grep -i 'cosmic-comp' | grep -iE 'touch|libinput|input device|seat|DSI|output' | tail -30

hr "4c. wayland sockets in $RT"
ls "$RT" 2>/dev/null | grep -E '^wayland-[0-9]+$' || echo "none found (wrong user or session not running)"

[ "${1:-}" = live ] || { echo; echo "static checks done; re-run with 'live' for event capture"; exit 0; }

hr "LIVE: tap the screen several times in the next 15s"
DEV=$(for e in /dev/input/event*; do udevadm info -q property -n "$e" 2>/dev/null | grep -q 'ID_INPUT_TOUCHSCREEN=1' && echo "$e" && break; done)
SOCK=$(ls "$RT" 2>/dev/null | grep -E '^wayland-[0-9]+$' | head -1)
if [ -n "$DEV" ] && command -v libinput >/dev/null 2>&1; then
    sudo timeout 15 libinput debug-events --device "$DEV" > /tmp/touch-libinput.log 2>&1 &
fi
if [ -n "$SOCK" ] && command -v cosmic-term >/dev/null 2>&1; then
    # A throwaway client: logs what cosmic-comp actually sends it.
    XDG_RUNTIME_DIR=$RT WAYLAND_DISPLAY=$SOCK WAYLAND_DEBUG=client \
        timeout 15 cosmic-term > /tmp/touch-wayland.log 2>&1 &
    echo "(a cosmic-term window opened; tap inside it too)"
fi
wait
echo "-- libinput: $(cnt TOUCH_DOWN /tmp/touch-libinput.log) TOUCH_DOWN events"
grep -m3 TOUCH_DOWN /tmp/touch-libinput.log 2>/dev/null
echo "-- client: seat capabilities (7 = pointer|keyboard|touch):"
grep -m2 'wl_seat.*capabilities' /tmp/touch-wayland.log 2>/dev/null
echo "-- client: $(cnt 'wl_touch.*down' /tmp/touch-wayland.log) wl_touch.down events"
grep -m3 'wl_touch.*down' /tmp/touch-wayland.log 2>/dev/null
echo "full logs: /tmp/touch-libinput.log /tmp/touch-wayland.log"
