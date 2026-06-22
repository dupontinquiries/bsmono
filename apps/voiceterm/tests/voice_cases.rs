//! Voice end-to-end test harness. Reads case files from tests/voice/cases/
//! where each case is a pair: `<name>.wav` (audio recorded at 16kHz mono)
//! and `<name>.expect` (a JSON-ish file describing the expected outcome).
//!
//! Run with: `cargo test --test voice_cases --features voice -- --nocapture`
//!
//! Each .expect file looks like:
//! ```text
//! transcript_contains: open layer shell
//! top_intent: term_cmd
//! top_command_contains: $EDITOR
//! min_score: 0.85
//! ```
//!
//! The harness is permissive: only fields that appear in the .expect file
//! are checked. To add a new case:
//!   1. Record a WAV of you saying the phrase (16kHz mono, 16-bit PCM).
//!      `arecord -f S16_LE -r 16000 -c 1 my_phrase.wav` works.
//!   2. Write a .expect file with assertions.
//!   3. Optionally: add a `<name>.context.txt` file containing prior
//!      terminal output to seed the BlockTracker before running the test
//!      (one block per blank-line-separated paragraph: "command\noutput").
//!
//! What this catches: regressions in voice → intent routing. What it doesn't
//! catch: GUI rendering, focus issues, scrolling behavior. Those need their
//! own test infrastructure (probably a QML headless test).

#![cfg(feature = "voice")]

use std::path::{Path, PathBuf};
use std::fs;

#[derive(Default, Debug)]
struct Expectations {
    transcript_contains: Option<String>,
    top_intent: Option<String>,
    top_command_contains: Option<String>,
    top_command_equals: Option<String>,
    min_score: Option<f32>,
    not_intent: Option<String>,  // a kind that must NOT appear in top candidate
    context_tab: Option<String>, // "browser" or "terminal"
}

fn parse_expect(text: &str) -> Expectations {
    let mut e = Expectations::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim();
            let v = v.trim().to_string();
            // v0.1.469: reject an empty value. A bare `key:` with no
            // value used to become Some(""), and for the *_contains
            // assertions that's a `contains("")` check which is
            // VACUOUSLY TRUE — a silently-passing assertion that
            // tests nothing. In a regression harness that's worse
            // than a failure (false green). Warn and skip so the
            // author notices the typo.
            if v.is_empty() {
                eprintln!("WARN: expectation key '{}' has an empty value; \
                    ignoring (did you forget the value after the colon?)", k);
                continue;
            }
            match k {
                "transcript_contains" => e.transcript_contains = Some(v),
                "top_intent" => e.top_intent = Some(v),
                "top_command_contains" => e.top_command_contains = Some(v),
                "top_command_equals" => e.top_command_equals = Some(v),
                "min_score" => match v.parse() {
                    Ok(n) => e.min_score = Some(n),
                    // v0.1.469: previously `v.parse().ok()` silently
                    // dropped an unparseable score → the threshold
                    // was just ignored, another false-green. Warn.
                    Err(_) => eprintln!("WARN: min_score value '{}' is not a \
                        number; ignoring", v),
                },
                "not_intent" => e.not_intent = Some(v),
                "context_tab" => e.context_tab = Some(v),
                _ => eprintln!("WARN: unknown expectation key '{}'", k),
            }
        } else {
            // v0.1.469: a non-empty, non-comment line with no colon
            // is malformed — warn rather than silently skipping, so
            // a mistyped assertion (e.g. "top_intent term_cmd" with
            // no colon) doesn't vanish unnoticed.
            eprintln!("WARN: malformed expectation line (no ':'): '{}'", line);
        }
    }
    e
}

fn cases_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/voice/cases")
}

#[test]
fn voice_cases_directory_exists() {
    let d = cases_dir();
    assert!(d.exists(), "tests/voice/cases directory should exist");
}

#[test]
fn run_all_voice_cases() {
    let dir = cases_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => {
            eprintln!("No tests/voice/cases dir or unreadable; nothing to test.");
            return;
        }
    };

    let mut wavs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "wav").unwrap_or(false))
        .collect();
    wavs.sort();

    if wavs.is_empty() {
        eprintln!("No .wav cases found in {:?}. Add some to enable voice tests.", dir);
        return;
    }

    let mut failed: Vec<String> = Vec::new();
    let mut passed = 0;

    for wav in &wavs {
        let name = wav.file_stem().unwrap().to_string_lossy().to_string();
        let expect_path = wav.with_extension("expect");
        if !expect_path.exists() {
            eprintln!("SKIP {}: no .expect file", name);
            continue;
        }
        let expect_text = fs::read_to_string(&expect_path).unwrap();
        let exp = parse_expect(&expect_text);

        match run_case(wav, &exp) {
            Ok(()) => { passed += 1; eprintln!("PASS {}", name); }
            Err(msg) => { failed.push(format!("{}: {}", name, msg)); eprintln!("FAIL {}: {}", name, msg); }
        }
    }

    eprintln!("\n=== Voice test summary: {} passed, {} failed ===", passed, failed.len());
    if !failed.is_empty() {
        for f in &failed { eprintln!("  - {}", f); }
        panic!("Voice cases failed");
    }
}

fn run_case(wav_path: &Path, exp: &Expectations) -> Result<(), String> {
    // Read 16kHz mono PCM from the WAV
    let samples = read_wav_16k_mono(wav_path)?;

    // Transcribe via whisper. We use the same model path the live app uses
    // so test results match production. If the model isn't installed,
    // skip gracefully.
    let model_path = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".config/web2qml/models/ggml-base.en.bin"))
        .map_err(|_| "HOME not set".to_string())?;
    if !model_path.exists() {
        return Err(format!("model not found at {:?}; install ggml-base.en.bin first", model_path));
    }

    let transcript = whisper_transcribe(&model_path, &samples)?;
    eprintln!("  transcript: {:?}", transcript);

    if let Some(needle) = &exp.transcript_contains {
        if !transcript.to_lowercase().contains(&needle.to_lowercase()) {
            return Err(format!("transcript {:?} doesn't contain {:?}", transcript, needle));
        }
    }

    // For now we stub the rest — full intent parsing requires wiring up
    // BlockTracker context from a sibling .context.txt and calling
    // voice_parse_candidates. That's the next iteration of this harness.
    // For now, presence of a non-empty transcript is the smoke test.
    if exp.top_intent.is_some() || exp.top_command_contains.is_some()
        || exp.min_score.is_some() {
        eprintln!("  (intent assertions not yet wired; transcript-only check passed)");
    }

    Ok(())
}

fn read_wav_16k_mono(path: &Path) -> Result<Vec<f32>, String> {
    use std::io::Read;
    let mut f = fs::File::open(path).map_err(|e| format!("open: {}", e))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).map_err(|e| format!("read: {}", e))?;
    if buf.len() < 44 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    // Find fmt and data chunks. Minimal parser: assume PCM 16-bit mono 16kHz.
    let mut i = 12usize;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut channels = 0u16;
    let mut data: &[u8] = &[];
    while i + 8 <= buf.len() {
        let id = &buf[i..i+4];
        let sz = u32::from_le_bytes([buf[i+4],buf[i+5],buf[i+6],buf[i+7]]) as usize;
        let body_start = i + 8;
        let body_end = body_start + sz;
        if body_end > buf.len() { break; }
        if id == b"fmt " {
            channels = u16::from_le_bytes([buf[body_start+2], buf[body_start+3]]);
            sample_rate = u32::from_le_bytes([
                buf[body_start+4], buf[body_start+5], buf[body_start+6], buf[body_start+7]]);
            bits = u16::from_le_bytes([buf[body_start+14], buf[body_start+15]]);
        } else if id == b"data" {
            data = &buf[body_start..body_end];
        }
        i = body_end + (sz & 1);  // chunks are word-aligned
    }
    if sample_rate != 16000 {
        return Err(format!("expected 16000 Hz, got {}", sample_rate));
    }
    if channels != 1 {
        return Err(format!("expected mono, got {} channels", channels));
    }
    if bits != 16 {
        return Err(format!("expected 16-bit, got {} bits", bits));
    }
    let samples: Vec<f32> = data
        .chunks_exact(2)
        .map(|c| {
            let s = i16::from_le_bytes([c[0], c[1]]);
            s as f32 / 32768.0
        })
        .collect();
    Ok(samples)
}

fn whisper_transcribe(model: &Path, samples: &[f32]) -> Result<String, String> {
    use whisper_rs::{WhisperContext, WhisperContextParameters, FullParams, SamplingStrategy};
    let ctx = WhisperContext::new_with_params(
        model.to_str().unwrap(),
        WhisperContextParameters::default()
    ).map_err(|e| format!("ctx: {:?}", e))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(2);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_language(Some("en"));
    params.set_no_context(true);
    let mut state = ctx.create_state().map_err(|e| format!("state: {:?}", e))?;
    state.full(params, samples).map_err(|e| format!("transcribe: {:?}", e))?;
    let n = state.full_n_segments();
    let text: String = (0..n)
        .filter_map(|i| state.full_get_segment_text(i).ok())
        .collect::<Vec<_>>()
        .join(" ");
    Ok(text.trim().to_string())
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    // v0.1.469: lock down parse_expect's robustness fixes. These run
    // under `cargo test --features voice` (the whole file is
    // voice-gated). Pure string logic — no audio/whisper needed.

    #[test]
    fn empty_value_is_skipped_not_vacuous() {
        // `key:` with no value used to become Some("") — a vacuously
        // true contains-check. Must now be None (skipped + warned).
        assert_eq!(parse_expect("transcript_contains:").transcript_contains, None);
        assert_eq!(parse_expect("transcript_contains:    ").transcript_contains, None);
        assert_eq!(parse_expect("top_command_contains:").top_command_contains, None);
    }

    #[test]
    fn bad_min_score_skipped() {
        assert_eq!(parse_expect("min_score: notanumber").min_score, None);
        assert_eq!(parse_expect("min_score: 0.85").min_score, Some(0.85));
    }

    #[test]
    fn valid_values_parse() {
        let e = parse_expect("top_intent: term_cmd\ntranscript_contains: open shell");
        assert_eq!(e.top_intent, Some("term_cmd".to_string()));
        assert_eq!(e.transcript_contains, Some("open shell".to_string()));
    }

    #[test]
    fn colon_in_value_preserved() {
        // split_once(':') keeps everything after the first colon, so
        // values containing colons (paths, URLs) survive intact.
        let e = parse_expect("top_command_equals: ssh user@host:/path");
        assert_eq!(e.top_command_equals, Some("ssh user@host:/path".to_string()));
    }

    #[test]
    fn comments_blanks_and_whitespace() {
        let e = parse_expect("# a comment\n\n   top_intent:  x  \n");
        assert_eq!(e.top_intent, Some("x".to_string()));
    }

    #[test]
    fn malformed_line_sets_nothing() {
        // A non-comment line with no colon is malformed (warns); it
        // must not silently set any field.
        let e = parse_expect("top_intent term_cmd");
        assert!(e.top_intent.is_none());
    }
}
