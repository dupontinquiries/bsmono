//! Eyes — what a creature can actually *resolve*, per species and genome.
//!
//! This is the resolution layer that sits above [`crate::sim::vision`] (which
//! answers "is there a clear line within the cone and range, unoccluded"). Here
//! we answer "even with a clear line, is the target big enough on the retina, or
//! moving enough, for THIS eye to register it?" That's what stops grazing fauna
//! from noticing a player flying far overhead while a keen-eyed dragon picks the
//! same speck out of the sky.
//!
//! An [`EyeProfile`] carries a field-of-view cone, a finite range, an angular
//! **resolution** (expressed as a `radius / distance` ratio, so no trig is
//! needed — it stays deterministic like the rest of the sim), and a
//! **motion gain** that lowers the resolving bar for moving targets. Presets
//! span keen aerial hunters to the near-blind lauscher to fully eyeless genomes,
//! and [`EyeProfile::with_eyesight`] lets a genome's eyesight gene sharpen or
//! blur any of them.

/// How clearly an eye registers a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detection {
    /// Out of cone/range, or too small to resolve while still.
    Unseen,
    /// Caught only by its movement.
    Motion,
    /// Plainly resolved.
    Clear,
}

impl Detection {
    pub fn is_detected(self) -> bool {
        !matches!(self, Detection::Unseen)
    }
}

/// An eye's capabilities. Distances in blocks; the cone is stored as the cosine²
/// of its half-angle (sqrt/trig-free comparison); acuity is the minimum
/// `radius / distance` ratio a target must subtend to be resolved clearly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeProfile {
    /// cos² of the view-cone half-angle. Larger = narrower cone. (≈0.03 ≈ 160°
    /// total, ≈0.58 ≈ 80° total.)
    pub fov_cos2: f32,
    /// Maximum sighting distance; beyond it nothing registers.
    pub range: f32,
    /// Minimum apparent size (`radius / distance`) to resolve clearly. Smaller =
    /// sharper.
    pub acuity_ratio: f32,
    /// How much motion lowers the resolving bar, `[0,1]`.
    pub motion_gain: f32,
    /// Night-vision factor `[0,1]` (reserved for light-level wiring).
    pub low_light: f32,
}

/// Reference speed (blocks/sec) at which a target counts as "fully moving".
pub const MOTION_REF_SPEED: f32 = 4.0;

impl EyeProfile {
    /// Grazing fauna: wide cone (~160°), modest range, average sharpness.
    pub const FAUNA: EyeProfile = EyeProfile { fov_cos2: 0.029, range: 48.0, acuity_ratio: 0.012, motion_gain: 0.6, low_light: 0.3 };
    /// Aerial hunter (dragons/raptors): a narrow (~80°), keen, long-range cone —
    /// the eye that plucks a flyer from the sky.
    pub const AERIAL_HUNTER: EyeProfile = EyeProfile { fov_cos2: 0.585, range: 200.0, acuity_ratio: 0.0022, motion_gain: 0.8, low_light: 0.4 };
    /// Big terrestrial predator (dinos): solid range, keys hard on motion.
    pub const DINO: EyeProfile = EyeProfile { fov_cos2: 0.206, range: 70.0, acuity_ratio: 0.008, motion_gain: 0.9, low_light: 0.2 };
    /// The lauscher: nearly blind, tiny range, poor acuity — it hunts by ear.
    pub const LAUSCHER: EyeProfile = EyeProfile { fov_cos2: 0.770, range: 8.0, acuity_ratio: 0.06, motion_gain: 0.3, low_light: 0.1 };
    /// No eyes at all.
    pub const EYELESS: EyeProfile = EyeProfile { fov_cos2: 1.0, range: 0.0, acuity_ratio: 1000.0, motion_gain: 0.0, low_light: 0.0 };

    /// Scale a profile by an eyesight gene in `[0,1]`: better eyesight reaches
    /// farther and resolves finer; a weak genome is short-sighted and blurry.
    /// 0.5 leaves the base unchanged.
    pub fn with_eyesight(self, eyesight01: f32) -> EyeProfile {
        let g = eyesight01.clamp(0.0, 1.0);
        EyeProfile {
            range: self.range * (0.4 + 1.2 * g),       // 0.4x .. 1.6x
            acuity_ratio: self.acuity_ratio * (1.6 - 1.2 * g), // 1.6x blurry .. 0.4x sharp
            ..self
        }
    }
}

/// Decide how an eye at `eye_pos` looking along `facing` (need not be unit-length)
/// registers a target of `radius` at `target_pos` moving at `target_speed`
/// (blocks/sec). Gates: range, then the FOV cone (sqrt-free), then resolution —
/// big enough ⇒ `Clear`; smaller but moving fast enough ⇒ `Motion`; else
/// `Unseen`. (Occlusion is the caller's job via [`crate::sim::vision`].)
pub fn sees(
    eye: EyeProfile,
    eye_pos: [f32; 3],
    facing: [f32; 3],
    target_pos: [f32; 3],
    radius: f32,
    target_speed: f32,
) -> Detection {
    let d = [target_pos[0] - eye_pos[0], target_pos[1] - eye_pos[1], target_pos[2] - eye_pos[2]];
    let dist2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if eye.range <= 0.0 || dist2 > eye.range * eye.range {
        return Detection::Unseen;
    }
    // FOV cone, sqrt-free: dot must be ahead (>0) and dot² ≥ cos²·|d|²·|facing|².
    let dot = d[0] * facing[0] + d[1] * facing[1] + d[2] * facing[2];
    let fl2 = facing[0] * facing[0] + facing[1] * facing[1] + facing[2] * facing[2];
    if fl2 > 1e-9 && dist2 > 1e-9 && (dot <= 0.0 || dot * dot < eye.fov_cos2 * dist2 * fl2) {
        return Detection::Unseen;
    }
    let dist = dist2.sqrt();
    let apparent = if dist <= 1e-4 { f32::MAX } else { radius / dist };
    if apparent >= eye.acuity_ratio {
        return Detection::Clear;
    }
    let motion = (target_speed / MOTION_REF_SPEED).clamp(0.0, 1.0);
    let motion_threshold = eye.acuity_ratio * (1.0 - eye.motion_gain * motion);
    if motion > 0.0 && apparent >= motion_threshold {
        Detection::Motion
    } else {
        Detection::Unseen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FWD: [f32; 3] = [1.0, 0.0, 0.0];

    #[test]
    fn out_of_cone_or_range_is_unseen() {
        assert_eq!(sees(EyeProfile::FAUNA, [0.0; 3], FWD, [-10.0, 0.0, 0.0], 0.5, 0.0), Detection::Unseen);
        assert_eq!(sees(EyeProfile::FAUNA, [0.0; 3], FWD, [100.0, 0.0, 0.0], 0.5, 0.0), Detection::Unseen);
    }

    #[test]
    fn a_high_flyer_is_seen_by_a_dragon_but_not_by_grazing_fauna() {
        let eye = [0.0, 0.0, 0.0];
        let up = [10.0, 90.0, 0.0]; // ~90 blocks overhead
        let facing = [0.1, 1.0, 0.0]; // looking up
        assert_eq!(sees(EyeProfile::FAUNA, eye, facing, up, 0.5, 1.0), Detection::Unseen, "fauna miss the high flyer");
        assert!(sees(EyeProfile::AERIAL_HUNTER, eye, facing, up, 0.5, 1.0).is_detected(), "the dragon spots it");
    }

    #[test]
    fn motion_reveals_what_stillness_hides() {
        // 44 blocks: inside fauna range (48) but just under clear acuity.
        let eye = [0.0; 3];
        let target = [44.0, 0.0, 0.0];
        assert_eq!(sees(EyeProfile::FAUNA, eye, FWD, target, 0.5, 0.0), Detection::Unseen, "frozen: unseen");
        assert_eq!(sees(EyeProfile::FAUNA, eye, FWD, target, 0.5, 4.0), Detection::Motion, "moving: caught as motion");
    }

    #[test]
    fn eyeless_never_sees_and_a_lauscher_only_close() {
        let eye = [0.0; 3];
        assert_eq!(sees(EyeProfile::EYELESS, eye, FWD, [1.0, 0.0, 0.0], 1.0, 4.0), Detection::Unseen);
        let far = [20.0, 0.0, 0.0];
        assert_eq!(sees(EyeProfile::LAUSCHER, eye, FWD, far, 0.5, 0.0), Detection::Unseen);
        assert!(sees(EyeProfile::FAUNA, eye, FWD, far, 0.5, 0.0).is_detected());
        assert!(sees(EyeProfile::LAUSCHER, eye, FWD, [3.0, 0.0, 0.0], 0.5, 0.0).is_detected());
    }

    #[test]
    fn the_eyesight_gene_sharpens_or_blurs_the_eye() {
        let eye = [0.0; 3];
        let target = [60.0, 0.0, 0.0]; // beyond base fauna range (48)
        let keen = EyeProfile::FAUNA.with_eyesight(1.0);
        let weak = EyeProfile::FAUNA.with_eyesight(0.0);
        assert!(keen.range > weak.range && keen.acuity_ratio < weak.acuity_ratio);
        assert!(sees(keen, eye, FWD, target, 0.5, 0.0).is_detected(), "keen eyes reach 60 blocks");
        assert_eq!(sees(weak, eye, FWD, target, 0.5, 0.0), Detection::Unseen, "weak eyes fall short");
    }
}
