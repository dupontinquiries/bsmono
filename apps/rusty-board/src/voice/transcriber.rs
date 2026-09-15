//! transcriber — whisper, tuned for a keyboard rather than for a transcript.
//!
//! ## Where the time goes, and what this does about it
//!
//! whisper always runs its ENCODER over a 30 s window: 1500 audio-context frames, whatever
//! the input length. A partial in the streaming loop is a few seconds of audio, so the
//! encoder was spending most of its time on silence padding. `audio_ctx` sizes the encoder
//! to the audio actually present (50 frames per second, plus a margin, floored at
//! [`MIN_AUDIO_CTX`]) — the same trick whisper.cpp's own `stream` example uses, and on a
//! phone it is the difference between partials that keep up and partials that fall behind.
//!
//! The rest, in decreasing order of effect:
//!
//! - **No temperature fallback on partials.** When the decoder is unsure, whisper re-decodes
//!   at rising temperatures — up to five extra passes on noisy audio. A partial is replaced
//!   600 ms later anyway; the final pass keeps the fallback.
//! - **`single_segment` + `no_timestamps` on partials.** One segment, no timestamp tokens to
//!   decode; the text is all we read.
//! - **One `WhisperState` per transcriber**, reused. `create_state` allocates the KV caches
//!   every call; a state is reusable across `full` calls and this struct is only ever driven
//!   by one worker at a time (a mutex makes that a fact rather than a hope).
//! - **`.en` models get `language = "en"` explicitly.** With `auto`, whisper runs a language
//!   detection pass on every call; an English-only model has nothing to detect.
//! - **The fingerprint corpus.** A committed utterance is remembered as (acoustic fingerprint,
//!   length, text) — `audio_kit::fingerprint`, 64 bits, a Goertzel spectrogram pooled to an
//!   8×8 grid — and a later utterance within [`FP_TOL`] bits AND 20% of the length reuses the
//!   text without running whisper at all. The corpus persists at
//!   `$XDG_DATA_HOME/rusty-board/voice-fingerprints.tsv`, so the words you actually say
//!   accrue across sessions. Conservative on purpose: for a keyboard a confident wrong word is
//!   worse than a slow right one, so only COMMITTED text is remembered (`remember`), never a
//!   partial, and the tolerance is tight. `RUSTY_BOARD_VOICE_FPCACHE=0` turns it off.
//!
//! Every call records a [`Timing`] (`last_timing`) so the UI can show what it cost — which
//! pass, how much audio, which `audio_ctx`, how many milliseconds, whether the corpus hit.
//!
//! ## What is unchanged
//!
//! Greedy sampling, `suppress_blank`, no translation. Model selection lives in
//! `model_path`/voice-kit (`ggml-base.en-q5_1` preferred, `tiny` after); a faster model is a
//! config `model_path` away, and the timing line tells you whether you need one.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

/// Encoder frames per second of audio (whisper's mel is 100 frames/s, the conv front-end
/// strides by 2).
const CTX_FRAMES_PER_SEC: f32 = 50.0;
/// Extra frames past the audio so the last word is not cut by the encoder's receptive field.
const CTX_MARGIN: i32 = 64;
/// Below this the encoder starts mishearing short words; whisper.cpp's stream example uses
/// 768 for ~15 s windows and people run 512 for short commands. 512 is a third of full cost.
pub const MIN_AUDIO_CTX: i32 = 512;
/// whisper's maximum.
const MAX_AUDIO_CTX: i32 = 1500;
/// Partials longer than this get segments, not one; the window is bounded upstream anyway.
const SINGLE_SEGMENT_MAX_SECS: f32 = 12.0;

/// Fingerprint Hamming tolerance (of 64 bits) for a corpus hit. Tight — see the module doc.
pub const FP_TOL: u32 = 3;
/// Corpus size. Oldest evicted.
pub const FP_CAP: usize = 512;

/// Which kind of pass this is; the cost structure differs, so the parameters do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// A streaming partial: replaced within a second, so speed over the last few percent.
    Partial,
    /// The result: keep whisper's fallbacks.
    Final,
}

/// What the last call cost, for the UI's status line and the log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    pub pass: Pass,
    pub audio_secs: f32,
    pub audio_ctx: i32,
    pub ms: u32,
    /// The text came from the fingerprint corpus; whisper did not run.
    pub corpus_hit: bool,
}

impl Timing {
    /// One short line: `partial 2.1s ctx512 180ms` or `final 4.0s corpus 0ms`.
    pub fn describe(&self) -> String {
        let pass = match self.pass {
            Pass::Partial => "partial",
            Pass::Final => "final",
        };
        if self.corpus_hit {
            format!("{pass} {:.1}s corpus {}ms", self.audio_secs, self.ms)
        } else {
            format!("{pass} {:.1}s ctx{} {}ms", self.audio_secs, self.audio_ctx, self.ms)
        }
    }
}

pub struct Transcriber {
    ctx: WhisperContext,
    language: String,
    threads: i32,
    /// The model filename says `.en`: no detection pass, ever.
    english_only: bool,
    /// Reused across calls; `None` until the first.
    state: Mutex<Option<WhisperState>>,
    corpus: Mutex<voice_kit::fpcache::FingerprintCache>,
    corpus_path: Option<PathBuf>,
    corpus_enabled: bool,
    last: Mutex<Option<Timing>>,
}

impl Transcriber {
    pub fn new(model_path: &Path, language: &str, threads: i32) -> Result<Self> {
        let model_str = model_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("model path is not valid UTF-8"))?;

        info!("Loading Whisper model from {}", model_str);
        let ctx_params = WhisperContextParameters::default();
        let ctx = WhisperContext::new_with_params(model_str, ctx_params)
            .with_context(|| format!("loading whisper model {}", model_str))?;

        let english_only = model_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.contains(".en"))
            .unwrap_or(false);
        let language = if english_only && language != "en" {
            debug!("model is English-only; language '{}' → 'en' (skips the detection pass)", language);
            "en".to_string()
        } else {
            language.to_string()
        };

        let corpus_enabled = std::env::var("RUSTY_BOARD_VOICE_FPCACHE").map(|v| v != "0").unwrap_or(true);
        let corpus_path = corpus_file();
        let mut corpus = voice_kit::fpcache::FingerprintCache::new(FP_CAP, FP_TOL);
        if corpus_enabled {
            if let Some(p) = &corpus_path {
                let n = load_corpus(p, &mut corpus);
                if n > 0 {
                    info!("voice fingerprint corpus: {} utterances from {}", n, p.display());
                }
            }
        }

        Ok(Self {
            ctx,
            language,
            threads: threads.max(1),
            english_only,
            state: Mutex::new(None),
            corpus: Mutex::new(corpus),
            corpus_path,
            corpus_enabled,
            last: Mutex::new(None),
        })
    }

    /// Run a one-shot transcription on mono 16 kHz f32 samples — a [`Pass::Final`].
    pub fn transcribe(&self, samples: &[f32]) -> Result<String> {
        self.transcribe_with(samples, Pass::Final)
    }

    /// Transcribe with the parameters for `pass`. Consults the corpus first.
    pub fn transcribe_with(&self, samples: &[f32], pass: Pass) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let started = Instant::now();
        let audio_secs = samples.len() as f32 / 16_000.0;

        if self.corpus_enabled {
            let fp = audio_kit::fingerprint::fingerprint(samples, 16_000.0);
            let hit = self.corpus.lock().unwrap().lookup_len(fp, samples.len() as u32).map(str::to_string);
            if let Some(text) = hit {
                let t = Timing { pass, audio_secs, audio_ctx: 0, ms: started.elapsed().as_millis() as u32, corpus_hit: true };
                debug!("{}", t.describe());
                *self.last.lock().unwrap() = Some(t);
                return Ok(text);
            }
        }

        let audio_ctx = audio_ctx_for(audio_secs);
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_translate(false);
        if self.english_only || self.language != "auto" {
            params.set_language(Some(&self.language));
        }
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_no_timestamps(true);
        params.set_audio_ctx(audio_ctx);
        match pass {
            Pass::Partial => {
                params.set_single_segment(audio_secs <= SINGLE_SEGMENT_MAX_SECS);
                params.set_no_context(true);
                params.set_temperature_inc(0.0); // no fallback re-decodes: the next partial IS the retry
            }
            Pass::Final => {
                params.set_single_segment(false);
                params.set_no_context(true); // each utterance is its own; no prompt from the last
            }
        }

        let text = {
            let mut guard = self.state.lock().unwrap();
            if guard.is_none() {
                *guard = Some(self.ctx.create_state().context("creating whisper state")?);
            }
            let state = guard.as_mut().expect("state just created");
            state.full(params, samples).context("running whisper inference")?;
            // whisper-rs 0.16 API (see Cargo.toml for why we are pinned there):
            // `full_n_segments` returns a plain i32 rather than a Result, and
            // the per-segment text moved behind `get_segment(i) -> Option<_>`
            // instead of `full_get_segment_text(i) -> Result<String>`. Both
            // changes are why bumping off 0.13 needed a call-site edit and not
            // only a version pin.
            let n_segments = state.full_n_segments();
            let mut text = String::new();
            for i in 0..n_segments {
                // A missing segment is skipped rather than failing the whole
                // utterance: the count and the accessor are two calls into the
                // same C state, and losing one segment of dictation beats
                // losing the sentence.
                let Some(seg) = state.get_segment(i) else { continue };
                let chunk = seg
                    .to_str_lossy()
                    .with_context(|| format!("decoding segment {}", i))?;
                text.push_str(&chunk);
            }
            text.trim().to_string()
        };

        let t = Timing { pass, audio_secs, audio_ctx, ms: started.elapsed().as_millis() as u32, corpus_hit: false };
        debug!("{} → {:?}", t.describe(), text);
        *self.last.lock().unwrap() = Some(t);
        Ok(text)
    }

    /// Remember a COMMITTED utterance in the corpus: the audio it came from (16 kHz mono) and
    /// the text the user accepted. Never call this with a partial. Persists.
    pub fn remember(&self, samples: &[f32], text: &str) {
        if !self.corpus_enabled || text.trim().is_empty() || samples.len() < 16_000 / 2 {
            return;
        }
        let fp = audio_kit::fingerprint::fingerprint(samples, 16_000.0);
        if fp == 0 {
            return;
        }
        let mut corpus = self.corpus.lock().unwrap();
        corpus.insert_len(fp, samples.len() as u32, text.trim().to_string());
        if let Some(p) = &self.corpus_path {
            if let Err(e) = save_corpus(p, &corpus) {
                warn!("voice corpus not saved to {}: {}", p.display(), e);
            }
        }
    }

    /// What the last call cost; `None` before the first.
    pub fn last_timing(&self) -> Option<Timing> {
        *self.last.lock().unwrap()
    }

    pub fn corpus_len(&self) -> usize {
        self.corpus.lock().unwrap().len()
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    pub fn english_only(&self) -> bool {
        self.english_only
    }
}

/// Encoder context for `secs` of audio: what is there plus a margin, floored and capped.
pub fn audio_ctx_for(secs: f32) -> i32 {
    let frames = (secs.max(0.0) * CTX_FRAMES_PER_SEC).ceil() as i32 + CTX_MARGIN;
    frames.clamp(MIN_AUDIO_CTX, MAX_AUDIO_CTX)
}

/// `$XDG_DATA_HOME/rusty-board/voice-fingerprints.tsv` (or `~/.local/share/...`).
fn corpus_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("rusty-board").join("voice-fingerprints.tsv"))
}

/// `<fp hex>\t<len samples>\t<text>` per line. Malformed lines are skipped, not fatal: a corpus
/// is a cache, and a cache that refuses to load is just empty.
fn load_corpus(path: &Path, into: &mut voice_kit::fpcache::FingerprintCache) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else { return 0 };
    let mut n = 0;
    for line in text.lines() {
        if let Some((fp, len, text)) = parse_corpus_line(line) {
            into.insert_len(fp, len, text.to_string());
            n += 1;
        }
    }
    n
}

fn parse_corpus_line(line: &str) -> Option<(u64, u32, &str)> {
    let mut it = line.splitn(3, '\t');
    let fp = u64::from_str_radix(it.next()?.trim(), 16).ok()?;
    let len = it.next()?.trim().parse::<u32>().ok()?;
    let text = it.next()?.trim();
    if fp == 0 || text.is_empty() {
        return None;
    }
    Some((fp, len, text))
}

fn save_corpus(path: &Path, corpus: &voice_kit::fpcache::FingerprintCache) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = String::new();
    for (fp, len, text) in corpus.entries() {
        out.push_str(&format!("{fp:016x}\t{len}\t{}\n", text.replace(['\t', '\n'], " ")));
    }
    // Whole-file rewrite through a temp name: a crash mid-write leaves the old corpus, not half.
    let tmp = path.with_extension("tsv.tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, path)
}

// `find_default_model` lives in `voice::model_path` (outside the
// voice feature gate) so its tests run in --no-default-features
// builds too. Re-export here for backward compatibility with the
// existing call sites that import it from this module.
pub use super::model_path::find_default_model;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encoder_is_sized_to_the_audio_with_a_floor_and_a_cap() {
        assert_eq!(audio_ctx_for(0.0), MIN_AUDIO_CTX);
        assert_eq!(audio_ctx_for(2.0), MIN_AUDIO_CTX, "2 s = 100 frames + margin, still under the floor");
        assert_eq!(audio_ctx_for(12.0), 600 + CTX_MARGIN);
        assert_eq!(audio_ctx_for(30.0), MAX_AUDIO_CTX);
        assert_eq!(audio_ctx_for(90.0), MAX_AUDIO_CTX);
        // monotone: more audio never gets a smaller encoder
        let mut last = 0;
        for s in 0..40 {
            let c = audio_ctx_for(s as f32);
            assert!(c >= last);
            last = c;
        }
    }

    #[test]
    fn corpus_lines_round_trip_and_garbage_is_skipped() {
        let mut c = voice_kit::fpcache::FingerprintCache::new(8, FP_TOL);
        c.insert_len(0xdead_beef_0000_0001, 24_000, "open mail".into());
        c.insert_len(0x0123_4567_89ab_cdef, 40_000, "tab\tinside".into());
        let dir = std::env::temp_dir().join(format!("rb-corpus-{}", std::process::id()));
        let path = dir.join("voice-fingerprints.tsv");
        save_corpus(&path, &c).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("deadbeef00000001\t24000\topen mail"), "{text}");
        assert!(text.contains("tab inside"), "a tab in the text is flattened, not written as a column");
        // garbage lines
        std::fs::write(&path, format!("{text}not-hex\t1\tx\n\n{:016x}\t5\t\n", 7u64)).unwrap();
        let mut back = voice_kit::fpcache::FingerprintCache::new(8, FP_TOL);
        assert_eq!(load_corpus(&path, &mut back), 2, "two good lines, the rest skipped");
        assert_eq!(back.lookup_len(0xdead_beef_0000_0001, 24_000), Some("open mail"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timing_describes_both_paths() {
        let t = Timing { pass: Pass::Partial, audio_secs: 2.1, audio_ctx: 512, ms: 180, corpus_hit: false };
        assert_eq!(t.describe(), "partial 2.1s ctx512 180ms");
        let t = Timing { pass: Pass::Final, audio_secs: 4.0, audio_ctx: 0, ms: 0, corpus_hit: true };
        assert_eq!(t.describe(), "final 4.0s corpus 0ms");
    }
}
