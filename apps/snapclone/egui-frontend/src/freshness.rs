//! freshness — how confident the wayfinder should look.
//!
//! `snapclone-location`'s `freshness` module already answers this: it computes a
//! `Confidence` from a fix's age and accuracy, maps it to a `PresentationTier`,
//! and even supplies an `opacity` and a `last_seen_label`. Three of those four
//! are view functions, and the port had no consumer for any of them — the
//! wayfinder drew arrows without asking whether the position behind them was
//! worth pointing at.
//!
//! Used as a **dependency**, and `tier_for` delegates rather than re-deriving.
//! The sixth duplication in this port was a `pending()` that already existed one
//! crate over, and the failure mode there is the one here too: a second
//! derivation of "is this fix good enough" would eventually disagree with the
//! opacity drawn beside it.
//!
//! ## Four confidences collapse to three drawings, deliberately
//!
//! `Live` and `Recent` both get an arrow; the difference is opacity and an
//! approximate marker. `Stale` still gets a faded arrow because a rough
//! direction is genuinely useful when you are looking for someone. `LastKnown`
//! gets **no arrow at all** — the crate calls it *"not a navigation target"*,
//! and an arrow implies a direction to walk in. Drawing a solid arrow from a
//! twenty-minute-old fix is how someone ends up crossing a field toward a
//! person who left.
//!
//! ## The label is what replaces the arrow, not what accompanies it
//!
//! When there is no arrow the user still needs to know why, and "last seen 20m
//! ago" answers it. Showing that label under a live arrow would be noise; the
//! arrow already says the position is current.

use snapclone_location::freshness::{self, Confidence, PresentationTier};
// Re-exported so a shell can hold a policy without importing the location
// crate itself — this module IS the frontend's freshness surface.
pub use snapclone_location::freshness::FreshnessPolicy;

/// What the wayfinder should draw for one node.
#[derive(Debug, Clone, PartialEq)]
pub struct NodePresentation {
    pub tier: PresentationTier,
    /// 0..=1, from the crate's own scale.
    pub opacity: f32,
    /// Shown only when there is no arrow.
    pub label: Option<String>,
    /// Whether to mark the distance as approximate.
    pub approximate: bool,
}

impl NodePresentation {
    /// Should an arrow be drawn at all?
    pub fn has_arrow(&self) -> bool { self.tier != PresentationTier::LastSeenLabel }

    /// Is this position worth navigating toward?
    ///
    /// Same question as `has_arrow` today, and kept separate on purpose: a
    /// caller deciding whether to route, or to announce a bearing by voice, is
    /// asking about the DATA, while `has_arrow` asks about the drawing. Fusing
    /// them means the day a tier gains a dotted arrow, voice starts announcing
    /// bearings it should not.
    pub fn is_navigable(&self) -> bool {
        self.tier != PresentationTier::LastSeenLabel
    }
}

/// Decide what to draw for a fix of a given age and accuracy.
pub fn presentation(policy: &FreshnessPolicy, age_secs: i64, accuracy_m: f32)
    -> NodePresentation {
    let conf = freshness::confidence(policy, age_secs, accuracy_m);
    let tier = freshness::presentation(conf);
    NodePresentation {
        tier,
        opacity: freshness::opacity(conf),
        // The label REPLACES the arrow rather than accompanying it — under a
        // live arrow it would be noise, since the arrow already says the
        // position is current.
        label: match tier {
            PresentationTier::LastSeenLabel => Some(freshness::last_seen_label(age_secs)),
            _ => None,
        },
        approximate: conf != Confidence::Live,
    }
}

/// Draw a node's bearing, faded and marked according to its freshness.
pub fn node_ui(ui: &mut egui::Ui, name: &str, bearing_deg: f32, distance_m: f32,
               p: &NodePresentation, c: &uikit_egui::PaletteColors) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(24.0, 24.0),
                                               egui::Sense::hover());
        if p.has_arrow() {
            let a = bearing_deg.to_radians();
            let dir = egui::vec2(a.sin(), -a.cos());
            let col = c.accent.gamma_multiply(p.opacity);
            ui.painter().arrow(rect.center() - dir * 9.0, dir * 18.0,
                               egui::Stroke::new(2.0_f32, col));
        }
        ui.vertical(|ui| {
            ui.colored_label(c.text.gamma_multiply(p.opacity.max(0.5)), name);
            match &p.label {
                // No arrow: the label is the answer to "why not".
                Some(l) => ui.colored_label(c.text_weak,
                    egui::RichText::new(l).small()),
                None => {
                    let d = if p.approximate {
                        format!("~{:.0} m", distance_m)
                    } else {
                        format!("{:.0} m", distance_m)
                    };
                    ui.colored_label(c.text_weak, egui::RichText::new(d).small())
                }
            };
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> FreshnessPolicy { FreshnessPolicy::default() }

    /// Walk outward in age until the tier changes, so the tests describe
    /// BEHAVIOUR rather than pinning the crate's tunable thresholds — those are
    /// documented as tunable, and a test that froze them would fail the day
    /// someone tuned them without anything being wrong.
    fn first_age_with(tier: PresentationTier, accuracy_m: f32) -> Option<i64> {
        (0..100_000).step_by(5)
            .map(|a| a as i64)
            .find(|a| presentation(&policy(), *a, accuracy_m).tier == tier)
    }

    #[test]
    fn a_fresh_accurate_fix_gets_a_solid_arrow() {
        let p = presentation(&policy(), 0, 5.0);
        assert_eq!(p.tier, PresentationTier::SolidArrow);
        assert!(p.has_arrow());
        assert!(p.is_navigable());
    }

    #[test]
    fn a_fresh_fix_is_not_marked_approximate() {
        // The "~" is a claim about precision; putting it on a live fix would
        // make the marker meaningless everywhere else.
        assert!(!presentation(&policy(), 0, 5.0).approximate);
    }

    #[test]
    fn a_stale_fix_still_gets_an_arrow_because_direction_helps() {
        // A rough bearing is genuinely useful when you are looking for someone,
        // which is why Stale is a FADED arrow rather than no arrow.
        let age = first_age_with(PresentationTier::FadedArrow, 5.0)
            .expect("some age is faded");
        let p = presentation(&policy(), age, 5.0);
        assert!(p.has_arrow());
        assert!(p.approximate, "but marked as approximate");
    }

    #[test]
    fn a_last_known_fix_gets_no_arrow_at_all() {
        // The crate calls it "not a navigation target", and an arrow implies a
        // direction to walk in — that is how someone crosses a field toward a
        // person who left.
        let age = first_age_with(PresentationTier::LastSeenLabel, 5.0)
            .expect("some age is last-known");
        let p = presentation(&policy(), age, 5.0);
        assert!(!p.has_arrow());
        assert!(!p.is_navigable());
    }

    #[test]
    fn the_label_appears_only_when_the_arrow_does_not() {
        // Under a live arrow it would be noise: the arrow already says the
        // position is current.
        assert!(presentation(&policy(), 0, 5.0).label.is_none());
        let age = first_age_with(PresentationTier::LastSeenLabel, 5.0).unwrap();
        let p = presentation(&policy(), age, 5.0);
        assert!(p.label.is_some());
        assert!(!p.label.unwrap().is_empty(), "and it says something");
    }

    #[test]
    fn opacity_never_reaches_zero_for_a_drawn_arrow() {
        // An invisible arrow is worse than no arrow: the user sees a gap where
        // a node should be and cannot tell whether it is missing or faint.
        for age in [0i64, 60, 600, 3_600] {
            let p = presentation(&policy(), age, 5.0);
            if p.has_arrow() { assert!(p.opacity > 0.0, "age {age}"); }
        }
    }

    #[test]
    fn confidence_falls_monotonically_with_age() {
        // A fix cannot become more trustworthy by getting older, and a
        // non-monotonic policy would make the display flicker between tiers as
        // the clock ticks.
        let mut last = 1.1f32;
        for age in (0..7_200).step_by(60) {
            let o = presentation(&policy(), age as i64, 5.0).opacity;
            assert!(o <= last + 1e-6, "opacity rose at age {age}: {o} > {last}");
            last = o;
        }
    }

    #[test]
    fn a_coarse_fix_is_downgraded_even_when_it_is_new() {
        // Accuracy matters as much as age: a fix taken this second with a
        // 2km radius does not deserve a solid arrow.
        let fresh_precise = presentation(&policy(), 0, 5.0);
        let fresh_coarse = presentation(&policy(), 0, 2_000.0);
        assert_eq!(fresh_precise.tier, PresentationTier::SolidArrow);
        assert!(fresh_coarse.opacity <= fresh_precise.opacity);
    }

    #[test]
    fn drawing_and_navigating_are_asked_separately() {
        // Fused today and kept apart on purpose: the day a tier gains a dotted
        // arrow, voice must not start announcing bearings it should not.
        let p = presentation(&policy(), 0, 5.0);
        assert_eq!(p.has_arrow(), p.is_navigable());
    }

    #[test]
    fn a_negative_age_is_handled_rather_than_trusted() {
        // Clock skew between peers is normal, and a fix from the "future" must
        // not render as more live than live.
        let p = presentation(&policy(), -600, 5.0);
        assert!(p.opacity <= 1.0);
        assert!(p.opacity > 0.0);
    }
}
