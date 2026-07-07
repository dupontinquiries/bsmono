//! sim::block_placement — can this block go here?
//!
//! The engine has `world/flora_placement` for plants and `sim/attach_frame` for
//! rigging, but no general **building** rule: when a player aims at a face and
//! clicks, may the held block occupy the target cell? This is that decision,
//! pure. It deliberately takes *derived flags* about the target cell and its
//! neighbourhood — not `BlockId` — so it stays std-only and unit-testable; the
//! caller reads the world once (via `BlockId::is_replaceable`, `is_solid`,
//! `is_fluid`, reach, entity overlap, claims) and hands the booleans in. One
//! ordered rule set decides `Placed` or a specific [`PlaceDenied`] reason, so
//! the UI can show *why* a placement failed. Deterministic, panic-free.

/// Facts about the target cell and its surroundings, gathered by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaceContext {
    /// The target cell currently accepts a block (air, tall grass, snow layer,
    /// or a replaceable fluid the caller decided is displaceable).
    pub target_replaceable: bool,
    /// The target cell holds a fluid (water/lava).
    pub target_is_fluid: bool,
    /// The cell directly below the target is solid (support underneath).
    pub below_solid: bool,
    /// The face being placed against is a solid surface (for attached blocks).
    pub against_solid: bool,
    /// A player/creature volume overlaps the target cell — can't build inside.
    pub occupied_by_entity: bool,
    /// The target is within the player's build reach.
    pub within_reach: bool,
    /// The target is protected (spawn protection, another player's claim).
    pub protected: bool,
}

impl Default for PlaceContext {
    /// A plain buildable cell: reachable, empty, unclaimed, solid ground below.
    fn default() -> PlaceContext {
        PlaceContext {
            target_replaceable: true,
            target_is_fluid: false,
            below_solid: true,
            against_solid: true,
            occupied_by_entity: false,
            within_reach: true,
            protected: false,
        }
    }
}

/// Placement requirements of the specific block being placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BlockPlaceRules {
    /// Needs a solid cell directly below (torch, sapling, snow, gravel-on-ground).
    pub requires_support: bool,
    /// Needs a solid face to attach to (ladder, lever, wall torch).
    pub requires_attachment: bool,
    /// May displace a fluid it's placed into (most solid blocks can; a torch can't).
    pub can_replace_fluid: bool,
}

/// Why a placement was refused. Ordered from most to least fundamental so the
/// UI/feedback can key off the single returned reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceDenied {
    OutOfReach,
    Protected,
    Occupied,
    IntoFluid,
    NotReplaceable,
    NoSupport,
    NoAttachment,
}

/// The outcome of a placement check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceResult {
    Placed,
    Blocked(PlaceDenied),
}

impl PlaceResult {
    pub fn is_placed(self) -> bool {
        matches!(self, PlaceResult::Placed)
    }
    pub fn denied(self) -> Option<PlaceDenied> {
        match self {
            PlaceResult::Blocked(d) => Some(d),
            PlaceResult::Placed => None,
        }
    }
}

/// Decide whether `rules`' block may be placed in the cell described by `ctx`.
/// Checks run most-fundamental first, so the returned reason is the primary
/// blocker.
pub fn evaluate(ctx: &PlaceContext, rules: &BlockPlaceRules) -> PlaceResult {
    use PlaceDenied::*;
    if !ctx.within_reach {
        return PlaceResult::Blocked(OutOfReach);
    }
    if ctx.protected {
        return PlaceResult::Blocked(Protected);
    }
    if ctx.occupied_by_entity {
        return PlaceResult::Blocked(Occupied);
    }
    // A fluid cell counts as "replaceable" only for blocks allowed to displace it.
    if ctx.target_is_fluid && !rules.can_replace_fluid {
        return PlaceResult::Blocked(IntoFluid);
    }
    if !ctx.target_replaceable && !ctx.target_is_fluid {
        return PlaceResult::Blocked(NotReplaceable);
    }
    if rules.requires_support && !ctx.below_solid {
        return PlaceResult::Blocked(NoSupport);
    }
    if rules.requires_attachment && !ctx.against_solid {
        return PlaceResult::Blocked(NoAttachment);
    }
    PlaceResult::Placed
}

/// Convenience: just "can I?" without the reason.
pub fn can_place(ctx: &PlaceContext, rules: &BlockPlaceRules) -> bool {
    evaluate(ctx, rules).is_placed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_solid_block_places_on_open_ground() {
        assert_eq!(evaluate(&PlaceContext::default(), &BlockPlaceRules::default()), PlaceResult::Placed);
    }

    #[test]
    fn out_of_reach_is_the_first_refusal() {
        let ctx = PlaceContext { within_reach: false, protected: true, occupied_by_entity: true, ..Default::default() };
        assert_eq!(evaluate(&ctx, &BlockPlaceRules::default()), PlaceResult::Blocked(PlaceDenied::OutOfReach));
    }

    #[test]
    fn protection_outranks_occupancy_and_the_rest() {
        let ctx = PlaceContext { protected: true, occupied_by_entity: true, ..Default::default() };
        assert_eq!(evaluate(&ctx, &BlockPlaceRules::default()), PlaceResult::Blocked(PlaceDenied::Protected));
    }

    #[test]
    fn cannot_build_inside_an_entity() {
        let ctx = PlaceContext { occupied_by_entity: true, ..Default::default() };
        assert_eq!(evaluate(&ctx, &BlockPlaceRules::default()), PlaceResult::Blocked(PlaceDenied::Occupied));
    }

    #[test]
    fn a_solid_block_may_displace_fluid_but_a_torch_may_not() {
        let ctx = PlaceContext { target_is_fluid: true, target_replaceable: false, ..Default::default() };
        // solid block: allowed to replace fluid
        let solid = BlockPlaceRules { can_replace_fluid: true, ..Default::default() };
        assert_eq!(evaluate(&ctx, &solid), PlaceResult::Placed);
        // torch: cannot go into fluid
        let torch = BlockPlaceRules { can_replace_fluid: false, requires_attachment: true, ..Default::default() };
        assert_eq!(evaluate(&ctx, &torch), PlaceResult::Blocked(PlaceDenied::IntoFluid));
    }

    #[test]
    fn a_full_cell_rejects_placement() {
        let ctx = PlaceContext { target_replaceable: false, ..Default::default() };
        assert_eq!(evaluate(&ctx, &BlockPlaceRules::default()), PlaceResult::Blocked(PlaceDenied::NotReplaceable));
    }

    #[test]
    fn a_support_needing_block_falls_without_ground() {
        let ctx = PlaceContext { below_solid: false, ..Default::default() };
        let sapling = BlockPlaceRules { requires_support: true, ..Default::default() };
        assert_eq!(evaluate(&ctx, &sapling), PlaceResult::Blocked(PlaceDenied::NoSupport));
        // ...but a freestanding block is fine floating (no support requirement)
        assert_eq!(evaluate(&ctx, &BlockPlaceRules::default()), PlaceResult::Placed);
    }

    #[test]
    fn an_attaching_block_needs_a_solid_face() {
        let ctx = PlaceContext { against_solid: false, ..Default::default() };
        let ladder = BlockPlaceRules { requires_attachment: true, ..Default::default() };
        assert_eq!(evaluate(&ctx, &ladder), PlaceResult::Blocked(PlaceDenied::NoAttachment));
    }

    #[test]
    fn can_place_matches_evaluate() {
        let ctx = PlaceContext::default();
        let rules = BlockPlaceRules::default();
        assert_eq!(can_place(&ctx, &rules), evaluate(&ctx, &rules).is_placed());
        let bad = PlaceContext { within_reach: false, ..Default::default() };
        assert!(!can_place(&bad, &rules));
    }

    #[test]
    fn evaluation_is_deterministic() {
        let ctx = PlaceContext { below_solid: false, target_is_fluid: true, ..Default::default() };
        let rules = BlockPlaceRules { requires_support: true, can_replace_fluid: false, ..Default::default() };
        assert_eq!(evaluate(&ctx, &rules), evaluate(&ctx, &rules));
    }
}
