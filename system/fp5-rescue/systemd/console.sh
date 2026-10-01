#!/bin/sh
# The rescue console, modelled on postmarketOS's initramfs debug shell (init_functions.sh:
# buffyboard on the framebuffer console + a getty). A NORMAL login prompt, never an automatic
# root shell: whoever holds the phone does not get one for free.
VT=3
STATE=/run/fp5-rescue.prev-vt
if [ "$1" = "--restore" ]; then
    [ -r "$STATE" ] && chvt "$(cat "$STATE")" 2>/dev/null
    rm -f "$STATE"
    pkill -x buffyboard 2>/dev/null
    exit 0
fi
fgconsole > "$STATE" 2>/dev/null || echo 1 > "$STATE"
modprobe uinput 2>/dev/null
# A large font: a phone console at 1224x2700 is unreadable otherwise.
for f in /usr/share/consolefonts/ter-132n.psf.gz /usr/share/consolefonts/ter-128n.psf.gz; do
    [ -r "$f" ] && { setfont "$f" -C /dev/tty$VT 2>/dev/null; break; }
done
chvt $VT
buffyboard &
exec /usr/sbin/agetty --noclear tty$VT linux
