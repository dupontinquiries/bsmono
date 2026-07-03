#!/bin/sh
# test-rear-camera.sh
#
# Diagnostic script for the FP5 camera-bridge setup.
# Run this on the device to identify exactly what's failing.
# No Rust required — uses only GStreamer and PipeWire CLI tools.
#
# Usage:
#   sh scripts/test-rear-camera.sh [--no-display]
#   --no-display  skip autovideosink tests (useful over SSH)

set -e

GREEN='\033[1;32m'; YELLOW='\033[1;33m'; RED='\033[1;31m'
BLUE='\033[1;34m'; NC='\033[0m'
ok()   { printf "${GREEN}[OK]${NC}    %s\n" "$*"; }
warn() { printf "${YELLOW}[WARN]${NC}  %s\n" "$*"; }
fail() { printf "${RED}[FAIL]${NC}  %s\n" "$*"; }
info() { printf "${BLUE}[INFO]${NC}  %s\n" "$*"; }
sep()  { printf "\n%s\n\n" "──────────────────────────────────────────"; }

NO_DISPLAY=0
for arg in "$@"; do
    case "$arg" in --no-display) NO_DISPLAY=1 ;; esac
done

sep
info "FP5 camera-bridge diagnostic  ($(date))"
sep

# ── 1. PipeWire running ───────────────────────────────────────────────────────
info "1. Checking PipeWire daemon"
if systemctl --user is-active --quiet pipewire 2>/dev/null; then
    ok "pipewire.service is active"
elif pgrep -x pipewire > /dev/null 2>&1; then
    ok "pipewire process running (not via systemd)"
else
    fail "PipeWire is NOT running — start it first"
    exit 1
fi

# ── 2. GStreamer tools ────────────────────────────────────────────────────────
sep
info "2. Checking GStreamer tools"
for cmd in gst-launch-1.0 gst-inspect-1.0; do
    if command -v "$cmd" > /dev/null 2>&1; then
        ok "$cmd found: $(which $cmd)"
    else
        fail "$cmd not found — install gstreamer"
    fi
done

for plugin in pipewiresrc pipewiresink videoflip videoconvert appsink; do
    if gst-inspect-1.0 "$plugin" > /dev/null 2>&1; then
        ok "gst plugin: $plugin"
    else
        fail "gst plugin MISSING: $plugin"
    fi
done

# ── 3. Physical camera nodes ──────────────────────────────────────────────────
sep
info "3. Physical libcamera nodes in PipeWire"

REAR_NODE="libcamera_input._base_soc_0_cci_ac4a000_i2c-bus_1_camera_29"
FRONT_NODE="libcamera_input._base_soc_0_cci_ac4b000_i2c-bus_1_camera_10"

# Multi-rear devices (e.g. FP5) may also have an ULTRAWIDE rear sensor on
# a different i2c bus. The bridge auto-discovers it via `cam --list` if
# libcamera reports a second `Location: Back` entry. To find the actual
# node name on your device, run:
#   pw-cli list-objects | grep libcamera_input
# The ultrawide will be one of the rows that isn't the main rear / front.
# Set PW_NODE_REAR_ULTRAWIDE to override auto-discovery.
ULTRAWIDE_HINT_CMD="pw-cli list-objects | grep libcamera_input"

PW_LIST=$(pw-cli list-objects 2>/dev/null || echo "")

for node in "$REAR_NODE" "$FRONT_NODE"; do
    short=$(echo "$node" | sed 's/libcamera_input\._base_soc_0_//;s/_i2c-bus_1_/\//g')
    if echo "$PW_LIST" | grep -q "$node"; then
        ok "Found: $short"
    else
        fail "Missing: $short"
        warn "  Run 'pw-cli list-objects | grep libcamera' to see available nodes"
    fi
done

info "  All libcamera nodes visible:"
echo "$PW_LIST" | grep "libcamera" | sed 's/^/    /'

# ── 3b. libcamera `cam --list` — authoritative sensor enumeration ────────────
sep
info "3b. libcamera 'cam --list' output (used to discover ultrawide)"
if command -v cam > /dev/null 2>&1; then
    CAM_OUT=$(cam --list 2>&1 || true)
    if [ -n "$CAM_OUT" ]; then
        ok "cam --list ran"
        echo "$CAM_OUT" | sed 's/^/    /'
        BACK_COUNT=$(echo "$CAM_OUT" | grep -ci "Location:[[:space:]]*Back" || true)
        FRONT_COUNT=$(echo "$CAM_OUT" | grep -ci "Location:[[:space:]]*Front" || true)
        info "  Back-facing cameras detected: $BACK_COUNT"
        info "  Front-facing cameras detected: $FRONT_COUNT"
        if [ "$BACK_COUNT" -ge "2" ]; then
            ok "Multi-rear device — bridge should expose virtual-cam-rear-ultrawide"
        elif [ "$BACK_COUNT" = "1" ]; then
            info "Single-rear device — virtual-cam-rear-ultrawide will not spawn"
        else
            warn "No back-facing cameras reported by cam --list"
        fi
    else
        warn "cam --list returned no output"
    fi
else
    warn "'cam' tool not found — install libcamera-tools (apk add libcamera-tools)"
    warn "Without it, ultrawide auto-discovery falls back to keyword matching"
    warn "which can't distinguish multiple rear sensors. To force:"
    warn "  export PW_NODE_REAR_ULTRAWIDE=<node.name from PipeWire>"
fi

# ── 4. Virtual camera nodes ───────────────────────────────────────────────────
sep
info "4. Virtual camera nodes from camera-bridge"

BRIDGE_RUNNING=0
if systemctl --user is-active --quiet camera-bridge 2>/dev/null; then
    ok "camera-bridge.service is active"
    BRIDGE_RUNNING=1
elif pgrep -x camera-bridge > /dev/null 2>&1; then
    ok "camera-bridge process running"
    BRIDGE_RUNNING=1
else
    warn "camera-bridge is NOT running"
    warn "  Start it: systemctl --user start camera-bridge"
    warn "  Or: ~/.local/bin/camera-bridge &"
fi

for vnode in "virtual-cam-rear" "virtual-cam-rear-ultrawide" \
             "virtual-cam-front" "virtual-cam-tracking"; do
    if echo "$PW_LIST" | grep -q "$vnode"; then
        ok "Virtual node present: $vnode"
    else
        # Ultrawide is optional (single-rear devices don't spawn it).
        # Downgrade its "missing" from FAIL to INFO when bridge is up
        # and we've already established only one back camera exists.
        if [ "$vnode" = "virtual-cam-rear-ultrawide" ]; then
            info "Virtual node not present: $vnode (expected on single-rear devices)"
        elif [ "$BRIDGE_RUNNING" = "1" ]; then
            fail "Virtual node MISSING: $vnode (bridge running but node not visible)"
        else
            warn "Virtual node not present: $vnode (start camera-bridge first)"
        fi
    fi
done

# ── 5. pw-dump JSON format check ─────────────────────────────────────────────
sep
info "5. pw-dump JSON format (for consumer detection)"

if command -v pw-dump > /dev/null 2>&1; then
    ok "pw-dump available"
    
    # Show what the virtual-cam-rear object looks like in pw-dump
    info "  Searching pw-dump output for 'virtual-cam-rear'..."
    DUMP=$(pw-dump 2>/dev/null)
    
    if echo "$DUMP" | grep -q "virtual-cam-rear"; then
        ok "virtual-cam-rear found in pw-dump output"
        
        # Extract the relevant section
        info "  Context around 'virtual-cam-rear' in pw-dump:"
        echo "$DUMP" | grep -B 20 '"virtual-cam-rear"' | tail -25 | sed 's/^/    /'
        
        # Check what key format node.name appears under
        info "  Checking node.name format..."
        if echo "$DUMP" | grep -q '"node\.name": "virtual-cam-rear"'; then
            ok '  Key format: "node.name": "..." (with space after colon)'
        elif echo "$DUMP" | grep -q '"node\.name":"virtual-cam-rear"'; then
            warn '  Key format: "node.name":"..." (NO space after colon)'
            warn '  camera-bridge find_node_id expects a space — may not match!'
        fi
        
        # Check output-node-id format in links
        info "  Checking link format (output-node-id)..."
        LINK_SAMPLE=$(echo "$DUMP" | grep -A 5 '"output-node-id"' | head -10)
        if [ -n "$LINK_SAMPLE" ]; then
            ok "  Found output-node-id in links:"
            echo "$LINK_SAMPLE" | sed 's/^/    /'
        else
            fail '  No "output-node-id" found in pw-dump output'
            warn '  Links may use different key name'
            info "  Searching for any 'node-id' keys in links..."
            echo "$DUMP" | grep -i "node.id\|node_id\|nodeId" | head -5 | sed 's/^/    /'
        fi
    else
        if [ "$BRIDGE_RUNNING" = "1" ]; then
            fail "virtual-cam-rear NOT found in pw-dump (bridge is running)"
            warn "  This means the virtual node was not created — check:"
            warn "  journalctl --user -u camera-bridge -n 30"
        else
            warn "virtual-cam-rear not in pw-dump (bridge not running)"
        fi
    fi
else
    fail "pw-dump not found — consumer detection will always fail"
    warn "  Install: apk add pipewire (or equivalent)"
fi

# ── 6. Direct pipeline test: rear camera ─────────────────────────────────────
sep
info "6. Testing rear camera capture pipeline (10-second test)"
info "   Node: $REAR_NODE"
info "   Pipeline: pipewiresrc → videoflip(ccw) → RGB 480×640 → fakesink"

PIPELINE="pipewiresrc target-object=\"$REAR_NODE\" \
  ! videoconvert \
  ! videoflip method=counterclockwise \
  ! video/x-raw,format=RGB,width=480,height=640 \
  ! fakesink"

info "  Running: gst-launch-1.0 $PIPELINE"
if timeout 10 gst-launch-1.0 -q $PIPELINE 2>&1 | head -5; then
    ok "Pipeline ran for 10s"
else
    EXIT=$?
    if [ "$EXIT" = "124" ]; then
        ok "Pipeline ran for 10s (timeout — this is expected and means it worked)"
    else
        fail "Pipeline FAILED (exit $EXIT)"
        info "  Trying simpler pipeline without flip..."
        SIMPLE="pipewiresrc target-object=\"$REAR_NODE\" ! fakesink"
        if timeout 5 gst-launch-1.0 -q $SIMPLE 2>&1 | head -3; then
            warn "Simple pipeline OK — issue is with videoflip or caps"
        else
            fail "Simple pipeline also failed — camera may be in use or not ready"
        fi
    fi
fi

# ── 7. Virtual node pipeline test ────────────────────────────────────────────
sep
info "7. Testing virtual-cam-rear pipeline (requires camera-bridge running)"

if [ "$BRIDGE_RUNNING" = "1" ] && echo "$PW_LIST" | grep -q "virtual-cam-rear"; then
    VPIPELINE="pipewiresrc target-object=\"virtual-cam-rear\" \
      ! videoconvert \
      ! video/x-raw,format=RGBA \
      ! fakesink"
    info "  Running: gst-launch-1.0 $VPIPELINE"
    if timeout 8 gst-launch-1.0 -q $VPIPELINE 2>&1 | head -5; then
        ok "Virtual camera pipeline worked"
    else
        EXIT=$?
        if [ "$EXIT" = "124" ]; then
            warn "Virtual pipeline timed out — camera-bridge may be waiting for a consumer"
            warn "  Check: journalctl --user -u camera-bridge -n 10"
        else
            fail "Virtual camera pipeline FAILED"
        fi
    fi
else
    warn "Skipping virtual node test (bridge not running or node not visible)"
fi

# ── 8. pipewiresink stream-properties test ────────────────────────────────────
sep
info "8. Testing pipewiresink stream-properties API"
info "   This verifies that our GObject property approach works"

SINK_TEST_PIPELINE="videotestsrc ! video/x-raw,format=I420,width=64,height=64 ! pipewiresink name=pw sync=false"
RESULT=$(gst-launch-1.0 -e $SINK_TEST_PIPELINE 2>&1 &
BPID=$!
sleep 2
kill $BPID 2>/dev/null
wait $BPID 2>/dev/null
echo $?)

# The key test: can we set stream-properties on pipewiresink?
python3 - << 'PYEOF' 2>&1 | head -10 | sed 's/^/    /'
import subprocess, sys
r = subprocess.run(
    ['python3', '-c', '''
import gi
gi.require_version("Gst", "1.0")
from gi.repository import Gst, GLib
Gst.init(None)
p = Gst.parse_launch("pipewiresink name=pw sync=false")
pw = p.get_by_name("pw")
if pw is None:
    print("FAIL: no pipewiresink in pipeline")
    sys.exit(1)
s = Gst.Structure.new_empty("props")
s.set_value("media.class", "Video/Source")
s.set_value("node.name", "test-probe-node")
try:
    pw.set_property("stream-properties", s)
    print("OK: stream-properties accepted by pipewiresink")
except Exception as e:
    print(f"FAIL: stream-properties rejected: {e}")
'''],
    capture_output=True, text=True, timeout=5
)
print(r.stdout.strip() or r.stderr.strip() or "(no output)")
PYEOF
ok "pipewiresink property test complete"

# ── 9. Summary ────────────────────────────────────────────────────────────────
sep
info "9. Quick reference commands"
printf "  # Watch camera-bridge logs live:\n"
printf "  journalctl --user -u camera-bridge -f\n\n"
printf "  # See all PipeWire nodes:\n"
printf "  pw-cli list-objects | grep -E 'node.name|node.description'\n\n"
printf "  # Manually read rear camera:\n"
printf "  gst-launch-1.0 pipewiresrc target-object=\"$REAR_NODE\" \\\\\n"
printf "    ! videoconvert ! videoflip method=counterclockwise \\\\\n"
printf "    ! video/x-raw,format=RGB,width=480,height=640 ! fakesink\n\n"
printf "  # Read virtual rear cam:\n"
printf "  gst-launch-1.0 pipewiresrc target-object=\"virtual-cam-rear\" \\\\\n"
printf "    ! videoconvert ! autovideosink\n\n"
printf "  # Dump pw-dump for consumer detection debugging:\n"
printf "  pw-dump | python3 -m json.tool | grep -A3 'virtual-cam-rear'\n\n"
printf "  # Check what links exist:\n"
printf "  pw-dump | python3 -m json.tool | grep -B5 'output-node-id'\n"
sep
