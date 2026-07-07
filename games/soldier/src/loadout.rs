//! loadout.rs — the player's chosen kit: a primary weapon, a sidearm, and up to three perks. Perks are
//! pure modifiers the game reads when it builds a run (reload speed, move speed, recoil, stealth, a jetpack
//! boost, ammo scavenging). Everything here is plain data so it serialises cleanly into the on-disk DB.

use crate::weapons::WeaponKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perk {
    QuickReload,
    Scavenger,
    Camo,
    Jetpack,
    Lightweight,
    Steady,
    ExtraMag,
    /// Snap to aim faster — a shorter ADS time for quicker target acquisition.
    Marksman,
    /// Health regenerates faster and starts sooner after taking fire.
    Regen,
    /// Bigger hip-fire accuracy and faster weapon swap — an aggressive close-quarters kit.
    Gunslinger,
}

impl Perk {
    pub const ALL: [Perk; 10] = [
        Perk::QuickReload,
        Perk::Scavenger,
        Perk::Camo,
        Perk::Jetpack,
        Perk::Lightweight,
        Perk::Steady,
        Perk::ExtraMag,
        Perk::Marksman,
        Perk::Regen,
        Perk::Gunslinger,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Perk::QuickReload => "Quick Reload",
            Perk::Scavenger => "Scavenger",
            Perk::Camo => "Active Camo",
            Perk::Jetpack => "Jetpack",
            Perk::Lightweight => "Lightweight",
            Perk::Steady => "Steady Aim",
            Perk::ExtraMag => "Extended Mag",
            Perk::Marksman => "Marksman",
            Perk::Regen => "Fast Regen",
            Perk::Gunslinger => "Gunslinger",
        }
    }

    /// A short glyph used as a flat icon in the menu (no image assets).
    pub fn icon(self) -> &'static str {
        match self {
            Perk::QuickReload => "↻",
            Perk::Scavenger => "$",
            Perk::Camo => "◇",
            Perk::Jetpack => "▲",
            Perk::Lightweight => "»",
            Perk::Steady => "=",
            Perk::ExtraMag => "+",
            Perk::Marksman => "◎",
            Perk::Regen => "✚",
            Perk::Gunslinger => "⚡",
        }
    }

    pub fn desc(self) -> &'static str {
        match self {
            Perk::QuickReload => "Reload ~40% faster.",
            Perk::Scavenger => "Pick up far more ammo from kills.",
            Perk::Camo => "Enemies spot you at much shorter range.",
            Perk::Jetpack => "A mid-air thrust boost — hold jump to rise.",
            Perk::Lightweight => "Move and sprint noticeably faster.",
            Perk::Steady => "Recoil reduced ~30%.",
            Perk::ExtraMag => "Larger starting magazines and reserve.",
            Perk::Marksman => "Aim down sights noticeably faster.",
            Perk::Regen => "Health regenerates faster, and sooner after being hit.",
            Perk::Gunslinger => "Much tighter hip-fire — run-and-gun accuracy.",
        }
    }

    pub fn from_id(id: u8) -> Option<Perk> {
        Perk::ALL.get(id as usize).copied()
    }
    pub fn id(self) -> u8 {
        Perk::ALL.iter().position(|&p| p == self).unwrap_or(0) as u8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loadout {
    pub primary: WeaponKind,
    pub secondary: WeaponKind,
    pub perks: [Option<Perk>; 3],
    /// Index into `legendary::POOL` for an equipped legendary primary (overrides `primary`).
    pub legendary: Option<usize>,
    /// The chosen Star Jumper class (health pool + grenade + super).
    pub class: crate::class::ClassKind,
}

impl Default for Loadout {
    fn default() -> Self {
        Self {
            primary: WeaponKind::Ar,
            secondary: WeaponKind::Pistol,
            perks: [Some(Perk::QuickReload), Some(Perk::Lightweight), None],
            legendary: None,
            class: crate::class::ClassKind::Trooper,
        }
    }
}

impl Loadout {
    pub fn has(&self, p: Perk) -> bool {
        self.perks.iter().any(|x| *x == Some(p))
    }

    /// Toggle a perk in/out of the three slots (fills the first empty slot, or removes if present).
    pub fn toggle_perk(&mut self, p: Perk) {
        if let Some(slot) = self.perks.iter_mut().find(|x| **x == Some(p)) {
            *slot = None;
            return;
        }
        if let Some(empty) = self.perks.iter_mut().find(|x| x.is_none()) {
            *empty = Some(p);
        }
    }

    // ── Aggregate effects the game applies ──────────────────────────────────────
    /// The equipped legendary (if any).
    pub fn legend(&self) -> Option<crate::legendary::Legendary> {
        self.legendary.and_then(|i| crate::legendary::POOL.get(i).copied())
    }
    /// The weapon actually wielded as primary — the legendary's base archetype if one is equipped.
    pub fn primary_kind(&self) -> WeaponKind {
        self.legend().map(|l| l.base).unwrap_or(self.primary)
    }
    /// Outgoing-damage multiplier from an equipped legendary's perks.
    pub fn damage_mul(&self) -> f32 {
        self.legend().map(|l| l.damage_mul()).unwrap_or(1.0)
    }
    pub fn reload_mul(&self) -> f32 {
        let base = if self.has(Perk::QuickReload) { 0.6 } else { 1.0 };
        base * self.legend().map(|l| l.reload_mul()).unwrap_or(1.0)
    }
    pub fn move_mul(&self) -> f32 {
        if self.has(Perk::Lightweight) {
            1.14
        } else {
            1.0
        }
    }
    pub fn recoil_mul(&self) -> f32 {
        if self.has(Perk::Steady) {
            0.7
        } else {
            1.0
        }
    }
    pub fn detect_mul(&self) -> f32 {
        if self.has(Perk::Camo) {
            0.55
        } else {
            1.0
        }
    }
    pub fn ammo_mul(&self) -> f32 {
        if self.has(Perk::Scavenger) {
            2.2
        } else {
            1.0
        }
    }
    pub fn mag_mul(&self) -> f32 {
        if self.has(Perk::ExtraMag) {
            1.5
        } else {
            1.0
        }
    }
    pub fn jetpack(&self) -> bool {
        self.has(Perk::Jetpack)
    }
    /// ADS-time multiplier — Marksman aims down sights faster (lower is faster).
    pub fn ads_mul(&self) -> f32 {
        if self.has(Perk::Marksman) {
            0.65
        } else {
            1.0
        }
    }
    /// Health-regen rate multiplier and how much sooner regen kicks in after taking fire.
    pub fn regen_mul(&self) -> f32 {
        if self.has(Perk::Regen) {
            1.8
        } else {
            1.0
        }
    }
    /// Hip-fire spread multiplier — Gunslinger tightens it (lower is tighter).
    pub fn hipfire_mul(&self) -> f32 {
        if self.has(Perk::Gunslinger) {
            0.6
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_perks_respects_three_slots() {
        let mut l = Loadout { primary: WeaponKind::Smg, secondary: WeaponKind::Pistol, perks: [None, None, None], legendary: None, class: crate::class::ClassKind::Trooper };
        l.toggle_perk(Perk::Camo);
        l.toggle_perk(Perk::Jetpack);
        l.toggle_perk(Perk::Steady);
        l.toggle_perk(Perk::Scavenger); // no empty slot left
        assert!(l.has(Perk::Camo) && l.has(Perk::Jetpack) && l.has(Perk::Steady));
        assert!(!l.has(Perk::Scavenger), "fourth perk rejected");
        l.toggle_perk(Perk::Camo); // remove
        assert!(!l.has(Perk::Camo));
        l.toggle_perk(Perk::Scavenger); // now fits
        assert!(l.has(Perk::Scavenger));
    }

    #[test]
    fn perk_ids_round_trip() {
        for p in Perk::ALL {
            assert_eq!(Perk::from_id(p.id()), Some(p));
        }
    }

    #[test]
    fn new_perks_change_their_stats() {
        let mut l = Loadout { primary: WeaponKind::Ar, secondary: WeaponKind::Pistol, perks: [None, None, None], legendary: None, class: crate::class::ClassKind::Trooper };
        assert_eq!(l.ads_mul(), 1.0);
        assert_eq!(l.regen_mul(), 1.0);
        assert_eq!(l.hipfire_mul(), 1.0);
        l.toggle_perk(Perk::Marksman);
        l.toggle_perk(Perk::Regen);
        l.toggle_perk(Perk::Gunslinger);
        assert!(l.ads_mul() < 1.0, "marksman should speed ADS");
        assert!(l.regen_mul() > 1.0, "regen should speed healing");
        assert!(l.hipfire_mul() < 1.0, "gunslinger should tighten hip-fire");
    }
}
