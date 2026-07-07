//! Pylons — the configurable, owner-bound evolution of the beacon. A pylon is
//! attuned to whoever placed it: the owner feels the **boon** of its effect,
//! everyone else feels the **bane**. (Pick a beneficial effect and only you get
//! it; pick a harmful one and only others do — the nether-beacon rule.)
//!
//! What makes them a builder's tool is the effect **field**: its shape is
//! configurable, so instead of one fixed radius you sculpt where the power
//! reaches — a dome over a base, a defensive box around a wall, a column up a
//! tower, a directed beam down a corridor. Drop several and their fields stack.
//! The power structure beneath (the polished-blackstone pyramid) sets the tier,
//! which sets the field's size.
//!
//! Pure, integer, deterministic.

/// Identifies the attuned owner (player id). 0 = unattuned.
pub type OwnerId = u64;

/// The status effects a pylon can project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PylonEffect {
    // Boons — only the owner receives these.
    Speed,
    Haste,
    Strength,
    JumpBoost,
    Regeneration,
    Resistance,
    Saturation,
    // Banes — only non-owners receive these.
    Slowness,
    MiningFatigue,
    Weakness,
    Poison,
    Wither,
    Hunger,
}

impl PylonEffect {
    /// Beneficial effects reach the owner; harmful effects reach everyone else.
    pub fn is_beneficial(self) -> bool {
        use PylonEffect::*;
        matches!(self, Speed | Haste | Strength | JumpBoost | Regeneration | Resistance | Saturation)
    }
}

/// The geometry of a pylon's effect field — the part a builder shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldShape {
    /// All-around ball (the classic beacon range).
    Sphere { radius: u16 },
    /// Upper hemisphere — covers a base from the ground up, nothing below.
    Dome { radius: u16 },
    /// A vertical pillar: horizontal `radius`, rising `height` above the pylon.
    Column { radius: u16, height: u16 },
    /// An axis-aligned box of half-extents (a room/wall ward).
    Boxed { half: [u16; 3] },
    /// A directed cylinder: down `dir` (a unit-ish direction) for `length`,
    /// `radius` wide — power down a hallway.
    Beam { dir: [i32; 3], length: u16, radius: u16 },
}

fn sq(v: i64) -> i64 {
    v * v
}

impl FieldShape {
    /// Is `pos` inside the field of a pylon at `center`?
    pub fn contains(self, center: [i32; 3], pos: [i32; 3]) -> bool {
        let rel = [
            (pos[0] - center[0]) as i64,
            (pos[1] - center[1]) as i64,
            (pos[2] - center[2]) as i64,
        ];
        match self {
            FieldShape::Sphere { radius } => {
                sq(rel[0]) + sq(rel[1]) + sq(rel[2]) <= sq(radius as i64)
            }
            FieldShape::Dome { radius } => {
                rel[1] >= 0 && sq(rel[0]) + sq(rel[1]) + sq(rel[2]) <= sq(radius as i64)
            }
            FieldShape::Column { radius, height } => {
                rel[1] >= 0
                    && rel[1] <= height as i64
                    && sq(rel[0]) + sq(rel[2]) <= sq(radius as i64)
            }
            FieldShape::Boxed { half } => {
                rel[0].abs() <= half[0] as i64
                    && rel[1].abs() <= half[1] as i64
                    && rel[2].abs() <= half[2] as i64
            }
            FieldShape::Beam { dir, length, radius } => {
                let d = [dir[0] as i64, dir[1] as i64, dir[2] as i64];
                let dd = sq(d[0]) + sq(d[1]) + sq(d[2]);
                if dd == 0 {
                    return false;
                }
                // Projection t (in dir-lengths) = (rel·dir)/dd; keep it integer.
                let t_num = rel[0] * d[0] + rel[1] * d[1] + rel[2] * d[2];
                if t_num < 0 || t_num > length as i64 * dd {
                    return false; // behind the pylon or past the beam's end
                }
                // Perpendicular distance² = |rel|² − (rel·dir)²/dd.
                let rel2 = sq(rel[0]) + sq(rel[1]) + sq(rel[2]);
                let perp2 = rel2 - t_num * t_num / dd;
                perp2 <= sq(radius as i64)
            }
        }
    }
}

/// A placed, attuned pylon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pylon {
    pub owner: OwnerId,
    pub center: [i32; 3],
    pub effect: PylonEffect,
    pub shape: FieldShape,
    pub active: bool,
}

impl Pylon {
    /// What this pylon does to `subject` standing at `pos`. `None` unless the
    /// subject is inside the field AND on the right side of the owner rule:
    /// a boon reaches only the owner, a bane reaches only everyone else.
    pub fn affect(&self, pos: [i32; 3], subject: OwnerId) -> Option<PylonEffect> {
        if !self.active || !self.shape.contains(self.center, pos) {
            return None;
        }
        let is_owner = subject == self.owner;
        if self.effect.is_beneficial() == is_owner {
            Some(self.effect)
        } else {
            None
        }
    }
}

/// Effects on `subject` at `pos` from a whole network of pylons — fields stack,
/// so overlapping pylons layer their boons/banes.
pub fn effects_from(pylons: &[Pylon], pos: [i32; 3], subject: OwnerId) -> Vec<PylonEffect> {
    pylons.iter().filter_map(|p| p.affect(pos, subject)).collect()
}

/// Field radius granted by the power pyramid's layer count (à la the beacon —
/// more polished-blackstone tiers reach farther). 0 layers → inactive (0).
pub fn radius_for_layers(layers: u8) -> u16 {
    if layers == 0 {
        0
    } else {
        (layers as u16) * 16 + 8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: OwnerId = 7;
    const THEM: OwnerId = 99;

    #[test]
    fn boon_reaches_owner_bane_reaches_others() {
        // A Saturation pylon: only I get fed.
        let sat = Pylon {
            owner: ME,
            center: [0, 64, 0],
            effect: PylonEffect::Saturation,
            shape: FieldShape::Sphere { radius: 20 },
            active: true,
        };
        assert_eq!(sat.affect([5, 64, 0], ME), Some(PylonEffect::Saturation), "owner gets the boon");
        assert_eq!(sat.affect([5, 64, 0], THEM), None, "others get nothing from a boon");

        // A Slowness pylon: only others are slowed.
        let slow = Pylon { effect: PylonEffect::Slowness, ..sat };
        assert_eq!(slow.affect([5, 64, 0], THEM), Some(PylonEffect::Slowness), "intruders are slowed");
        assert_eq!(slow.affect([5, 64, 0], ME), None, "I'm immune to my own bane");
    }

    #[test]
    fn the_field_must_contain_the_subject_and_be_active() {
        let p = Pylon {
            owner: ME,
            center: [0, 64, 0],
            effect: PylonEffect::Speed,
            shape: FieldShape::Sphere { radius: 10 },
            active: true,
        };
        assert!(p.affect([3, 64, 0], ME).is_some(), "inside");
        assert!(p.affect([50, 64, 0], ME).is_none(), "outside the radius");
        let off = Pylon { active: false, ..p };
        assert!(off.affect([3, 64, 0], ME).is_none(), "inactive pylon does nothing");
    }

    #[test]
    fn shapes_cover_what_they_should() {
        let c = [0, 64, 0];
        // Dome: above only.
        let dome = FieldShape::Dome { radius: 10 };
        assert!(dome.contains(c, [0, 68, 0]), "above the pylon");
        assert!(!dome.contains(c, [0, 60, 0]), "nothing below the dome");
        // Column: tall thin pillar.
        let col = FieldShape::Column { radius: 2, height: 30 };
        assert!(col.contains(c, [1, 90, 0]), "up the column");
        assert!(!col.contains(c, [8, 66, 0]), "outside the column's girth");
        assert!(!col.contains(c, [0, 40, 0]), "below the pylon");
        // Box: a warded room.
        let boxed = FieldShape::Boxed { half: [5, 3, 5] };
        assert!(boxed.contains(c, [5, 67, -5]), "corner of the box");
        assert!(!boxed.contains(c, [5, 68, -5]), "just above the lid");
    }

    #[test]
    fn beam_points_down_a_corridor() {
        let c = [0, 64, 0];
        let beam = FieldShape::Beam { dir: [1, 0, 0], length: 20, radius: 2 };
        assert!(beam.contains(c, [10, 64, 0]), "straight down the beam");
        assert!(beam.contains(c, [10, 64, 1], ), "within the beam's girth");
        assert!(!beam.contains(c, [10, 64, 5]), "outside the beam's girth");
        assert!(!beam.contains(c, [-10, 64, 0]), "nothing behind the pylon");
        assert!(!beam.contains(c, [30, 64, 0]), "past the beam's end");
    }

    #[test]
    fn fields_stack_across_a_network() {
        let speed = Pylon {
            owner: ME, center: [0, 64, 0], effect: PylonEffect::Speed,
            shape: FieldShape::Sphere { radius: 20 }, active: true,
        };
        let regen = Pylon {
            owner: ME, center: [4, 64, 0], effect: PylonEffect::Regeneration,
            shape: FieldShape::Sphere { radius: 20 }, active: true,
        };
        let got = effects_from(&[speed, regen], [2, 64, 0], ME);
        assert!(got.contains(&PylonEffect::Speed) && got.contains(&PylonEffect::Regeneration),
            "overlapping pylons layer their boons");
    }

    #[test]
    fn bigger_pyramid_reaches_farther() {
        assert_eq!(radius_for_layers(0), 0, "no pyramid → inactive");
        assert!(radius_for_layers(4) > radius_for_layers(1), "more tiers reach farther");
    }
}
