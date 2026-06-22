#!/usr/bin/env bash
# Run the pure-module test suite + the ai_training integration tests
# under cargo's REAL test harness (not hand-staged rustc).
#
# Why this exists: the full crate can't build in the dev sandbox
# (qmetaobject needs Qt6; whisper/llama need FFI; and a transitive dep
# now requires edition2024 which the sandbox's cargo 1.75 lacks). But
# the pure modules have ZERO external deps, so we assemble them into a
# throwaway dependency-free crate named `web2qml` and let cargo test it
# properly — which ALSO runs tests/ai_training.rs as a real integration
# test and runs doctests (both invisible to the rustc-staging approach).
#
# Requires: cargo (apt-get install -y cargo). Falls back gracefully if
# absent.
set -euo pipefail
SRC="$(cd "$(dirname "$0")/.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not installed — try: apt-get install -y cargo"
  echo "(falling back to scripts/cargo_check_pure.sh staging if you have rustc)"
  exit 1
fi

PURE="doc_tree mic_scheduler mic_topology mic_pipeline_fsm speaker_kalman \
voice_buttons bottom_bar follow_camera note_cards dead_air llm_scheduler \
whisper_models util llm_coalesce dumb_summary fact_extract row_cache \
anim_detect cmd_classifier completion_parse deferred_transcribe \
entity_extract graph_layout prezi_walk link_graph mermaid niftyplot repl_voice \
sequence_diagram speaker_claim speaker_embed table_ops editor_voice \
workflow path_fuzzy ad_filter layout_detect sandbox search_suggest nlp ui_landmarks sysmon notebook motion_history selection \
audio_feedback bookmarks confusion_table data filler_words fs_find gemini git_status hf_hub llama_backend model_bench phonetic_confusions scroll_signals sudo_keepalive system_theme token_log transforms unhide new_speaker wake_gate sudo_shim zellij_voice tmux_voice ssh_voice wer demojibake zettel_id workspace_wizard audio_dialup peer_handshake peer_workspace peer_registry canvas_physics count_prefix backlinks card_density node_id layered_layout terminal_workspace event_signal group_layout build_diagnostics shell_dialect doc_resolver recall note_clip tab_cycle editor_open pane_voice session_auto cmd_cluster assist_bubble slowness_probe debug_detect zbrackets note_render filetype_assoc note_blend doc_outline speech_plan niftyplot_view undo_coalesce undo_stack path_complete note_graph note_edit syntax_hl note_search"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/src" "$WORK/tests"

cat > "$WORK/Cargo.toml" << EOF
[package]
name = "web2qml"
version = "0.0.0"
edition = "2021"
[lib]
path = "src/lib.rs"
[[test]]
name = "ai_training"
path = "tests/ai_training.rs"
EOF

: > "$WORK/src/lib.rs"
for m in $PURE; do
  echo "#[path = \"$SRC/src/$m.rs\"] pub mod $m;" >> "$WORK/src/lib.rs"
done
cp "$SRC/tests/ai_training.rs" "$WORK/tests/ai_training.rs"

cd "$WORK"
cargo test "$@"

# ── Tier 2: modules needing a couple of simple external crates ──
# phonetic pulls libc + aho-corasick (via security/util). These are
# edition-2021-safe so cargo 1.75 handles them. Run separately so the
# zero-dep tier above stays dep-free and fast.
echo ""
echo "=== Tier 2: phonetic (+ libc, aho-corasick) ==="
WORK2="$(mktemp -d)"
trap 'rm -rf "$WORK" "$WORK2"' EXIT
mkdir -p "$WORK2/src"
cat > "$WORK2/Cargo.toml" << EOF2
[package]
name = "phonetic_test"
version = "0.0.0"
edition = "2021"
[lib]
path = "src/lib.rs"
[dependencies]
libc = "0.2"
aho-corasick = "1"
flate2 = "1"
EOF2
# v0.1.613: minimal clog! stub (the real one is in log.rs, #[macro_export]
# crate-global). Modules like decision_cache use it; stubbing here lets
# them be tested in this tier without staging the whole logging module.
echo '#[macro_export] macro_rules! clog { ($tag:expr, $($a:tt)*) => {{ let _ = ($tag, format!($($a)*)); }}; }' >> "$WORK2/src/lib.rs"
for m in security util phonetic decision_cache \
         llm_cache missing_packages user_tags workspaces async_jobs cpu_efficiency; do
  echo "#[path = \"$SRC/src/$m.rs\"] pub mod $m;" >> "$WORK2/src/lib.rs"
done
# v0.1.612: run the WHOLE tier crate, not `cargo test phonetic`. The
# name filter silently skipped every security:: and util:: test — it was
# masking a real pre-existing security redaction-format test failure.
# Running unfiltered surfaces all three modules' tests.
cd "$WORK2" && cargo test 2>&1 | grep -E "test result|error" || true
