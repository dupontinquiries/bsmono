//! sim::sub_discipline — sub-discipline access, depth tier, and renown title.
//!
//! From `character_panel_reference §3/§7`: a bender whose **resonance_depth** in
//! a discipline exceeds 0.70 qualifies for its sub-discipline (Water → Tidal /
//! "Oceanbinder"). The "WARLOCK SUBCLASS" label is a *depth-tier* classification
//! of that resonance_depth. Separately, a **renown** ladder (…Known…Legendary)
//! governs social recognition: only at Known renown does the player's registered
//! sub-discipline name (Oceanbinder) become their displayed title — otherwise it
//! defaults to the canonical name (Tidal Practitioner). This module is those
//! three pure mappings. `resonance_depth` comes from the item/genome side
//! (`sim::resonance`, `item_genetics`); renown is earned socially. Std-only,
//! deterministic.

/// Resonance depth at or above which a sub-discipline unlocks.
pub const SUB_DISCIPLINE_THRESHOLD: f32 = 0.70;

/// Does this resonance_depth qualify for sub-discipline access?
pub fn sub_discipline_unlocked(resonance_depth: f32) -> bool {
    resonance_depth >= SUB_DISCIPLINE_THRESHOLD
}

/// Depth-tier classification of a discipline's resonance_depth. "Warlock" is the
/// band that grants sub-discipline access (the panel's WARLOCK SUBCLASS label).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DepthTier {
    Untuned,
    Attuned,
    Adept,
    Warlock,
    Archon,
}

impl DepthTier {
    pub fn label(self) -> &'static str {
        match self {
            DepthTier::Untuned => "Untuned",
            DepthTier::Attuned => "Attuned",
            DepthTier::Adept => "Adept",
            DepthTier::Warlock => "Warlock",
            DepthTier::Archon => "Archon",
        }
    }
}

/// Classify resonance_depth into a depth tier. Warlock begins exactly at the
/// sub-discipline threshold, so `depth_tier(d) >= Warlock` iff the sub-discipline
/// is unlocked.
pub fn depth_tier(resonance_depth: f32) -> DepthTier {
    let d = resonance_depth;
    if d < 0.30 {
        DepthTier::Untuned
    } else if d < 0.50 {
        DepthTier::Attuned
    } else if d < SUB_DISCIPLINE_THRESHOLD {
        DepthTier::Adept
    } else if d < 0.90 {
        DepthTier::Warlock
    } else {
        DepthTier::Archon
    }
}

/// Social recognition ladder within the stance/Dojo system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RenownTier {
    Unknown,
    Known,
    Respected,
    Renowned,
    Legendary,
}

impl RenownTier {
    pub fn label(self) -> &'static str {
        match self {
            RenownTier::Unknown => "Unknown",
            RenownTier::Known => "Known",
            RenownTier::Respected => "Respected",
            RenownTier::Renowned => "Renowned",
            RenownTier::Legendary => "Legendary",
        }
    }
    /// Rung index (0 = Unknown).
    pub fn index(self) -> u8 {
        match self {
            RenownTier::Unknown => 0,
            RenownTier::Known => 1,
            RenownTier::Respected => 2,
            RenownTier::Renowned => 3,
            RenownTier::Legendary => 4,
        }
    }
}

/// Map accumulated renown to a tier. Thresholds are illustrative/tunable.
pub fn renown_tier(renown: u32) -> RenownTier {
    if renown >= 10_000 {
        RenownTier::Legendary
    } else if renown >= 2_000 {
        RenownTier::Renowned
    } else if renown >= 500 {
        RenownTier::Respected
    } else if renown >= 100 {
        RenownTier::Known
    } else {
        RenownTier::Unknown
    }
}

/// The displayed sub-discipline title. A player's `registered_name` (e.g.
/// "Oceanbinder") only shows once they've reached **Known** renown; below that,
/// or if they never registered one, the `canonical` name (e.g. "Tidal
/// Practitioner") is used.
pub fn display_title<'a>(registered_name: Option<&'a str>, renown: RenownTier, canonical: &'a str) -> &'a str {
    match registered_name {
        Some(name) if renown >= RenownTier::Known => name,
        _ => canonical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_discipline_unlocks_at_seventy_percent() {
        assert!(!sub_discipline_unlocked(0.69));
        assert!(sub_discipline_unlocked(0.70));
        assert!(sub_discipline_unlocked(0.80));
    }

    #[test]
    fn depth_tiers_band_correctly_and_warlock_marks_access() {
        assert_eq!(depth_tier(0.10), DepthTier::Untuned);
        assert_eq!(depth_tier(0.40), DepthTier::Attuned);
        assert_eq!(depth_tier(0.60), DepthTier::Adept);
        assert_eq!(depth_tier(0.75), DepthTier::Warlock); // doc's ~0.75–0.80 case
        assert_eq!(depth_tier(0.95), DepthTier::Archon);
        // Warlock (or higher) iff sub-discipline unlocked.
        for &d in &[0.0f32, 0.5, 0.69, 0.70, 0.85, 1.0] {
            assert_eq!(depth_tier(d) >= DepthTier::Warlock, sub_discipline_unlocked(d), "at {d}");
        }
    }

    #[test]
    fn renown_ladder_thresholds() {
        assert_eq!(renown_tier(0), RenownTier::Unknown);
        assert_eq!(renown_tier(99), RenownTier::Unknown);
        assert_eq!(renown_tier(100), RenownTier::Known);
        assert_eq!(renown_tier(500), RenownTier::Respected);
        assert_eq!(renown_tier(2_000), RenownTier::Renowned);
        assert_eq!(renown_tier(10_000), RenownTier::Legendary);
        assert!(RenownTier::Legendary > RenownTier::Known);
    }

    #[test]
    fn title_shows_registered_name_only_at_known_renown() {
        // Registered "Oceanbinder", but not yet Known ⇒ canonical.
        assert_eq!(
            display_title(Some("Oceanbinder"), RenownTier::Unknown, "Tidal Practitioner"),
            "Tidal Practitioner"
        );
        // Known+ ⇒ the registered name shows.
        assert_eq!(
            display_title(Some("Oceanbinder"), RenownTier::Known, "Tidal Practitioner"),
            "Oceanbinder"
        );
        assert_eq!(
            display_title(Some("Oceanbinder"), RenownTier::Legendary, "Tidal Practitioner"),
            "Oceanbinder"
        );
        // Never registered ⇒ always canonical.
        assert_eq!(
            display_title(None, RenownTier::Legendary, "Tidal Practitioner"),
            "Tidal Practitioner"
        );
    }

    #[test]
    fn deterministic() {
        assert_eq!(depth_tier(0.73), depth_tier(0.73));
        assert_eq!(renown_tier(1234), renown_tier(1234));
    }
}
