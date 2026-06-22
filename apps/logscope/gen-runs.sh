#!/usr/bin/env bash
# gen-runs.sh — batch-run logscope-cli over a range of example scripts, capturing each to its own files.
#
#   ./gen-runs.sh range 1-4 maxtokens 1536 subdir ./runs/mylatestrun/ model ~/models/x.gguf
#   ./gen-runs.sh range 2 model mock                       # one script, deterministic mock backend
#   ./gen-runs.sh model ~/models/x.gguf                    # all scripts in ./examples
#
# Arguments are "key value" pairs (no dashes). All optional:
#   range     N | N-M      which numbered scripts (NN-*.logscope) to run        [default: all in examples/]
#   maxtokens N            per-reply generation cap -> --max (NOT context size)  [default: 1536]
#   subdir    DIR          where to write <name>.md (stdout) + <name>.log (err)  [default: ./runs/<timestamp>/]
#   model     PATH | mock  gguf path, or "mock" for the deterministic backend    [default: mock]
#   system    TEXT         system prompt -> --system ('' is best for R1-Distill) [default: '' ]
#   examples  DIR          directory holding the .logscope scripts               [default: ./examples]
#   cli       PATH         the logscope-cli binary                               [default: auto-detect]
#   final     md|ascii|git whole-tree dump format -> --final                     [default: md]
set -uo pipefail

range=""; maxtokens=1536; subdir=""; model="mock"; system=""; examples="./examples"; cli=""; final="md"
nokv=""; check=""; checkfile=""
kvtree="";

while [ $# -gt 0 ]; do
  key="$1"
  if [ $# -lt 2 ]; then echo "gen-runs.sh: missing value for '$key'" >&2; exit 2; fi
  val="$2"
  case "$key" in
    range)              range="$val" ;;
    maxtokens|max)      maxtokens="$val" ;;
    subdir|out)         subdir="$val" ;;
    model)              model="$val" ;;
    system)             system="$val" ;;
    examples)           examples="$val" ;;
    cli)                cli="$val" ;;
    final)              final="$val" ;;
    nokv)               nokv="$val" ;;          # on  → disable the KV-prefix-tree speedup (full prefill/turn)
    check)              check="$val" ;;         # shell cmd to compile a node's code (enables real :check verdicts)
    checkfile)          checkfile="$val" ;;
    kvtree)             kvtree="$val" ;;     # keep N branch tips warm (the prefix tree) — needed for branchy reuse     # path to drop code into for a real-crate check
    *) echo "gen-runs.sh: unknown arg '$key'" >&2; exit 2 ;;
  esac
  shift 2
done

# locate the binary if not given (works from the suite root or a logscope-egui checkout)
if [ -z "$cli" ]; then
  for c in ./target/release/logscope-cli ./target/debug/logscope-cli \
           ./crates/logscope-egui/target/release/logscope-cli ./crates/logscope-egui/target/debug/logscope-cli \
           "$(command -v logscope-cli 2>/dev/null || true)"; do
    if [ -n "$c" ] && [ -x "$c" ]; then cli="$c"; break; fi
  done
fi
if [ -z "$cli" ]; then
  echo "gen-runs.sh: logscope-cli not found. Build it:" >&2
  echo "  cargo build --release --bin logscope-cli --no-default-features [--features wgpu-engine]" >&2
  echo "or pass its path:  cli /path/to/logscope-cli" >&2
  exit 2
fi

[ -z "$subdir" ] && subdir="./runs/$(date +%Y%m%d-%H%M%S)/"
mkdir -p "$subdir"

if [ "$model" = "mock" ] || [ -z "$model" ]; then backend=(--mock); else backend=(--model "$model"); fi
extra=()
[ "$nokv" = "on" ] || [ "$nokv" = "1" ] && extra+=(--no-kv)
[ -n "$check" ] && extra+=(--check "$check")
[ -n "$checkfile" ] && extra+=(--check-file "$checkfile")
[ -n "$kvtree" ] && extra+=(--kv-tree "$kvtree")

# collect scripts: a range of NN-*.logscope, else every .logscope in examples/
scripts=()
if [ -n "$range" ]; then
  lo="${range%%-*}"; hi="${range##*-}"
  case "$lo$hi" in *[!0-9]*) echo "gen-runs.sh: range must be N or N-M" >&2; exit 2 ;; esac
  for n in $(seq "$lo" "$hi"); do
    nn=$(printf "%02d" "$n")
    f=$(ls "$examples"/${nn}-*.logscope "$examples"/*/${nn}-*.logscope 2>/dev/null | head -1)
    if [ -n "$f" ]; then scripts+=("$f"); else echo "# no ${nn}-*.logscope in $examples (or its subdirs) — skipping" >&2; fi
  done
else
  for f in "$examples"/*.logscope; do [ -e "$f" ] && scripts+=("$f"); done
fi
if [ "${#scripts[@]}" -eq 0 ]; then echo "gen-runs.sh: no scripts to run" >&2; exit 1; fi

echo "# cli=$cli"
echo "# model=$model  maxtokens=$maxtokens  system='$system'  ${extra[*]}  ->  $subdir"
printf '%-30s %8s %9s %9s\n' "script" "nodes" "wall(s)" "gen(s)"
total0=$(date +%s)
for f in "${scripts[@]}"; do
  base=$(basename "$f" .logscope)
  t0=$(date +%s)
  "$cli" "${backend[@]}" "${extra[@]}" --max "$maxtokens" --system "$system" --final "$final" "$f" \
        > "$subdir/$base.md" 2> "$subdir/$base.log"
  rc=$?
  t1=$(date +%s)
  nodes=$(grep -oE "final tree: [0-9]+" "$subdir/$base.log" | grep -oE "[0-9]+" | tail -1); [ -z "$nodes" ] && nodes="?"
  gens=$(grep -oE "gen: [0-9]+ calls, [0-9.]+s" "$subdir/$base.log" | grep -oE "[0-9.]+s" | tr -d s); [ -z "$gens" ] && gens="-"
  [ "$rc" -ne 0 ] && base="$base (rc=$rc!)"
  printf '%-30s %8s %9s %9s\n' "$base" "$nodes" "$((t1 - t0))" "$gens"
done
echo "# all done in $(( $(date +%s) - total0 ))s  ->  $subdir  (stdout=*.md, stderr=*.log)"
[ "$nokv" = "on" ] && echo "# (KV reuse was OFF — compare gen(s) against a run without 'nokv on' to measure the prefix tree)"
