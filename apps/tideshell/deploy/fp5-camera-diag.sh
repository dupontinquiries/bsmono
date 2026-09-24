#!/bin/sh
# Why does the camera work under plasma-mobile but not under cosmic?
# Read-only checks of every link from the kernel to the app, run as the
# phone user INSIDE the cosmic session (over ssh with the session env, or
# from a terminal in the session):  sh fp5-camera-diag.sh | tee camera-diag.txt
# The same driver worked under plasma, so the usual culprits are session
# plumbing: user services not started under this session, the video group
# missing on the session's process, WirePlumber's libcamera monitor off, or
# the app enumerating through a portal that cosmic does not provide.
set -u
UID_N=$(id -u)
RT=${XDG_RUNTIME_DIR:-/run/user/$UID_N}
hr() { printf '\n== %s\n' "$*"; }
ok() { echo "PASS  $*"; }
warn() { echo "WARN  $*"; }
bad() { echo "FAIL  $*"; }

hr "1. kernel: media/video nodes and their permissions"
ls -l /dev/media* /dev/video* 2>/dev/null || bad "no /dev/media* or /dev/video* (driver not bound; compare dmesg | grep -i camss)"
for d in /dev/media*; do [ -e "$d" ] && [ -r "$d" ] && [ -w "$d" ] && ok "rw $d" || { [ -e "$d" ] && bad "cannot open $d as $(id -un)"; }; done
echo "groups of THIS process: $(id -Gn)"
id -Gn | tr ' ' '\n' | grep -qx video && ok "in the video group" || bad "not in the video group in this session (plasma's session had it?): sudo adduser $(id -un) video, log out and in"
[ -n "${XDG_SESSION_TYPE:-}" ] && echo "session type: $XDG_SESSION_TYPE  desktop: ${XDG_CURRENT_DESKTOP:-unset}"

hr "2. libcamera sees the camera?"
if command -v cam >/dev/null 2>&1; then
    LIBCAMERA_LOG_LEVELS=*:WARN timeout 15 cam -l 2>&1 | head -20
else
    warn "libcamera 'cam' tool missing (apk add libcamera-tools); trying python/gst below"
fi
ls /usr/lib/libcamera/ 2>/dev/null | sed 's/^/      pipeline: /' | head -5
ls /usr/share/libcamera/ipa/ 2>/dev/null | sed 's/^/      ipa: /' | head -5
ls /etc/libcamera /usr/share/libcamera/pipeline 2>/dev/null | head -5

hr "3. PipeWire and WirePlumber in THIS session"
pgrep -u "$UID_N" -a pipewire | head -3 || bad "pipewire not running for this user"
pgrep -u "$UID_N" -a wireplumber | head -2 || bad "wireplumber not running (camera nodes come from its libcamera monitor)"
echo "DBUS_SESSION_BUS_ADDRESS=${DBUS_SESSION_BUS_ADDRESS:-unset}"
systemctl --user is-active pipewire wireplumber 2>&1 | sed 's/^/      systemd --user: /'
systemctl --user show-environment 2>/dev/null | grep -E 'WAYLAND_DISPLAY|XDG_CURRENT_DESKTOP|DBUS' | sed 's/^/      user env: /'
if command -v pw-cli >/dev/null 2>&1; then
    n=$(pw-cli ls Node 2>/dev/null | grep -c 'media.class = "Video/Source"')
    [ "$n" -gt 0 ] && ok "$n Video/Source node(s) in PipeWire" || bad "no Video/Source nodes in PipeWire (the libcamera monitor produced nothing)"
    pw-cli ls Node 2>/dev/null | grep -B4 'Video/Source' | grep -E 'node.name|node.description|api' | sed 's/^/      /' | head -8
else
    warn "pw-cli missing (apk add pipewire-tools)"
fi

hr "4. WirePlumber libcamera monitor"
ls /usr/lib/pipewire-0.3/libpipewire-module-* 2>/dev/null | grep -i camera | sed 's/^/      /'
ls /usr/lib/spa-0.2/libcamera/ 2>/dev/null | sed 's/^/      spa plugin: /' || bad "no spa-0.2/libcamera plugin (apk add pipewire-libcamera, name unverified)"
for f in /usr/share/wireplumber/wireplumber.conf /etc/wireplumber/wireplumber.conf ~/.config/wireplumber/wireplumber.conf; do
    [ -f "$f" ] && { echo "      $f:"; grep -nE 'libcamera' "$f" | sed 's/^/        /' | head -5; }
done
for d in /usr/share/wireplumber/wireplumber.conf.d /etc/wireplumber/wireplumber.conf.d ~/.config/wireplumber/wireplumber.conf.d; do
    [ -d "$d" ] && grep -lE 'libcamera' "$d"/* 2>/dev/null | sed 's/^/      drop-in: /'
done
journalctl --user -u wireplumber -b --no-pager 2>/dev/null | grep -iE 'libcamera|camera' | tail -8 | sed 's/^/      wp log: /'

hr "5. the app's path: portal or direct?"
pgrep -a xdg-desktop-portal 2>/dev/null | sed 's/^/      /' || warn "no xdg-desktop-portal running (a Camera portal request would hang; direct PipeWire/libcamera access does not need it)"
busctl --user list 2>/dev/null | grep -iE 'portal' | cut -c1-70 | sed 's/^/      bus: /'
ls /usr/share/xdg-desktop-portal/portals/ 2>/dev/null | sed 's/^/      portal backends: /'
command -v gst-launch-1.0 >/dev/null 2>&1 && { echo "      gst libcamera plugin: $(gst-inspect-1.0 libcamerasrc 2>/dev/null | head -1 || echo missing)"; echo "      gst pipewire plugin: $(gst-inspect-1.0 pipewiresrc 2>/dev/null | head -1 || echo missing)"; }

hr "6. a 3-second capture attempt (the decisive test)"
if command -v cam >/dev/null 2>&1; then
    LIBCAMERA_LOG_LEVELS=*:INFO timeout 10 cam -c 1 --capture=3 2>&1 | tail -12 | sed 's/^/      /'
elif command -v gst-launch-1.0 >/dev/null 2>&1; then
    timeout 10 gst-launch-1.0 libcamerasrc ! identity eos-after=3 ! fakesink 2>&1 | tail -6 | sed 's/^/      /'
else
    warn "no capture tool (apk add libcamera-tools)"
fi
echo
echo "Read it bottom-up: if 6 works, the driver and libcamera are fine and the app's enumeration path (5) is the problem; if 6 fails with a permission error, it is 1; if it works but 3 lists no Video/Source, it is 4 (WirePlumber's libcamera monitor); if pipewire/wireplumber are not running at all under cosmic, the session does not start user services (compare 'systemctl --user' under plasma)."
