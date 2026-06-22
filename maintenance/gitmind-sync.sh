#!/usr/bin/env sh
# gitmind-sync.sh — (re)build the gitmind object store from the current git graph.
#
# .gitmind/store.redb is gitignored and reproducible, so git and gitmind stay
# side-by-side without bloating .git. Because the store is never a tracked git
# object, `gin import` (which reads the git graph) never re-ingests it — no
# self-reference. Run after clone or whenever you want to refresh the view.
set -eu
ROOT=$(git rev-parse --show-toplevel)
GIN="$ROOT/apps/gitmind/target/release/gin"
[ -x "$GIN" ] || GIN="$ROOT/apps/gitmind/target/debug/gin"
if [ ! -x "$GIN" ]; then
  echo "gin not built — building the CLI (GUI is excluded via default-members)…"
  ( cd "$ROOT/apps/gitmind" && cargo build --release --bin gin )
  GIN="$ROOT/apps/gitmind/target/release/gin"
fi
echo "importing git graph into .gitmind …"
( cd "$ROOT" && "$GIN" import )
if [ -f "$ROOT/.gitmind/store.redb" ]; then
  echo "done: $(du -sh "$ROOT/.gitmind/store.redb" | cut -f1) at .gitmind/store.redb"
fi
echo "tip: 'gin export-native /tmp/check && git -C /tmp/check ...' to re-verify fidelity."
