#!/bin/sh
# Headless screenshot of an egui binary under Xvfb — the fleet recipe from
# shared/vidkit/CLAUDE.md (Xvfb + openbox + ImageMagick `import`), packaged so
# every app in this tree is shot the same way.
#
#   tools/shot.sh <out.png> <WxH> <settle-seconds> [-k "xdotool key ..."] -- <cmd...>
#
# Environment for the app: DISPLAY=:99, software GL. The X server is started
# once and reused across calls (it stays up in the background); the app is
# killed after the capture. Extra xdotool commands (-k) run after the settle
# delay, then a second settle of 1.5 s happens before the capture, so a
# keypress that opens a page is what gets photographed.
set -e
OUT="$1"; SIZE="$2"; SETTLE="$3"; shift 3
KEYS=""
while [ "$1" != "--" ]; do
  case "$1" in
    -k) KEYS="$KEYS
$2"; shift 2;;
    *) echo "bad arg $1" >&2; exit 2;;
  esac
done
shift
export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe WAYLAND_DISPLAY=
if ! xdpyinfo >/dev/null 2>&1; then
  Xvfb :99 -screen 0 1400x1000x24 +extension GLX >/dev/null 2>&1 &
  sleep 2
  openbox >/dev/null 2>&1 &
  sleep 1
fi
"$@" >"${OUT%.png}.log" 2>&1 &
PID=$!
sleep "$SETTLE"
WID=$(xdotool search --onlyvisible --pid "$PID" 2>/dev/null | head -1 || true)
if [ -z "$WID" ]; then WID=$(xdotool search --onlyvisible --name . 2>/dev/null | tail -1 || true); fi
if [ -n "$WID" ]; then
  W=${SIZE%x*}; H=${SIZE#*x}
  xdotool windowsize "$WID" "$W" "$H" || true
  xdotool windowmove "$WID" 0 0 || true
  xdotool windowactivate "$WID" 2>/dev/null || true
  sleep 1.5
fi
export WID
# Each -k line is a whole xdotool command line; $WID is available to it
# (`mousemove --window $WID x y click 1` clicks at window coordinates).
# A line starting with "sleep" is a plain pause instead.
printf '%s\n' "$KEYS" | while IFS= read -r k; do
  case "$k" in
    "") ;;
    sleep*) sh -c "$k";;
    *) sh -c "xdotool $k"; sleep 0.6;;
  esac
done
if [ -n "$KEYS" ]; then sleep 1.5; fi
if [ -n "$WID" ]; then
  import -window "$WID" "$OUT" || import -window root "$OUT"
else
  import -window root "$OUT"
fi
kill "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true
echo "wrote $OUT (wid=$WID)"
#!/bin/sh
# Headless screenshot of an egui binary under Xvfb — the fleet recipe from
# shared/vidkit/CLAUDE.md (Xvfb + openbox + ImageMagick `import`), packaged so
# every app in this tree is shot the same way.
#
#   tools/shot.sh <out.png> <WxH> <settle-seconds> [-k "xdotool key ..."] -- <cmd...>
#
# Environment for the app: DISPLAY=:99, software GL. The X server is started
# once and reused across calls (it stays up in the background); the app is
# killed after the capture. Extra xdotool commands (-k) run after the settle
# delay, then a second settle of 1.5 s happens before the capture, so a
# keypress that opens a page is what gets photographed.
set -e
OUT="$1"; SIZE="$2"; SETTLE="$3"; shift 3
KEYS=""
while [ "$1" != "--" ]; do
  case "$1" in
    -k) KEYS="$KEYS
$2"; shift 2;;
    *) echo "bad arg $1" >&2; exit 2;;
  esac
done
shift
export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe WAYLAND_DISPLAY=
if ! xdpyinfo >/dev/null 2>&1; then
  Xvfb :99 -screen 0 1400x1000x24 +extension GLX >/dev/null 2>&1 &
  sleep 2
  openbox >/dev/null 2>&1 &
  sleep 1
fi
"$@" >"${OUT%.png}.log" 2>&1 &
PID=$!
sleep "$SETTLE"
WID=$(xdotool search --onlyvisible --pid "$PID" 2>/dev/null | head -1 || true)
if [ -z "$WID" ]; then WID=$(xdotool search --onlyvisible --name . 2>/dev/null | tail -1 || true); fi
if [ -n "$WID" ]; then
  W=${SIZE%x*}; H=${SIZE#*x}
  xdotool windowsize "$WID" "$W" "$H" || true
  xdotool windowmove "$WID" 0 0 || true
  xdotool windowactivate "$WID" 2>/dev/null || true
  sleep 1.5
fi
export WID
# Each -k line is a whole xdotool command line; $WID is available to it
# (`mousemove --window $WID x y click 1` clicks at window coordinates).
# A line starting with "sleep" is a plain pause instead.
printf '%s\n' "$KEYS" | while IFS= read -r k; do
  case "$k" in
    "") ;;
    sleep*) sh -c "$k";;
    *) sh -c "xdotool $k"; sleep 0.6;;
  esac
done
if [ -n "$KEYS" ]; then sleep 1.5; fi
if [ -n "$WID" ]; then
  import -window "$WID" "$OUT" || import -window root "$OUT"
else
  import -window root "$OUT"
fi
kill "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true
echo "wrote $OUT (wid=$WID)"
