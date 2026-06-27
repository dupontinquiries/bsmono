//! Whisper transcription wrapper.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct Transcriber {
    ctx: WhisperContext,
    language: String,
    threads: i32,
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

        Ok(Self {
            ctx,
            language: language.to_string(),
            threads: threads.max(1),
        })
    }

    /// Run a one-shot transcription on the given mono 16 kHz f32 samples.
    pub fn transcribe(&self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }

        let mut state = self
            .ctx
            .create_state()
            .context("creating whisper state")?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_translate(false);
        if self.language != "auto" {
            params.set_language(Some(&self.language));
        }
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);

        debug!(
            "Transcribing {} samples ({:.1}s)",
            samples.len(),
            samples.len() as f32 / 16_000.0
        );
        state
            .full(params, samples)
            .context("running whisper inference")?;

        let n_segments = state.full_n_segments().context("getting segment count")?;
        let mut text = String::new();
        for i in 0..n_segments {
            let seg = state
                .full_get_segment_text(i)
                .with_context(|| format!("getting segment {}", i))?;
            text.push_str(&seg);
        }
        Ok(text.trim().to_string())
    }

    pub fn language(&self) -> &str {
        &self.language
    }
}

// `find_default_model` lives in `voice::model_path` (outside the
// voice feature gate) so its tests run in --no-default-features
// builds too. Re-export here for backward compatibility with the
// existing call sites that import it from this module.
pub use super::model_path::find_default_model;
