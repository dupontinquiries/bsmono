#!/usr/bin/env bash
# dev.sh — one entry point, two audiences.
#
# The problem this solves, from a real session: `cargo check` in blackstar-rs
# failed on alsa-sys for an entire day of work, and the fix was in its own
# Cargo.toml two lines from the error — "build with --no-default-features for a
# silent binary with zero extra system deps". audio is simply in the default
# feature set. Nothing was broken; the wrong invocation was being repeated.
#
# THE DEFAULTS MUST NOT CHANGE. A human running `cargo run --release` should get
# sound, and every packaging script, .desktop file and CI job depends on the
# default feature set meaning "the real app". Fixing an agent's build by
# removing audio from `default` would ship a silent binary to everyone.
#
# So the split is in the INVOCATION, not the manifest:
#
#   dev.sh run   <app>   what a person wants: release, all default features
#   dev.sh check <app>   what an agent wants: headless, no system libraries
#   dev.sh test  <app>   headless tests
#   dev.sh env           print what the agent profile sets, and why
#
# `run` is deliberately the shortest word and the first line of help, because
# the human path is the one that must be obvious.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

# ── the agent profile ────────────────────────────────────────────────────────
#
# Every value here was learned by hitting the wall it avoids.
#
# CARGO_BUILD_JOBS=1     This box has ~3 GB and one core. Parallel rustc
#                        processes exceed it TOGETHER and get killed with no
#                        message — the failure presents as a silent stall or a
#                        bogus linker error, not as OOM. That misdiagnosis cost
#                        several sessions.
# CARGO_PROFILE_*_DEBUG=0  Debug info is most of the memory and all of the disk
#                        in a check build, and nothing here reads a backtrace.
#                        fleet_build.sh already does this.
# --no-default-features  Skips audio (alsa), display (winit) and other system
#                        libraries an agent container does not have.
agent_env() {
    export CARGO_BUILD_JOBS=1
    export CARGO_PROFILE_DEV_DEBUG=0
    export CARGO_PROFILE_TEST_DEBUG=0
    # Out of the repo: a target dir inside it gets picked up by `git status`,
    # swept into an archive, or committed by accident.
    export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/dev-target}"
}

# Where an app lives. Apps are separate workspaces, so a name has to be resolved
# to a directory rather than passed to a root-level cargo.
app_dir() {
    local name="$1"
    for base in apps games system shared; do
        [[ -d "$ROOT/$base/$name" ]] && { echo "$ROOT/$base/$name"; return 0; }
    done
    # egui frontends are a crate inside their app
    for base in apps games; do
        [[ -d "$ROOT/$base/$name/egui-frontend" ]] && { echo "$ROOT/$base/$name/egui-frontend"; return 0; }
    done
    echo "no such app: $name" >&2
    echo "try: ls $ROOT/apps $ROOT/games" >&2
    return 1
}

# Does this crate HAVE a no-default-features build worth using?
#
# Asked rather than assumed: a crate whose default set is empty gains nothing,
# and passing the flag to one that keeps its whole library behind a feature
# produces an empty build that passes and proves nothing — which is worse than
# a failure because it looks like success.
has_optional_defaults() {
    grep -qE '^default = \[.+\]' "$1/Cargo.toml" 2>/dev/null
}

usage() {
    cat <<EOF
dev.sh — build entry points for humans and agents

  dev.sh run   <app> [args]   release + default features (audio, display)
  dev.sh check <app>          headless check, no system libraries
  dev.sh test  <app>          headless tests
  dev.sh env                  print the agent profile and the reason for each

  apps live in apps/, games/, system/, shared/ — a bare name is resolved.

The difference matters: `run` is what the app IS. `check` is what fits in a
container with no sound card and no display. Neither changes Cargo.toml, so a
packaged build is unaffected by anything here.
EOF
}

cmd="${1:-}"; shift || true
case "$cmd" in
    run)
        dir="$(app_dir "${1:?app name required}")"; shift || true
        # No agent_env: a human build should use every core and produce a
        # binary with the features the app ships with.
        cd "$dir" && exec cargo run --release "$@"
        ;;
    check)
        dir="$(app_dir "${1:?app name required}")"; shift || true
        agent_env
        cd "$dir"
        if has_optional_defaults "$dir"; then
            exec cargo check --all-targets --no-default-features "$@"
        else
            # Said out loud rather than silently doing something different, so
            # a green check is not mistaken for a headless one.
            echo "note: $dir has no optional default features — checking as-is" >&2
            exec cargo check --all-targets "$@"
        fi
        ;;
    test)
        dir="$(app_dir "${1:?app name required}")"; shift || true
        agent_env
        cd "$dir"
        if has_optional_defaults "$dir"; then
            exec cargo test --no-default-features "$@"
        else
            exec cargo test "$@"
        fi
        ;;
    env)
        agent_env
        # --export emits shell-eval'able lines. Separate from the annotated
        # form because the reasons are the point of the default output, and a
        # comment in an eval is noise.
        if [[ "${1:-}" == "--export" ]]; then
            echo "export CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS"
            echo "export CARGO_PROFILE_DEV_DEBUG=$CARGO_PROFILE_DEV_DEBUG"
            echo "export CARGO_PROFILE_TEST_DEBUG=$CARGO_PROFILE_TEST_DEBUG"
            echo "export CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
            exit 0
        fi
        echo "CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS        # 3 GB box: parallel rustc gets OOM-killed silently"
        echo "CARGO_PROFILE_DEV_DEBUG=$CARGO_PROFILE_DEV_DEBUG         # debug info is most of the memory, nothing reads it"
        echo "CARGO_PROFILE_TEST_DEBUG=$CARGO_PROFILE_TEST_DEBUG        # same"
        echo "CARGO_TARGET_DIR=$CARGO_TARGET_DIR   # outside the repo so git never sees it"
        echo
        echo "and --no-default-features, which skips alsa/winit/etc."
        echo "eval \"\$($0 env --export)\" to apply these to your own shell."
        ;;
    ""|-h|--help|help) usage ;;
    *) echo "unknown command: $cmd" >&2; usage; exit 2 ;;
esac
