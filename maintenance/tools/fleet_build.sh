#!/usr/bin/env bash
# fleet_build.sh — does every app still compile against the shared crates?
#
# Written after two apps were found broken only because someone went looking.
# av1tuner's TESTS did not compile (a struct gained a field and a test helper
# was not updated), and tomescope's BINARY did not (Toasts moved into a module,
# eframe::App gained a required method, SortKey::all changed shape). Both had
# been broken for a while, and neither showed up anywhere.
#
# The shape of the failure is what makes it worth a script: a shared crate
# changes, its own tests pass, every consumer that nobody rebuilt silently rots.
# The apps are separate cargo workspaces, so there is no single `cargo test` that
# would have caught it.
#
# Checks the LIB, the BINS and the TESTS separately, because they fail
# independently — av1tuner's library was fine while its tests were not, so a
# plain `cargo build` would have reported it healthy.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FILTER="${1:-}"
FAILED=()
OK=0

check() {
    local dir="$1" label="$2" mode="$3"
    ( cd "$dir" && CARGO_PROFILE_DEV_DEBUG=0 timeout 600 cargo check -q $mode 2>&1 ) >/tmp/fleet.$$ 2>&1
    # A missing system toolchain is an ENVIRONMENT gap, not code rot, and
    # reporting it as a failure teaches people to ignore this script. Qt, GTK and
    # OpenAL are all legitimately absent in a headless build.
    if grep -qE 'failed to run custom build command for .(qttypes|qmetaobject|gdk-pixbuf-sys|gtk4-sys|glib-sys|openal|openssl-sys|alsa-sys|libudev-sys|dbus|pango-sys|cairo-sys|atk-sys|gio-sys|gobject-sys|graphene-sys|gdk4-sys)' /tmp/fleet.$$; then
        local dep; dep=$(grep -oE 'custom build command for `[^`]+`' /tmp/fleet.$$ | head -1)
        echo "  SKIP $label — needs a system toolchain ($dep)"
        rm -f /tmp/fleet.$$
        return
    fi
    if grep -qE '^error' /tmp/fleet.$$; then
        local n; n=$(grep -cE '^error' /tmp/fleet.$$)
        FAILED+=("$label ($n errors)")
        echo "  FAIL $label — $n errors"
        grep -E '^error' /tmp/fleet.$$ | head -2 | sed 's/^/       /'
    else
        OK=$((OK+1))
    fi
    rm -f /tmp/fleet.$$
}

for manifest in "$ROOT"/apps/*/Cargo.toml "$ROOT"/apps/*/egui-frontend/Cargo.toml; do
    [ -f "$manifest" ] || continue
    dir="$(dirname "$manifest")"
    label="${dir#"$ROOT"/apps/}"
    [ -n "$FILTER" ] && [[ "$label" != *"$FILTER"* ]] && continue
    echo "==> $label"
    # --all-targets covers bins, examples and tests in one pass; they rot
    # independently and a lib-only check reports a broken app as healthy.
    check "$dir" "$label" "--all-targets"
done

echo
if [ ${#FAILED[@]} -eq 0 ]; then
    echo "all $OK targets compile"
    exit 0
fi
echo "${#FAILED[@]} broken, $OK ok:"
printf '  %s\n' "${FAILED[@]}"
exit 1
