//! Central configuration. Every magic number from the original Python
//! (`n_mels`, `time_frames`, `db_floor/ceil`, `latent`, `g_filters`, ...) lives
//! here so train-time and gui-time agree exactly. Saved to `config.json` next to
//! the checkpoints so the GUI can reconstruct the right-shaped model.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_griffin_iters_rt() -> usize {
    8
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    // --- audio / mel front-end ---
    pub sr: u32,
    pub n_fft: usize,
    pub hop: usize,
    pub n_mels: usize,
    pub time_frames: usize, // width of a training crop (== one generated "beat").
    // 64 frames @ hop 256 / 22050 Hz ~= 0.74 s: a small window keeps the
    // generator + Griffin-Lim cheap and the realtime latency low.
    pub fmin: f32,
    pub fmax: f32,
    pub db_floor: f32,
    pub db_ceil: f32,

    // --- model ---
    pub latent: usize,
    pub g_filters: usize,
    pub d_filters: usize,
    pub sparse_threshold: f32,

    // --- training ---
    pub lr: f64,
    pub beta1: f64,
    pub label_smooth: f32,
    pub batch_size: usize,
    pub epochs: usize,
    pub steps_per_epoch: usize,
    pub ckpt_every: usize, // epochs

    // --- griffin-lim (mel -> audio) ---
    pub griffin_iters: usize, // high-quality, fresh-phase (offline / Save WAV)
    #[serde(default = "default_griffin_iters_rt")]
    pub griffin_iters_rt: usize, // realtime, warm-started across beats (GUI)

    // --- paths ---
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub ckpt_dir: PathBuf,
    pub audio_exts: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        let sr = 22_050u32;
        Self {
            sr,
            n_fft: 1024,
            hop: 256,
            n_mels: 128,
            time_frames: 64,
            fmin: 0.0,
            fmax: sr as f32 / 2.0,
            db_floor: -80.0,
            db_ceil: 40.0,

            latent: 128,
            g_filters: 256,
            d_filters: 64,
            sparse_threshold: 0.05,

            lr: 2e-4,
            beta1: 0.5,
            label_smooth: 0.9,
            batch_size: 16,
            epochs: 20_000,
            steps_per_epoch: 64,
            ckpt_every: 200,

            griffin_iters: 48,
            griffin_iters_rt: default_griffin_iters_rt(),

            data_dir: PathBuf::from("./data"),
            cache_dir: PathBuf::from("./cache"),
            ckpt_dir: PathBuf::from("./checkpoints"),
            audio_exts: ["opus", "ogg", "wav", "flac", "mp3", "m4a", "aac"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

impl Config {
    pub fn n_freq(&self) -> usize {
        self.n_fft / 2 + 1
    }

    pub fn save_json(&self, path: &std::path::Path) -> anyhow::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn load_json(path: &std::path::Path) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }
}
