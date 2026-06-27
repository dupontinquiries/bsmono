#!/usr/bin/env bash
# build/deploy.sh — ship phone-target builds to the Fairphone 5 fleet via gitmind.
#
# gitmind is both the GATE and the distribution layer:
#   desktop : develop -> commit -> `gin check` records pass/fail per commit
#   deploy  : pick a GREEN commit -> push it to each device in manifest/fleet.toml
#   device  : `gin app build|install ... --use --desktop` activates it on the phone
# Because .gitmind/store.redb rebuilds from the git graph, the git repo itself is
# the sync channel — a device `git pull` to the green commit is the transport; the
# binary is then either built there (mode=build) or pinned from a cross-build
# (mode=pin).
#
# Per-app modes (set in fleet.toml):
#   build  phone builds natively (aarch64, NO cross sysroot needed):
#            gin app build <name> --commit <C> --target phone --release --use --desktop
#   pin    desktop cross-builds aarch64-musl, pins the binary, phone activates:
#            (desktop) cargo build --release --target aarch64-unknown-linux-musl
#            gin app install <name> <bin> --commit <C> --target phone --use --desktop
#
# PLAN-printing by default; real execution needs fleet SSH + a green commit (--run).
#   build/deploy.sh                       # plan: green-candidate -> whole fleet
#   build/deploy.sh --commit <oid>        # plan a specific commit (must be green)
#   build/deploy.sh --only fp5-1          # one device
#   build/deploy.sh --run --commit <oid>  # execute (needs gin + ssh to the fleet)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN=0; COMMIT=""; ONLY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --run) RUN=1 ;;
    --commit) COMMIT="${2:-}"; shift ;;
    --only) ONLY="${2:-}"; shift ;;
    -h|--help) sed -n '2,28p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac; shift
done
GIN="$ROOT/apps/gitmind/target/release/gin"; [ -x "$GIN" ] || GIN="gin"
FLEET="$ROOT/manifest/fleet.toml"
[ -f "$FLEET" ] || { echo "no manifest/fleet.toml — copy the template and fill in devices" >&2; exit 1; }

echo "deploy — $([ "$RUN" = 1 ] && echo RUN || echo PLAN)"

# 1. resolve the commit to deploy: explicit --commit (verify green) or latest green candidate.
if [ -n "$COMMIT" ]; then
  echo "commit: $COMMIT  (will verify green via: gin check --commit $COMMIT)"
else
  echo "commit: <latest green> — resolve via: gin check --candidates  (newest ✓)"
  [ "$RUN" = 1 ] && COMMIT="$("$GIN" check --candidates 2>/dev/null | sed -n 's/^✓ \([0-9a-f]\{7,\}\).*/\1/p' | head -1)"
  [ "$RUN" = 1 ] && [ -z "$COMMIT" ] && { echo "no green candidates from gin check" >&2; exit 1; }
fi
echo

# 2. fleet × apps -> deploy plan, cross-referenced with projects.toml phone-targets.
python3 - "$ROOT" "$FLEET" "${ONLY}" "${COMMIT:-GREEN}" <<'PY'
import sys, os, tomllib
root, fleet_path, only, commit = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
P = tomllib.load(open(os.path.join(root,"manifest","projects.toml"),"rb"))
# which projects legitimately target the phone?
phone_ok=set()
for grp,items in P.items():
    if not isinstance(items,dict): continue
    for name,m in items.items():
        if isinstance(m,dict) and "phone" in (m.get("targets") or []): phone_ok.add(name)
F = tomllib.load(open(fleet_path,"rb")).get("fleet",{})
if not F: print("  (fleet.toml has no [fleet.*] devices)"); sys.exit(0)
for dev,cfg in F.items():
    if only and only!=dev: continue
    host=cfg.get("host","?"); user=cfg.get("user","user"); mode=cfg.get("mode","build")
    print(f"▸ {dev}  ({user}@{host}, mode={mode})")
    print(f"    ssh {user}@{host}  git -C <repo> fetch && git checkout {commit}   # sync the green commit")
    for app in cfg.get("apps",[]):
        warn = "" if app in phone_ok else "   ⚠ not marked target=phone in projects.toml"
        if mode=="build":
            print(f"    [{app}] (on device) gin app build {app} --commit {commit} --target phone --release --use --desktop{warn}")
        else:  # pin
            print(f"    [{app}] (desktop)   cargo build --release --target aarch64-unknown-linux-musl   # in {app}'s dir{warn}")
            print(f"    [{app}] (device)    gin app install {app} <bin> --commit {commit} --target phone --use --desktop")
PY
echo
echo "notes:"
echo "  - build mode needs the phone's build deps (rustup target / Qt6 / Vulkan headers); heavy"
echo "    apps (unendlich) are better as mode=pin once a cross sysroot exists."
echo "  - 'gin check' must have recorded the commit green first (build/verify stage)."
echo "  - real run needs reachable fleet SSH; this stub prints the gitmind commands it would issue."
