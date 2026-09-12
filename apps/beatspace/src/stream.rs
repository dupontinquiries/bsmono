//! The stream: a track that never restarts.
//!
//! Three pieces, none of which touch an audio device or a window, so all
//! three run in a test:
//!
//! * `Conductor` — the slow state that steps once per window. Today the only
//!   implementation is `LatentWalk`, the GUI's `0.9·cur + 0.1·target`
//!   glide moved out of `gen_loop` and given a beat-phase accumulator. The
//!   trait is the seam the learned conductor (GRU over encoded windows) will
//!   fill; the renderer does not know which it has.
//! * `Streamer` — owns the generator, the previous window's planes and the
//!   Griffin-Lim stream state, and turns one conductor step into one
//!   `StreamWindow` of audio: `generate_next` with the previous window as
//!   context, then `continue_stream` to invert it in time-aligned fashion.
//! * `OverlapAdd` — the sample-level core of the stream player. Windows are
//!   placed `advance` samples after the previous one and the overlapping
//!   region is equal-power crossfaded. Pull samples out; ask how many are
//!   queued so generation can pace to the audio clock rather than a timer.
//!
//! `player.rs` (behind the `app` feature) wraps `OverlapAdd` in the cpal
//! callback; `gui.rs` drives `Streamer` from its generator thread.

use anyhow::Result;
use candle_core::Device;

use crate::config::Config;
use crate::dsp::{GlStream, Mel, StreamWindow};
use crate::model::Generator;

/// What the renderer conditions each window on, beyond the audio before it.
pub struct Step<'a> {
    /// The latent for this window.
    pub z: &'a [f32],
    /// Beat phase of each column, or `None` when the model has no phase
    /// planes.
    pub phase: Option<&'a [f32]>,
}

/// The slow state of the track. One `step` per window.
pub trait Conductor {
    /// Advance by one window toward `target` and return this window's step.
    fn step(&mut self, target: &[f32]) -> Step<'_>;
    /// The current latent, for display.
    fn current(&self) -> &[f32];
    /// Beat phase at the START of the next window, in [0, 1).
    fn beat_phase(&self) -> f32;
}

/// The glide the GUI always had, plus a beat clock.
///
/// The phase advances by `beats_per_window` each step — so at 120 BPM with
/// a 60-frame stride at hop 256 / 22050 Hz each window is ~1.39 beats, and
/// the renderer sees each column's position in the bar. No learned state
/// yet; this is the conductor seam with the simplest thing behind it.
pub struct LatentWalk {
    cur: Vec<f32>,
    phase: f32,
    beats_per_window: f32,
    /// Per-column phase for the window being rendered, if the model wants it.
    cols: Option<Vec<f32>>,
    /// Frames between window starts: `beats_per_window` is per stride.
    stride: usize,
    smoothing: f32,
}

impl LatentWalk {
    pub fn new(cfg: &Config) -> Self {
        Self {
            cur: vec![0.0; cfg.latent],
            phase: 0.0,
            beats_per_window: cfg.beats_per_window(),
            cols: if cfg.beat_phase { Some(vec![0.0; cfg.time_frames]) } else { None },
            stride: cfg.window_stride().max(1),
            smoothing: 0.1,
        }
    }

    /// Change tempo mid-stream. The beat-per-window rate follows; the phase
    /// itself continues from where it is, so a tempo change is not a jump.
    pub fn set_bpm(&mut self, cfg: &Config, bpm: f32) {
        let c = Config { bpm, ..cfg.clone() };
        self.beats_per_window = c.beats_per_window();
    }
}

impl Conductor for LatentWalk {
    fn step(&mut self, target: &[f32]) -> Step<'_> {
        let a = self.smoothing;
        for (c, t) in self.cur.iter_mut().zip(target.iter()) {
            *c = (1.0 - a) * *c + a * *t;
        }
        if let Some(cols) = &mut self.cols {
            // Column j is `j / stride` of a window-advance into the beats it
            // covers. The window is longer than its stride by the overlap;
            // those trailing columns run past one advance and into the next
            // window, which is what makes the two agree on the overlap.
            let per_col = self.beats_per_window / self.stride as f32;
            for (j, c) in cols.iter_mut().enumerate() {
                let p = self.phase + j as f32 * per_col;
                *c = p - p.floor();
            }
            self.phase = (self.phase + self.beats_per_window).fract();
        }
        Step { z: &self.cur, phase: self.cols.as_deref() }
    }
    fn current(&self) -> &[f32] {
        &self.cur
    }
    fn beat_phase(&self) -> f32 {
        self.phase
    }
}

/// Turns conductor steps into audio windows.
pub struct Streamer {
    g: Generator,
    mel: Mel,
    gl: GlStream,
    /// The previous window's planes, `stems x (n_mels x T)`, as the next
    /// window's context. `None` until the first window has been rendered.
    prev: Option<Vec<f32>>,
    cfg: Config,
    dev: Device,
}

impl Streamer {
    pub fn new(cfg: &Config, g: Generator, dev: Device) -> Self {
        Self { g, mel: Mel::new(cfg), gl: GlStream::new(), prev: None, cfg: cfg.clone(), dev }
    }

    /// Swap the generator (a checkpoint reload). The stream continues: the
    /// previous planes still stand as context, and the phase state carries.
    pub fn set_generator(&mut self, g: Generator) {
        self.g = g;
    }

    pub fn generator(&self) -> &Generator {
        &self.g
    }

    /// Forget the previous window and the phase state. The next window
    /// starts a new track rather than continuing this one.
    pub fn reset(&mut self) {
        self.prev = None;
        self.gl.reset();
    }

    /// The last planes rendered, for a spectrogram display or a save.
    pub fn last_planes(&self) -> Option<&[f32]> {
        self.prev.as_deref()
    }

    /// Render the next window of the stream.
    ///
    /// The first window has no previous one; when the generator needs a
    /// context it is given SILENCE — every plane at the unit-range floor —
    /// which is a context the model has seen at every clip start.
    pub fn next_window(&mut self, step: &Step<'_>, n_iter: usize) -> Result<StreamWindow> {
        let (t, m, c) = (self.cfg.time_frames, self.cfg.n_mels, self.cfg.stems);
        let silence;
        let prev: Option<&[f32]> = match &self.prev {
            Some(p) => Some(p.as_slice()),
            None if self.cfg.ctx_frames > 0 => {
                silence = vec![-1.0f32; c * m * t];
                Some(silence.as_slice())
            }
            None => None,
        };
        let (planes, _, _) = self.g.generate_next(
            step.z, prev, self.cfg.ctx_frames, step.phase, m, t, &self.dev,
        )?;
        let w = self.mel.continue_stream(&planes, c, t, self.cfg.overlap_frames, n_iter, &mut self.gl);
        self.prev = Some(planes);
        Ok(w)
    }

    /// A clean, high-iteration render of the LAST window from a fresh phase,
    /// for Save WAV. Does not disturb the stream's state.
    pub fn render_last_hq(&self, n_iter: usize) -> Option<Vec<f32>> {
        let planes = self.prev.as_ref()?;
        let (t, c) = (self.cfg.time_frames, self.cfg.stems);
        let mut st = GlStream::new();
        Some(self.mel.continue_stream(planes, c, t, self.cfg.overlap_frames, n_iter, &mut st).audio)
    }
}

/// Sample-level overlap-add of stream windows.
///
/// Holds a queue of samples. `push` places a window `advance` samples after
/// the previous window's start; where the new window overlaps what is
/// already queued, the two are equal-power crossfaded. `pull` drains from
/// the front. Everything is at ONE sample rate — the caller resamples to
/// the device rate before pushing, exactly as the loop player does.
pub struct OverlapAdd {
    buf: Vec<f32>,
    /// Index into `buf` where the most recently pushed window starts.
    last_start: usize,
}

impl Default for OverlapAdd {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlapAdd {
    pub fn new() -> Self {
        Self { buf: Vec::new(), last_start: 0 }
    }

    /// Samples queued and not yet pulled.
    pub fn queued(&self) -> usize {
        self.buf.len()
    }

    /// Add a window. `advance` is in the pushed window's sample rate and
    /// counts from the start of the previous window; the first window's
    /// `advance` is ignored.
    pub fn push(&mut self, audio: &[f32], advance: usize) {
        if self.buf.is_empty() {
            self.buf.extend_from_slice(audio);
            self.last_start = 0;
            return;
        }
        let start = self.last_start + advance;
        // If playback has already drained past where this window should
        // begin, it is late: place it at the front rather than in the past.
        let start = start.min(self.buf.len());
        let overlap = self.buf.len() - start;
        let fade = overlap.min(audio.len());
        for i in 0..fade {
            // Equal-power: cos out, sin in, so the sum of squares stays flat
            // through the crossfade.
            let x = (i as f32 + 0.5) / fade as f32 * std::f32::consts::FRAC_PI_2;
            let (out, inn) = (x.cos(), x.sin());
            self.buf[start + i] = self.buf[start + i] * out + audio[i] * inn;
        }
        self.buf.extend_from_slice(&audio[fade..]);
        self.last_start = start;
    }

    /// Fill `out` from the front of the queue, zeros if it runs dry.
    /// Returns how many real samples were written.
    pub fn pull(&mut self, out: &mut [f32]) -> usize {
        let n = out.len().min(self.buf.len());
        out[..n].copy_from_slice(&self.buf[..n]);
        for o in &mut out[n..] {
            *o = 0.0;
        }
        self.buf.drain(..n);
        self.last_start = self.last_start.saturating_sub(n);
        n
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.last_start = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_add_places_windows_at_the_advance_and_crossfades_the_overlap() {
        let mut oa = OverlapAdd::new();
        oa.push(&[1.0; 10], 0);
        assert_eq!(oa.queued(), 10);
        // Second window starts 6 in, so it overlaps the last 4 samples.
        oa.push(&[1.0; 10], 6);
        assert_eq!(oa.queued(), 16);
        let mut out = vec![0.0; 16];
        assert_eq!(oa.pull(&mut out), 16);
        // Outside the overlap, untouched.
        assert!(out[..6].iter().all(|&x| (x - 1.0).abs() < 1e-6));
        assert!(out[10..].iter().all(|&x| (x - 1.0).abs() < 1e-6));
        // Inside it, an equal-power blend of two equal signals: cos + sin
        // ranges from ~1 at the ends to sqrt(2) in the middle, never below 1.
        for i in 6..10 {
            assert!(out[i] >= 1.0 - 1e-6 && out[i] <= std::f32::consts::SQRT_2 + 1e-6, "sample {i} = {}", out[i]);
        }
        // The power stays flat for a crossfade between a signal and itself
        // inverted: cos^2 + sin^2 = 1.
        let mut oa = OverlapAdd::new();
        oa.push(&[1.0; 4], 0);
        oa.push(&[-1.0; 4], 0);
        let mut out = vec![0.0; 4];
        oa.pull(&mut out);
        for (i, x) in out.iter().enumerate() {
            let phase = (i as f32 + 0.5) / 4.0 * std::f32::consts::FRAC_PI_2;
            assert!((x - (phase.cos() - phase.sin())).abs() < 1e-6);
        }
    }

    #[test]
    fn overlap_add_pull_runs_dry_with_zeros_and_keeps_the_anchor() {
        let mut oa = OverlapAdd::new();
        oa.push(&[0.5; 8], 0);
        let mut out = vec![9.0; 5];
        assert_eq!(oa.pull(&mut out), 5);
        assert_eq!(oa.queued(), 3);
        // The next window's advance is measured from the previous window's
        // start, which has been drained past: it lands at the front.
        oa.push(&[0.25; 4], 8);
        assert_eq!(oa.queued(), 7);
        let mut out = vec![9.0; 10];
        assert_eq!(oa.pull(&mut out), 7);
        assert!(out[7..].iter().all(|&x| x == 0.0));
        assert_eq!(oa.queued(), 0);
    }

    #[test]
    fn overlap_add_advance_is_from_the_previous_start_not_the_end() {
        let mut oa = OverlapAdd::new();
        oa.push(&[1.0; 10], 0);
        oa.push(&[2.0; 10], 6); // starts at 6
        oa.push(&[3.0; 10], 6); // starts at 12
        assert_eq!(oa.queued(), 22);
        let mut out = vec![0.0; 22];
        oa.pull(&mut out);
        assert!((out[16] - 3.0).abs() < 1e-6, "past every overlap the third window is untouched");
        assert!((out[11] - 2.0).abs() < 1e-6, "between overlaps the second window is untouched");
    }

    #[test]
    fn the_latent_walk_glides_and_advances_the_beat_phase_per_window() {
        let cfg = Config { time_frames: 64, overlap_frames: 4, beat_phase: true, bpm: 120.0, ..Config::default() };
        let mut w = LatentWalk::new(&cfg);
        let target = vec![1.0; cfg.latent];
        let (z0, cols) = {
            let s = w.step(&target);
            (s.z[0], s.phase.expect("phase columns").to_vec())
        };
        assert!((z0 - 0.1).abs() < 1e-6, "first step is a tenth of the way");
        assert_eq!(cols.len(), 64);
        assert_eq!(cols[0], 0.0);
        assert!(cols.windows(2).all(|p| p[1] > p[0] || p[1] < 0.1), "phase is monotone modulo wrap");
        // 60 frames at hop 256 / 22050 Hz at 120 BPM = 1.3932 beats.
        let expect: f32 = 60.0 * 256.0 / 22050.0 * 2.0;
        assert!((w.beat_phase() - expect.fract()).abs() < 1e-4, "{} vs {}", w.beat_phase(), expect.fract());
        // Column `stride` of this window should equal column 0 of the next:
        // the next window's phase is where the stride lands.
        let phase_at_stride = cols[cfg.window_stride()];
        let s2 = w.step(&target);
        assert!((s2.phase.unwrap()[0] - phase_at_stride).abs() < 1e-4);
    }

    /// The whole playback pipeline minus cpal: conductor -> renderer ->
    /// inverter -> overlap-add, on an untrained tiny dynamic model. What
    /// this asserts is plumbing — sample counts, advance, finiteness, that
    /// each window really conditions on the previous one — not sound.
    #[test]
    fn the_streamer_renders_continuing_windows_into_the_overlap_add() {
        let cfg = crate::smoke::tiny_dynamic_config();
        let dev = Device::Cpu;
        let g = Generator::new(&cfg, &dev).unwrap();
        let mut st = Streamer::new(&cfg, g, dev);
        let mut walk = LatentWalk::new(&cfg);
        let mut oa = OverlapAdd::new();
        let target = vec![0.7; cfg.latent];
        let advance = cfg.window_stride() * cfg.hop;
        let mut planes = Vec::new();
        for i in 0..3 {
            let w = {
                let step = walk.step(&target);
                st.next_window(&step, 2).unwrap()
            };
            assert_eq!(w.advance, advance);
            assert_eq!(w.audio.len(), (cfg.time_frames - 1) * cfg.hop);
            assert!(w.audio.iter().all(|x| x.is_finite() && x.abs() <= 0.9 + 1e-4), "window {i}");
            oa.push(&w.audio, w.advance);
            planes.push(st.last_planes().unwrap().to_vec());
        }
        // Three windows, two advances, one window length: what is queued.
        assert_eq!(oa.queued(), 2 * advance + (cfg.time_frames - 1) * cfg.hop);
        // Each window was generated with the previous one as context: with
        // the latent gliding toward one target the planes must still differ,
        // and the model must see the difference (context goes through the
        // encoder, so equal planes here would mean the context was ignored
        // or the previous planes were not handed on).
        let diff: f32 = planes[1].iter().zip(&planes[2]).map(|(a, b)| (a - b).abs()).sum::<f32>() / planes[1].len() as f32;
        assert!(diff > 0.0);
        // The HQ render of the last window is a full-length, finite signal.
        let hq = st.render_last_hq(4).unwrap();
        assert_eq!(hq.len(), (cfg.time_frames - 1) * cfg.hop);
        assert!(hq.iter().all(|x| x.is_finite()));
        // Reset forgets the context; the next window is a fresh start.
        st.reset();
        assert!(st.last_planes().is_none());
    }

    #[test]
    fn the_latent_walk_without_beat_phase_has_no_columns() {
        let cfg = Config::default();
        let mut w = LatentWalk::new(&cfg);
        assert!(w.step(&vec![0.0; cfg.latent]).phase.is_none());
    }
}
