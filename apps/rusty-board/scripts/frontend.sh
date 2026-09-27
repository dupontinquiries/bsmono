#!/usr/bin/env bash
# ============================================================
# RustyBoard — frontend.sh (sourced by install.sh / redeploy.sh)
#
# ONE definition of "which frontend, how is it built, where does the
# binary land". install.sh and redeploy.sh each carried their own copy
# of this and had already drifted: install.sh knew about --egui,
# redeploy.sh built the GTK app unconditionally and installed
# target/release/rusty-board, so a device set up with --egui got the
# GTK binary back on its first redeploy.
#
# Frontends:
#   egui  (default, the v2 keyboard) — apps/rusty-board/egui-frontend,
#         its own Cargo package. Built with --features wayland,layer so
#         it types into the focused window and docks as a layer surface;
#         without `wayland` it is a preview that types into itself.
#   gtk   — the GTK4/libadwaita original in apps/rusty-board/src.
#
# Usage from a script:
#   source "$(dirname "${BASH_SOURCE[0]}")/frontend.sh"
#   frontend_select "$FRONTEND"      # "egui" | "gtk"
#   frontend_build                   # cargo build --release, right dir + features
#   frontend_binary                  # prints the built binary's path
#   frontend_check_deps              # prints missing pkg-config deps, one per line
# ============================================================

FRONTEND_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND_NAME="egui"
# Cargo features for the egui build:
#   wayland — the input-method backend (types into the focused window)
#   layer   — the docked zwlr_layer_shell_v1 surface (bottom edge, exclusive
#             zone, never focused; falls back to a toplevel window at runtime
#             if the compositor has no layer shell)
# `voice` adds whisper-rs dictation (a native whisper.cpp build: cmake,
# a C++ compiler, ALSA headers; and a ggml model in ~/models or
# ~/.local/share/rusty-board/models — ggml-base.en-q5_1.gguf preferred,
# tiny for speed). Leave it to the person:
#   RUSTY_BOARD_EGUI_FEATURES="wayland,layer,voice" ./scripts/redeploy.sh
FRONTEND_EGUI_FEATURES="${RUSTY_BOARD_EGUI_FEATURES:-wayland,layer}"

frontend_select() {
    case "${1:-egui}" in
        egui|gtk) FRONTEND_NAME="$1" ;;
        *) echo "frontend.sh: unknown frontend '$1' (egui|gtk)" >&2; return 2 ;;
    esac
}

# The package directory the build runs in.
frontend_dir() {
    case "$FRONTEND_NAME" in
        egui) echo "$FRONTEND_ROOT/egui-frontend" ;;
        gtk)  echo "$FRONTEND_ROOT" ;;
    esac
}

# Where cargo puts release artifacts for that directory. Honours
# CARGO_TARGET_DIR, because the sandbox build notes in CLAUDE.md say to
# shunt target/ elsewhere and a script that ignores that installs a
# binary that does not exist.
frontend_target_dir() {
    if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
        echo "$CARGO_TARGET_DIR"
    else
        echo "$(frontend_dir)/target"
    fi
}

frontend_binary() {
    case "$FRONTEND_NAME" in
        egui) echo "$(frontend_target_dir)/release/rusty-board-egui" ;;
        gtk)  echo "$(frontend_target_dir)/release/rusty-board" ;;
    esac
}

frontend_build() {
    local dir; dir="$(frontend_dir)"
    case "$FRONTEND_NAME" in
        egui)
            # Explicit feature set, never the crate's defaults: the defaults now
            # include wayland+layer, so --no-wayland (FRONTEND_EGUI_FEATURES="")
            # would otherwise build the full keyboard anyway.
            local feats="app${FRONTEND_EGUI_FEATURES:+,$FRONTEND_EGUI_FEATURES}"
            echo "==> cargo build --release --no-default-features --features $feats  (in $dir)"
            ( cd "$dir" && cargo build --release --no-default-features --features "$feats" )
            ;;
        gtk)
            echo "==> cargo build --release  (in $dir)"
            ( cd "$dir" && cargo build --release )
            ;;
    esac
}

# Missing system dependencies for the selected frontend, one per line.
# The egui build needs NONE of gtk4/libadwaita/gtk4-layer-shell; listing
# them for it (as install.sh did) sent people installing 200 MB of GTK
# headers for a build that never links them.
frontend_check_deps() {
    command -v pkg-config >/dev/null 2>&1 || echo "pkg-config"
    if ! pkg-config --exists xkbcommon 2>/dev/null; then
        echo "libxkbcommon-dev (or libxkbcommon-devel) — keymap synthesis for the Wayland virtual keyboard"
    fi
    if [[ "$FRONTEND_NAME" == "gtk" ]]; then
        pkg-config --exists gtk4 2>/dev/null        || echo "libgtk-4-dev (or gtk4-devel)"
        pkg-config --exists libadwaita-1 2>/dev/null || echo "libadwaita-1-dev (or libadwaita-devel)"
        pkg-config --exists gtk4-layer-shell-0 2>/dev/null || echo "libgtk4-layer-shell-dev (or gtk4-layer-shell-devel) — Wayland docking"
    fi
}

# Per-distro one-liner for whatever is missing.
frontend_deps_hint() {
    if [[ "$FRONTEND_NAME" == "gtk" ]]; then
        cat <<'HINT'
On Debian/Ubuntu:  sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libgtk4-layer-shell-dev libxkbcommon-dev libasound2-dev
On Fedora:         sudo dnf install gcc pkgconf-pkg-config gtk4-devel libadwaita-devel gtk4-layer-shell-devel libxkbcommon-devel alsa-lib-devel
On Arch:           sudo pacman -S base-devel pkgconf gtk4 libadwaita gtk4-layer-shell libxkbcommon alsa-lib
On Alpine (pmOS):  sudo apk add build-base pkgconf gtk4.0-dev libadwaita-dev gtk4-layer-shell-dev libxkbcommon-dev alsa-lib-dev
HINT
    else
        cat <<'HINT'
On Debian/Ubuntu:  sudo apt install build-essential pkg-config libxkbcommon-dev
On Fedora:         sudo dnf install gcc pkgconf-pkg-config libxkbcommon-devel
On Arch:           sudo pacman -S base-devel pkgconf libxkbcommon
On Alpine (pmOS):  sudo apk add build-base pkgconf libxkbcommon-dev
HINT
    fi
}
