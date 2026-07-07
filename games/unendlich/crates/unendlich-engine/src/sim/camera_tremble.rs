//! `sim::camera_tremble` — a pure camera-shake core. Game events (a dragon
//! footfall, a roar, an impact, a wing-beat liftoff, a heavy landing) inject an
//! impulse; each frame the shake decays and emits a small positional + angular
//! offset the renderer adds to the camera before building the view matrix.
//! Deterministic (no RNG, no wall-clock — driven entirely by accumulated `dt`),
//! so the same event sequence always produces the same shake. Panic-free.

/// What shook the camera. Each source has its own punchiness and frequency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tremor {
    Footfall, // a foot landing — small, snappy, frequent
    Roar,     // a bellow — medium, lower frequency, lingers
    Impact,   // a hit / breath blast connecting — the biggest jolt
    Liftoff,  // wings beating the ground away — medium, low frequency
    Landing,  // the dragon touching down — heavy thud
}

/// Peak shake amplitude (world units for position) a unit-strength impulse adds.
fn base_amp(k: Tremor) -> f32 {
    match k {
        Tremor::Footfall => 0.018,
        Tremor::Roar => 0.045,
        Tremor::Impact => 0.13,
        Tremor::Liftoff => 0.07,
        Tremor::Landing => 0.11,
    }
}

/// Dominant shake frequency (rad/s) for each source.
fn base_freq(k: Tremor) -> f32 {
    match k {
        Tremor::Footfall => 30.0,
        Tremor::Roar => 13.0,
        Tremor::Impact => 24.0,
        Tremor::Liftoff => 11.0,
        Tremor::Landing => 18.0,
    }
}

/// The shake to add to the camera this frame. Position is in world units; the
/// angles are radians and small. All zero when the camera is at rest.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrembleOffset {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

impl TrembleOffset {
    /// Largest absolute positional component — handy for tests / gating.
    pub fn pos_magnitude(&self) -> f32 {
        self.pos[0].abs().max(self.pos[1].abs()).max(self.pos[2].abs())
    }
}

/// How fast shake energy bleeds off (per second). ~`1/CAM_DECAY` time constant.
const CAM_DECAY: f32 = 7.0;
/// Energy never stacks past this (so a flurry of footfalls doesn't nauseate).
const ENERGY_CAP: f32 = 0.22;

/// Camera-shake state carried on the camera between frames.
#[derive(Clone, Copy, Debug, Default)]
pub struct TrembleState {
    energy: f32, // current shake amplitude
    freq: f32,   // dominant frequency of the most recent impulse
    t: f32,      // accumulated time (drives the oscillation)
    phase: f32,  // shifts each impulse so repeats don't look identical
}

impl TrembleState {
    pub fn new() -> Self {
        TrembleState { energy: 0.0, freq: base_freq(Tremor::Footfall), t: 0.0, phase: 0.0 }
    }

    /// Inject a shake. `strength` (≥0) scales the source's base amplitude — e.g.
    /// a bigger dragon's footfall or a harder impact passes strength > 1.
    pub fn impulse(&mut self, kind: Tremor, strength: f32) {
        let a = base_amp(kind) * strength.max(0.0);
        // a new jolt sets the floor and adds a little on top, capped
        self.energy = (self.energy.max(a) + a * 0.35).min(ENERGY_CAP);
        self.freq = base_freq(kind);
        self.phase += 1.7; // decorrelate successive shakes
    }

    /// Advance the shake by `dt` and return this frame's camera offset.
    pub fn step(&mut self, dt: f32) -> TrembleOffset {
        let dt = dt.max(0.0);
        self.t += dt;
        // exponential-ish decay, stable for any dt
        let keep = (1.0 - CAM_DECAY * dt).clamp(0.0, 1.0);
        self.energy *= keep;
        if self.energy < 1e-4 {
            self.energy = 0.0;
            return TrembleOffset::default();
        }
        let e = self.energy;
        let w = self.freq;
        let t = self.t;
        let p = self.phase;
        TrembleOffset {
            pos: [
                e * (w * t + p).sin(),
                e * 0.7 * (w * 1.3 * t + p * 1.7).sin(),
                e * 0.5 * (w * 0.9 * t + p).sin(),
            ],
            yaw: e * 0.30 * (w * 0.85 * t + p).sin(),
            pitch: e * 0.40 * (w * 1.27 * t + 1.0).sin(),
            roll: e * 0.55 * (w * 1.10 * t).sin(),
        }
    }

    /// True while the camera is still visibly shaking.
    pub fn is_active(&self) -> bool {
        self.energy > 1e-4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_camera_has_no_offset() {
        let mut s = TrembleState::new();
        let o = s.step(0.016);
        assert_eq!(o, TrembleOffset::default());
        assert!(!s.is_active());
    }

    #[test]
    fn an_impulse_shakes_then_settles() {
        let mut s = TrembleState::new();
        s.impulse(Tremor::Impact, 1.0);
        let mut peak = 0.0f32;
        for _ in 0..3 {
            peak = peak.max(s.step(0.016).pos_magnitude());
        }
        assert!(peak > 0.02, "an impact visibly shakes the camera");
        // let it ring out
        for _ in 0..120 {
            s.step(0.016);
        }
        assert!(!s.is_active(), "the shake decays to rest");
        assert_eq!(s.step(0.016), TrembleOffset::default());
    }

    #[test]
    fn impact_hits_harder_than_a_footfall() {
        let mag = |k: Tremor| {
            let mut s = TrembleState::new();
            s.impulse(k, 1.0);
            let mut peak = 0.0f32;
            for _ in 0..4 {
                peak = peak.max(s.step(0.016).pos_magnitude());
            }
            peak
        };
        assert!(mag(Tremor::Impact) > mag(Tremor::Footfall), "an impact out-shakes a footfall");
        assert!(mag(Tremor::Landing) > mag(Tremor::Footfall), "a heavy landing out-shakes a footfall");
    }

    #[test]
    fn energy_is_capped_against_a_flurry() {
        let mut s = TrembleState::new();
        for _ in 0..50 {
            s.impulse(Tremor::Footfall, 3.0);
        }
        // even a spammed flurry can't drive the offset past the cap envelope
        let mut peak = 0.0f32;
        for _ in 0..4 {
            peak = peak.max(s.step(0.001).pos_magnitude());
        }
        assert!(peak <= ENERGY_CAP + 1e-3, "shake is bounded ({peak})");
    }

    #[test]
    fn deterministic() {
        let run = || {
            let mut s = TrembleState::new();
            s.impulse(Tremor::Roar, 1.2);
            let mut acc = 0.0f32;
            for _ in 0..30 {
                let o = s.step(0.02);
                acc += o.pos_magnitude() + o.roll.abs() + o.yaw.abs() + o.pitch.abs();
            }
            acc
        };
        assert_eq!(run(), run(), "same events ⇒ same shake");
    }

    #[test]
    fn dt_zero_is_safe() {
        let mut s = TrembleState::new();
        s.impulse(Tremor::Liftoff, 1.0);
        let o = s.step(0.0);
        assert!(o.pos_magnitude().is_finite());
    }
}
