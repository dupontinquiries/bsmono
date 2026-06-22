#!/bin/sh
# wiring-audit.sh — census of which crate modules are actually composed.
# Usage: wiring-audit.sh <src_dir> [entry_file]
# Prints, per module: external-reference count, and flags orphans (0 refs).
# A module with a `pub struct`/`pub enum` holding state and 0 refs is a likely
# dead SERVICE (should be wired); a pure-fn module with 0 refs is an unused
# KERNEL (fine to leave until composed).
SRC="${1:-crates/unendlich-engine/src/sim}"
ENTRY="${2:-}"
total=0; orphan=0; dead_service=0
for f in "$SRC"/*.rs; do
  m=$(basename "$f" .rs); [ "$m" = "mod" ] && continue
  total=$((total+1))
  refs=$(grep -rlE "\b$m::|::$m\b" crates --include=*.rs 2>/dev/null | grep -v "/$m.rs" | grep -cv '^$')
  if [ "$refs" -eq 0 ]; then
    orphan=$((orphan+1))
    if grep -qE 'pub struct .*\{|pub struct [A-Z].*\(|impl .* for ' "$f" && grep -qE 'pub fn (new|with_|step|tick|update|apply|run)\b' "$f"; then
      echo "DEAD-SERVICE  $m"; dead_service=$((dead_service+1))
    else
      echo "unused-kernel $m"
    fi
  fi
done
echo "----"
echo "total=$total orphaned=$orphan dead_services=$dead_service kernels=$((orphan-dead_service))"
[ -n "$ENTRY" ] && echo "entry=$ENTRY"
