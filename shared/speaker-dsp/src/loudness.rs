//! loudness — the ear's bass deficit grows as the volume falls, so the chain gives back some.
//!
//! ISO 226 equal-loudness contours say the same thing from the listener's side: the quieter a
//! sound, the more bass level it takes to sound as loud as the midrange. A fixed tuning therefore
//! sounds right at one volume and thin at every other. This maps the sink volume to two
//! corrections:
//!
//! * `vbass_scale` — more virtual-bass harmonics when quiet (the lever that actually works on a
//!   phone: a conventional low-shelf boost at 100 Hz would add energy the speaker cannot
//!   radiate, and spend battery doing it), and
//! * `warmth_db` — a small lift in the low mids (see [`crate::chain`]), which the speaker CAN
//!   radiate.
//!
//! The magnitudes are deliberately modest. This is the SHAPE of the ISO 226 effect, not a
//! reproduction of its tables, and a restrained loudness curve is what keeps a tune smooth rather
//! than boomy.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessComp {
    /// Volume (0..1) at and above which no compensation is applied.
    pub reference_level: f32,
    /// At silence the virtual-bass amount is multiplied by `1 + max_vbass_boost`.
    pub max_vbass_boost: f32,
    /// At silence the low-mid lift reaches this many dB.
    pub max_warmth_db: f32,
}

impl Default for LoudnessComp {
    fn default() -> Self {
        LoudnessComp { reference_level: 0.75, max_vbass_boost: 0.8, max_warmth_db: 2.5 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Correction {
    pub vbass_scale: f32,
    pub warmth_db: f32,
}

impl LoudnessComp {
    /// The correction at listening `level` (0 = silent, 1 = full). Smooth and monotone: it never
    /// steps, because a step in the tuning is audible as the volume key is pressed.
    pub fn at(&self, level: f32) -> Correction {
        let level = if level.is_finite() { level.clamp(0.0, 1.0) } else { self.reference_level };
        let r = self.reference_level.clamp(1e-3, 1.0);
        let deficit = ((r - level) / r).clamp(0.0, 1.0);
        // Quadratic: gentle near the reference, firmer toward silence, where the ear's deficit
        // is steepest.
        let d = deficit * deficit;
        Correction { vbass_scale: 1.0 + self.max_vbass_boost * d, warmth_db: self.max_warmth_db * d }
    }
}
