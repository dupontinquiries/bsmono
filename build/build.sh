#!/usr/bin/env bash
# build/build.sh — cross-target build orchestrator.
# Reads manifest/projects.toml + targets.toml, expands to (project × target),
# and either PRINTS THE PLAN (default) or RUNS it (--run). Real execution is
# host-target cargo builds; cross targets and C++/cmake/hybrid projects print
# what they'd run (they need cross sysroots / Qt6 / cmake deps — hardware stage).
# Flatpak generation: see build/flatpak.sh.
#   build/build.sh                      # plan, all projects/targets
#   build/build.sh --only snapclone     # one project
#   build/build.sh --target phone       # one target id
#   build/build.sh --run --only peek    # actually build (host cargo only)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN=0; ONLY=""; TGT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --run) RUN=1 ;;
    --only) ONLY="${2:-}"; shift ;;
    --target) TGT="${2:-}"; shift ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac; shift
done
HOST="$( { rustc -vV 2>/dev/null || true; } | sed -n 's/^host: //p' || true)"; HOST="${HOST:-x86_64-unknown-linux-gnu}"

rows="$(python3 - "$ROOT" <<'PY'
import sys, os, tomllib
root = sys.argv[1]
P = tomllib.load(open(os.path.join(root,"manifest","projects.toml"),"rb"))
T = tomllib.load(open(os.path.join(root,"manifest","targets.toml"),"rb"))["targets"]
def expand(ids):
    if "all" in ids: return [(k,v["triple"]) for k,v in T.items()]
    return [(i,T[i]["triple"]) for i in ids if i in T]
for grp, items in P.items():
    if not isinstance(items, dict): continue
    for name, m in items.items():
        if not isinstance(m, dict) or "path" not in m: continue
        for tid, triple in expand(m.get("targets", [])):
            print("\t".join([name, m["path"], m.get("kind","cargo"),
                             str(m.get("flatpak","false")).lower(), tid, triple]))
PY
)"

echo "build orchestrator — $([ "$RUN" = 1 ] && echo RUN || echo PLAN)   host=$HOST"
[ -n "$ONLY" ] && echo "filter: only=$ONLY"
[ -n "$TGT" ] && echo "filter: target=$TGT"
echo

printf '%s\n' "$rows" | { last=""
while IFS=$'\t' read -r name path kind fp tid triple; do
  [ -z "$name" ] && continue
  [ -n "$ONLY" ] && [ "$ONLY" != "$name" ] && continue
  [ -n "$TGT" ] && [ "$TGT" != "$tid" ] && continue
  if [ "$name" != "$last" ]; then echo "▸ $name ($kind, flatpak=$fp)"; last="$name"; fi
  if [ "$RUN" = 1 ] && [ "$kind" = "cargo" ] && [ "$triple" = "$HOST" ]; then
    printf '    [%s] cargo build --release … ' "$tid"
    if ( cd "$ROOT/$path" && cargo build --release >/dev/null 2>&1 ); then echo "✓"; else echo "✗ (cd $path && cargo build --release)"; fi
  else
    case "$kind" in
      cargo)        plan="cargo build --release --target $triple" ;;
      hybrid)       plan="cmake (C++ libs) → cargo --target $triple" ;;
      cmake)        plan="cmake -B build-$tid && cmake --build build-$tid" ;;
      python+rust)  plan="package (.py + .rs); not cross-built" ;;
      *)            plan="(unknown kind)" ;;
    esac
    note=""
    if [ "$RUN" = 1 ]; then
      [ "$triple" != "$HOST" ] && note="  ← cross: needs 'rustup target add $triple' or cross"
      [ "$kind" != "cargo" ] && note="  ← needs C++/cmake deps (hardware stage)"
    fi
    printf '    [%s] %s%s\n' "$tid" "$plan" "$note"
  fi
done; }
