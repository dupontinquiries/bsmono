//! Time-stretch: play a source at a different speed without changing its
//! pitch.
//!
//! WSOLA — waveform-similarity overlap-add. Output is built from
//! Hann-windowed frames of the input overlap-added at a fixed synthesis
//! hop; each frame is taken from near its nominal input position
//! (`output_pos * rate`), the exact spot chosen by cross-correlating a
//! small neighbourhood against where the previous frame would naturally
//! continue, so consecutive frames join in phase. Pitch is untouched
//! because every frame is a verbatim slice of the input; time changes
//! because the slices are taken from further apart (or closer) than they
//! are played back.
//!
//! Chosen over a phase vocoder because it keeps transients — drum hits are
//! copied whole, not smeared across bins — and over plain resampling
//! because a beat locked to a slower tempo should not drop a fifth.
//! Quality degrades past roughly 0.5x–2x, as it does in every WSOLA; a
//! clip that far from its tempo is a musical decision the user will hear.
//!
//! STREAMING AND RANDOM ACCESS: the renderer asks for arbitrary output
//! ranges — the next block, a loop wrap, a seek. A `Stretcher` holds the
//! overlap tail and the last input position so consecutive requests join
//! seamlessly; a request that is not the continuation of the last one
//! restarts from a frame boundary, which costs a few milliseconds of
//! re-synthesis and nothing audible.

/// Synthesis hop and frame. 512-sample hop at 22050 Hz is ~23 ms; the
/// frame is two hops so Hann windows at 50% overlap sum to one.
const HOP: usize = 512;
const FRAME: usize = 2 * HOP;
/// How far around the nominal input position the best-matching frame is
/// looked for. ~12 ms: enough to line up a period of anything above ~85 Hz.
const SEARCH: usize = 256;

pub struct Stretcher {
    rate: f64,
    /// Output sample index the next synthesised frame starts at.
    next_out: usize,
    /// Input index the previous frame was taken from, for the continuation
    /// the next frame is matched against.
    prev_in: Option<usize>,
    /// The overlap-added tail beyond `next_out - HOP`: the second half of
    /// the last frame, which the next frame's first half is added to.
    tail: Vec<f32>,
    /// Finished output not yet handed out, from `out_base`.
    ready: Vec<f32>,
    out_base: usize,
    window: Vec<f32>,
}

impl Stretcher {
    pub fn new(rate: f64) -> Self {
        let window: Vec<f32> = (0..FRAME)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FRAME as f32).cos())
            .collect();
        Self { rate: rate.max(0.05), next_out: 0, prev_in: None, tail: vec![0.0; HOP], ready: Vec::new(), out_base: 0, window }
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Restart at output position `out_pos` (rounded down to a hop).
    fn restart(&mut self, out_pos: usize) {
        let start = out_pos / HOP * HOP;
        self.next_out = start;
        self.prev_in = None;
        self.tail = vec![0.0; HOP];
        self.ready.clear();
        self.out_base = start;
    }

    /// Length of the stretched output for `src_len` input samples.
    pub fn output_len(rate: f64, src_len: usize) -> usize {
        (src_len as f64 / rate.max(0.05)).round() as usize
    }

    /// Fill `out` with output samples `[out_pos, out_pos + out.len())` of
    /// `src` stretched by `rate`. Past the end of the material the output
    /// is silence. Returns how many samples were real (non-silence-padded).
    pub fn render(&mut self, src: &[f32], out_pos: usize, out: &mut [f32]) -> usize {
        let n = out.len();
        if n == 0 || src.is_empty() {
            for o in out.iter_mut() {
                *o = 0.0;
            }
            return 0;
        }
        // A request that does not continue the last one restarts.
        if out_pos < self.out_base || out_pos > self.out_base + self.ready.len() + FRAME * 4 {
            self.restart(out_pos);
        }
        // Drop what is before the request.
        if out_pos > self.out_base {
            let skip = (out_pos - self.out_base).min(self.ready.len());
            self.ready.drain(..skip);
            self.out_base += skip;
            if out_pos > self.out_base {
                // Still short: synthesise up to it.
                while self.out_base + self.ready.len() < out_pos && self.synth_frame(src) {}
                let skip = (out_pos - self.out_base).min(self.ready.len());
                self.ready.drain(..skip);
                self.out_base += skip;
            }
        }
        // Synthesise until the request is covered or the material ends.
        while self.ready.len() < n && self.synth_frame(src) {}
        let have = self.ready.len().min(n);
        out[..have].copy_from_slice(&self.ready[..have]);
        for o in &mut out[have..] {
            *o = 0.0;
        }
        self.ready.drain(..have);
        self.out_base += have;
        have
    }

    /// Add one frame at `next_out`. Returns false when the material is
    /// exhausted.
    fn synth_frame(&mut self, src: &[f32]) -> bool {
        let nominal = (self.next_out as f64 * self.rate) as isize;
        if nominal as usize + HOP > src.len() + HOP {
            return false;
        }
        // Where the previous frame would continue naturally, and the
        // candidate offsets around the nominal position.
        let take_from = match self.prev_in {
            None => nominal.max(0) as usize,
            Some(prev) => {
                let natural = prev + HOP;
                let lo = (nominal - SEARCH as isize).max(0) as usize;
                let hi = (nominal + SEARCH as isize).max(0) as usize;
                let (mut best, mut best_score) = (nominal.max(0) as usize, f32::MIN);
                // Compare the candidate's first half against the natural
                // continuation's first half, at a coarse stride for speed.
                let cmp = HOP.min(src.len().saturating_sub(natural));
                if cmp < 32 {
                    nominal.max(0) as usize
                } else {
                    let mut cand = lo;
                    while cand <= hi {
                        if cand + cmp <= src.len() {
                            let mut acc = 0.0f32;
                            let mut i = 0;
                            while i < cmp {
                                acc += src[natural + i] * src[cand + i];
                                i += 4;
                            }
                            if acc > best_score {
                                best_score = acc;
                                best = cand;
                            }
                        }
                        cand += 4;
                    }
                    best
                }
            }
        };
        if take_from >= src.len() {
            return false;
        }
        // Overlap-add: first half onto the tail, second half becomes the
        // new tail. The frame is zero-padded past the material's end.
        let mut frame = vec![0.0f32; FRAME];
        for (i, f) in frame.iter_mut().enumerate() {
            *f = src.get(take_from + i).copied().unwrap_or(0.0) * self.window[i];
        }
        let mut first: Vec<f32> = self.tail.clone();
        for i in 0..HOP {
            first[i] += frame[i];
        }
        self.tail.copy_from_slice(&frame[HOP..]);
        self.ready.extend_from_slice(&first);
        self.prev_in = Some(take_from);
        self.next_out += HOP;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, secs: f32, sr: u32) -> Vec<f32> {
        (0..(secs * sr as f32) as usize).map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / sr as f32).sin()).collect()
    }

    fn zero_crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|p| (p[0] < 0.0) != (p[1] < 0.0)).count()
    }

    #[test]
    fn rate_one_reproduces_the_input_after_the_first_hop() {
        let src = sine(440.0, 1.0, 22050);
        let mut st = Stretcher::new(1.0);
        let mut out = vec![0.0f32; src.len()];
        st.render(&src, 0, &mut out);
        // Hann OLA at 50% is unity, so after the ramp-in the output IS the
        // input (the search may pick a nearby period, which for a pure
        // sine is the same waveform).
        let err: f32 = (HOP..src.len() - FRAME).map(|i| (out[i] - src[i]).abs()).fold(0.0, f32::max);
        assert!(err < 0.05, "max error {err}");
    }

    #[test]
    fn stretching_changes_the_length_and_keeps_the_pitch() {
        let sr = 22050;
        let src = sine(220.0, 1.0, sr);
        for rate in [0.5f64, 0.75, 1.5, 2.0] {
            let n = Stretcher::output_len(rate, src.len());
            assert_eq!(n, (src.len() as f64 / rate).round() as usize);
            let mut st = Stretcher::new(rate);
            let mut out = vec![0.0f32; n];
            let real = st.render(&src, 0, &mut out);
            assert!(real >= n - FRAME * 2, "rate {rate}: only {real} of {n} samples were material");
            // Pitch: zero crossings per second of output equal the input's.
            let body = &out[FRAME..real.saturating_sub(FRAME)];
            let zc_per_s = zero_crossings(body) as f32 / (body.len() as f32 / sr as f32);
            assert!((zc_per_s - 440.0).abs() < 12.0, "rate {rate}: {zc_per_s} crossings/s, want 440");
            // Level: no overlap-add gain error.
            let peak = body.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
            assert!(peak > 0.9 && peak < 1.1, "rate {rate}: peak {peak}");
        }
    }

    #[test]
    fn a_random_access_request_equals_the_streamed_one() {
        let src = sine(330.0, 0.6, 22050);
        let rate = 1.3;
        let n = Stretcher::output_len(rate, src.len());
        let mut whole = vec![0.0f32; n];
        Stretcher::new(rate).render(&src, 0, &mut whole);
        // Streamed in blocks of an odd size.
        let mut st = Stretcher::new(rate);
        let mut streamed = vec![0.0f32; n];
        let mut pos = 0;
        while pos < n {
            let len = 700.min(n - pos);
            st.render(&src, pos, &mut streamed[pos..pos + len]);
            pos += len;
        }
        let err: f32 = whole.iter().zip(&streamed).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-5, "streamed differs from whole: {err}");
        // A seek into the middle: after the restart's ramp the signal is
        // the same sine at the same level.
        let mut seek = vec![0.0f32; 4096];
        let at = n / 2;
        Stretcher::new(rate).render(&src, at, &mut seek);
        let body = &seek[FRAME..];
        let peak = body.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(peak > 0.9, "seek landed in silence: peak {peak}");
    }

    #[test]
    fn past_the_material_the_output_is_silence_and_counted_as_such() {
        let src = sine(220.0, 0.1, 22050);
        let mut st = Stretcher::new(1.0);
        let mut out = vec![1.0f32; 8192];
        let real = st.render(&src, 0, &mut out);
        assert!(real < 8192 && real >= src.len() - FRAME);
        assert!(out[real..].iter().all(|&x| x == 0.0));
        assert_eq!(st.render(&[], 0, &mut out), 0);
    }
}
