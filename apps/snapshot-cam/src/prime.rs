//! prime — a trigger arms the shutter; the sharpest frame in the next
//! second is the photo.
//!
//! Pressing a volume key or shaking the phone moves it: the frame at the
//! instant of the trigger is the blurriest one there is. So a trigger does
//! not take a photo, it PRIMES one — for `WINDOW_MS` every new frame is
//! scored for sharpness (`stack::sharpness`, variance of the Laplacian over
//! the middle of the frame) and the best is kept. When the window closes,
//! that frame is the photo. With the phone held still after the press, the
//! best frame is the steadiest; with the phone on a table, it is simply a
//! frame from within the second.

use capture_kit::RawFrame;

/// How long a primed shot looks for its frame.
pub const WINDOW_MS: f64 = 1000.0;

#[derive(Debug, Clone)]
pub struct Prime {
    started_ms: f64,
    last_seq: Option<u64>,
    best: Option<(f32, RawFrame)>,
    pub scored: u32,
}

impl Prime {
    pub fn new(now_ms: f64) -> Prime { Prime { started_ms: now_ms, last_seq: None, best: None, scored: 0 } }

    /// Score a frame (once per frame), keep it if it is the sharpest yet.
    pub fn feed(&mut self, f: &RawFrame) {
        if self.last_seq == Some(f.seq) || !f.pixels_plausible() { return; }
        self.last_seq = Some(f.seq);
        let s = crate::stack::sharpness(&f.rgba, f.width, f.height);
        self.scored += 1;
        if self.best.as_ref().is_none_or(|(b, _)| s > *b) { self.best = Some((s, f.clone())); }
    }

    /// Seconds left, for the countdown ring.
    pub fn remaining(&self, now_ms: f64) -> f32 { ((WINDOW_MS - (now_ms - self.started_ms)) / 1000.0).max(0.0) as f32 }
    pub fn done(&self, now_ms: f64) -> bool { now_ms - self.started_ms >= WINDOW_MS }
    /// The chosen frame.
    pub fn take(self) -> Option<RawFrame> { self.best.map(|(_, f)| f) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(seq: u64, sharp: bool) -> RawFrame {
        let (w, h) = (64u32, 48u32);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| {
            let v = if sharp { if (i % w / 2 + i / w / 2) % 2 == 0 { 220 } else { 30 } } else { 120 + (i % w / 16) as u8 };
            [v, v, v, 255]
        }).collect();
        RawFrame { width: w, height: h, captured_ms: seq, seq, rgba: rgba.into() }
    }

    #[test]
    fn the_sharpest_frame_in_the_window_is_the_photo() {
        let mut p = Prime::new(0.0);
        p.feed(&frame(1, false)); // the jolt of the press
        p.feed(&frame(2, true));  // held still
        p.feed(&frame(2, true));  // the same frame again: not rescored
        p.feed(&frame(3, false));
        assert_eq!(p.scored, 3);
        assert!(!p.done(999.0) && p.done(1000.0));
        assert!((p.remaining(250.0) - 0.75).abs() < 1e-3);
        assert_eq!(p.take().unwrap().seq, 2);
        assert!(Prime::new(0.0).take().is_none(), "no frames, no photo");
    }
}
