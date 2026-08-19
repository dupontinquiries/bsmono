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
TESTED=()          # "label PASS 128" / "label FAIL 3"
TEST_PIDS=()       # tests running in the background while the next crate builds

# TESTS RUN IN THE BACKGROUND, one per crate that compiled.
#
# The build of the NEXT crate is the idle time: cargo is single-crate-serial
# here because the apps are separate workspaces, so a test run costs nothing
# but the cores it borrows. Bounded by FLEET_TEST_JOBS because a fleet-wide
# `cargo test` at full width will thrash a laptop and swap a phone.
#
# `check` cannot become `test` in one step: a crate that FAILS to compile has
# no tests to run, and reporting "0 tests" for it would hide the build error
# behind a green number.
TEST_JOBS="${FLEET_TEST_JOBS:-2}"
RUN_TESTS="${FLEET_TEST:-1}"

reap_tests() {
    # Collect finished test runs. `wait -n` blocks for ANY of them, which is
    # what keeps the build moving rather than waiting on the slowest.
    while [ "${#TEST_PIDS[@]}" -ge "$1" ]; do
        wait -n 2>/dev/null || true
        local live=()
        for pid in "${TEST_PIDS[@]}"; do
            kill -0 "$pid" 2>/dev/null && live+=("$pid")
        done
        TEST_PIDS=("${live[@]}")
        # No progress possible — everything already exited.
        [ "${#live[@]}" -ge "$1" ] || break
    done
}

start_tests() {
    local dir="$1" label="$2"
    [ "$RUN_TESTS" = "1" ] || return
    reap_tests "$TEST_JOBS"
    (
        cd "$dir" || exit 0
        # --no-fail-fast: one failing crate in a workspace should not hide the
        # rest, which is the same reason this script exists at all.
        CARGO_PROFILE_DEV_DEBUG=0 timeout 900 cargo test -q --no-fail-fast \
            > "$LOGDIR/$(echo "$label" | tr '/' '-').test.log" 2>&1
        echo "$?" > "$LOGDIR/$(echo "$label" | tr '/' '-').test.rc"
    ) &
    TEST_PIDS+=($!)
}

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
    if ! grep -qE '^error' /tmp/fleet.$$; then
        # Compiled — hand it to a background test run and carry on building.
        start_tests "$dir" "$label"
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

# Wait for the last test runs. They were started as their crate finished
# building, so by here most have already landed.
if [ "$RUN_TESTS" = "1" ] && [ "${#TEST_PIDS[@]}" -gt 0 ]; then
    echo
    echo "==> waiting on ${#TEST_PIDS[@]} test run(s)"
    wait
fi

SUMMARY="$LOGDIR/summary.txt"
{
    echo "fleet build — $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "commit: $(git -C "$(dirname "$0")/../.." rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo "host:   $(uname -srm)"
    echo "rustc:  $(rustc --version 2>/dev/null || echo absent)"
    echo
} > "$SUMMARY"

TEST_FAIL=()
if [ "$RUN_TESTS" = "1" ]; then
    for rc in "$LOGDIR"/*.test.rc; do
        [ -e "$rc" ] || continue
        label="$(basename "$rc" .test.rc)"
        log="${rc%.rc}.log"
        # The COUNT comes from cargo's own summary line, not from the exit
        # code: a crate with several test binaries prints several, and summing
        # them is the number a person wants.
        passed=$(grep -hoE '^test result: ok\. [0-9]+' "$log" 2>/dev/null \
                 | grep -oE '[0-9]+' | paste -sd+ - | bc 2>/dev/null || echo 0)
        if [ "$(cat "$rc")" = "0" ]; then
            TESTED+=("$label PASS ${passed:-0}")
        else
            # A timeout and a real failure are different problems and the exit
            # code distinguishes them: 124 is timeout(1)'s.
            if [ "$(cat "$rc")" = "124" ]; then
                TEST_FAIL+=("$label TIMEOUT after 900s")
            else
                n=$(grep -cE '^(test .* FAILED|error)' "$log" 2>/dev/null || echo "?")
                TEST_FAIL+=("$label FAIL ($n)")
            fi
        fi
    done
fi

{
    if [ ${#FAILED[@]} -eq 0 ]; then
        echo "BUILD: all $OK targets compile"
    else
        echo "BUILD: ${#FAILED[@]} broken, $OK ok"
        printf '  %s\n' "${FAILED[@]}"
    fi
    echo
    if [ "$RUN_TESTS" = "1" ]; then
        total=$(printf '%s\n' "${TESTED[@]}" | awk '{s+=$3} END{print s+0}')
        echo "TESTS: ${#TESTED[@]} crate(s) green, $total test(s) passed, ${#TEST_FAIL[@]} crate(s) failing"
        [ ${#TEST_FAIL[@]} -gt 0 ] && printf '  %s\n' "${TEST_FAIL[@]}"
    else
        echo "TESTS: skipped (FLEET_TEST=0)"
    fi
} | tee -a "$SUMMARY"

# ONE ZIP, so a failure can be handed over whole.
#
# The reason this script exists is that broken crates went unnoticed; the
# reason logs get lost is that /tmp is per-machine and a report gets pasted
# without them. Everything needed to read a failure travels together.
ZIP="${FLEET_ZIP:-$LOGDIR/fleet-logs.zip}"
if command -v zip >/dev/null 2>&1; then
    ( cd "$LOGDIR" && zip -qr "$ZIP" . -x '*.zip' ) && echo && echo "logs + summary: $ZIP"
else
    # tar is on every machine this fleet targets; zip is not on a phone.
    ZIP="${ZIP%.zip}.tar.gz"
    ( cd "$LOGDIR" && tar czf "$ZIP" --exclude='*.tar.gz' . ) && echo && echo "logs + summary: $ZIP"
fi

# Non-zero for a build break OR a test break: a script that only fails on
# compilation teaches people that green means working, which is the belief
# this whole file exists to correct.
[ ${#FAILED[@]} -eq 0 ] && [ ${#TEST_FAIL[@]} -eq 0 ] && exit 0
exit 1
