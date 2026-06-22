#!/usr/bin/env bash
# sim-workflow.sh — TTS-driven workflow simulation harness (v0.1.655).
#
# Two layers, both runnable WITHOUT a whisper model or recorded audio:
#   Layer 1 (audio plumbing): espeak-ng synthesizes real speech for a
#     scripted workflow; WAVs can be fed to the actual phonetic pipeline
#     (compute_mfcc + acoustic_index::fingerprint). Needs espeak-ng.
#   Layer 2 (intent routing): stages voice_intent + its dep closure as a
#     standalone crate and RUNS workflow->intent assertions. High-value
#     regression net — it caught recall phrases being swallowed by the
#     audit-log arm (fixed v0.1.655).
#
# HONEST SCOPE
#   Validates: audio PLUMBING, intent ROUTING, relative behaviour.
#   Does NOT validate: real-human speaker-ID accuracy (espeak voices are
#     spectrally near-identical), or whisper transcription QUALITY (no model
#     in CI; on-device via tests/voice_cases.rs).
#
# USAGE
#   scripts/sim-workflow.sh           # layer 1 (if espeak) + layer 2
#   scripts/sim-workflow.sh --intent  # layer 2 only
set -euo pipefail
SRC="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${1:-all}"
TTS_DIR="${TTS_DIR:-/tmp/sim-tts}"

WORKFLOW=(
  "open terminal"
  "run cargo test"
  "walk me through my last session"
  "what did i do last time"
  "edit src slash main dot rs at line forty two"
  "go to apple dot com"
)

if [ "$MODE" != "--intent" ]; then
  if command -v espeak-ng >/dev/null 2>&1; then
    echo "## Layer 1: synthesize real speech for the workflow"
    mkdir -p "$TTS_DIR"; i=0
    for phrase in "${WORKFLOW[@]}"; do
      espeak-ng -v en-us -s 150 -w "$TTS_DIR/p$i.wav" "$phrase" 2>/dev/null || true
      dur=$(python3 -c "import wave;w=wave.open('$TTS_DIR/p$i.wav');print(round(w.getnframes()/w.getframerate(),2))" 2>/dev/null || echo "?")
      printf "  p%-2d %-40s %ss\n" "$i" "\"$phrase\"" "$dur"
      i=$((i+1))
    done
    echo "  WAVs in $TTS_DIR — feed to the phonetic pipeline (docs/design/sim_harness.md)"
    echo
  else
    echo "## Layer 1 skipped: espeak-ng not installed (apt-get install -y espeak-ng)"; echo
  fi
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "## Layer 2 skipped: cargo not installed"; exit 0
fi
echo "## Layer 2: workflow -> intent simulation (staged voice_intent)"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/src" "$WORK/tests"
cat > "$WORK/Cargo.toml" << EOF
[package]
name = "sim_intent"
version = "0.0.0"
edition = "2021"
[lib]
path = "src/lib.rs"
[dependencies]
libc = "0.2"
aho-corasick = "1"
EOF
echo '#[macro_export] macro_rules! clog { ($tag:expr, $($a:tt)*) => {{ let _ = ($tag, format!($($a)*)); }}; }' > "$WORK/src/lib.rs"
for m in security util async_jobs llm_cache sysmon phonetic confusion_table \
         editor_open speakers system_info token_stats \
         voice_intent voice_intent_es voice_intent_fr; do
  echo "#[path = \"$SRC/src/$m.rs\"] pub mod $m;" >> "$WORK/src/lib.rs"
done
cat > "$WORK/tests/sim.rs" << 'EOF'
use sim_intent::voice_intent::{parse, Intent};
fn ck(p: &str, ok: impl Fn(&Intent)->bool, lbl: &str, f: &mut Vec<String>) {
    let g = parse(p); let pass = ok(&g);
    println!("  [{}] \"{}\" -> {:?}", if pass {"ok"} else {"FAIL"}, p, g);
    if !pass { f.push(format!("\"{}\" expected {} got {:?}", p, lbl, g)); }
}
#[test]
fn workflow_routes() {
    let mut f = Vec::new();
    ck("open terminal", |i| matches!(i, Intent::SwitchTab(t) if t=="term"), "SwitchTab(term)", &mut f);
    ck("walk me through my last session", |i| matches!(i, Intent::WalkLastSession), "WalkLastSession", &mut f);
    ck("what did i do last time", |i| matches!(i, Intent::RecallCommands(_)), "RecallCommands", &mut f);
    ck("how did i fix the docker build", |i| matches!(i, Intent::RecallCommands(q) if q=="the docker build"), "RecallCommands(q)", &mut f);
    ck("edit src/dcp.rs at line 42", |i| matches!(i, Intent::OpenInEditor(_)), "OpenInEditor", &mut f);
    ck("open the tracer in helix", |i| matches!(i, Intent::OpenInEditor(_)), "OpenInEditor", &mut f);
    ck("open google", |i| !matches!(i, Intent::OpenInEditor(_)), "not OpenInEditor", &mut f);
    assert!(f.is_empty(), "routing failures:\n{}", f.join("\n"));
}
#[test]
fn spanish_parser_routes() {
    let g = sim_intent::voice_intent_es::parse_es("abre terminal");
    println!("  es 'abre terminal' -> {:?}", g);
    assert!(matches!(g, Intent::SwitchTab(ref t) if t=="term"), "got {:?}", g);
}
EOF
cd "$WORK" && cargo test --test sim -- --nocapture 2>&1 | grep -E "\[ok\]|\[FAIL\]|test result|failures" || true
echo
echo "Scope: plumbing + routing + relative behaviour only. Real-voice accuracy"
echo "+ whisper quality are on-device (no model / no real voices in CI)."
