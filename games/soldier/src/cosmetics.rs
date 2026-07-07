//! cosmetics.rs — purely-cosmetic-plus-light-stat customization the player picks in the menu and the game
//! persists: a crosshair/reticle style and colour, a weapon skin that tints the viewmodel, and an armor
//! "frame" drawn from the armor taxonomy (Standard / Recontour / Assault / EVA / EOD / Myth). Armor frames
//! carry a small stat tradeoff (starting plate vs. move speed) so the choice reads in play, plus a colour
//! that tints the gloved hands on the viewmodel.

use egui::Color32;

#[inline]
fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// A small shared palette used for reticle and armor colours (kept as an enum so it serialises by index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accent {
    White,
    Green,
    Cyan,
    Amber,
    Crimson,
    Magenta,
    Azure,
}
impl Accent {
    pub const ALL: [Accent; 7] = [
        Accent::White,
        Accent::Green,
        Accent::Cyan,
        Accent::Amber,
        Accent::Crimson,
        Accent::Magenta,
        Accent::Azure,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Accent::White => "White",
            Accent::Green => "Green",
            Accent::Cyan => "Cyan",
            Accent::Amber => "Amber",
            Accent::Crimson => "Crimson",
            Accent::Magenta => "Magenta",
            Accent::Azure => "Azure",
        }
    }
    pub fn color(self) -> Color32 {
        match self {
            Accent::White => rgb(226, 232, 238),
            Accent::Green => rgb(120, 235, 130),
            Accent::Cyan => rgb(120, 220, 235),
            Accent::Amber => rgb(245, 200, 90),
            Accent::Crimson => rgb(240, 90, 80),
            Accent::Magenta => rgb(235, 120, 220),
            Accent::Azure => rgb(120, 170, 255),
        }
    }
    pub fn id(self) -> u8 {
        Accent::ALL.iter().position(|&a| a == self).unwrap_or(0) as u8
    }
    pub fn from_id(id: u8) -> Accent {
        Accent::ALL.get(id as usize).copied().unwrap_or(Accent::White)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reticle {
    Cross,
    Dot,
    CrossDot,
    Circle,
    Chevron,
    Brackets,
    TBar,
}
impl Reticle {
    pub const ALL: [Reticle; 7] = [
        Reticle::Cross,
        Reticle::Dot,
        Reticle::CrossDot,
        Reticle::Circle,
        Reticle::Chevron,
        Reticle::Brackets,
        Reticle::TBar,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Reticle::Cross => "Cross",
            Reticle::Dot => "Dot",
            Reticle::CrossDot => "Cross+Dot",
            Reticle::Circle => "Circle",
            Reticle::Chevron => "Chevron",
            Reticle::Brackets => "Brackets",
            Reticle::TBar => "T-Bar",
        }
    }
    pub fn id(self) -> u8 {
        Reticle::ALL.iter().position(|&a| a == self).unwrap_or(0) as u8
    }
    pub fn from_id(id: u8) -> Reticle {
        Reticle::ALL.get(id as usize).copied().unwrap_or(Reticle::Cross)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GunSkin {
    Standard,
    Urban,
    Desert,
    Woodland,
    Arctic,
    Crimson,
    Carbon,
    Gold,
}
impl GunSkin {
    pub const ALL: [GunSkin; 8] = [
        GunSkin::Standard,
        GunSkin::Urban,
        GunSkin::Desert,
        GunSkin::Woodland,
        GunSkin::Arctic,
        GunSkin::Crimson,
        GunSkin::Carbon,
        GunSkin::Gold,
    ];
    pub fn name(self) -> &'static str {
        match self {
            GunSkin::Standard => "Standard",
            GunSkin::Urban => "Urban",
            GunSkin::Desert => "Desert",
            GunSkin::Woodland => "Woodland",
            GunSkin::Arctic => "Arctic",
            GunSkin::Crimson => "Crimson",
            GunSkin::Carbon => "Carbon",
            GunSkin::Gold => "Gold",
        }
    }
    /// Tint applied to the weapon body. `None` keeps the weapon's own colour.
    pub fn tint(self) -> Option<Color32> {
        match self {
            GunSkin::Standard => None,
            GunSkin::Urban => Some(rgb(74, 78, 86)),
            GunSkin::Desert => Some(rgb(150, 128, 86)),
            GunSkin::Woodland => Some(rgb(70, 84, 54)),
            GunSkin::Arctic => Some(rgb(176, 188, 198)),
            GunSkin::Crimson => Some(rgb(120, 44, 44)),
            GunSkin::Carbon => Some(rgb(34, 36, 40)),
            GunSkin::Gold => Some(rgb(196, 158, 70)),
        }
    }
    pub fn id(self) -> u8 {
        GunSkin::ALL.iter().position(|&a| a == self).unwrap_or(0) as u8
    }
    pub fn from_id(id: u8) -> GunSkin {
        GunSkin::ALL.get(id as usize).copied().unwrap_or(GunSkin::Standard)
    }
}

/// Armor frames from the taxonomy. Each trades starting plate against move speed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmorFrame {
    Standard,
    Recontour,
    Assault,
    Eva,
    Eod,
    Myth,
}
impl ArmorFrame {
    pub const ALL: [ArmorFrame; 6] = [
        ArmorFrame::Standard,
        ArmorFrame::Recontour,
        ArmorFrame::Assault,
        ArmorFrame::Eva,
        ArmorFrame::Eod,
        ArmorFrame::Myth,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ArmorFrame::Standard => "Standard Frame",
            ArmorFrame::Recontour => "Recontour",
            ArmorFrame::Assault => "Assault Frame",
            ArmorFrame::Eva => "EVA-Class",
            ArmorFrame::Eod => "EOD Frame",
            ArmorFrame::Myth => "Myth-Class",
        }
    }
    pub fn desc(self) -> &'static str {
        match self {
            ArmorFrame::Standard => "Balanced default war shell.",
            ArmorFrame::Recontour => "Stealth frame: fast, lightly plated.",
            ArmorFrame::Assault => "Forward-pressure: heavy plate, steady pace.",
            ArmorFrame::Eva => "Sealed exo-environment shell, balanced.",
            ArmorFrame::Eod => "Blast frame: max plate, slow.",
            ArmorFrame::Myth => "Experimental: glass cannon, very fast.",
        }
    }
    /// Starting armor plate (absorbs damage before health).
    pub fn armor(self) -> f32 {
        match self {
            ArmorFrame::Standard => 25.0,
            ArmorFrame::Recontour => 10.0,
            ArmorFrame::Assault => 50.0,
            ArmorFrame::Eva => 30.0,
            ArmorFrame::Eod => 80.0,
            ArmorFrame::Myth => 0.0,
        }
    }
    /// Move-speed multiplier from the frame's bulk.
    pub fn speed_mul(self) -> f32 {
        match self {
            ArmorFrame::Standard => 1.0,
            ArmorFrame::Recontour => 1.12,
            ArmorFrame::Assault => 0.94,
            ArmorFrame::Eva => 1.0,
            ArmorFrame::Eod => 0.82,
            ArmorFrame::Myth => 1.18,
        }
    }
    pub fn id(self) -> u8 {
        ArmorFrame::ALL.iter().position(|&a| a == self).unwrap_or(0) as u8
    }
    pub fn from_id(id: u8) -> ArmorFrame {
        ArmorFrame::ALL.get(id as usize).copied().unwrap_or(ArmorFrame::Standard)
    }
}

/// The full set of cosmetic choices, persisted with the profile/loadout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cosmetics {
    pub reticle: Reticle,
    pub reticle_color: Accent,
    pub skin: GunSkin,
    pub armor: ArmorFrame,
    pub armor_color: Accent,
    /// The mix-and-match cosmetic armor pieces shown on the character (purely visual).
    pub set: crate::character::ArmorSet,
}
impl Default for Cosmetics {
    fn default() -> Self {
        Self {
            reticle: Reticle::Cross,
            reticle_color: Accent::White,
            skin: GunSkin::Standard,
            armor: ArmorFrame::Standard,
            armor_color: Accent::Green,
            set: crate::character::ArmorSet::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for r in Reticle::ALL {
            assert_eq!(Reticle::from_id(r.id()), r);
        }
        for s in GunSkin::ALL {
            assert_eq!(GunSkin::from_id(s.id()), s);
        }
        for a in ArmorFrame::ALL {
            assert_eq!(ArmorFrame::from_id(a.id()), a);
        }
        for c in Accent::ALL {
            assert_eq!(Accent::from_id(c.id()), c);
        }
    }

    #[test]
    fn armor_tradeoff_is_monotone_ish() {
        // The heaviest frame has the most plate and the slowest speed.
        assert!(ArmorFrame::Eod.armor() > ArmorFrame::Standard.armor());
        assert!(ArmorFrame::Eod.speed_mul() < ArmorFrame::Standard.speed_mul());
        assert!(ArmorFrame::Myth.speed_mul() > ArmorFrame::Standard.speed_mul());
    }
}
