#!/bin/sh
# Two snapclone-egui instances side by side under Xvfb, each with its own
# XDG home and listen port, both on the shared gossip port (SO_REUSEPORT).
#   tools/snap2.sh <out.png> <settle> [-a "xdotool cmd for A"] [-b "cmd for B"] [-s secs]...
# Commands run in the order given; $WA / $WB are the window ids.
set -e
OUT="$1"; SETTLE="$2"; shift 2
export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe WAYLAND_DISPLAY=
if ! xdpyinfo >/dev/null 2>&1; then
  Xvfb :99 -screen 0 1400x1000x24 +extension GLX >/dev/null 2>&1 & sleep 2; openbox >/dev/null 2>&1 & sleep 1
fi
BIN=/home/claude/target-snap/debug/snapclone-egui
A_CODE=$(grep -o "sc-[0-9a-f]*" /home/claude/snapA/config/snapclone/identity.conf | head -1)
XDG_CONFIG_HOME=/home/claude/snapA/config XDG_DATA_HOME=/home/claude/snapA/data XDG_RUNTIME_DIR=/home/claude/snapA \
  SNAPCLONE_TEST_PATTERN=1 $BIN > /home/claude/snapA/run.log 2>&1 &
PA=$!
sleep 1
XDG_CONFIG_HOME=/home/claude/snapB/config XDG_DATA_HOME=/home/claude/snapB/data XDG_RUNTIME_DIR=/home/claude/snapB \
  SNAPCLONE_TEST_PATTERN=1 SNAPCLONE_TEST_PATTERN_CODE="$A_CODE" $BIN > /home/claude/snapB/run.log 2>&1 &
PB=$!
sleep "$SETTLE"
WA=$(xdotool search --onlyvisible --pid "$PA" | head -1)
WB=$(xdotool search --onlyvisible --pid "$PB" | head -1)
export WA WB
xdotool windowsize "$WA" 420 820; xdotool windowmove "$WA" 0 0
xdotool windowsize "$WB" 420 820; xdotool windowmove "$WB" 480 0
sleep 1.5
while [ "$1" != "" ]; do
  case "$1" in
    -a) xdotool windowactivate "$WA" 2>/dev/null || true; sleep 0.3; sh -c "xdotool $2"; sleep 0.8; shift 2;;
    -b) xdotool windowactivate "$WB" 2>/dev/null || true; sleep 0.3; sh -c "xdotool $2"; sleep 0.8; shift 2;;
    -s) sleep "$2"; shift 2;;
    *) echo "bad arg $1" >&2; exit 2;;
  esac
done
sleep 1
import -window root -crop 900x820+0+0 "$OUT"
kill "$PA" "$PB" 2>/dev/null || true
wait "$PA" "$PB" 2>/dev/null || true
echo "wrote $OUT"
#!/bin/sh
# Two snapclone-egui instances side by side under Xvfb, each with its own
# XDG home and listen port, both on the shared gossip port (SO_REUSEPORT).
#   tools/snap2.sh <out.png> <settle> [-a "xdotool cmd for A"] [-b "cmd for B"] [-s secs]...
# Commands run in the order given; $WA / $WB are the window ids.
set -e
OUT="$1"; SETTLE="$2"; shift 2
export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe WAYLAND_DISPLAY=
if ! xdpyinfo >/dev/null 2>&1; then
  Xvfb :99 -screen 0 1400x1000x24 +extension GLX >/dev/null 2>&1 & sleep 2; openbox >/dev/null 2>&1 & sleep 1
fi
BIN=/home/claude/target-snap/debug/snapclone-egui
A_CODE=$(grep -o "sc-[0-9a-f]*" /home/claude/snapA/config/snapclone/identity.conf | head -1)
XDG_CONFIG_HOME=/home/claude/snapA/config XDG_DATA_HOME=/home/claude/snapA/data XDG_RUNTIME_DIR=/home/claude/snapA \
  SNAPCLONE_TEST_PATTERN=1 $BIN > /home/claude/snapA/run.log 2>&1 &
PA=$!
sleep 1
XDG_CONFIG_HOME=/home/claude/snapB/config XDG_DATA_HOME=/home/claude/snapB/data XDG_RUNTIME_DIR=/home/claude/snapB \
  SNAPCLONE_TEST_PATTERN=1 SNAPCLONE_TEST_PATTERN_CODE="$A_CODE" $BIN > /home/claude/snapB/run.log 2>&1 &
PB=$!
sleep "$SETTLE"
WA=$(xdotool search --onlyvisible --pid "$PA" | head -1)
WB=$(xdotool search --onlyvisible --pid "$PB" | head -1)
export WA WB
xdotool windowsize "$WA" 420 820; xdotool windowmove "$WA" 0 0
xdotool windowsize "$WB" 420 820; xdotool windowmove "$WB" 480 0
sleep 1.5
while [ "$1" != "" ]; do
  case "$1" in
    -a) xdotool windowactivate "$WA" 2>/dev/null || true; sleep 0.3; sh -c "xdotool $2"; sleep 0.8; shift 2;;
    -b) xdotool windowactivate "$WB" 2>/dev/null || true; sleep 0.3; sh -c "xdotool $2"; sleep 0.8; shift 2;;
    -s) sleep "$2"; shift 2;;
    *) echo "bad arg $1" >&2; exit 2;;
  esac
done
sleep 1
import -window root -crop 900x820+0+0 "$OUT"
kill "$PA" "$PB" 2>/dev/null || true
wait "$PA" "$PB" 2>/dev/null || true
echo "wrote $OUT"
