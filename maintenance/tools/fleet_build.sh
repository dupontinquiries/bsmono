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
#
# `--all-targets` is doing that, and it is the line worth defending. A change
# that only touches a lib module still needs it: voiceterm's binary built a
# struct with four of its six fields, `cargo test --lib` passed because the
# binary is not a lib target, and the error sat in main for a full session.
#
# So when working on one crate, `cargo check --all-targets` is the equivalent of
# this script for that crate. `--lib` alone is the check that misses exactly the
# code a user runs.
set -uo pipefail

# For a SINGLE app, use dev.sh rather than copying an invocation from here:
#
#   maintenance/tools/dev.sh check <app>    headless, no system libs
#   maintenance/tools/dev.sh run   <app>    release + audio, what a person wants
#
# It carries the --no-default-features rule this script does not, which is why
# `cargo check` on blackstar-rs failed on alsa-sys for a whole session.

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FILTER="${1:-}"
# Full logs for failures, so a 137-error build can be read without running it
# again. Under /tmp rather than in the repo: these are transient and a stray
# 4 MB log committed by accident is worse than one that vanishes on reboot.
LOGDIR="${FLEET_LOGDIR:-/tmp/fleet-logs}"
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

        # DISTINCT MESSAGES, GROUPED AND COUNTED, not the first two lines.
        #
        # `head -2` was actively misleading on a large failure: 137 errors in
        # ambient-learner are almost all ONE missing import cascading, and two
        # sample lines cannot say which. Worse, cargo prints its "could not
        # compile" summary LAST, so on a small failure head -2 could show the
        # summary and nothing about the cause.
        #
        # Collapsing identical messages and counting them puts the shape of the
        # failure first: "132x unresolved import" is a one-line fix, "17 distinct
        # type errors" is an afternoon.
        grep -E '^error' /tmp/fleet.$$ \
            | grep -vE '^error: could not compile' \
            | sed -E 's/^error(\[[A-Z0-9]+\])?: //' \
            | sort | uniq -c | sort -rn | head -5 \
            | sed -E 's/^ *([0-9]+) /       \1x /'

        # WHERE, not just what. An error message without a file is a search;
        # cargo's `--> path:line` lines are the actionable half and were being
        # dropped entirely. Shown for the first few distinct files so a
        # cascading failure does not bury the list.
        grep -E '^\s+--> ' /tmp/fleet.$$ \
            | sed -E 's/^\s+--> //' | cut -d: -f1 \
            | sort -u | head -3 \
            | sed 's/^/       in /'

        # The summary line names the CRATE that failed, which in a workspace is
        # not always the one being built — fao-codec's failure is in a member,
        # and knowing which saves opening the whole tree.
        grep -m1 -E '^error: could not compile' /tmp/fleet.$$ \
            | sed -E 's/^error: /       /' | cut -c1-100

        # Keep the full log for anything that failed, and say where. The old
        # script deleted it unconditionally, so reproducing a 137-error failure
        # meant running the whole build again.
        mkdir -p "$LOGDIR"
        cp /tmp/fleet.$$ "$LOGDIR/${label//\//-}.log"
        echo "       full log: $LOGDIR/${label//\//-}.log"
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
