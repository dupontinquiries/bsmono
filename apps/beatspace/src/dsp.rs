//! Self-contained DSP: audio <-> mel <-> audio.
//!
//! Design note: because training now happens in Rust, analysis (audio->mel) and
//! synthesis (mel->audio via Griffin-Lim) share ONE mel filterbank and ONE
//! normalization. Internal consistency — not bit-exact librosa parity — is what
//! makes generated spectrograms invert into coherent sound.
//!
//! Pipeline (matches the original kBM3 numbers):
//!   audio --STFT--> |.|^2 --mel--> power_to_db --clip[floor,ceil]--> unit[-1,1]
//! and the exact inverse, with Griffin-Lim recovering phase.

use crate::config::Config;
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

/// Persistent Griffin-Lim phase carried between consecutive realtime beats.
/// Holds the last complex spectrogram so the next inversion can warm-start from
/// it. Opaque on purpose — callers just create one and pass it back in.
#[derive(Default)]
pub struct GlState {
    spec: Vec<Complex32>,
}

impl GlState {
    pub fn new() -> Self {
        Self { spec: Vec::new() }
    }
}

/// Griffin-Lim state for a CONTINUING stream, one entry per stem.
///
/// `GlState` reuses the previous window's phase index-for-index, which is the
/// right seed when the same content is being re-rendered (the loop player,
/// gliding z). A continuation is a different window in time: its first
/// `overlap` frames coincide with the previous window's LAST `overlap`, and
/// everything after is new. So the seed here is time-aligned — those frames
/// take the previous phase directly, and the rest are propagated forward at
/// each bin's centre frequency, which is what a stationary partial would do.
#[derive(Default)]
pub struct GlStream {
    /// Previous window's complex spectrogram per stem, `(n_freq x T)` each.
    prev: Vec<Vec<Complex32>>,
    /// Output gain, held across windows. The loop path peak-normalises each
    /// window on its own, which on a stream would pump the level every
    /// window; this attacks instantly (never clips) and releases slowly.
    gain: f32,
}

impl GlStream {
    pub fn new() -> Self {
        Self { prev: Vec::new(), gain: 1.0 }
    }
    pub fn reset(&mut self) {
        self.prev.clear();
        self.gain = 1.0;
    }
}

/// One rendered window of a stream, ready for the overlap-add player.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamWindow {
    /// Mono audio at the model sample rate.
    pub audio: Vec<f32>,
    /// Samples after the START of the previous window that this one starts.
    /// The player crossfades whatever the two overlap by.
    pub advance: usize,
}

pub struct Mel {
    #[allow(dead_code)]
    sr: u32,
    n_fft: usize,
    hop: usize,
    n_mels: usize,
    n_freq: usize,
    db_floor: f32,
    db_ceil: f32,

    window: Vec<f32>,         // periodic hann, len n_fft
    mel_fb: Vec<f32>,         // (n_mels x n_freq) row-major  — analysis
    mel_inv: Vec<f32>,        // (n_freq x n_mels) row-major  — synthesis (pseudo-inverse)

    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
}

impl Mel {
    pub fn new(cfg: &Config) -> Self {
        let n_freq = cfg.n_freq();
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(cfg.n_fft);
        let ifft = planner.plan_fft_inverse(cfg.n_fft);

        // periodic hann window (fftbins=True equivalent)
        let window: Vec<f32> = (0..cfg.n_fft)
            .map(|n| 0.5 - 0.5 * (2.0 * PI * n as f32 / cfg.n_fft as f32).cos())
            .collect();

        let mel_fb = build_mel_filterbank(cfg.sr, cfg.n_fft, cfg.n_mels, cfg.fmin, cfg.fmax);
        let mel_inv = build_mel_pseudo_inverse(&mel_fb, cfg.n_mels, n_freq);

        Self {
            sr: cfg.sr,
            n_fft: cfg.n_fft,
            hop: cfg.hop,
            n_mels: cfg.n_mels,
            n_freq,
            db_floor: cfg.db_floor,
            db_ceil: cfg.db_ceil,
            window,
            mel_fb,
            mel_inv,
            fft,
            ifft,
        }
    }

    #[allow(dead_code)]
    pub fn n_mels(&self) -> usize {
        self.n_mels
    }

    // ---------- analysis: audio -> unit mel (n_mels x T), row-major ----------

    /// Returns (mel_unit flattened row-major as (n_mels, T), T).
    pub fn wav_to_mel(&self, audio: &[f32]) -> (Vec<f32>, usize) {
        let frames = self.stft_power(audio); // (n_freq x T)
        let t = frames.len() / self.n_freq;
        let mut mel = vec![0.0f32; self.n_mels * t];
        // mel = mel_fb (n_mels x n_freq) @ power (n_freq x T)
        for m in 0..self.n_mels {
            let fb_row = &self.mel_fb[m * self.n_freq..(m + 1) * self.n_freq];
            for col in 0..t {
                let mut acc = 0.0f32;
                for f in 0..self.n_freq {
                    acc += fb_row[f] * frames[f * t + col];
                }
                // power_to_db then -> unit
                let db = 10.0 * (acc.max(1e-10)).log10();
                mel[m * t + col] = self.db_to_unit(db);
            }
        }
        (mel, t)
    }

    fn db_to_unit(&self, db: f32) -> f32 {
        let db = db.clamp(self.db_floor, self.db_ceil);
        (db - self.db_floor) / (self.db_ceil - self.db_floor) * 2.0 - 1.0
    }

    fn unit_to_db(&self, unit: f32) -> f32 {
        let unit = unit.clamp(-1.0, 1.0);
        (unit + 1.0) / 2.0 * (self.db_ceil - self.db_floor) + self.db_floor
    }

    /// Power spectrogram (n_freq x T) -> unit mel (n_mels x T). The second
    /// half of `wav_to_mel`, split out so several stems can share one STFT.
    fn power_to_unit_mel(&self, power: &[f32], t: usize) -> Vec<f32> {
        let mut mel = vec![0.0f32; self.n_mels * t];
        for m in 0..self.n_mels {
            let fb_row = &self.mel_fb[m * self.n_freq..(m + 1) * self.n_freq];
            for col in 0..t {
                let mut acc = 0.0f32;
                for f in 0..self.n_freq {
                    acc += fb_row[f] * power[f * t + col];
                }
                let db = 10.0 * (acc.max(1e-10)).log10();
                mel[m * t + col] = self.db_to_unit(db);
            }
        }
        mel
    }

    /// Everything the cache stores for one clip: `stems` unit-mel planes,
    /// each `(n_mels x T)` row-major, and a beat phase per frame.
    ///
    /// `stems == 1` is exactly `wav_to_mel`. `stems == 2` splits the power
    /// spectrogram into harmonic and percussive parts first (HPSS), so the
    /// planes are "instruments" and "drums" — an approximation, but one that
    /// costs two median filters and no model, and it gives the generator two
    /// things to co-vary rather than one mixture to disentangle on its own.
    pub fn analyse(&self, audio: &[f32], stems: usize) -> Analysis {
        let power = self.stft_power(audio);
        let t = power.len() / self.n_freq;
        let phase = beat_phase(&power, self.n_freq, t, self.hop, self.sr);
        let planes = match stems {
            1 => vec![self.power_to_unit_mel(&power, t)],
            _ => {
                let (h, p) = hpss(&power, self.n_freq, t, 17, 17);
                vec![self.power_to_unit_mel(&h, t), self.power_to_unit_mel(&p, t)]
            }
        };
        Analysis { planes, phase, t }
    }

    // ---------- synthesis: unit mel (n_mels x T) -> audio ----------

    /// unit mel (n_mels x T) -> linear-frequency magnitude (n_freq x T).
    fn mel_to_mag(&self, mel_unit: &[f32], t: usize) -> Vec<f32> {
        // 1) unit -> mel power
        let mut mel_power = vec![0.0f32; self.n_mels * t];
        for i in 0..mel_unit.len().min(mel_power.len()) {
            let db = self.unit_to_db(mel_unit[i]);
            mel_power[i] = 10f32.powf(db / 10.0); // db_to_power
        }
        // 2) invert mel: lin_power (n_freq x T) = mel_inv (n_freq x n_mels) @ mel_power
        let mut mag = vec![0.0f32; self.n_freq * t];
        for f in 0..self.n_freq {
            let inv_row = &self.mel_inv[f * self.n_mels..(f + 1) * self.n_mels];
            for col in 0..t {
                let mut acc = 0.0f32;
                for m in 0..self.n_mels {
                    acc += inv_row[m] * mel_power[m * t + col];
                }
                mag[f * t + col] = acc.max(0.0).sqrt(); // power -> magnitude
            }
        }
        mag
    }

    /// High-quality, stateless inversion (fresh random-free zero phase, full
    /// iterations). Use this for offline renders / Save WAV.
    pub fn mel_to_wav(&self, mel_unit: &[f32], t: usize, n_iter: usize) -> Vec<f32> {
        let mag = self.mel_to_mag(mel_unit, t);
        let mut st = GlState::new();
        self.griffin_lim(&mag, t, n_iter, &mut st)
    }

    /// Realtime inversion: re-seeds Griffin-Lim with the phase from the previous
    /// beat (carried in `st`). Because the latent glides, consecutive beats are
    /// nearly identical, so a warm start converges in a handful of iterations
    /// instead of the dozens a cold start needs — this is the main realtime win.
    pub fn mel_to_wav_warm(
        &self,
        mel_unit: &[f32],
        t: usize,
        n_iter: usize,
        st: &mut GlState,
    ) -> Vec<f32> {
        let mag = self.mel_to_mag(mel_unit, t);
        self.griffin_lim(&mag, t, n_iter, st)
    }

    /// Render the next window of a stream and mix its stems.
    ///
    /// `planes` is `stems` unit-mel planes of `(n_mels x t)` each, laid end to
    /// end — the generator's output for one window. The window starts
    /// `t - overlap` frames after the previous one; the returned
    /// `StreamWindow::advance` says the same thing in samples.
    ///
    /// Each stem is inverted on its own and summed in the audio domain. Drums
    /// and instruments want different phase behaviour — a transient is not a
    /// propagated partial — and separate inversions let each converge to its
    /// own answer rather than fighting over one phase field. They run in
    /// parallel; Griffin-Lim is the realtime cost and this doubles it.
    pub fn continue_stream(
        &self,
        planes: &[f32],
        stems: usize,
        t: usize,
        overlap: usize,
        n_iter: usize,
        st: &mut GlStream,
    ) -> StreamWindow {
        use rayon::prelude::*;
        let per = self.n_mels * t;
        let overlap = overlap.min(t);
        if st.prev.len() != stems {
            st.prev = vec![Vec::new(); stems];
        }
        // The previous phases are taken out so the stems can be inverted
        // without sharing `st` across threads, and put back below.
        let prev = std::mem::take(&mut st.prev);
        let rendered: Vec<(Vec<f32>, Vec<Complex32>)> = (0..stems)
            .into_par_iter()
            .map(|s| {
                let mag = self.mel_to_mag(&planes[s * per..(s + 1) * per], t);
                let seed = self.continuation_seed(&mag, t, overlap, &prev[s]);
                self.griffin_lim_from(&mag, t, n_iter, seed)
            })
            .collect();
        let n_out = rendered.first().map(|r| r.0.len()).unwrap_or(0);
        let mut audio = vec![0.0f32; n_out];
        let mut specs = Vec::with_capacity(stems);
        for (a, spec) in rendered {
            for (o, x) in audio.iter_mut().zip(a.iter()) {
                *o += x;
            }
            specs.push(spec);
        }
        st.prev = specs;

        // Limiter-shaped gain: drop instantly if this window would clip,
        // recover slowly otherwise. Never a per-window normalisation.
        let peak = audio.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        let ceiling = if peak > 1e-6 { (0.9 / peak).min(1.0) } else { 1.0 };
        st.gain = if ceiling < st.gain { ceiling } else { 0.9 * st.gain + 0.1 * ceiling };
        for x in &mut audio {
            *x *= st.gain;
        }
        StreamWindow { audio, advance: (t - overlap) * self.hop }
    }

    /// The initial complex spectrogram for a continuation: previous phase on
    /// the overlapping frames, propagated phase after them.
    ///
    /// Frame `j` of the new window is frame `t - overlap + j` of the previous
    /// one for `j < overlap`. Past that, each bin's phase advances by its
    /// centre frequency times the hop: `2*pi * k/n_fft * hop`. That is the
    /// phase-vocoder assumption, exactly right for a steady partial at the bin
    /// centre and a better start than zero for everything else — Griffin-Lim
    /// then corrects it. With no previous window the first column is zero
    /// phase and the rest advance from it, which is the same rule minus the
    /// history — an all-zero seed is a pulse train, and a worse start.
    fn continuation_seed(
        &self,
        mag: &[f32],
        t: usize,
        overlap: usize,
        prev: &[Complex32],
    ) -> Vec<Complex32> {
        let n = self.n_freq * t;
        let mut spec = vec![Complex32::new(0.0, 0.0); n];
        let prev_t = if self.n_freq > 0 { prev.len() / self.n_freq } else { 0 };
        let warm = prev_t >= overlap && prev_t > 0;
        for f in 0..self.n_freq {
            let step = 2.0 * PI * (f as f32) / (self.n_fft as f32) * (self.hop as f32);
            let mut last = 0.0f32; // phase of the previous column
            for j in 0..t {
                let ph = if warm && j < overlap {
                    let p = prev[f * prev_t + (prev_t - overlap + j)];
                    if p.norm_sqr() > 1e-24 { p.im.atan2(p.re) } else { last }
                } else if j == 0 {
                    0.0
                } else {
                    last + step
                };
                last = ph;
                spec[f * t + j] = Complex32::from_polar(mag[f * t + j], ph);
            }
        }
        spec
    }

    /// Griffin-Lim iterations from a given complex seed. Returns the audio and
    /// the final spectrogram (for the next seed). No normalisation.
    fn griffin_lim_from(
        &self,
        mag: &[f32],
        t: usize,
        n_iter: usize,
        mut spec: Vec<Complex32>,
    ) -> (Vec<f32>, Vec<Complex32>) {
        let n = self.n_freq * t;
        let mut audio = self.istft(&spec, t);
        for _ in 0..n_iter.max(1) {
            let est = self.stft_complex(&audio, t);
            for i in 0..n {
                let ph = est[i];
                let norm = (ph.re * ph.re + ph.im * ph.im).sqrt();
                let unit = if norm > 1e-12 {
                    Complex32::new(ph.re / norm, ph.im / norm)
                } else {
                    Complex32::new(1.0, 0.0)
                };
                spec[i] = unit * mag[i];
            }
            audio = self.istft(&spec, t);
        }
        (audio, spec)
    }

    /// Griffin-Lim with externally-held phase state. If `st` already holds a
    /// spectrogram of the right size, its phase seeds the first estimate;
    /// otherwise we start from zero phase. The final phase is stored back into
    /// `st` for the next call.
    fn griffin_lim(&self, mag: &[f32], t: usize, n_iter: usize, st: &mut GlState) -> Vec<f32> {
        let n = self.n_freq * t;
        let mut spec = vec![Complex32::new(0.0, 0.0); n];
        let warm = st.spec.len() == n;
        for i in 0..n {
            if warm {
                // reuse previous phase, swap in the new target magnitude
                let p = st.spec[i];
                let norm = (p.re * p.re + p.im * p.im).sqrt();
                if norm > 1e-12 {
                    spec[i] = Complex32::new(p.re / norm * mag[i], p.im / norm * mag[i]);
                } else {
                    spec[i] = Complex32::new(mag[i], 0.0);
                }
            } else {
                spec[i] = Complex32::new(mag[i], 0.0); // zero phase
            }
        }
        let mut audio = self.istft(&spec, t);
        for _ in 0..n_iter.max(1) {
            // re-estimate phase from current audio, keep target magnitude
            let est = self.stft_complex(&audio, t);
            for i in 0..n {
                let ph = est[i];
                let norm = (ph.re * ph.re + ph.im * ph.im).sqrt();
                let unit = if norm > 1e-12 {
                    Complex32::new(ph.re / norm, ph.im / norm)
                } else {
                    Complex32::new(1.0, 0.0)
                };
                spec[i] = unit * mag[i];
            }
            audio = self.istft(&spec, t);
        }
        // carry phase to the next beat
        st.spec.clear();
        st.spec.extend_from_slice(&spec);
        // gentle peak normalize so realtime morphs stay at a stable level
        let peak = audio.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        if peak > 1e-6 {
            let g = 0.9 / peak;
            for s in &mut audio {
                *s *= g;
            }
        }
        audio
    }

    // ---------- raw STFT helpers ----------

    fn n_frames(&self, len: usize) -> usize {
        // center=True: pad n_fft/2 each side; frames = 1 + len/hop
        1 + len / self.hop
    }

    /// Power spectrogram (n_freq x T) row-major. center-padded, reflected edges.
    fn stft_power(&self, audio: &[f32]) -> Vec<f32> {
        let t = self.n_frames(audio.len());
        let padded = reflect_pad(audio, self.n_fft / 2);
        let mut out = vec![0.0f32; self.n_freq * t];
        let mut buf = vec![Complex32::new(0.0, 0.0); self.n_fft];
        for frame in 0..t {
            let start = frame * self.hop;
            for n in 0..self.n_fft {
                let s = padded.get(start + n).copied().unwrap_or(0.0);
                buf[n] = Complex32::new(s * self.window[n], 0.0);
            }
            self.fft.process(&mut buf);
            for f in 0..self.n_freq {
                let c = buf[f];
                out[f * t + frame] = c.re * c.re + c.im * c.im; // power
            }
        }
        out
    }

    /// Complex STFT (n_freq x T) row-major.
    fn stft_complex(&self, audio: &[f32], t: usize) -> Vec<Complex32> {
        let padded = reflect_pad(audio, self.n_fft / 2);
        let mut out = vec![Complex32::new(0.0, 0.0); self.n_freq * t];
        let mut buf = vec![Complex32::new(0.0, 0.0); self.n_fft];
        for frame in 0..t {
            let start = frame * self.hop;
            for n in 0..self.n_fft {
                let s = padded.get(start + n).copied().unwrap_or(0.0);
                buf[n] = Complex32::new(s * self.window[n], 0.0);
            }
            self.fft.process(&mut buf);
            for f in 0..self.n_freq {
                out[f * t + frame] = buf[f];
            }
        }
        out
    }

    /// Inverse STFT with windowed overlap-add + COLA normalization.
    fn istft(&self, spec: &[Complex32], t: usize) -> Vec<f32> {
        let out_len = self.hop * (t - 1) + self.n_fft;
        let mut out = vec![0.0f32; out_len];
        let mut wsum = vec![0.0f32; out_len]; // sum of window^2 for normalization
        let mut buf = vec![Complex32::new(0.0, 0.0); self.n_fft];
        let scale = 1.0 / self.n_fft as f32; // rustfft inverse is unnormalized

        for frame in 0..t {
            // rebuild full hermitian spectrum
            for f in 0..self.n_freq {
                buf[f] = spec[f * t + frame];
            }
            for f in self.n_freq..self.n_fft {
                let mirror = self.n_fft - f;
                buf[f] = spec[mirror * t + frame].conj();
            }
            self.ifft.process(&mut buf);
            let start = frame * self.hop;
            for n in 0..self.n_fft {
                let w = self.window[n];
                out[start + n] += buf[n].re * scale * w;
                wsum[start + n] += w * w;
            }
        }
        for i in 0..out_len {
            if wsum[i] > 1e-8 {
                out[i] /= wsum[i];
            }
        }
        // trim the center padding to recover the original length region
        let pad = self.n_fft / 2;
        let begin = pad.min(out_len);
        let end = out_len.saturating_sub(pad).max(begin);
        out[begin..end].to_vec()
    }
}

/// What `Mel::analyse` returns for one clip.
pub struct Analysis {
    /// `stems` planes, each `(n_mels x t)` row-major in unit range.
    pub planes: Vec<Vec<f32>>,
    /// Beat phase in [0, 1) per frame, from `beat_phase`.
    pub phase: Vec<f32>,
    pub t: usize,
}

// ---------------- harmonic / percussive separation ----------------

/// Median of a small slice, by partial sort. `buf` is scratch.
fn median_into(buf: &mut Vec<f32>) -> f32 {
    let n = buf.len();
    if n == 0 {
        return 0.0;
    }
    let (_, m, _) = buf.select_nth_unstable_by(n / 2, |a, b| a.total_cmp(b));
    *m
}

/// Harmonic/percussive source separation on a power spectrogram
/// `(n_freq x t)` row-major (Fitzgerald 2010, soft masks with p = 2).
///
/// A sustained note is a horizontal line in the spectrogram and a drum hit
/// is a vertical one, so a median filter along time keeps the first and a
/// median filter along frequency keeps the second. The two filtered
/// magnitudes become soft masks that sum to one, so the returned stems sum
/// back to the input power — which is what lets the synthesiser mix them by
/// adding audio.
///
/// `kh`/`kp` are the median kernel lengths in frames and bins; 17 is
/// librosa's default for both. Returns `(harmonic, percussive)`, each the
/// same shape as `power`.
pub fn hpss(power: &[f32], n_freq: usize, t: usize, kh: usize, kp: usize) -> (Vec<f32>, Vec<f32>) {
    let n = n_freq * t;
    debug_assert_eq!(power.len(), n);
    let mag: Vec<f32> = power.iter().map(|&p| p.max(0.0).sqrt()).collect();
    let (kh, kp) = (kh.max(1) | 1, kp.max(1) | 1); // odd kernels
    let (rh, rp) = (kh / 2, kp / 2);

    // Harmonic: median along time, per bin. Edges are clamped to the valid
    // range rather than padded with zeros — a zero pad would make every clip
    // start and end "percussive".
    let mut harm = vec![0.0f32; n];
    let mut buf = Vec::with_capacity(kh);
    for f in 0..n_freq {
        let row = &mag[f * t..(f + 1) * t];
        for j in 0..t {
            let lo = j.saturating_sub(rh);
            let hi = (j + rh + 1).min(t);
            buf.clear();
            buf.extend_from_slice(&row[lo..hi]);
            harm[f * t + j] = median_into(&mut buf);
        }
    }
    // Percussive: median along frequency, per frame.
    let mut perc = vec![0.0f32; n];
    let mut col = Vec::with_capacity(kp);
    for j in 0..t {
        for f in 0..n_freq {
            let lo = f.saturating_sub(rp);
            let hi = (f + rp + 1).min(n_freq);
            col.clear();
            col.extend((lo..hi).map(|g| mag[g * t + j]));
            perc[f * t + j] = median_into(&mut col);
        }
    }
    // Soft masks, p = 2: h^2 / (h^2 + p^2). Applied to POWER, so the two
    // outputs sum to the input where the masks sum to one.
    let mut out_h = vec![0.0f32; n];
    let mut out_p = vec![0.0f32; n];
    for i in 0..n {
        let (h2, p2) = (harm[i] * harm[i], perc[i] * perc[i]);
        let denom = h2 + p2;
        let (mh, mp) = if denom > 1e-20 { (h2 / denom, p2 / denom) } else { (0.5, 0.5) };
        out_h[i] = power[i] * mh;
        out_p[i] = power[i] * mp;
    }
    (out_h, out_p)
}

// ---------------- beat phase ----------------

/// Per-frame beat phase in [0, 1) for a power spectrogram `(n_freq x t)`.
///
/// One global tempo per clip: an onset envelope (half-wave-rectified spectral
/// flux on log-compressed magnitude), its autocorrelation over the 60–200 BPM
/// lag range weighted toward 120, the best lag refined by parabolic
/// interpolation, then the beat offset that lines the most onset energy up
/// with the grid. No beat tracking — a steady grid is what the generator is
/// asked to follow at playback, so it is what it should see in training.
///
/// A clip too short to autocorrelate returns all zeros, which the renderer
/// reads as "phase unknown" rather than "downbeat everywhere": the planes it
/// receives are sin/cos, and (0, 1) is one point on the circle like any other.
pub fn beat_phase(power: &[f32], n_freq: usize, t: usize, hop: usize, sr: u32) -> Vec<f32> {
    let fps = sr as f32 / hop.max(1) as f32; // frames per second
    let lag_of = |bpm: f32| 60.0 * fps / bpm;
    let (lag_min, lag_max) = (lag_of(200.0).floor().max(2.0) as usize, lag_of(60.0).ceil() as usize);
    if t < 2 * lag_max + 2 || n_freq == 0 {
        return vec![0.0; t];
    }

    // Onset envelope.
    let mut onset = vec![0.0f32; t];
    for j in 1..t {
        let mut acc = 0.0f32;
        for f in 0..n_freq {
            let cur = (1.0 + 10.0 * power[f * t + j].max(0.0).sqrt()).ln();
            let prev = (1.0 + 10.0 * power[f * t + j - 1].max(0.0).sqrt()).ln();
            acc += (cur - prev).max(0.0);
        }
        onset[j] = acc;
    }
    // Remove the slow trend so sustained loudness changes do not read as beats.
    let win = (fps * 0.5).round().max(1.0) as usize;
    let mut detr = vec![0.0f32; t];
    let mut sum = 0.0f32;
    let mut q = std::collections::VecDeque::with_capacity(win + 1);
    for j in 0..t {
        q.push_back(onset[j]);
        sum += onset[j];
        if q.len() > win {
            sum -= q.pop_front().unwrap_or(0.0);
        }
        detr[j] = (onset[j] - sum / q.len() as f32).max(0.0);
    }

    // Autocorrelation over the tempo lag range, with a log-normal prior
    // around 120 BPM (sigma one octave) against octave errors.
    let mut best = (0usize, f32::MIN);
    let mut scores = vec![0.0f32; lag_max + 2];
    for lag in lag_min..=lag_max {
        let mut acc = 0.0f32;
        for j in lag..t {
            acc += detr[j] * detr[j - lag];
        }
        let bpm = 60.0 * fps / lag as f32;
        let prior = (-0.5 * (bpm / 120.0).log2().powi(2)).exp();
        let s = acc * prior / (t - lag) as f32;
        scores[lag] = s;
        if s > best.1 {
            best = (lag, s);
        }
    }
    let lag = best.0;
    if lag == 0 || best.1 <= 0.0 {
        return vec![0.0; t];
    }
    // Parabolic refinement of the peak for a fractional period.
    let period = if lag > lag_min && lag < lag_max {
        let (a, b, c) = (scores[lag - 1], scores[lag], scores[lag + 1]);
        let denom = a - 2.0 * b + c;
        let delta = if denom.abs() > 1e-12 { 0.5 * (a - c) / denom } else { 0.0 };
        lag as f32 + delta.clamp(-0.5, 0.5)
    } else {
        lag as f32
    };

    // Beat offset: the phase origin that collects the most onset energy.
    let mut best_off = (0usize, f32::MIN);
    for off in 0..lag {
        let mut acc = 0.0f32;
        let mut k = 0.0f32;
        loop {
            let pos = (off as f32 + k * period).round() as usize;
            if pos >= t {
                break;
            }
            acc += detr[pos];
            k += 1.0;
        }
        if acc > best_off.1 {
            best_off = (off, acc);
        }
    }
    let off = best_off.0 as f32;
    (0..t)
        .map(|j| {
            let x = (j as f32 - off) / period;
            x - x.floor()
        })
        .collect()
}

fn reflect_pad(x: &[f32], pad: usize) -> Vec<f32> {
    if x.is_empty() {
        return vec![0.0; 2 * pad];
    }
    let n = x.len();
    let mut out = Vec::with_capacity(n + 2 * pad);
    for i in 0..pad {
        // reflect without repeating the edge sample
        let idx = (pad - i).min(n - 1);
        out.push(x[idx]);
    }
    out.extend_from_slice(x);
    for i in 0..pad {
        let idx = n.saturating_sub(2 + i);
        out.push(x[idx]);
    }
    out
}

// ---------------- mel filterbank (slaney scale + slaney norm) ----------------

fn hz_to_mel(f: f32) -> f32 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f32).ln() / 27.0;
    if f >= min_log_hz {
        min_log_mel + (f / min_log_hz).ln() / logstep
    } else {
        f / f_sp
    }
}

fn mel_to_hz(mel: f32) -> f32 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f32).ln() / 27.0;
    if mel >= min_log_mel {
        min_log_hz * (logstep * (mel - min_log_mel)).exp()
    } else {
        f_sp * mel
    }
}

/// (n_mels x n_freq) row-major filterbank.
fn build_mel_filterbank(sr: u32, n_fft: usize, n_mels: usize, fmin: f32, fmax: f32) -> Vec<f32> {
    let n_freq = n_fft / 2 + 1;
    let fft_freqs: Vec<f32> = (0..n_freq)
        .map(|i| i as f32 * sr as f32 / n_fft as f32)
        .collect();

    let min_mel = hz_to_mel(fmin);
    let max_mel = hz_to_mel(fmax);
    let mel_points: Vec<f32> = (0..n_mels + 2)
        .map(|i| {
            let mel = min_mel + (max_mel - min_mel) * i as f32 / (n_mels + 1) as f32;
            mel_to_hz(mel)
        })
        .collect();

    let mut fb = vec![0.0f32; n_mels * n_freq];
    for m in 0..n_mels {
        let lower = mel_points[m];
        let center = mel_points[m + 1];
        let upper = mel_points[m + 2];
        let dl = (center - lower).max(1e-6);
        let du = (upper - center).max(1e-6);
        // slaney area normalization
        let enorm = 2.0 / (upper - lower).max(1e-6);
        for (f, &freq) in fft_freqs.iter().enumerate() {
            let lo = (freq - lower) / dl;
            let hi = (upper - freq) / du;
            let w = lo.min(hi).max(0.0);
            fb[m * n_freq + f] = w * enorm;
        }
    }
    fb
}

/// Pseudo-inverse of the mel filterbank: (n_freq x n_mels), via the right inverse
/// M^T (M M^T + eps I)^{-1}. Computed once at startup.
fn build_mel_pseudo_inverse(mel_fb: &[f32], n_mels: usize, n_freq: usize) -> Vec<f32> {
    // A = M M^T  (n_mels x n_mels)
    let mut a = vec![0.0f32; n_mels * n_mels];
    for i in 0..n_mels {
        let ri = &mel_fb[i * n_freq..(i + 1) * n_freq];
        for j in 0..n_mels {
            let rj = &mel_fb[j * n_freq..(j + 1) * n_freq];
            let mut acc = 0.0f32;
            for f in 0..n_freq {
                acc += ri[f] * rj[f];
            }
            a[i * n_mels + j] = acc;
        }
    }
    // ridge for numerical stability
    let mut tr = 0.0f32;
    for i in 0..n_mels {
        tr += a[i * n_mels + i];
    }
    let eps = 1e-6 * (tr / n_mels as f32).max(1e-6);
    for i in 0..n_mels {
        a[i * n_mels + i] += eps;
    }
    let a_inv = invert_spd(&a, n_mels);

    // mel_inv = M^T (n_freq x n_mels) @ A_inv (n_mels x n_mels)  -> (n_freq x n_mels)
    let mut inv = vec![0.0f32; n_freq * n_mels];
    for f in 0..n_freq {
        for j in 0..n_mels {
            let mut acc = 0.0f32;
            for k in 0..n_mels {
                // M^T[f][k] == mel_fb[k][f]
                acc += mel_fb[k * n_freq + f] * a_inv[k * n_mels + j];
            }
            inv[f * n_mels + j] = acc;
        }
    }
    inv
}

/// Invert a symmetric positive-definite matrix via Cholesky. n x n, row-major.
fn invert_spd(a: &[f32], n: usize) -> Vec<f32> {
    // Cholesky: a = L L^T
    let mut l = vec![0.0f32; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut sum = a[i * n + j];
            for k in 0..j {
                sum -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                l[i * n + j] = sum.max(1e-12).sqrt();
            } else {
                l[i * n + j] = sum / l[j * n + j];
            }
        }
    }
    // invert by solving L L^T x = e_col for each column
    let mut inv = vec![0.0f32; n * n];
    let mut y = vec![0.0f32; n];
    let mut x = vec![0.0f32; n];
    for col in 0..n {
        // forward solve L y = e_col
        for i in 0..n {
            let mut sum = if i == col { 1.0 } else { 0.0 };
            for k in 0..i {
                sum -= l[i * n + k] * y[k];
            }
            y[i] = sum / l[i * n + i];
        }
        // back solve L^T x = y
        for i in (0..n).rev() {
            let mut sum = y[i];
            for k in (i + 1)..n {
                sum -= l[k * n + i] * x[k];
            }
            x[i] = sum / l[i * n + i];
        }
        for i in 0..n {
            inv[i * n + col] = x[i];
        }
    }
    inv
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_cfg() -> Config {
        Config { sr: 8_000, n_fft: 256, hop: 64, n_mels: 32, fmax: 4_000.0, ..Config::default() }
    }

    /// A sustained tone plus periodic clicks: the harmonic stem should hold
    /// the tone and the percussive stem the clicks.
    fn tone_and_clicks(sr: u32, secs: f32, bpm: f32) -> Vec<f32> {
        let n = (sr as f32 * secs) as usize;
        let period = (60.0 / bpm * sr as f32) as usize;
        (0..n)
            .map(|i| {
                let tone = 0.3 * (2.0 * PI * 440.0 * i as f32 / sr as f32).sin();
                let k = i % period;
                let click = if k < 40 { 0.8 * (1.0 - k as f32 / 40.0) } else { 0.0 };
                tone + click
            })
            .collect()
    }

    #[test]
    fn hpss_stems_sum_back_to_the_input_power() {
        let cfg = small_cfg();
        let mel = Mel::new(&cfg);
        let audio = tone_and_clicks(cfg.sr, 2.0, 120.0);
        let power = mel.stft_power(&audio);
        let t = power.len() / mel.n_freq;
        let (h, p) = hpss(&power, mel.n_freq, t, 17, 17);
        for i in 0..power.len() {
            assert!((h[i] + p[i] - power[i]).abs() <= 1e-4 * power[i].max(1e-6), "bin {i}");
        }
    }

    #[test]
    fn hpss_puts_the_tone_in_the_harmonic_stem_and_the_clicks_in_the_percussive() {
        let cfg = small_cfg();
        let mel = Mel::new(&cfg);
        let audio = tone_and_clicks(cfg.sr, 2.0, 120.0);
        let power = mel.stft_power(&audio);
        let t = power.len() / mel.n_freq;
        let (h, p) = hpss(&power, mel.n_freq, t, 17, 17);
        // The 440 Hz bin: n_fft 256 at 8 kHz is 31.25 Hz per bin -> bin 14.
        let bin = (440.0 / (cfg.sr as f32 / cfg.n_fft as f32)).round() as usize;
        let row = |x: &[f32]| x[bin * t..(bin + 1) * t].iter().sum::<f32>();
        assert!(row(&h) > 3.0 * row(&p), "tone bin: harmonic {} vs percussive {}", row(&h), row(&p));
        // A click frame: broadband, so the high bins (well above the tone)
        // should be percussive.
        let hi = |x: &[f32]| (mel.n_freq / 2..mel.n_freq).map(|f| x[f * t..(f + 1) * t].iter().sum::<f32>()).sum::<f32>();
        assert!(hi(&p) > 3.0 * hi(&h), "high bins: percussive {} vs harmonic {}", hi(&p), hi(&h));
    }

    #[test]
    fn analyse_with_one_stem_is_wav_to_mel() {
        let cfg = small_cfg();
        let mel = Mel::new(&cfg);
        let audio = tone_and_clicks(cfg.sr, 1.0, 120.0);
        let (m, t) = mel.wav_to_mel(&audio);
        let a = mel.analyse(&audio, 1);
        assert_eq!(a.t, t);
        assert_eq!(a.planes.len(), 1);
        assert_eq!(a.planes[0], m);
        assert_eq!(a.phase.len(), t);
    }

    #[test]
    fn beat_phase_finds_a_120_bpm_click_track() {
        let cfg = small_cfg();
        let mel = Mel::new(&cfg);
        let audio = tone_and_clicks(cfg.sr, 6.0, 120.0);
        let power = mel.stft_power(&audio);
        let t = power.len() / mel.n_freq;
        let phase = beat_phase(&power, mel.n_freq, t, cfg.hop, cfg.sr);
        assert_eq!(phase.len(), t);
        assert!(phase.iter().all(|p| (0.0..1.0).contains(p)));
        // Recover the period from where the phase wraps.
        let wraps: Vec<usize> = (1..t).filter(|&j| phase[j] < phase[j - 1]).collect();
        assert!(wraps.len() >= 8, "expected many beats in 6 s, got {}", wraps.len());
        let mean_period = (wraps[wraps.len() - 1] - wraps[0]) as f32 / (wraps.len() - 1) as f32;
        let expect = 0.5 * cfg.sr as f32 / cfg.hop as f32; // 120 BPM in frames
        assert!((mean_period - expect).abs() < 0.5, "period {mean_period} vs {expect}");
        // And the wraps land on the clicks: click k starts at sample k*period.
        let click_frame = |k: usize| (k as f32 * 0.5 * cfg.sr as f32 / cfg.hop as f32).round() as usize;
        for w in wraps.iter().take(4) {
            let nearest = (0..20).map(click_frame).map(|c| (c as isize - *w as isize).abs()).min().unwrap();
            assert!(nearest <= 2, "wrap at frame {w} is {nearest} frames from a click");
        }
    }

    #[test]
    fn beat_phase_of_a_short_clip_is_all_zero() {
        let cfg = small_cfg();
        let mel = Mel::new(&cfg);
        let audio = tone_and_clicks(cfg.sr, 0.2, 120.0);
        let power = mel.stft_power(&audio);
        let t = power.len() / mel.n_freq;
        assert!(beat_phase(&power, mel.n_freq, t, cfg.hop, cfg.sr).iter().all(|&p| p == 0.0));
    }

    #[test]
    fn continuation_seed_takes_the_previous_phase_on_the_overlap_and_advances_after() {
        let cfg = small_cfg();
        let mel = Mel::new(&cfg);
        let t = 8;
        let overlap = 3;
        let n = mel.n_freq * t;
        // A previous window with a distinctive phase per (bin, frame).
        let prev: Vec<Complex32> = (0..n)
            .map(|i| Complex32::from_polar(1.0, (i as f32 * 0.37) % (2.0 * PI) - PI))
            .collect();
        let mag = vec![1.0f32; n];
        let seed = mel.continuation_seed(&mag, t, overlap, &prev);
        for f in 0..mel.n_freq {
            for j in 0..overlap {
                let want = prev[f * t + (t - overlap + j)].arg();
                let got = seed[f * t + j].arg();
                assert!((want - got).abs() < 1e-5, "bin {f} frame {j}: {want} vs {got}");
            }
            let step = 2.0 * PI * f as f32 / cfg.n_fft as f32 * cfg.hop as f32;
            for j in overlap..t {
                let d = (seed[f * t + j] * seed[f * t + j - 1].conj()).arg();
                // Compared on the circle: a step of exactly pi reads as +pi
                // or -pi depending on rounding, and both are right.
                let err = ((d - step + PI).rem_euclid(2.0 * PI) - PI).abs();
                assert!(err < 1e-4, "bin {f} frame {j}: advance {d} vs {step}");
            }
        }
    }

    #[test]
    fn continuation_seed_with_no_history_starts_at_zero_phase_and_advances() {
        let cfg = small_cfg();
        let mel = Mel::new(&cfg);
        let (t, n) = (4, mel.n_freq * 4);
        let seed = mel.continuation_seed(&vec![0.5; n], t, 2, &[]);
        for f in 0..mel.n_freq {
            let c0 = seed[f * t];
            assert!((c0.re - 0.5).abs() < 1e-6 && c0.im.abs() < 1e-6, "bin {f} column 0 is not zero phase");
            let step = 2.0 * PI * f as f32 / cfg.n_fft as f32 * cfg.hop as f32;
            for j in 1..t {
                let d = (seed[f * t + j] * seed[f * t + j - 1].conj()).arg();
                let err = ((d - step + PI).rem_euclid(2.0 * PI) - PI).abs();
                assert!(err < 1e-4, "bin {f} frame {j}");
                assert!((seed[f * t + j].norm() - 0.5).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn continue_stream_advances_by_the_stride_and_never_clips() {
        let cfg = Config { time_frames: 32, ..small_cfg() };
        let mel = Mel::new(&cfg);
        let (t, overlap, stems) = (32usize, 4usize, 2usize);
        let audio = tone_and_clicks(cfg.sr, 1.0, 120.0);
        let a = mel.analyse(&audio, stems);
        // Two consecutive real windows, as the generator would emit them.
        let take = |start: usize| -> Vec<f32> {
            let mut out = Vec::with_capacity(stems * cfg.n_mels * t);
            for s in 0..stems {
                for m in 0..cfg.n_mels {
                    out.extend_from_slice(&a.planes[s][m * a.t + start..m * a.t + start + t]);
                }
            }
            out
        };
        let mut st = GlStream::new();
        let w0 = mel.continue_stream(&take(0), stems, t, overlap, 4, &mut st);
        let w1 = mel.continue_stream(&take(t - overlap), stems, t, overlap, 4, &mut st);
        assert_eq!(w0.advance, (t - overlap) * cfg.hop);
        assert_eq!(w0.audio.len(), (t - 1) * cfg.hop);
        assert_eq!(w1.audio.len(), w0.audio.len());
        assert!(st.gain <= 1.0 && st.gain > 0.0);
        for w in [&w0, &w1] {
            let peak = w.audio.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
            assert!(peak <= 0.9 + 1e-4, "peak {peak}");
            assert!(peak > 1e-3, "silent window");
        }
    }
}
