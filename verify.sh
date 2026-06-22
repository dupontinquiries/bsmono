#!/usr/bin/env bash
# verify.sh — static "suspenders" over the monorepo. No compiling.
#
#   [rustcheck] tools/rustcheck/rustcheck.py --fast (Tier-0 static). TRUSTWORTHY:
#               low false-positive; reports real syntax/structural breakage.
#   [wiring]    COARSE candidate scan using peek's heuristic generalized to each
#               layout. Flags modules with state+lifecycle and no `module::` token
#               reference. KNOWN TO OVER-REPORT: it cannot see wiring done via
#               `mod m;` / `use`/ re-export (verified: bo-suite 7/7 flags were
#               false positives). Treat output as candidates only; the authority
#               is the real `peek` binary (resolves the actual module graph) at
#               the build stage. Not a pass/fail gate.
#   [drift]     spec-only today; reported, not run. Build stage.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
RC="$ROOT/tools/rustcheck/rustcheck.py"
PER_PROJECT_TIMEOUT="${WIRING_TIMEOUT:-90}"
WIRING="${WIRING:-1}"   # set WIRING=0 to skip the coarse wiring scan

projects=$(find "$ROOT/apps" "$ROOT/system" -maxdepth 1 -mindepth 1 -type d | sort)
projects="$projects
$ROOT/tools/peek"

wiring_scan() { # $1=project dir -> "total orphans candidate_dead_services"
  local P="$1" total=0 orphan=0 dead=0 m refs f
  while IFS= read -r f; do
    m=$(basename "$f" .rs)
    { [ "$m" = "mod" ] || [ "$m" = "lib" ] || [ "$m" = "main" ]; } && continue
    total=$((total+1))
    refs=$(grep -rlE "\b$m::|::$m\b" "$P" --include=*.rs 2>/dev/null | grep -v "/$m.rs" | grep -cv '^$')
    if [ "$refs" -eq 0 ]; then
      orphan=$((orphan+1))
      if grep -qE 'pub (struct|enum) ' "$f" 2>/dev/null && \
         grep -qE 'pub fn (new|with_|step|tick|update|apply|run)\b' "$f" 2>/dev/null; then
        dead=$((dead+1))
      fi
    fi
  done < <(find "$P" -name '*.rs' -not -path '*/target/*' 2>/dev/null)
  echo "$total $orphan $dead"
}

rc_problems() { # reads rustcheck json on stdin -> "<n> <kind>"
  python3 -c 'import sys,json
try: d=json.load(sys.stdin); s=d.get("summary",{}) or {}
except Exception: print("? parse"); raise SystemExit
n=(s.get("static_issues",0)+s.get("code_errors",0)+s.get("shader_errors",0))
print(n, "blocked" if s.get("blocked") else "ok")'
}

printf "%-26s | %-20s | %-26s\n" "project" "rustcheck --fast" "wiring CANDIDATES (m/orph/svc)"
printf -- "---------------------------+----------------------+----------------------------\n"
agg_rc=0
for P in $projects; do
  [ -d "$P" ] || continue
  name=$(basename "$P")
  out=$(python3 "$RC" --fast --json "$P" 2>/dev/null); rc=$?
  pr=$(printf '%s' "$out" | rc_problems 2>/dev/null); set -- $pr; n="${1:-?}"
  if [ "$n" = "?" ]; then rcs="(err)"; elif [ "$n" -gt 0 ] 2>/dev/null; then rcs="PROBLEMS($n)"; agg_rc=$((agg_rc+1)); elif [ "$rc" = 2 ]; then rcs="inconclusive"; else rcs="clean"; fi
  if [ "$WIRING" = 1 ]; then
    if w=$(timeout "$PER_PROJECT_TIMEOUT" bash -c "$(declare -f wiring_scan); wiring_scan '$P'" 2>/dev/null); then
      set -- $w; ws="$1/$2/$3"
    else ws="(timeout)"; fi
  else ws="(skipped)"; fi
  printf "%-26s | %-20s | %-26s\n" "$name" "$rcs" "$ws"
done
echo
echo "[drift]  spec-only — playground compiles; cargo-drift subcommands not implemented. Build stage."
echo
echo "VERDICT (trustworthy signal = rustcheck): projects-with-real-problems=$agg_rc"
echo "Wiring column is CANDIDATES only (over-reports; confirm with real peek). Not a gate."
