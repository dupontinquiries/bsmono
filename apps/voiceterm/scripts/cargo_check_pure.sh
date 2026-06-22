#!/usr/bin/env bash
# Real `cargo check` + `cargo test` on the dependency-free pure modules.
#
# WHY: the in-repo validators (qt_struct_check, qml_lint, brace_check)
# are STRUCTURAL — they don't do name resolution or type checking, which
# is how the v0.1.519 `q` build error slipped through. cargo+rustc 1.75
# are available and crates.io fetch works, but the FULL crate can't
# `cargo check` here because qmetaobject needs system Qt. However, the
# pure logic modules use ZERO external crates and form a closed set
# (mic_pipeline_fsm refs its two siblings via crate::), so they CAN be
# type-checked + tested for real in a throwaway project. This is a far
# stronger guarantee than the regex checks for these modules.
#
# Usage: bash scripts/cargo_check_pure.sh
# Run from the repo root. Re-copies the modules fresh each time.

set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
PROJ="/tmp/realcheck"

# The dependency-free pure modules (verified: no external crate use;
# only crate:: refs are mic_pipeline_fsm → mic_topology/mic_scheduler,
# which resolve as crate-root siblings here).
MODS=(doc_tree mic_scheduler mic_topology mic_pipeline_fsm \
      speaker_kalman voice_buttons bottom_bar follow_camera \
      note_cards dead_air llm_scheduler whisper_models util llm_coalesce \
      dumb_summary fact_extract row_cache \
      anim_detect cmd_classifier completion_parse deferred_transcribe \
      entity_extract graph_layout link_graph mermaid niftyplot \
      repl_voice sequence_diagram speaker_claim speaker_embed table_ops \
      editor_voice workflow path_fuzzy \
      ad_filter layout_detect sandbox search_suggest nlp \
      ui_landmarks)

if [ ! -d "$PROJ" ]; then
  cargo new --lib "$PROJ" >/dev/null 2>&1
fi
mkdir -p "$PROJ/src/mods"

# lib.rs declares each module via #[path].
{
  for m in "${MODS[@]}"; do
    echo "#[path = \"mods/$m.rs\"] pub mod $m;"
  done
} > "$PROJ/src/lib.rs"

# Copy fresh each run so this always reflects the live source.
for m in "${MODS[@]}"; do
  cp "$REPO/src/$m.rs" "$PROJ/src/mods/$m.rs"
done

cd "$PROJ"
echo "=== cargo check (name resolution + types) ==="
cargo check 2>&1 | grep -E "error|warning|Finished" || true
echo ""
echo "=== cargo test (compile + run all suites) ==="
cargo test 2>&1 | grep -E "test result|error\[|FAILED|panicked" || true
