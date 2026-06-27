#!/usr/bin/env bash
# build/deploy.sh — get an app onto the Fairphone 5 fleet.
#
# Reality check: your apps are GUI apps (Qt6 / GTK4 / Vulkan). `rustup target add
# aarch64-unknown-linux-musl` only installs Rust's std for musl — NOT Qt6/GTK4/Mesa
# built for musl. So you cannot cross-compile them on the laptop without a full
# aarch64-musl sysroot. The realistic path is to BUILD ON THE PHONE: postmarketOS
# ships those libraries as apk packages. Pure-Rust binaries (gin, peek) cross-build
# on the laptop fine — use mode=cross for those.
#
# ── one-time phone setup (the script can't do this; do it once per device) ──
#   sudo apk add rust cargo git
#   # + the app's toolkit:
#   #   voiceterm / bo-suite (Qt):  sudo apk add qt6-qtbase-dev qt6-qtdeclarative-dev pulseaudio-dev
#   #   logscope / av1tuner (GTK):  sudo apk add gtk4.0-dev
#   #   unendlich (Vulkan):         sudo apk add vulkan-headers mesa-dev
#   git clone <repo-or-bundle> ~/monorepo      # the phone's working checkout
#
# Modes (per device, in manifest/fleet.toml):
#   build  (default): ssh phone → git checkout <commit> → cargo build --release →
#                     gin import → gin app install <app> … --use --desktop
#   cross  (pure-Rust only): laptop cross-builds aarch64-musl → scp → gin app install
#
# PLAN by default (prints the exact ssh/cargo/scp). --run executes.
#   build/deploy.sh --app voiceterm --only fp5-1            # show me what it will do
#   build/deploy.sh --app voiceterm --only fp5-1 --run      # do it
#   build/deploy.sh --app gin       --only fp5-1 --run      # gin: laptop cross-build (set mode=cross)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN=0; APP=""; ONLY=""; COMMIT=""
while [ $# -gt 0 ]; do case "$1" in
  --run) RUN=1 ;;
  --app) APP="${2:-}"; shift ;;
  --only) ONLY="${2:-}"; shift ;;
  --commit) COMMIT="${2:-}"; shift ;;
  -h|--help) sed -n '2,35p' "$0"; exit 0 ;;
  *) echo "unknown arg: $1" >&2; exit 2 ;;
esac; shift; done
[ -n "$APP" ] || { echo "need --app <name>. phone-target apps: voiceterm unendlich ambient-learner bo-suite snapclone photosync (or gin/peek via mode=cross)" >&2; exit 2; }
FLEET="$ROOT/manifest/fleet.toml"
[ -f "$FLEET" ] || { echo "no manifest/fleet.toml (copy the template, fill in device addresses)" >&2; exit 1; }
COMMIT="${COMMIT:-$(git -C "$ROOT" rev-parse --short HEAD)}"
# gin (pure-Rust CLI) lives in apps/gitmind, binary name 'gin'
APPDIR="apps/$APP"; BIN="$APP"
[ "$APP" = "gin" ] && APPDIR="apps/gitmind" && BIN="gin"

echo "deploy app=$APP commit=$COMMIT — $([ "$RUN" = 1 ] && echo RUN || echo PLAN)"
do_run() { echo "    \$ $*"; if [ "$RUN" = 1 ]; then "$@"; fi; }

python3 - "$FLEET" "$ONLY" <<'PY' | while IFS='|' read -r DEV HOST USER MODE REPO; do
import sys, tomllib
F=tomllib.load(open(sys.argv[1],"rb")).get("fleet",{}); only=sys.argv[2]
for d,c in F.items():
    if only and only!=d: continue
    print("|".join([d, c.get("host",""), c.get("user","user"), c.get("mode","build"), c.get("repo","~/monorepo")]))
PY
  echo "▸ $DEV  ($USER@$HOST, mode=$MODE)"
  if [ "$MODE" = "cross" ]; then
    echo "  laptop cross-build (pure-Rust apps only) → scp → register on phone:"
    do_run sh -c "cd '$ROOT/$APPDIR' && cargo build --release --target aarch64-unknown-linux-musl --bin $BIN"
    LBIN="$ROOT/$APPDIR/target/aarch64-unknown-linux-musl/release/$BIN"
    do_run scp "$LBIN" "$USER@$HOST:~/$BIN"
    do_run ssh "$USER@$HOST" "gin app install $APP ~/$BIN --commit $COMMIT --target phone --use --desktop"
  else
    echo "  phone pulls the commit, builds natively, registers (needs rust + the app's apk deps already installed):"
    do_run ssh "$USER@$HOST" "cd $REPO && git fetch --all && git checkout $COMMIT && cd $APPDIR && cargo build --release --bin $BIN && gin import && gin app install $APP target/release/$BIN --commit $COMMIT --target phone --use --desktop"
  fi
done
echo
echo "next: open the app from your launcher (gin app install --desktop made an entry), or 'gin app run $APP' on the phone."
