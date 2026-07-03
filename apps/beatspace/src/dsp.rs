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
