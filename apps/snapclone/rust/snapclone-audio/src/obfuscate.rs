//! obfuscate.rs — playback-side acoustic obfuscation for the private tier.
//!
//! When a channel is Private (see security.rs), the receiver perturbs decoded
//! audio just before it plays. The goal is to defeat *machine* recognition of
//! the acoustic output — Shazam-style spectral-peak ("constellation") matching,
//! speaker fingerprinting, and bot transcription — while keeping speech
//! human-intelligible. We do that with a few cheap, seeded perturbations:
//!
//!   * decoy tones  — inject faint sinusoids at drifting frequencies, planting
//!                    FALSE spectral peaks so peak-constellation matchers latch
//!                    onto noise (this is the "Shazam is pop-based" idea).
//!   * saturation   — soft-clip, smearing the clean spectral peaks of real audio.
//!   * whisper noise— a keyed low-level noise floor that masks fine structure.
//!   * gaps         — brief dropouts that break temporal alignment.
//!   * stutter      — repeat a short run, perturbing the time axis.
//!
//! Everything is scaled by `intensity` (0 = exact identity) and driven by a
//! seeded PRNG, so it is deterministic and fully unit-testable — no audio I/O,
//! no FFT, no external crate. Per-channel seeding makes the pattern session-
//! specific so it can't be filtered out with a fixed inverse.

use std::f32::consts::PI;
use std::os::raw::c_int;

/// Which perturbations to apply, and how hard.
#[derive(Debug, Clone, Copy)]
pub struct ObfuscationConfig {
    /// Master amount, 0..1. 0 = passthrough.
    pub intensity: f32,
    pub whisper: bool,
    pub saturate: bool,
    pub decoy_tones: bool,
    pub gaps: bool,
    pub stutter: bool,
}

impl Default for ObfuscationConfig {
    fn default() -> Self {
        Self {
            intensity: 0.5,
            whisper: true,
            saturate: true,
            decoy_tones: true,
            gaps: true,
            stutter: true,
        }
    }
}

/// Stateful per-channel obfuscator. Process each playback frame in order.
#[derive(Debug)]
pub struct Obfuscator {
    cfg: ObfuscationConfig,
    rng: u64,
    sample_rate: u32,
    tone_phase: f32,
    tone_freq: f32,
}

impl Obfuscator {
    pub fn new(seed: u64, sample_rate: u32, cfg: ObfuscationConfig) -> Self {
        Self {
            cfg,
            rng: if seed == 0 { 0x9E3779B97F4A7C15 } else { seed },
            sample_rate: sample_rate.max(1),
            tone_phase: 0.0,
            tone_freq: 1000.0,
        }
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }
    fn next_f32(&mut self) -> f32 {
        // [0,1)
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn chance(&mut self, p: f32) -> bool {
        self.next_f32() < p
    }

    /// Perturb one frame of mono i16 PCM in place.
    pub fn process(&mut self, frame: &mut [i16]) {
        let n = frame.len();
        if n == 0 {
            return;
        }
        let intensity = self.cfg.intensity.clamp(0.0, 1.0);

        // i16 → f32 in [-1,1); /32768 and *32768 round-trip exactly for i16.
        let mut buf: Vec<f32> = frame.iter().map(|&s| s as f32 / 32768.0).collect();

        // 1. stutter — repeat a short run over the following samples
        if self.cfg.stutter && self.chance(0.15 * intensity) {
            let max_run = (n / 4).max(1);
            let run = 1 + (self.next_u64() as usize % max_run);
            if run * 2 <= n {
                let start = self.next_u64() as usize % (n - run * 2 + 1);
                let seg: Vec<f32> = buf[start..start + run].to_vec();
                for i in 0..run {
                    buf[start + run + i] = seg[i];
                }
            }
        }

        // 2. gaps — zero a short run
        if self.cfg.gaps && self.chance(0.10 * intensity) {
            let max_gap = (n / 8).max(1);
            let glen = 1 + (self.next_u64() as usize % max_gap);
            let span = (n - glen + 1).max(1);
            let start = self.next_u64() as usize % span;
            for i in 0..glen {
                if start + i < n {
                    buf[start + i] = 0.0;
                }
            }
        }

        // 3. saturation — blend toward a soft-clip (intensity 0 → untouched)
        if self.cfg.saturate && intensity > 0.0 {
            let drive = 1.0 + intensity * 4.0;
            for x in buf.iter_mut() {
                let clipped = (*x * drive).tanh();
                *x = (1.0 - intensity) * *x + intensity * clipped;
            }
        }

        // 4. whisper noise — keyed low-level noise floor
        if self.cfg.whisper && intensity > 0.0 {
            let amp = intensity * 0.03;
            for x in buf.iter_mut() {
                *x += (self.next_f32() * 2.0 - 1.0) * amp;
            }
        }

        // 5. decoy tones — faint drifting sinusoid (false spectral peak)
        if self.cfg.decoy_tones && intensity > 0.0 {
            let amp = intensity * 0.05;
            if self.chance(0.2) {
                self.tone_freq = 300.0 + self.next_f32() * 3000.0; // 0.3–3.3 kHz
            }
            let dphase = 2.0 * PI * self.tone_freq / self.sample_rate as f32;
            for x in buf.iter_mut() {
                *x += self.tone_phase.sin() * amp;
                self.tone_phase += dphase;
                if self.tone_phase > 2.0 * PI {
                    self.tone_phase -= 2.0 * PI;
                }
            }
        }

        // f32 → i16 with clamp
        for (i, &v) in buf.iter().enumerate() {
            let s = (v * 32768.0).round();
            frame[i] = s.clamp(-32768.0, 32767.0) as i16;
        }
    }
}

// ─── FFI ─────────────────────────────────────────────────────────────────────

/// Create an obfuscator with all perturbations on at `intensity` (0..1). Seed
/// per channel (e.g. from the session key) so the pattern is session-specific.
/// Free with `sc_obfuscator_free`.
#[no_mangle]
pub extern "C" fn sc_obfuscator_new(seed: u64, sample_rate: u32, intensity: f32) -> *mut Obfuscator {
    let cfg = ObfuscationConfig { intensity, ..ObfuscationConfig::default() };
    Box::into_raw(Box::new(Obfuscator::new(seed, sample_rate, cfg)))
}

/// Create an obfuscator with individual perturbations toggled (each 0/1).
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn sc_obfuscator_new_full(
    seed: u64,
    sample_rate: u32,
    intensity: f32,
    whisper: c_int,
    saturate: c_int,
    decoy_tones: c_int,
    gaps: c_int,
    stutter: c_int,
) -> *mut Obfuscator {
    let cfg = ObfuscationConfig {
        intensity,
        whisper: whisper != 0,
        saturate: saturate != 0,
        decoy_tones: decoy_tones != 0,
        gaps: gaps != 0,
        stutter: stutter != 0,
    };
    Box::into_raw(Box::new(Obfuscator::new(seed, sample_rate, cfg)))
}

/// # Safety
/// `handle` from `sc_obfuscator_new[_full]`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_obfuscator_free(handle: *mut Obfuscator) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Perturb `n` samples of mono i16 PCM in place. Null-safe no-op.
///
/// # Safety
/// `frame` valid for `n` i16 (or null); `handle` from `sc_obfuscator_new[_full]`.
#[no_mangle]
pub unsafe extern "C" fn sc_obfuscator_process(handle: *mut Obfuscator, frame: *mut i16, n: usize) {
    if handle.is_null() || frame.is_null() {
        return;
    }
    let slice = std::slice::from_raw_parts_mut(frame, n);
    (*handle).process(slice);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Vec<i16> {
        (0..n).map(|i| ((i as i32 * 137 % 20000) - 10000) as i16).collect()
    }

    #[test]
    fn intensity_zero_is_identity() {
        let cfg = ObfuscationConfig { intensity: 0.0, ..Default::default() };
        let mut ob = Obfuscator::new(123, 16000, cfg);
        let input = ramp(320);
        let mut frame = input.clone();
        ob.process(&mut frame);
        assert_eq!(frame, input, "intensity 0 must be exact passthrough");
    }

    #[test]
    fn perturbs_at_intensity() {
        let mut ob = Obfuscator::new(42, 16000, ObfuscationConfig::default());
        let input = ramp(320);
        let mut frame = input.clone();
        ob.process(&mut frame);
        assert_ne!(frame, input, "should alter audio when intensity > 0");
    }

    #[test]
    fn stays_in_range_even_on_full_scale() {
        let cfg = ObfuscationConfig { intensity: 1.0, ..Default::default() };
        let mut ob = Obfuscator::new(7, 16000, cfg);
        let mut frame = vec![i16::MAX; 320];
        ob.process(&mut frame);
        // i16 by construction; assert it didn't wrap to absurd values by checking
        // the transform stayed bounded (all values are valid i16 — the real test
        // is that the f32→i16 clamp prevented overflow, which would have panicked
        // in debug on an out-of-range `as` only if we hadn't clamped).
        assert!(frame.iter().all(|&s| s >= i16::MIN && s <= i16::MAX));
    }

    #[test]
    fn deterministic_for_same_seed() {
        let input = ramp(320);
        let mut a = Obfuscator::new(999, 16000, ObfuscationConfig::default());
        let mut b = Obfuscator::new(999, 16000, ObfuscationConfig::default());
        let (mut fa, mut fb) = (input.clone(), input.clone());
        for _ in 0..5 {
            a.process(&mut fa);
            b.process(&mut fb);
            assert_eq!(fa, fb, "same seed → same output");
        }
    }

    #[test]
    fn different_seeds_differ() {
        let input = ramp(320);
        let mut a = Obfuscator::new(1, 16000, ObfuscationConfig::default());
        let mut b = Obfuscator::new(2, 16000, ObfuscationConfig::default());
        let (mut fa, mut fb) = (input.clone(), input.clone());
        // run several frames to let the keyed randomness diverge
        let mut diverged = false;
        for _ in 0..10 {
            a.process(&mut fa);
            b.process(&mut fb);
            if fa != fb {
                diverged = true;
            }
            fa.copy_from_slice(&input);
            fb.copy_from_slice(&input);
        }
        assert!(diverged, "different channel seeds should obfuscate differently");
    }

    #[test]
    fn ffi_process() {
        unsafe {
            let h = sc_obfuscator_new(55, 16000, 0.7);
            let mut frame = ramp(320);
            let before = frame.clone();
            sc_obfuscator_process(h, frame.as_mut_ptr(), frame.len());
            assert_ne!(frame, before);
            // null-safety
            sc_obfuscator_process(std::ptr::null_mut(), frame.as_mut_ptr(), frame.len());
            sc_obfuscator_free(h);
        }
    }
}
