//! `sim::lore_attribution` — the six in-world source types every item's lore line
//! is attributed to.
//!
//! From `06_Crafting_and_Items/item_armor_lore_tooltips.docx`. Lore text appears in
//! italics beneath an item's mechanical lines, attributed to a source *within the
//! world*. The attribution format and tooltip structure are canonical (the lore
//! prose itself is placeholder). `sim::item_tooltip` renders the attribution
//! string; this module is the taxonomy behind it — which of the six sources an item
//! draws from, its format template, and the item classes each source covers.

/// The six in-world attribution source types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LoreAttribution {
    /// Teaching lines from bending dojo boards (a dojo or named master).
    DojoInscription,
    /// Fragmentary, partly-untranslated pre-civilisation shrine inscriptions.
    AncientShrineText,
    /// Administrative/technical records from the Router/Carrier/Keeper benders.
    RyujinTriadRecord,
    /// Naturalist or explorer field notes.
    FieldObservation,
    /// First-person accounts from benders, smiths, or hunters.
    PractitionerAccount,
    /// Undated/anonymous text recovered from ruins or wreckage.
    RecoveredFragment,
}

impl LoreAttribution {
    pub const ALL: [LoreAttribution; 6] = [
        LoreAttribution::DojoInscription,
        LoreAttribution::AncientShrineText,
        LoreAttribution::RyujinTriadRecord,
        LoreAttribution::FieldObservation,
        LoreAttribution::PractitionerAccount,
        LoreAttribution::RecoveredFragment,
    ];

    /// The canonical attribution format template (doc "Format" column).
    pub const fn format_hint(self) -> &'static str {
        match self {
            LoreAttribution::DojoInscription => "— Teaching inscription, {dojo/master}",
            LoreAttribution::AncientShrineText => "{partial transcription} — Shrine {location}, {wall}",
            LoreAttribution::RyujinTriadRecord => "Field record, Carrier designation {number}.",
            LoreAttribution::FieldObservation => "Observed by {title/name}, {region/year}.",
            LoreAttribution::PractitionerAccount => "Account of {name}, {role}.",
            LoreAttribution::RecoveredFragment => "Recovered fragment, {site}.",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            LoreAttribution::DojoInscription => "Dojo inscription",
            LoreAttribution::AncientShrineText => "Ancient Shrine text",
            LoreAttribution::RyujinTriadRecord => "Ryujin Triad record",
            LoreAttribution::FieldObservation => "Field observation",
            LoreAttribution::PractitionerAccount => "Practitioner account",
            LoreAttribution::RecoveredFragment => "Recovered fragment",
        }
    }

    /// Whether this source is partial/untranslated text (rendered with ellipses /
    /// gaps rather than clean prose): shrine inscriptions and recovered fragments.
    pub const fn is_fragmentary(self) -> bool {
        matches!(self, LoreAttribution::AncientShrineText | LoreAttribution::RecoveredFragment)
    }
}

/// The item classes the doc routes to each attribution source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemClass {
    /// Bending instruments / technique items → dojo inscriptions.
    BendingInstrument,
    /// Ancient items → shrine text.
    AncientArtifact,
    /// Brine or Storm items → Ryujin Triad records.
    BrineOrStorm,
    /// Materials and creature-sourced items → field observations.
    MaterialOrCreature,
    /// Weapons and armour → practitioner accounts.
    WeaponOrArmour,
    /// Items of unclear origin → recovered fragments.
    UnclearOrigin,
}

/// The attribution source the doc assigns to an item class.
pub const fn attribution_for(class: ItemClass) -> LoreAttribution {
    match class {
        ItemClass::BendingInstrument => LoreAttribution::DojoInscription,
        ItemClass::AncientArtifact => LoreAttribution::AncientShrineText,
        ItemClass::BrineOrStorm => LoreAttribution::RyujinTriadRecord,
        ItemClass::MaterialOrCreature => LoreAttribution::FieldObservation,
        ItemClass::WeaponOrArmour => LoreAttribution::PractitionerAccount,
        ItemClass::UnclearOrigin => LoreAttribution::RecoveredFragment,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_has_a_distinct_template() {
        let mut seen = std::collections::HashSet::new();
        for a in LoreAttribution::ALL {
            assert!(seen.insert(a.format_hint()), "dup template: {}", a.label());
            // Every template names its variable fields with braces.
            assert!(a.format_hint().contains('{'));
        }
        assert_eq!(seen.len(), 6);
    }

    #[test]
    fn item_classes_route_to_the_doc_sources() {
        assert_eq!(attribution_for(ItemClass::BendingInstrument), LoreAttribution::DojoInscription);
        assert_eq!(attribution_for(ItemClass::AncientArtifact), LoreAttribution::AncientShrineText);
        assert_eq!(attribution_for(ItemClass::BrineOrStorm), LoreAttribution::RyujinTriadRecord);
        assert_eq!(attribution_for(ItemClass::MaterialOrCreature), LoreAttribution::FieldObservation);
        assert_eq!(attribution_for(ItemClass::WeaponOrArmour), LoreAttribution::PractitionerAccount);
        assert_eq!(attribution_for(ItemClass::UnclearOrigin), LoreAttribution::RecoveredFragment);
    }

    #[test]
    fn fragmentary_sources() {
        assert!(LoreAttribution::AncientShrineText.is_fragmentary());
        assert!(LoreAttribution::RecoveredFragment.is_fragmentary());
        assert!(!LoreAttribution::DojoInscription.is_fragmentary());
        assert!(!LoreAttribution::PractitionerAccount.is_fragmentary());
    }
}
