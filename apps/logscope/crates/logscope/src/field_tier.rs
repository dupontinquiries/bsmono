//! field_tier — logscope as a **field-repair tool**: graceful degradation across the *whole* app
//! under resource constraints. The north-star scenario is the desert camper — broken hardware in
//! hand, little power, no cloud — where the tool has to stay useful all the way down to a dying
//! battery. This extends `model_selector`'s RAM-aware *model* tiers to the full feature set, so
//! the honest floor is **pure-heuristic diagnosis** (format sniff + severity + shorten/probe — all
//! offline, all cheap, no model load). Same "step-up, not guarantee" rule the audio stack follows,
//! applied to compute + power.
//!
//! Pure + lean-buildable: the render/inference code reads [`capabilities`] instead of hard-coding
//! which heavy features to run.

use crate::model_selector::ModelTier;

/// What logscope can sense about its host right now. Unknown signals default to the *generous*
/// end, so a capable machine that just doesn't report battery isn't needlessly crippled.
#[derive(Debug, Clone, Copy)]
pub struct ResourceState {
    /// Free RAM as a fraction of total (0..1). `model_selector` already reads `/proc/meminfo`.
    pub ram_free_frac: f32,
    /// Running on battery (not plugged in)?
    pub on_battery: bool,
    /// Battery charge fraction (0..1); 1.0 when plugged or unknown.
    pub battery_frac: f32,
    /// A manual floor — "make it last" — can force a lower tier but never a higher one.
    pub forced_floor: Option<FieldTier>,
}

impl Default for ResourceState {
    fn default() -> Self {
        ResourceState { ram_free_frac: 1.0, on_battery: false, battery_frac: 1.0, forced_floor: None }
    }
}

/// The operating tier — a graceful ladder, not a switch. Ordered so `min` clamps *down*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FieldTier {
    /// Critical: heuristics only. Triage the hardware, don't think about it. Works on fumes.
    Survival,
    /// Constrained: trim the expensive bits (scene, big model, whisper), keep the workflow.
    Frugal,
    /// Plenty of headroom (plugged in, or ample battery + RAM): the full experience.
    Full,
}

/// What a tier turns on. Read this instead of hard-coding heavy features.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub tier: FieldTier,
    /// Load an LLM at all? Survival runs with none (pure heuristics).
    pub use_llm: bool,
    /// Preferred model size when an LLM is used.
    pub model_tier: ModelTier,
    /// The Chat 3D scene — a battery/thermal hog, so Full only.
    pub mind_palace: bool,
    /// Heavy speech-to-text (whisper). Below Full, lean on the learned fastpath instead.
    pub whisper: bool,
    /// Animated backgrounds / live effects — off below Full.
    pub background_fx: bool,
}

/// Decide the tier from sensed resources. Thresholds are deliberately conservative — better to
/// under-promise and keep running than to load a 13B model and brown out.
pub fn assess(rs: &ResourceState) -> FieldTier {
    // hard floors first
    let natural = if rs.battery_frac < 0.15 || rs.ram_free_frac < 0.10 {
        FieldTier::Survival
    } else if rs.on_battery || rs.ram_free_frac < 0.25 || rs.battery_frac < 0.35 {
        FieldTier::Frugal
    } else {
        FieldTier::Full
    };
    // a manual floor can only pull DOWN
    match rs.forced_floor {
        Some(f) => natural.min(f),
        None => natural,
    }
}

/// The capability set for a tier.
pub fn capabilities(tier: FieldTier) -> Capabilities {
    match tier {
        FieldTier::Full => Capabilities {
            tier,
            use_llm: true,
            model_tier: ModelTier::Large,
            mind_palace: true,
            whisper: true,
            background_fx: true,
        },
        FieldTier::Frugal => Capabilities {
            tier,
            use_llm: true,
            model_tier: ModelTier::Small,
            mind_palace: false,
            whisper: false, // fastpath-first
            background_fx: false,
        },
        FieldTier::Survival => Capabilities {
            tier,
            use_llm: false, // no model load — heuristic diagnosis only
            model_tier: ModelTier::Small,
            mind_palace: false,
            whisper: false,
            background_fx: false,
        },
    }
}

/// Convenience: sense → tier → capabilities in one call.
pub fn assess_capabilities(rs: &ResourceState) -> Capabilities {
    capabilities(assess(rs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugged_in_ample_is_full() {
        let c = assess_capabilities(&ResourceState::default());
        assert_eq!(c.tier, FieldTier::Full);
        assert!(c.use_llm && c.mind_palace && c.whisper && c.background_fx);
        assert_eq!(c.model_tier, ModelTier::Large);
    }

    #[test]
    fn on_battery_trims_to_frugal() {
        let rs = ResourceState { on_battery: true, battery_frac: 0.8, ..Default::default() };
        let c = assess_capabilities(&rs);
        assert_eq!(c.tier, FieldTier::Frugal);
        assert!(c.use_llm, "still thinks, just smaller");
        assert_eq!(c.model_tier, ModelTier::Small);
        assert!(!c.mind_palace && !c.whisper && !c.background_fx, "drop the hogs");
    }

    #[test]
    fn dying_battery_or_low_ram_is_survival_no_llm() {
        let dying = ResourceState { on_battery: true, battery_frac: 0.10, ..Default::default() };
        assert_eq!(assess(&dying), FieldTier::Survival);
        let low_ram = ResourceState { ram_free_frac: 0.05, ..Default::default() };
        let c = assess_capabilities(&low_ram);
        assert_eq!(c.tier, FieldTier::Survival);
        assert!(!c.use_llm, "no model load on fumes — heuristic diagnosis only");
    }

    #[test]
    fn forced_floor_only_pulls_down() {
        // a full machine, but the user says "make it last" → Survival
        let rs = ResourceState { forced_floor: Some(FieldTier::Survival), ..Default::default() };
        assert_eq!(assess(&rs), FieldTier::Survival);
        // forcing "Full" on a dying battery can't override the hard floor
        let dying = ResourceState {
            on_battery: true,
            battery_frac: 0.05,
            forced_floor: Some(FieldTier::Full),
            ..Default::default()
        };
        assert_eq!(assess(&dying), FieldTier::Survival, "can't force UP past the hardware reality");
    }
}
