//! Firefox-style scroll feel as pure physics (no egui), so the curve can be unit-tested.
//!
//! The model encodes the insight that within a multi-tap scroll gesture, the *first* impulses are
//! usually "distance want" (cover ground) and later ones are "precision want" (settle on a target).
//! So a fresh gesture starts with a boosted sensitivity that DECAYS EXPONENTIALLY back toward 1.0 as
//! the gesture continues — fast to fling, precise to nudge — while momentum + friction keep the
//! offset a continuous function of time rather than discrete jumps. The boost is bounded so the
//! applied offset still tracks the finger closely (R² ≥ 0.9 over a representative gesture); past that
//! it would stop feeling like "the content follows my finger".
//!
//! Usage (egui side): every frame, feed raw scroll delta via `input`, then `tick(dt)` and apply the
//! returned offset delta to the ScrollArea. All tunables are public so feel can be dialed in live.

#[derive(Clone, Debug)]
pub struct ScrollPhysics {
    pub vel: f32,        // content units / second
    pub offset: f32,     // accumulated content offset (monotone with input sign)
    boost_age: f32,      // seconds since the current gesture began (drives the decay)
    last_t: f32,         // timestamp (sec) of the last input event
    started: bool,
    // ── tunables ──
    pub boost_max: f32,    // peak EXTRA sensitivity at gesture start (0.8 → first impulse is 1.8×)
    pub boost_decay: f32,  // 1/sec exponential decay of the boost back toward 1.0
    pub impulse_gain: f32, // raw-delta → velocity gain
    pub friction: f32,     // per-second velocity retention in (0,1); lower = stops sooner
    pub gesture_gap: f32,  // idle gap (sec) that resets a gesture (so a new fling re-boosts)
    pub settle_eps: f32,   // |vel| below this snaps to 0 (no endless micro-drift)
}

impl Default for ScrollPhysics {
    fn default() -> Self {
        Self {
            vel: 0.0, offset: 0.0, boost_age: 0.0, last_t: 0.0, started: false,
            boost_max: 0.85, boost_decay: 6.0, impulse_gain: 22.0,
            friction: 0.0025, gesture_gap: 0.18, settle_eps: 1.5,
        }
    }
}

impl ScrollPhysics {
    /// Sensitivity multiplier right now: 1 + boost·e^(−decay·age). Fresh gesture ≈ 1+boost_max;
    /// after sustained scrolling → 1.0 (precision).
    pub fn sensitivity(&self) -> f32 { 1.0 + self.boost_max * (-self.boost_decay * self.boost_age).exp() }

    /// A scroll impulse (wheel tick / trackpad delta) arrived at time `now` (seconds).
    pub fn input(&mut self, raw_delta: f32, now: f32) {
        if !self.started { self.started = true; self.boost_age = 0.0; }
        else {
            let gap = (now - self.last_t).max(0.0);
            if gap > self.gesture_gap { self.boost_age = 0.0; } // idle → new gesture, re-boost
            else { self.boost_age += gap; }                      // same gesture → boost decays
        }
        self.last_t = now;
        self.vel += raw_delta * self.sensitivity() * self.impulse_gain;
    }

    /// Advance `dt` seconds: integrate velocity into offset, apply friction, settle. Returns the
    /// offset DELTA applied this tick (what to scroll the view by).
    pub fn tick(&mut self, dt: f32) -> f32 {
        if dt <= 0.0 { return 0.0; }
        let before = self.offset;
        self.offset += self.vel * dt;
        self.vel *= self.friction.powf(dt);
        if self.vel.abs() < self.settle_eps { self.vel = 0.0; }
        self.offset - before
    }

    /// Has motion fully settled (no pending glide)?
    pub fn at_rest(&self) -> bool { self.vel == 0.0 }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    // Pearson R² between two equal-length series.
    fn r2(xs: &[f32], ys: &[f32]) -> f32 {
        let n = xs.len() as f32;
        let (mx, my) = (xs.iter().sum::<f32>() / n, ys.iter().sum::<f32>() / n);
        let mut sxy = 0.0; let mut sxx = 0.0; let mut syy = 0.0;
        for (&x, &y) in xs.iter().zip(ys) { sxy += (x - mx) * (y - my); sxx += (x - mx).powi(2); syy += (y - my).powi(2); }
        if sxx == 0.0 || syy == 0.0 { return 1.0; }
        let r = sxy / (sxx.sqrt() * syy.sqrt());
        r * r
    }

    #[test]
    fn boost_decays_to_one() {
        let mut s = ScrollPhysics::default();
        let s0 = s.sensitivity(); // fresh
        s.input(1.0, 0.0);
        // simulate sustained same-gesture taps (small gaps < gesture_gap)
        let mut t = 0.0;
        for _ in 0..40 { t += 0.016; s.input(1.0, t); }
        let s_late = s.sensitivity();
        assert!(s0 > 1.8 - 0.01, "fresh sensitivity ~1+boost_max, got {s0}");
        assert!(s_late < 1.05, "late-gesture sensitivity returns toward 1.0, got {s_late}");
        assert!(s_late < s0, "sensitivity decays within a gesture");
    }

    #[test]
    fn friction_settles_to_rest() {
        let mut s = ScrollPhysics::default();
        s.input(10.0, 0.0); // one fling
        let mut t = 0.0;
        for _ in 0..600 { t += 0.016; s.tick(0.016); }
        assert!(s.at_rest(), "velocity must reach exactly 0 (no endless drift), vel={}", s.vel);
    }

    #[test]
    fn idle_gap_reboosts() {
        let mut s = ScrollPhysics::default();
        s.input(1.0, 0.0);
        let mut t = 0.0; for _ in 0..10 { t += 0.016; s.input(1.0, t); }
        let decayed = s.sensitivity();
        // idle longer than gesture_gap, then a new impulse → boost should be fresh again
        s.input(1.0, t + 1.0);
        assert!(s.sensitivity() > decayed + 0.3, "an idle gap re-boosts the next gesture");
    }

    #[test]
    fn monotone_no_reversal() {
        // positive impulses must only ever move offset in the positive direction (no jank/bounce).
        let mut s = ScrollPhysics::default();
        let mut t = 0.0; let mut last = s.offset;
        for k in 0..30 {
            if k % 3 == 0 { s.input(2.0, t); }
            t += 0.016; s.tick(0.016);
            assert!(s.offset >= last - 1e-3, "offset went backwards on positive input");
            last = s.offset;
        }
    }

    #[test]
    fn flick_covers_more_than_dribble() {
        // distance-want: the SAME total delta delivered as one fast flick should cover more ground
        // than dribbled out slowly across separate gestures (acceleration rewards speed).
        let total = 12.0;
        let mut flick = ScrollPhysics::default();
        flick.input(total, 0.0);                       // all at once (one gesture)
        let mut t = 0.0; for _ in 0..400 { t += 0.016; flick.tick(0.016); }

        let mut dribble = ScrollPhysics::default();
        let mut t2 = 0.0;
        for _ in 0..12 {                               // 1 unit per gesture, idle between → no boost build, re-boost each
            dribble.input(1.0, t2);
            for _ in 0..30 { t2 += 0.016; dribble.tick(0.016); } // let it settle (idle) between taps
            t2 += 0.3;
        }
        assert!(flick.offset > dribble.offset, "flick {} should exceed dribble {}", flick.offset, dribble.offset);
    }

    #[test]
    fn content_tracks_finger_r2_above_0_9() {
        // The applied offset must stay a faithful (if accelerated) map of cumulative finger travel:
        // R² between cumulative raw input and the settled offset across a representative multi-tap
        // gesture must be ≥ 0.9, or the content has decorrelated from the finger.
        let mut s = ScrollPhysics::default();
        let mut raw_cum = 0.0;
        let mut xs = Vec::new(); let mut ys = Vec::new();
        let mut t = 0.0;
        for k in 0..50 {
            // a natural gesture: a few big impulses early, smaller ones later (distance → precision)
            let delta = if k < 8 { 6.0 } else if k < 20 { 3.0 } else { 1.0 };
            s.input(delta, t);
            raw_cum += delta;
            // let the view catch up between impulses (a couple of frames)
            for _ in 0..4 { t += 0.016; s.tick(0.016); }
            xs.push(raw_cum); ys.push(s.offset);
        }
        // drain remaining momentum
        for _ in 0..120 { t += 0.016; s.tick(0.016); }
        let r2v = r2(&xs, &ys);
        assert!(r2v >= 0.9, "content↔finger R² = {r2v:.4} (< 0.9): boost too aggressive");
    }
}
