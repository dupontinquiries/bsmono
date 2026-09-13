//! Central configuration. Every magic number from the original Python
//! (`n_mels`, `time_frames`, `db_floor/ceil`, `latent`, `g_filters`, ...) lives
//! here so train-time and gui-time agree exactly. Saved to `config.json` next to
//! the checkpoints so the GUI can reconstruct the right-shaped model.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_griffin_iters_rt() -> usize {
    8
}

// Defaults for the dynamic-track fields. Every one of them reproduces the
// legacy model exactly — one stem, no context, no beat phase, one window per
// sample — so a config.json written before these existed deserialises to the
// model that wrote it and its checkpoint still loads.
fn default_stems() -> usize {
    1
}
fn default_ctx_dim() -> usize {
    32
}
fn default_seq_windows() -> usize {
    1
}
fn default_rollout() -> bool {
    true
}
fn default_overlap_frames() -> usize {
    4
}
fn default_bpm() -> f32 {
    120.0
}
fn default_playback() -> Playback {
    Playback::Loop
}

/// How the GUI turns generated windows into sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Playback {
    /// The original behaviour: loop the current window, crossfade into the
    /// next one from its first sample when it arrives. Windows are
    /// independent textures.
    Loop,
    /// Each window is a continuation of the previous one: it starts
    /// `time_frames - overlap_frames` frames after it, its first frames are
    /// crossfaded over the previous tail, and Griffin-Lim is seeded from the
    /// previous phase in time-aligned fashion. The track never restarts.
    Stream,
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

    // --- dynamic tracks: representation ---
    /// Mel channels per frame. 1 is the mixed signal; 2 splits it into
    /// harmonic and percussive stems (HPSS, in dsp.rs) so drums and
    /// instruments are separate planes the generator emits together.
    #[serde(default = "default_stems")]
    pub stems: usize,
    /// Frames of the PREVIOUS window the renderer sees when producing the
    /// next one. 0 is the legacy unconditional generator; anything else makes
    /// every window a continuation.
    #[serde(default)]
    pub ctx_frames: usize,
    /// Width of the vector the context encoder reduces those frames to.
    #[serde(default = "default_ctx_dim")]
    pub ctx_dim: usize,
    /// Feed the renderer a per-frame beat phase (sin/cos planes) so it knows
    /// where in the beat each column falls. Estimated per clip at cache time.
    #[serde(default)]
    pub beat_phase: bool,

    // --- dynamic tracks: training ---
    /// Consecutive windows per training sample. 1 is the legacy i.i.d. crop;
    /// 2 or more trains the renderer as a continuation and adds the sequence
    /// discriminator, which scores adjacent windows side by side.
    #[serde(default = "default_seq_windows")]
    pub seq_windows: usize,
    /// Also score a FREE-RUNNING rollout — the renderer fed its own output as
    /// context for `seq_windows - 1` steps. Teacher forcing alone trains a
    /// model that is only ever asked to continue real audio, and playback
    /// asks it to continue itself.
    #[serde(default = "default_rollout")]
    pub rollout: bool,

    // --- dynamic tracks: playback / stitching ---
    /// Frames by which consecutive windows overlap, in training and in
    /// playback. The crossfade region, and the part of the new window the
    /// previous phase seeds directly.
    #[serde(default = "default_overlap_frames")]
    pub overlap_frames: usize,
    /// Tempo the playback conductor advances the beat phase at. Only used
    /// when `beat_phase` is on.
    #[serde(default = "default_bpm")]
    pub bpm: f32,
    #[serde(default = "default_playback")]
    pub playback: Playback,

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
    /// How many decoded clips the cache may hold, as a multiple of the batch.
    ///
    /// The cache exists so a batch does not re-decode; it does not need the
    /// whole library on disk. At 5,630 sources a full cache is tens of
    /// gigabytes of mel for a trainer that reads `batch_size` clips at a time.
    /// Zero means unbounded, which is the old behaviour and still right for a
    /// small set.
    pub cache_batches: usize,
    /// Ceiling on the in-memory mel cache, in bytes.
    ///
    /// The actual budget is the smaller of this and half of MemAvailable, so
    /// this is a cap rather than a target — it exists so a workstation with
    /// 192 GiB does not decide to hold 90 of them for a job that does not need
    /// it. Zero disables the in-memory tier entirely and every crop reads from
    /// disk, which is the old behaviour.
    pub mem_cache_bytes: u64,
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

            stems: default_stems(),
            ctx_frames: 0,
            ctx_dim: default_ctx_dim(),
            beat_phase: false,
            seq_windows: default_seq_windows(),
            rollout: default_rollout(),
            overlap_frames: default_overlap_frames(),
            bpm: default_bpm(),
            playback: default_playback(),

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
            // 32 batches of headroom: enough that a sampler drawing randomly
            // keeps hitting warm clips, small enough that the cache stays a
            // working set rather than a copy of the library.
            cache_batches: 32,
            // 16 GiB as the CEILING, not the target — the real budget is half
            // of MemAvailable, so a 4 GiB phone still lands around 1.5 GiB and
            // a workstation can hold a whole rotation window.
            //
            // The earlier 2 GiB was too low for the thing this is for. A
            // 512-clip window at ~10 MiB of mel is ~5 GiB, so a 2 GiB ceiling
            // meant most of the window still went to disk and the SSD writes
            // this exists to avoid happened anyway.
            mem_cache_bytes: 16 * 1024 * 1024 * 1024,
            ckpt_dir: PathBuf::from("./checkpoints"),
            audio_exts: ["opus", "ogg", "wav", "flac", "mp3", "m4a", "aac"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

impl Config {
    /// The dynamic-track preset on top of the defaults: two stems, an
    /// 8-frame context, beat phase, three-window sequences, streaming
    /// playback. One place to define it so the CLI, the smoke test and the
    /// GUI agree on what "dynamic" means.
    pub fn dynamic() -> Self {
        Self {
            stems: 2,
            ctx_frames: 8,
            beat_phase: true,
            seq_windows: 3,
            playback: Playback::Stream,
            ..Self::default()
        }
    }

    /// Is any of the conditional machinery on? When false the generator is
    /// the legacy `z -> window` map and `forward_train(z)` works unchanged.
    pub fn conditional(&self) -> bool {
        self.ctx_frames > 0 || self.beat_phase
    }

    /// Whether the trainer runs the sequence path.
    pub fn sequential(&self) -> bool {
        self.seq_windows >= 2
    }

    /// Frames between the starts of consecutive windows.
    pub fn window_stride(&self) -> usize {
        self.time_frames - self.overlap_frames
    }

    /// Frames of beat phase one window advances the conductor by, at `bpm`.
    pub fn beats_per_window(&self) -> f32 {
        (self.window_stride() * self.hop) as f32 / self.sr as f32 * self.bpm / 60.0
    }

    /// Shape constraints the model imposes, checked before anything allocates.
    ///
    /// The generator builds its seed grid as `n_mels / 16` by
    /// `time_frames / 16` and then upsamples ×2 four times, so both dimensions
    /// must be positive multiples of 16 or the output silently comes back the
    /// wrong size. "Silently" is the operative word and the reason this exists
    /// as a check rather than a comment: a seed grid of width 1 (from
    /// `time_frames = 16`) still runs — it is a 4-wide transposed convolution
    /// operating almost entirely on padding — and what it produces looks like
    /// a training failure rather than a configuration one. That cost a
    /// debugging session while building the smoke test.
    ///
    /// The latent and filter counts are checked for the plainer reason that
    /// zero is not a size, and `g_filters` is divided by 8 for the last
    /// deconvolution stage, so a small value quietly yields a one- or
    /// two-channel layer whose ReLU can zero the whole tensor.
    pub fn validate(&self) -> Result<(), String> {
        let mut bad: Vec<String> = Vec::new();
        for (name, v) in [("n_mels", self.n_mels), ("time_frames", self.time_frames)] {
            if v == 0 || v % 16 != 0 {
                bad.push(format!(
                    "{name} = {v} must be a positive multiple of 16 (the generator's seed grid \
is n_mels/16 by time_frames/16, upsampled x2 four times)"
                ));
            }
        }
        if self.g_filters < 32 {
            bad.push(format!(
                "g_filters = {} is too small: the last deconvolution stage is g_filters/8 \
channels, and below 4 channels a ReLU can zero the entire layer",
                self.g_filters
            ));
        }
        for (name, v) in [
            ("latent", self.latent),
            ("d_filters", self.d_filters),
            ("batch_size", self.batch_size),
            ("sr", self.sr as usize),
            ("n_fft", self.n_fft),
            ("hop", self.hop),
        ] {
            if v == 0 {
                bad.push(format!("{name} must be greater than zero"));
            }
        }
        if self.hop >= self.n_fft {
            bad.push(format!(
                "hop = {} must be smaller than n_fft = {} or the STFT windows do not overlap",
                self.hop, self.n_fft
            ));
        }
        if !(1..=2).contains(&self.stems) {
            bad.push(format!(
                "stems = {} must be 1 (mixed) or 2 (harmonic + percussive); dsp.rs has no \
other splitter",
                self.stems
            ));
        }
        if self.ctx_frames > self.time_frames {
            bad.push(format!(
                "ctx_frames = {} cannot exceed time_frames = {}: the context is a tail of the \
previous window",
                self.ctx_frames, self.time_frames
            ));
        }
        if self.ctx_frames > 0 && self.ctx_dim == 0 {
            bad.push("ctx_dim must be greater than zero when ctx_frames is set".into());
        }
        if self.overlap_frames >= self.time_frames {
            bad.push(format!(
                "overlap_frames = {} must be smaller than time_frames = {} or consecutive \
windows never advance",
                self.overlap_frames, self.time_frames
            ));
        }
        if self.seq_windows == 0 {
            bad.push("seq_windows must be at least 1".into());
        }
        if self.playback == Playback::Stream && self.overlap_frames < 2 {
            bad.push(format!(
                "overlap_frames = {} is too short for stream playback: frames are centred on \
hop boundaries, so the audio overlap is (overlap_frames - 1) hops",
                self.overlap_frames
            ));
        }
        if bad.is_empty() {
            Ok(())
        } else {
            Err(bad.join("\n  - "))
        }
    }

    /// The fields that determine the SHAPE of the saved tensors.
    ///
    /// Split out because resuming is where a changed shape does its damage:
    /// `load_state` writes into existing `Var`s, so a checkpoint trained at one
    /// `time_frames` cannot be loaded into a model built at another, and the
    /// failure surfaces deep inside a tensor assignment rather than at the
    /// point the operator changed the number.
    fn shape_key(&self) -> [usize; 9] {
        [
            self.n_mels,
            self.time_frames,
            self.latent,
            self.g_filters,
            self.d_filters,
            self.n_fft,
            // The conditional renderer: stems set the width of the last
            // deconvolution and the discriminators' first block; ctx_frames
            // (as on/off) and ctx_dim set fc's input width; beat_phase adds
            // two input planes to the last deconvolution.
            self.stems,
            if self.ctx_frames > 0 { self.ctx_dim } else { 0 },
            // The context encoder's first conv sees ctx_frames columns but
            // pools to a vector, so its weights do not depend on the count;
            // its PRESENCE does, and the ctx_dim entry carries that.
            self.beat_phase as usize,
        ]
    }

    /// Can weights saved under `other` be loaded into a model built from self?
    ///
    /// Names the specific field that differs, because "shape mismatch" sends
    /// the reader to look at their data.
    pub fn shape_compatible_with(&self, other: &Config) -> Result<(), String> {
        let (a, b) = (self.shape_key(), other.shape_key());
        if a == b {
            return Ok(());
        }
        let names = [
            "n_mels", "time_frames", "latent", "g_filters", "d_filters", "n_fft",
            "stems", "ctx_dim (0 when ctx_frames is 0)", "beat_phase",
        ];
        let diffs: Vec<String> = names
            .iter()
            .zip(a.iter().zip(b.iter()))
            .filter(|(_, (x, y))| x != y)
            .map(|(n, (x, y))| format!("{n}: checkpoint has {y}, you asked for {x}"))
            .collect();
        Err(diffs.join("\n  - "))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_config_is_valid() {
        // If this ever fails, every other test here is testing a shape nobody
        // ships.
        Config::default().validate().expect("default must validate");
    }

    #[test]
    fn the_smoke_test_shape_is_valid() {
        // smoke::tiny_config picks 32/32/64/32 deliberately; those choices were
        // arrived at by watching a too-small model produce exactly-zero output.
        // Pin them here so a later tidy-up cannot quietly undo it.
        let c = Config { n_mels: 32, time_frames: 32, latent: 16, g_filters: 64,
                         d_filters: 32, ..Config::default() };
        c.validate().expect("the tiny config must stay valid");
    }

    #[test]
    fn a_seed_grid_narrower_than_16_is_refused() {
        // time_frames = 16 gives a width-1 seed and a 4-wide transposed
        // convolution running almost entirely on padding. It RUNS, which is
        // the problem: the output looks like a training failure.
        let c = Config { time_frames: 24, ..Config::default() };
        let e = c.validate().unwrap_err();
        assert!(e.contains("time_frames = 24"), "must name the field and value: {e}");
        assert!(e.contains("multiple of 16"), "must name the constraint: {e}");
    }

    #[test]
    fn zero_dimensions_are_refused_by_name() {
        let c = Config { n_mels: 0, latent: 0, ..Config::default() };
        let e = c.validate().unwrap_err();
        assert!(e.contains("n_mels"), "{e}");
        assert!(e.contains("latent"), "{e}");
    }

    #[test]
    fn a_generator_too_narrow_to_survive_its_last_stage_is_refused() {
        // g_filters/8 is the last deconvolution's channel count. At 16 that is
        // two channels, where "both are negative so ReLU zeroes the layer" is a
        // coin flip rather than a pathology — observed, not theorised.
        let c = Config { g_filters: 16, ..Config::default() };
        assert!(c.validate().unwrap_err().contains("g_filters"));
    }

    #[test]
    fn a_hop_at_least_as_wide_as_the_window_is_refused() {
        let c = Config { n_fft: 256, hop: 256, ..Config::default() };
        assert!(c.validate().unwrap_err().contains("do not overlap"));
    }

    #[test]
    fn an_identical_config_is_shape_compatible() {
        let c = Config::default();
        c.shape_compatible_with(&c).expect("identical must be compatible");
    }

    #[test]
    fn a_non_shape_field_does_not_block_a_resume() {
        // Changing the learning rate, the epoch count or where the data lives
        // must NOT invalidate a checkpoint — those are the knobs an operator
        // legitimately turns between runs.
        let a = Config::default();
        let b = Config { lr: 1e-5, epochs: 5, batch_size: 64,
                         data_dir: "/elsewhere".into(), ..Config::default() };
        a.shape_compatible_with(&b).expect("only shape fields may block a resume");
    }

    #[test]
    fn a_changed_shape_names_the_field_and_both_values() {
        // The whole point: `load_state` writes into existing Vars, so this
        // otherwise fails deep inside a tensor assignment, far from the number
        // the operator actually changed.
        let a = Config { time_frames: 128, ..Config::default() };
        let b = Config::default(); // time_frames 64
        let e = a.shape_compatible_with(&b).unwrap_err();
        assert!(e.contains("time_frames"), "{e}");
        assert!(e.contains("64"), "must show the checkpoint's value: {e}");
        assert!(e.contains("128"), "must show the requested value: {e}");
        assert!(!e.contains("n_mels"), "must not report fields that match: {e}");
    }

    #[test]
    fn every_differing_shape_field_is_reported_not_just_the_first() {
        let a = Config { n_mels: 256, latent: 64, ..Config::default() };
        let e = a.shape_compatible_with(&Config::default()).unwrap_err();
        assert!(e.contains("n_mels") && e.contains("latent"), "{e}");
    }

    #[test]
    fn json_round_trips_through_a_file() {
        let dir = std::env::temp_dir().join("beatspace-cfg-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.json");
        let c = Config { time_frames: 128, ..Config::default() };
        c.save_json(&p).unwrap();
        let back = Config::load_json(&p).unwrap();
        // Shape equality is what resume actually depends on.
        c.shape_compatible_with(&back).expect("round trip must preserve shape");
        std::fs::remove_dir_all(&dir).ok();
    }
}
