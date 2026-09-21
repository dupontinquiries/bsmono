#!/bin/sh
# tideshell preflight for the Fairphone 5 (postmarketOS + cosmic). READ-ONLY:
# changes nothing. Run over ssh as the phone user while logged into cosmic:
#   sh preflight.sh | tee preflight.txt
# Every check prints PASS / WARN / FAIL. Any FAIL = stop and report.
set -u
UID_N=$(id -u)
RT=${XDG_RUNTIME_DIR:-/run/user/$UID_N}
fails=0
pass() { echo "PASS  $*"; }
warn() { echo "WARN  $*"; }
fail() { echo "FAIL  $*"; fails=$((fails + 1)); }
hr() { printf '\n== %s\n' "$*"; }

hr "system"
arch=$(uname -m)
[ "$arch" = aarch64 ] && pass "arch $arch" || fail "arch $arch (expected aarch64)"
grep -E '^(PRETTY_NAME|VERSION_ID)=' /etc/os-release | sed 's/^/      /'
echo "      kernel $(uname -r)"
df -h "$HOME" | tail -1 | awk '{print "      disk free (home): "$4}'
free -m | awk '/^Mem:/{print "      RAM total/avail MB: "$2" / "$7}'

hr "toolchain (builds happen ON the phone)"
if command -v cargo >/dev/null 2>&1; then
    pass "$(cargo --version)"
    echo "      $(rustc --version)  (tideshell is developed on 1.96; older may fail on dependency MSRV)"
else
    fail "cargo not found (apk add cargo, or rustup)"
fi
for pc in wayland-client egl; do
    if pkg-config --exists "$pc" 2>/dev/null; then pass "pkg-config $pc $(pkg-config --modversion $pc)"
    else warn "pkg-config $pc missing (build needs the -dev package; Alpine names likely wayland-dev / mesa-dev, unverified)"; fi
done

hr "cosmic session"
if pgrep -x cosmic-comp >/dev/null; then pass "cosmic-comp running (pid $(pgrep -x cosmic-comp | head -1))"; else fail "cosmic-comp not running: log into cosmic first"; fi
v=$(apk info -v 2>/dev/null | grep -E '^cosmic-comp-[0-9]' | head -1)
echo "      package: ${v:-unknown}  (patches in cosmic-comp/ are against epoch-1.8.0)"
pgrep -a cosmic-panel >/dev/null && warn "cosmic-panel running: its reserved space and tideshell's edge strips/bar may overlap (report a screenshot)" || pass "no cosmic-panel"
pgrep -x rusty-board >/dev/null && pass "rusty-board running" || warn "rusty-board not running (tideshell's Keyboard mode will report the failure)"
SOCK=$(ls "$RT" 2>/dev/null | grep -E '^wayland-[0-9]+$' | head -1)
[ -n "$SOCK" ] && pass "wayland socket $RT/$SOCK" || fail "no wayland socket in $RT"

hr "session bus (needed for the EI trackpad)"
if busctl --user list >/dev/null 2>&1; then
    pass "user session bus reachable"
    busctl --user list 2>/dev/null | grep -q com.system76.CosmicComp && pass "com.system76.CosmicComp on the bus" || warn "com.system76.CosmicComp not on the user bus: trackpad EI will fail"
    busctl --user list 2>/dev/null | grep -q io.rustyboard.Keyboard && pass "io.rustyboard.Keyboard on the bus" || warn "io.rustyboard.Keyboard not on the bus"
else
    warn "no user session bus from this shell (try: export DBUS_SESSION_BUS_ADDRESS=unix:path=$RT/bus)"
fi

hr "compositor protocols (tideshell needs these)"
if command -v wayland-info >/dev/null 2>&1 && [ -n "$SOCK" ]; then
    list=$(XDG_RUNTIME_DIR=$RT WAYLAND_DISPLAY=$SOCK wayland-info 2>/dev/null | grep -oE "interface: '[a-z0-9_]+'" | sed "s/interface: //; s/'//g")
    for i in zwlr_layer_shell_v1 ext_foreign_toplevel_list_v1 zcosmic_toplevel_info_v1 zcosmic_toplevel_manager_v1; do
        echo "$list" | grep -qx "$i" && pass "$i" || fail "$i missing (required)"
    done
    for i in wp_fractional_scale_manager_v1 wp_viewporter wp_cursor_shape_manager_v1 zwlr_output_manager_v1; do
        echo "$list" | grep -qx "$i" && pass "$i" || warn "$i missing (optional: tideshell falls back)"
    done
else
    warn "wayland-info not available (apk add wayland-utils) - protocol check skipped"
fi

hr "output"
if command -v wlr-randr >/dev/null 2>&1; then XDG_RUNTIME_DIR=$RT WAYLAND_DISPLAY=$SOCK wlr-randr 2>/dev/null | sed 's/^/      /'
else warn "wlr-randr not installed (apk add wlr-randr) - scale/transform not shown"; fi

hr "touch input"
if command -v libinput >/dev/null 2>&1; then
    sudo -n libinput list-devices 2>/dev/null | awk 'BEGIN{RS=""} /touch/' | sed 's/^/      /'
else warn "libinput CLI missing (apk add libinput-tools, name unverified)"; fi

hr "tideshell leftovers"
[ -f "$HOME/.config/tideshell/cursor_hide_timeout.saved" ] && warn "a previous tideshell run left the cursor override pending; tideshell restores it at start" || pass "no pending cursor override"

echo
[ "$fails" -eq 0 ] && echo "PREFLIGHT OK ($fails failures)" || echo "PREFLIGHT FAILED ($fails failures) - stop and report this output"
