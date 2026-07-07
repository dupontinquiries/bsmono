//! Leap — a ballistic jump model for hopping ledges and clearing gaps (the
//! basis for parkour). A creature's [`Leap`] is two genome-scaled capabilities:
//! `up` (launch speed, how high it can reach) and `forward` (horizontal speed,
//! how far it carries). Given gravity we can ask whether a jump clears a gap of
//! a certain width and lands on a platform a certain height above/below — which
//! is exactly the decision a fast, agile mob makes when it meets a ledge.
//!
//! Pure + deterministic. SI-ish units (blocks, blocks/s, blocks/s²).

/// Standard fall acceleration used for jumps, in blocks/s².
pub const GRAVITY: f32 = 20.0;

/// A creature's jumping capability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leap {
    /// Vertical launch speed (blocks/s) — sets peak height.
    pub up: f32,
    /// Horizontal launch speed (blocks/s) — sets reach.
    pub forward: f32,
}

impl Leap {
    pub fn new(up: f32, forward: f32) -> Leap {
        Leap { up: up.max(0.0), forward: forward.max(0.0) }
    }

    /// Genome-scaled leap: `strength` and `distance` in [0,1] map to sensible
    /// ranges (up ~3..9 blk/s, forward ~2..8 blk/s).
    pub fn from_genes(strength: f32, distance: f32) -> Leap {
        Leap::new(3.0 + strength.clamp(0.0, 1.0) * 6.0, 2.0 + distance.clamp(0.0, 1.0) * 6.0)
    }

    /// Peak height reached (blocks) under `gravity`.
    pub fn peak_height(&self, gravity: f32) -> f32 {
        if gravity <= 0.0 {
            return f32::INFINITY;
        }
        self.up * self.up / (2.0 * gravity)
    }

    /// Height of the jump arc after travelling `horizontal` blocks forward
    /// (blocks, relative to launch). Negative once it has fallen below launch.
    pub fn height_at(&self, horizontal: f32, gravity: f32) -> f32 {
        if self.forward <= 0.0 {
            return if horizontal <= 0.0 { 0.0 } else { f32::NEG_INFINITY };
        }
        let t = horizontal / self.forward;
        self.up * t - 0.5 * gravity * t * t
    }

    /// Can this leap clear a gap `gap_width` blocks across and land on a platform
    /// `rise` blocks above the launch (negative `rise` = lower)? True when the
    /// arc is at or above `rise` exactly as it reaches the far edge — i.e. the
    /// creature arrives at the landing with enough height.
    pub fn clears_gap(&self, gap_width: f32, rise: f32, gravity: f32) -> bool {
        if gap_width <= 0.0 {
            return rise <= self.peak_height(gravity);
        }
        if self.forward <= 0.0 {
            return false;
        }
        self.height_at(gap_width, gravity) >= rise
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stronger_leap_reaches_higher() {
        let weak = Leap::from_genes(0.0, 0.5);
        let strong = Leap::from_genes(1.0, 0.5);
        assert!(strong.peak_height(GRAVITY) > weak.peak_height(GRAVITY));
    }

    #[test]
    fn clears_a_wide_gap_only_with_enough_reach() {
        // Level gap (rise = 0). A long-jumper clears 4 blocks; a short one fails.
        let agile = Leap::from_genes(0.6, 1.0); // strong up, max forward
        let stubby = Leap::from_genes(0.6, 0.0); // little forward
        assert!(agile.clears_gap(4.0, 0.0, GRAVITY), "agile clears a 4-block gap");
        assert!(!stubby.clears_gap(4.0, 0.0, GRAVITY), "stubby can't");
    }

    #[test]
    fn can_hop_up_to_a_reachable_ledge_but_not_an_impossible_one() {
        let l = Leap::from_genes(1.0, 0.7); // up ~9 blk/s
        let peak = l.peak_height(GRAVITY);
        // A ledge a hair below the peak, a short hop away, is reachable.
        assert!(l.clears_gap(1.0, peak * 0.4, GRAVITY), "hops onto a modest ledge");
        // A ledge far above the peak height is impossible regardless of distance.
        assert!(!l.clears_gap(1.0, peak + 2.0, GRAVITY), "can't reach a ledge above its peak");
    }

    #[test]
    fn dropping_to_a_lower_ledge_is_easier_than_rising() {
        let l = Leap::from_genes(0.4, 0.6);
        // A gap where the far side is LOWER (negative rise) is clearable even when
        // the same gap at a higher landing is not.
        let gap = 3.0;
        assert!(l.clears_gap(gap, -2.0, GRAVITY), "can hop down across a gap");
        let high = l.clears_gap(gap, 2.0, GRAVITY);
        let low = l.clears_gap(gap, -2.0, GRAVITY);
        assert!(low || !high, "a lower landing is never harder than a higher one");
    }

    #[test]
    fn zero_gravity_floats_forever() {
        let l = Leap::from_genes(0.5, 0.5);
        assert!(l.peak_height(0.0).is_infinite());
        assert!(l.clears_gap(100.0, 50.0, 0.0), "with no gravity any rise within reach is clearable");
    }
}
