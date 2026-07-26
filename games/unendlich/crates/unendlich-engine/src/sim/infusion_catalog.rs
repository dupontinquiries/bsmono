//! `sim::infusion_catalog` — the NAMED infusion catalog from
//! `docs/design_library/06_Crafting_and_Items/crafting_infusion_genetics.docx`
//! §4.1/§4.2: the specific standard + compound infusions, which station performs
//! each, their slot cost, and (for compounds) the sway marks the item must
//! already carry. This is the recipe/eligibility layer that sits ON TOP of the
//! generic slot/relation mechanics in `sim::infusion` (`Infusable`): that core
//! answers "does a slot get consumed and how does the element relate"; THIS one
//! answers "is *this named infusion* legal here, and what does it cost".
//!
//! Pure, deterministic, std-only.

/// The elements an infusion can carry (the doc's infusion table uses Information
/// and Brine alongside the classic five).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfusionElement {
    Earth,
    Water,
    Fire,
    Air,
    Storm,
    Information,
    Brine,
}

/// Where an infusion can be performed (doc §4 station column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfusionStation {
    SeismicForge,
    TidalAnvil,
    VolcanicForge,
    StormTowerAnvil,
    RyujinRelayAnvil,
    ResonanceFont,
    AncientShrineForge,
}

/// Every documented infusion — seven standard (one per element) + three compound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfusionKind {
    SeismicBond,       // Earth
    TidalTemper,       // Water
    FlameDrink,        // Fire
    AltitudeBreath,    // Air
    ArcQuench,         // Storm
    CrystalSoak,       // Information
    MineralSuspension, // Brine
    SeismicTide,  // Earth + Water (compound)
    Stormwright,  // Storm + Air (compound)
    AncientEcho,  // Information + Earth (compound, Ancient Shrine)
}

/// Why an infusion couldn't be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfusionError {
    NotEnoughCapacity,
    WrongStation,
    AlreadyInfused,
    MissingSwayMarks,
}

impl InfusionKind {
    pub fn slot_cost(self) -> u8 {
        if self.is_compound() { 2 } else { 1 }
    }

    pub fn is_compound(self) -> bool {
        matches!(self, InfusionKind::SeismicTide | InfusionKind::Stormwright | InfusionKind::AncientEcho)
    }

    pub fn element(self) -> InfusionElement {
        use InfusionElement::*;
        use InfusionKind::*;
        match self {
            SeismicBond => Earth,
            TidalTemper => Water,
            FlameDrink => Fire,
            AltitudeBreath => Air,
            ArcQuench => Storm,
            CrystalSoak => Information,
            MineralSuspension => Brine,
            SeismicTide => Earth,
            Stormwright => Storm,
            AncientEcho => Information,
        }
    }

    pub fn required_marks(self) -> &'static [InfusionElement] {
        use InfusionElement::*;
        use InfusionKind::*;
        match self {
            SeismicTide => &[Earth, Water],
            Stormwright => &[Storm, Air],
            AncientEcho => &[Information, Earth],
            _ => &[],
        }
    }

    pub fn station_allows(self, station: InfusionStation) -> bool {
        use InfusionKind::*;
        use InfusionStation::*;
        if self.is_compound() && station == AncientShrineForge {
            return true; // the shrine accesses any compound solo (doc §4.2)
        }
        match self {
            SeismicBond => matches!(station, SeismicForge | ResonanceFont),
            TidalTemper => matches!(station, TidalAnvil),
            FlameDrink => matches!(station, VolcanicForge),
            AltitudeBreath => matches!(station, StormTowerAnvil),
            ArcQuench => matches!(station, StormTowerAnvil | RyujinRelayAnvil),
            CrystalSoak => matches!(station, ResonanceFont),
            MineralSuspension => matches!(station, TidalAnvil | SeismicForge),
            SeismicTide => false,
            Stormwright => matches!(station, StormTowerAnvil),
            AncientEcho => false,
        }
    }
}

/// Attempt an infusion. Returns the NEW remaining capacity on success, or an
/// [`InfusionError`]. `applied` = element list of infusions already on the item;
/// `sway_marks` = the elements the item carries sway marks for.
pub fn try_infuse(
    capacity: u8,
    applied: &[InfusionElement],
    sway_marks: &[InfusionElement],
    kind: InfusionKind,
    station: InfusionStation,
) -> Result<u8, InfusionError> {
    if !kind.station_allows(station) {
        return Err(InfusionError::WrongStation);
    }
    let cost = kind.slot_cost();
    if capacity < cost {
        return Err(InfusionError::NotEnoughCapacity);
    }
    if !kind.is_compound() && applied.contains(&kind.element()) {
        return Err(InfusionError::AlreadyInfused);
    }
    for req in kind.required_marks() {
        if !sway_marks.contains(req) {
            return Err(InfusionError::MissingSwayMarks);
        }
    }
    Ok(capacity - cost)
}

#[cfg(test)]
mod tests {
    use super::InfusionElement::*;
    use super::InfusionKind::*;
    use super::InfusionStation::*;
    use super::*;

    #[test]
    fn standard_costs_one_compound_costs_two() {
        assert_eq!(FlameDrink.slot_cost(), 1);
        assert_eq!(SeismicTide.slot_cost(), 2);
        assert!(Stormwright.is_compound() && !FlameDrink.is_compound());
    }
    #[test]
    fn standard_succeeds_at_its_station() {
        assert_eq!(try_infuse(3, &[], &[], FlameDrink, VolcanicForge), Ok(2));
    }
    #[test]
    fn wrong_station_rejected() {
        assert_eq!(try_infuse(3, &[], &[], FlameDrink, TidalAnvil), Err(InfusionError::WrongStation));
    }
    #[test]
    fn full_item_rejected() {
        assert_eq!(try_infuse(0, &[], &[], FlameDrink, VolcanicForge), Err(InfusionError::NotEnoughCapacity));
        assert_eq!(try_infuse(1, &[], &[Earth, Water], SeismicTide, AncientShrineForge), Err(InfusionError::NotEnoughCapacity));
    }
    #[test]
    fn one_per_element_standard() {
        assert_eq!(try_infuse(3, &[Fire], &[], FlameDrink, VolcanicForge), Err(InfusionError::AlreadyInfused));
    }
    #[test]
    fn compound_requires_both_marks() {
        assert_eq!(try_infuse(4, &[], &[Earth], SeismicTide, AncientShrineForge), Err(InfusionError::MissingSwayMarks));
        assert_eq!(try_infuse(4, &[], &[Earth, Water], SeismicTide, AncientShrineForge), Ok(2));
    }
    #[test]
    fn stormwright_tower_with_marks() {
        assert_eq!(try_infuse(3, &[], &[Storm, Air], Stormwright, StormTowerAnvil), Ok(1));
        assert_eq!(try_infuse(3, &[], &[Storm, Air], Stormwright, VolcanicForge), Err(InfusionError::WrongStation));
    }
    #[test]
    fn ancient_echo_shrine_only() {
        assert_eq!(try_infuse(4, &[], &[Information, Earth], AncientEcho, AncientShrineForge), Ok(2));
        assert_eq!(try_infuse(4, &[], &[Information, Earth], AncientEcho, SeismicForge), Err(InfusionError::WrongStation));
    }
}
