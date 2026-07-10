#!/usr/bin/env bash
# test-all-guis.sh — build + launch the egui GUIs for rapid "feel tests".
#
# Feel tests are about motion, gesture, and layout — things you have to *see and touch*, not assert.
# This launches each egui front-end in its own window so you can flick through them quickly.
#
# Usage:
#   build/test-all-guis.sh              # list the GUIs, then prompt for one
#   build/test-all-guis.sh <name>       # launch one (e.g. rusty-board, voiceterm, gitmind)
#   build/test-all-guis.sh all          # launch each in turn (close one to get the next)
#   build/test-all-guis.sh list         # just list them
#   build/test-all-guis.sh --check      # compile every GUI target WITHOUT launching (headless CI)
#   build/test-all-guis.sh --release <name|all>   # optimized build (smoother; slower first compile)
#
# Notes:
#   * eframe needs a display + system GL/Wayland/X11 libs to build & run, so run this on a desktop.
#   * The lib-only front-ends (rusty-board, voiceterm) ship a `feel` example that wraps them in a
#     minimal eframe app; gitmind runs its real shell via its `app` feature.

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# name | directory (relative to repo root) | run args passed to cargo
GUIS=(
  "rusty-board|apps/rusty-board/egui-frontend|run --example feel"
  "voiceterm|apps/voiceterm/egui-frontend|run --example feel"
  "gitmind|apps/gitmind/crates/gui-egui|run --features app"
)
# Not listed: ambient-learner's egui Live view exists (apps/ambient-learner/src/ui/egui) and is
# unit-tested, but it isn't wired into the app's run path yet and ambient-learner is a bin-only crate
# (no lib target to build a `feel` example against), so it has no runnable target to launch here.

BOLD=$'\033[1m'; DIM=$'\033[2m'; GREEN=$'\033[32m'; RED=$'\033[31m'; RESET=$'\033[0m'

field() { echo "$1" | cut -d'|' -f"$2"; }

list_guis() {
  echo "${BOLD}Feel-test GUIs:${RESET}"
  local i=1
  for g in "${GUIS[@]}"; do
    printf "  %d) %-16s ${DIM}%s${RESET}\n" "$i" "$(field "$g" 1)" "$(field "$g" 2)"
    i=$((i + 1))
  done
}

# run cargo in a GUI's dir; extra args ($3+) are inserted after the subcommand (e.g. --release)
run_gui() {
  local name="$1" dir="$2" args="$3" mode="${4:-run}"
  local profile_flag="${RELEASE:+--release}"
  if [ ! -d "$dir" ]; then
    printf "${RED}✗ %s: missing dir %s${RESET}\n" "$name" "$dir"; return 1
  fi
  if [ "$mode" = "check" ]; then
    # swap the run subcommand for build so we compile without launching
    local build_args="${args/run/build}"
    printf "${BOLD}▶ checking %s${RESET} ${DIM}(cargo %s %s)${RESET}\n" "$name" "$build_args" "$profile_flag"
    ( cd "$dir" && cargo $build_args $profile_flag )
  else
    printf "${BOLD}▶ launching %s${RESET} ${DIM}(cargo %s %s)${RESET} — close the window to continue\n" \
      "$name" "$args" "$profile_flag"
    ( cd "$dir" && cargo $args $profile_flag )
  fi
}

gui_by_name() { for g in "${GUIS[@]}"; do [ "$(field "$g" 1)" = "$1" ] && { echo "$g"; return; }; done; }

# ---- arg parsing ----
RELEASE=""
if [ "${1:-}" = "--release" ]; then RELEASE=1; shift; fi
CMD="${1:-}"

case "$CMD" in
  ""|menu)
    list_guis
    printf "\nName (or 'all'): "
    read -r pick
    CMD="$pick"
    ;;
esac

case "$CMD" in
  list) list_guis ;;
  --check|check)
    fails=0
    for g in "${GUIS[@]}"; do
      run_gui "$(field "$g" 1)" "$(field "$g" 2)" "$(field "$g" 3)" check || fails=$((fails + 1))
    done
    if [ "$fails" -eq 0 ]; then printf "${GREEN}✓ all GUI targets compile${RESET}\n"; else
      printf "${RED}✗ %d GUI target(s) failed to compile${RESET}\n" "$fails"; exit 1; fi
    ;;
  all)
    for g in "${GUIS[@]}"; do
      run_gui "$(field "$g" 1)" "$(field "$g" 2)" "$(field "$g" 3)" || true
    done
    ;;
  *)
    g="$(gui_by_name "$CMD")"
    if [ -z "$g" ]; then printf "${RED}unknown GUI '%s'${RESET}\n\n" "$CMD"; list_guis; exit 1; fi
    run_gui "$(field "$g" 1)" "$(field "$g" 2)" "$(field "$g" 3)"
    ;;
esac
