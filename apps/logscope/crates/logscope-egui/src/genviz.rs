//! Token-generation visualization state (pure, no egui): a stable, fluid view of what each worker is
//! doing, for the multi-worker fabric. Raw tok/s is jumpy and streams come and go; this smooths the
//! rate (EMA), pins each stream to a stable display slot so bars don't reshuffle, and exposes an
//! exponential-approach helper so the rendered bar glides to its target instead of snapping. egui
//! draws from this; the math is here so it can be tested.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GenState { Prefilling, Decoding, Stopped }

#[derive(Clone, Debug)]
pub struct Stream {
    pub id: u64,
    pub label: String,   // "model · author@device"
    pub tokens: usize,
    pub state: GenState,
    pub tok_s: f32,      // smoothed tokens/sec (EMA)
    pub slot: usize,     // stable display row; assigned once, never reshuffled
    last_t: f32,
    last_tokens: usize,
}

pub struct GenViz {
    streams: Vec<Stream>,
    next_slot: usize,
    pub alpha: f32, // EMA weight on the newest sample (0..1); lower = smoother/slower
}

impl Default for GenViz {
    fn default() -> Self { Self { streams: Vec::new(), next_slot: 0, alpha: 0.3 } }
}

impl GenViz {
    pub fn new() -> Self { Self::default() }
    pub fn active(&self) -> usize { self.streams.iter().filter(|s| s.state != GenState::Stopped).count() }
    fn find(&mut self, id: u64) -> Option<&mut Stream> { self.streams.iter_mut().find(|s| s.id == id) }

    /// A worker began generating. Creates the stream (or resets an existing id) in Prefilling, keeping
    /// its slot if it already had one so the row doesn't jump.
    pub fn begin(&mut self, id: u64, label: &str, now: f32) {
        if let Some(s) = self.streams.iter_mut().find(|s| s.id == id) {
            s.state = GenState::Prefilling; s.tokens = 0; s.tok_s = 0.0; s.last_t = now; s.last_tokens = 0;
            return;
        }
        let slot = self.next_slot; self.next_slot += 1;
        self.streams.push(Stream {
            id, label: label.to_string(), tokens: 0, state: GenState::Prefilling,
            tok_s: 0.0, slot, last_t: now, last_tokens: 0,
        });
    }

    /// `total` tokens have now been emitted by `id` at time `now`. Updates the count and folds the
    /// instantaneous rate into the EMA; first token flips Prefilling → Decoding.
    pub fn on_tokens(&mut self, id: u64, total: usize, now: f32) {
        let a = self.alpha;
        if let Some(s) = self.find(id) {
            let dt = (now - s.last_t).max(1e-3);
            let dtok = total.saturating_sub(s.last_tokens) as f32;
            let inst = dtok / dt;
            s.tok_s = if s.tokens == 0 { inst } else { a * inst + (1.0 - a) * s.tok_s };
            s.tokens = total;
            s.last_tokens = total;
            s.last_t = now;
            if s.state == GenState::Prefilling { s.state = GenState::Decoding; }
        }
    }

    pub fn on_stop(&mut self, id: u64) { if let Some(s) = self.find(id) { s.state = GenState::Stopped; } }

    /// Streams in stable slot order (for rows that never reshuffle).
    pub fn ordered(&self) -> Vec<&Stream> {
        let mut v: Vec<&Stream> = self.streams.iter().collect();
        v.sort_by_key(|s| s.slot);
        v
    }

    pub fn clear_stopped(&mut self) { self.streams.retain(|s| s.state != GenState::Stopped); }
}

/// Exponential approach: move `current` toward `target` by a time-correct fraction so a rendered
/// value (bar height, dot position) glides smoothly and frame-rate-independently. `rate` ≈ 1/sec of
/// approach speed. Pure; used by the egui layer each frame.
pub fn approach(current: f32, target: f32, dt: f32, rate: f32) -> f32 {
    if dt <= 0.0 { return current; }
    let k = 1.0 - (-rate * dt).exp(); // fraction of the gap to close this frame
    current + (target - current) * k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_transition() {
        let mut v = GenViz::new();
        v.begin(1, "m·a@d", 0.0);
        assert_eq!(v.ordered()[0].state, GenState::Prefilling);
        v.on_tokens(1, 1, 0.1);
        assert_eq!(v.ordered()[0].state, GenState::Decoding);
        v.on_stop(1);
        assert_eq!(v.ordered()[0].state, GenState::Stopped);
        assert_eq!(v.active(), 0);
    }

    #[test]
    fn slots_are_stable() {
        let mut v = GenViz::new();
        v.begin(10, "a", 0.0);
        v.begin(20, "b", 0.0);
        let slot_a = v.ordered().iter().find(|s| s.id == 10).unwrap().slot;
        // updating b, stopping/restarting a — a must keep its slot
        v.on_tokens(20, 5, 0.1);
        v.begin(10, "a", 0.2); // re-begin same id
        assert_eq!(v.ordered().iter().find(|s| s.id == 10).unwrap().slot, slot_a, "slot must persist across re-begin");
        assert!(v.ordered()[0].slot < v.ordered()[1].slot, "ordered() is slot-sorted");
    }

    #[test]
    fn rate_is_smoothed_and_positive() {
        let mut v = GenViz::new();
        v.begin(1, "m", 0.0);
        // ~10 tok/s steady
        let mut t = 0.0; let mut n = 0;
        for _ in 0..20 { t += 0.1; n += 1; v.on_tokens(1, n, t); }
        let r = v.ordered()[0].tok_s;
        assert!(r > 5.0 && r < 15.0, "smoothed rate near 10 tok/s, got {r}");
        // a smoothed series should not jump to a single fast sample
        v.on_tokens(1, n + 50, t + 0.01); // a burst
        let r2 = v.ordered()[0].tok_s;
        assert!(r2 < 5000.0 * 0.5, "EMA damps a single burst, got {r2}");
    }

    #[test]
    fn approach_converges_monotone() {
        let mut x = 0.0;
        let mut prev = -1.0;
        for _ in 0..200 { x = approach(x, 100.0, 0.016, 8.0); assert!(x >= prev, "monotone toward target"); prev = x; }
        assert!((x - 100.0).abs() < 1.0, "approaches the target, got {x}");
        // frame-rate independence: one big step ≈ many small steps over the same total time
        let big = approach(0.0, 100.0, 0.5, 8.0);
        let mut small = 0.0; for _ in 0..50 { small = approach(small, 100.0, 0.01, 8.0); }
        assert!((big - small).abs() < 2.0, "dt-correct: {big} vs {small}");
    }
}
