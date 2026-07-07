//! class.rs — the Star Jumper class pick. You choose a playstyle that sets your health pool, your thrown
//! equipment, and your super ability:
//!   * **Trooper** — the neutral baseline (100 HP, a frag grenade, no super). The default when nothing's chosen.
//!   * **Tank** — a huge health pool, an acid grenade that melts heavies, and a super that deploys two
//!     auto-targeting shoulder cannons for a few seconds.
//!   * **Assassin** — fragile but mobile, a gravity grenade that warps local gravity into a low-G well, and a
//!     gravity-slam super that launches you off the ground and detonates a shockwave where you land.

/// What the class throws as its equipment grenade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrenadeKind {
    /// Standard fragmentation blast (instant AoE damage).
    Frag,
    /// A gravity well: warps local gravity to near-zero for a few seconds (mobility + floats enemies).
    Gravity,
    /// An acid sprinkler puddle: lingering damage-over-time, doubled against heavies/tanks.
    Acid,
}

/// The class's super ability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuperKind {
    /// No super (Trooper).
    None,
    /// Two auto-targeting, auto-firing shoulder cannons for a short window (Tank).
    ShoulderCannons,
    /// Launch off the ground; the next landing detonates a damaging gravity shockwave (Assassin).
    GravitySlam,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassKind {
    Trooper,
    Tank,
    Assassin,
}

impl Default for ClassKind {
    fn default() -> Self {
        ClassKind::Trooper
    }
}

#[derive(Clone, Copy)]
pub struct ClassDef {
    pub name: &'static str,
    pub max_health: f32,
    pub grenade: GrenadeKind,
    pub super_kind: SuperKind,
    /// Multiplier on how fast the super charges (Tank's is slower; it's stronger).
    pub charge_rate: f32,
}

impl ClassKind {
    pub const ALL: [ClassKind; 3] = [ClassKind::Trooper, ClassKind::Tank, ClassKind::Assassin];

    pub fn id(self) -> u8 {
        ClassKind::ALL.iter().position(|&c| c == self).unwrap_or(0) as u8
    }
    pub fn from_id(id: u8) -> ClassKind {
        ClassKind::ALL.get(id as usize).copied().unwrap_or(ClassKind::Trooper)
    }

    pub fn def(self) -> ClassDef {
        match self {
            ClassKind::Trooper => ClassDef {
                name: "Trooper",
                max_health: 100.0,
                grenade: GrenadeKind::Frag,
                super_kind: SuperKind::None,
                charge_rate: 1.0,
            },
            ClassKind::Tank => ClassDef {
                name: "Tank",
                max_health: 250.0,
                grenade: GrenadeKind::Acid,
                super_kind: SuperKind::ShoulderCannons,
                charge_rate: 0.8,
            },
            ClassKind::Assassin => ClassDef {
                name: "Assassin",
                max_health: 70.0,
                grenade: GrenadeKind::Gravity,
                super_kind: SuperKind::GravitySlam,
                charge_rate: 1.25,
            },
        }
    }
    pub fn name(self) -> &'static str {
        self.def().name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_match_the_design() {
        // Tank is a bullet sponge with acid + cannons; Assassin is fragile with gravity tools.
        assert!(ClassKind::Tank.def().max_health > ClassKind::Trooper.def().max_health);
        assert!(ClassKind::Assassin.def().max_health < ClassKind::Trooper.def().max_health);
        assert_eq!(ClassKind::Tank.def().grenade, GrenadeKind::Acid);
        assert_eq!(ClassKind::Tank.def().super_kind, SuperKind::ShoulderCannons);
        assert_eq!(ClassKind::Assassin.def().grenade, GrenadeKind::Gravity);
        assert_eq!(ClassKind::Assassin.def().super_kind, SuperKind::GravitySlam);
        assert_eq!(ClassKind::Trooper.def().super_kind, SuperKind::None);
    }

    #[test]
    fn class_ids_round_trip() {
        for c in ClassKind::ALL {
            assert_eq!(ClassKind::from_id(c.id()), c);
        }
    }
}
