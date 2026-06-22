#!/usr/bin/env bash
# sim-corpus.sh — run a REAL power-user speech corpus through the intent
# parser and measure the false-positive rate (v0.1.657).
#
# Corpus: tests/corpus/asahi_commands.txt (153 command-like utterances) and
# tests/corpus/asahi_narration.txt (3757 thinking-aloud utterances), mined
# from the Asahi kernel-dev livestream transcripts. Real spontaneous speech
# from a power user — the best test fuel we have.
#
# The KEY metric is the false-positive rate on NARRATION: a kernel dev
# thinking aloud ("just run this on the host, effectively") must NOT trigger
# an action. Anything actiony on the narration set is a candidate bug.
#
# Stages voice_intent + dep closure (same trick as cargo_test_pure.sh) so it
# runs with no whisper model / no audio.
#
# USAGE: scripts/sim-corpus.sh
set -euo pipefail
SRC="$(cd "$(dirname "$0")/.." && pwd)"
command -v cargo >/dev/null 2>&1 || { echo "cargo not installed"; exit 0; }
[ -f "$SRC/tests/corpus/asahi_narration.txt" ] || { echo "corpus fixtures missing"; exit 1; }

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/src" "$WORK/tests"
cat > "$WORK/Cargo.toml" << EOF
[package]
name = "sim_corpus"
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
cat > "$WORK/tests/corpus.rs" << EOF
use sim_corpus::voice_intent::{parse, Intent};
use std::fs;
fn is_actiony(i: &Intent) -> bool {
    !matches!(i, Intent::Unknown | Intent::Freeform(_))
        && !format!("{:?}", i).starts_with("Show")
}
fn run(path: &str, label: &str, max_fp_pct: f32) {
    let txt = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = txt.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut actiony = 0; let mut samples = Vec::new();
    for line in &lines {
        let i = parse(line);
        if is_actiony(&i) { actiony += 1;
            if samples.len() < 20 { samples.push(format!("    \"{}\" -> {:?}", line, i)); } }
    }
    let pct = 100.0 * actiony as f32 / lines.len().max(1) as f32;
    println!("\n=== {} ({} utterances) — actiony {} ({:.1}%) ===", label, lines.len(), actiony, pct);
    for s in &samples { println!("{}", s); }
    assert!(pct <= max_fp_pct, "{}: {:.1}% actiony exceeds budget {:.1}%", label, pct, max_fp_pct);
}
#[test]
fn corpus_false_positive_budget() {
    // command-like: many SHOULD route (no upper assert, informational)
    run("$SRC/tests/corpus/asahi_commands.txt", "COMMAND-LIKE", 100.0);
    // narration: thinking-aloud must rarely misfire. Budget 1.0% (current
    // ~0.4%); a regression that pushes natural speech into actions trips it.
    run("$SRC/tests/corpus/asahi_narration.txt", "NARRATION (actiony=false positive)", 0.3);
}
EOF
cd "$WORK" && cargo test --test corpus -- --nocapture 2>&1 \
  | grep -E "===|->|result:|exceeds budget" | head -50
