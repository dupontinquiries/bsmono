//! ribbon — `SmartRibbon`. Tabs on the left, the active tab's actions on the
//! right, in one bottom-docked bar.
//!
//! The Qt header is explicit about why this exists next to `SegmentedTabStrip`
//! rather than replacing it: the strip is top-docked, full-width, equal
//! segments. The ribbon is the bottom-docked variant that ALSO carries
//! contextual actions, so its segments sit at their natural width on the left
//! to leave the right side room. Same visual language, different job.
//!
//! Bottom-docked is the whole point on a phone. The actions you press most are
//! the ones that should be under your thumb, and a top bar puts them at the
//! furthest reachable point on the screen.
//!
//! ## Actions belong to a tab, not to the bar
//!
//! On Contacts→Pending the ribbon offers accept-all and clear; on
//! Mailbox→Sent it offers retry-failed. Switching tabs swaps the whole set.
//! The failure this prevents is a stale action surviving a tab change and
//! firing against content it was never meant for — "clear" from the Pending tab
//! pressed while looking at All.
//!
//! ## Enabled is a question, asked again
//!
//! Qt stores a predicate per action and re-runs it on `refreshActions()` after
//! state changes — snaps arriving, a contact accepted. Here the same thing is a
//! `refresh` call taking a closure over action ids, because a stored closure
//! costs the type its `Debug`, `Clone` and `PartialEq`, and those are what make
//! the rest of this testable. The important half is unchanged: enablement is
//! re-derived from state, never remembered.
//!
//! And a disabled action does not fire. Qt gets that from `setEnabled(false)`
//! on the button; here `trigger` returns `None`, so a caller that reaches the
//! action by any other route — a keyboard shortcut, a test — hits the same
//! wall the pointer does.

use std::collections::HashMap;

/// The width below which action labels are dropped and only icons remain.
///
/// Below this the labels elide to the point of being unreadable, and an
/// unreadable label is worse than an icon alone: it takes the space AND says
/// nothing.
pub const LABEL_HIDE_WIDTH: f32 = 380.0;

/// A contextual action on the right of the ribbon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// Stable id, used to re-evaluate enablement and to report the trigger.
    pub id: String,
    /// SvgIcon name.
    pub icon: String,
    /// Short caption, hidden on narrow bars.
    pub label: String,
    pub enabled: bool,
}

impl Action {
    /// Enabled by default: an action registered without a predicate is always
    /// on, matching Qt's `a.enabled ? a.enabled() : true`.
    pub fn new(id: &str, icon: &str, label: &str) -> Self {
        Action { id: id.into(), icon: icon.into(), label: label.into(), enabled: true }
    }
}

/// One tab on the left half.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RibbonTab {
    pub label: String,
    pub badge: Option<u32>,
}

/// The ribbon.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmartRibbon {
    tabs: Vec<RibbonTab>,
    actions: HashMap<usize, Vec<Action>>,
    current: usize,
}

impl SmartRibbon {
    pub fn new(labels: &[&str]) -> Self {
        SmartRibbon {
            tabs: labels.iter().map(|l| RibbonTab { label: (*l).into(), badge: None }).collect(),
            actions: HashMap::new(),
            current: 0,
        }
    }

    pub fn tabs(&self) -> &[RibbonTab] { &self.tabs }
    pub fn current(&self) -> usize { self.current }

    /// Select a tab. Out of range is refused, as in the strip — a caller asking
    /// for a tab that is not there has a bug, and landing somewhere plausible
    /// hides it.
    pub fn set_current(&mut self, i: usize) -> bool {
        if i >= self.tabs.len() { return false; }
        self.current = i;
        true
    }

    /// `setTabActions`.
    pub fn set_tab_actions(&mut self, tab: usize, actions: Vec<Action>) {
        if tab < self.tabs.len() { self.actions.insert(tab, actions); }
    }

    /// The actions for the CURRENT tab, and only those. A tab with none shows
    /// none rather than inheriting its neighbour's.
    pub fn visible_actions(&self) -> &[Action] {
        self.actions.get(&self.current).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// `refreshActions`: re-ask the predicate for every action on every tab.
    ///
    /// All tabs, not just the visible one, so switching to a tab shows state
    /// that is already current rather than one frame of stale enablement.
    pub fn refresh(&mut self, mut enabled: impl FnMut(&str) -> bool) {
        for list in self.actions.values_mut() {
            for a in list.iter_mut() { a.enabled = enabled(&a.id); }
        }
    }

    /// Fire an action on the current tab. `None` when it is absent or disabled.
    pub fn trigger(&self, id: &str) -> Option<&Action> {
        self.visible_actions().iter().find(|a| a.id == id && a.enabled)
    }

    /// `setBadge`; zero removes the pill.
    pub fn set_badge(&mut self, tab: usize, count: u32) {
        if let Some(t) = self.tabs.get_mut(tab) {
            t.badge = if count == 0 { None } else { Some(count) };
        }
    }

    /// Capped like the strip's, so the two bars cannot disagree about what a
    /// large count looks like.
    pub fn badge_text(&self, tab: usize) -> Option<String> {
        self.tabs.get(tab).and_then(|t| t.badge)
            .map(|n| if n > 99 { "99+".into() } else { n.to_string() })
    }

    /// Do action labels fit, or is it icons only?
    pub fn show_labels(&self, width: f32) -> bool { width >= LABEL_HIDE_WIDTH }

    /// Natural segment widths — proportional to label length rather than equal,
    /// which is what leaves the right half room for the actions.
    pub fn tab_widths(&self, left_half: f32) -> Vec<f32> {
        let total: usize = self.tabs.iter().map(|t| t.label.chars().count().max(1)).sum();
        if total == 0 { return vec![]; }
        self.tabs.iter()
            .map(|t| left_half * t.label.chars().count().max(1) as f32 / total as f32)
            .collect()
    }
}

/// What the bar produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RibbonEvent {
    TabSelected(usize),
    /// The id of an action that was both present and enabled.
    Triggered(String),
}

/// Draw the ribbon.
pub fn ribbon_ui(ui: &mut egui::Ui, r: &mut SmartRibbon,
                 c: &uikit_egui::PaletteColors) -> Option<RibbonEvent> {
    let mut out = None;
    let width = ui.available_width();
    let labels = r.show_labels(width);
    let widths = r.tab_widths(width * 0.5);
    ui.horizontal(|ui| {
        for (i, t) in r.tabs().iter().enumerate() {
            let sel = i == r.current();
            let text = match r.badge_text(i) {
                Some(b) => format!("{}  {}", t.label, b),
                None => t.label.clone(),
            };
            let w = widths.get(i).copied().unwrap_or(60.0);
            if ui.add_sized([w, 32.0], egui::SelectableLabel::new(sel,
                egui::RichText::new(text).color(if sel { c.text } else { c.text_weak })))
                .clicked() {
                out = Some(RibbonEvent::TabSelected(i));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for a in r.visible_actions() {
                let caption = if labels { format!("{} {}", a.icon, a.label) } else { a.icon.clone() };
                // Disabled rather than hidden: an action that disappears reads
                // as a missing feature, one that is greyed reads as a missing
                // precondition — and the precondition is usually visible on the
                // same screen.
                if ui.add_enabled(a.enabled, egui::Button::new(caption)).clicked() {
                    out = Some(RibbonEvent::Triggered(a.id.clone()));
                }
            }
        });
    });
    if let Some(RibbonEvent::TabSelected(i)) = &out { r.set_current(*i); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ribbon() -> SmartRibbon {
        let mut r = SmartRibbon::new(&["All", "Unread", "Sent"]);
        r.set_tab_actions(1, vec![Action::new("read", "check", "Mark all read")]);
        r.set_tab_actions(2, vec![Action::new("retry", "refresh", "Retry failed")]);
        r
    }

    #[test]
    fn actions_belong_to_a_tab_and_swap_with_it() {
        // The failure this prevents: "clear" from the Pending tab surviving a
        // tab change and firing against content it was never meant for.
        let mut r = ribbon();
        assert!(r.visible_actions().is_empty(), "the All tab registered none");
        r.set_current(1);
        assert_eq!(r.visible_actions().len(), 1);
        assert_eq!(r.visible_actions()[0].id, "read");
        r.set_current(2);
        assert_eq!(r.visible_actions()[0].id, "retry");
    }

    #[test]
    fn a_tab_with_no_actions_shows_none_rather_than_its_neighbours() {
        let mut r = ribbon();
        r.set_current(1);
        r.set_current(0);
        assert!(r.visible_actions().is_empty());
    }

    #[test]
    fn an_action_registered_without_a_predicate_is_enabled() {
        // Qt's `a.enabled ? a.enabled() : true`.
        assert!(Action::new("x", "i", "X").enabled);
    }

    #[test]
    fn a_disabled_action_does_not_fire_by_any_route() {
        // Qt gets this from setEnabled(false) on the button. Putting it in
        // trigger() means a keyboard shortcut or a test hits the same wall the
        // pointer does.
        let mut r = ribbon();
        r.set_current(1);
        assert!(r.trigger("read").is_some());
        r.refresh(|_| false);
        assert!(r.trigger("read").is_none());
    }

    #[test]
    fn an_action_from_another_tab_cannot_be_triggered() {
        let mut r = ribbon();
        r.set_current(1);
        assert!(r.trigger("retry").is_none(), "that belongs to Sent");
    }

    #[test]
    fn an_unknown_action_id_triggers_nothing() {
        let mut r = ribbon();
        r.set_current(1);
        assert!(r.trigger("nonsense").is_none());
    }

    #[test]
    fn refresh_re_evaluates_every_tab_not_only_the_visible_one() {
        // Otherwise switching to a tab shows one frame of stale enablement.
        let mut r = ribbon();
        r.refresh(|id| id == "retry");
        r.set_current(1);
        assert!(r.trigger("read").is_none());
        r.set_current(2);
        assert!(r.trigger("retry").is_some(), "already correct on arrival");
    }

    #[test]
    fn enablement_is_re_derived_and_never_remembered() {
        let mut r = ribbon();
        r.set_current(1);
        r.refresh(|_| false);
        assert!(r.trigger("read").is_none());
        r.refresh(|_| true);
        assert!(r.trigger("read").is_some());
    }

    #[test]
    fn selecting_out_of_range_is_refused_like_the_strip() {
        let mut r = ribbon();
        assert!(!r.set_current(9));
        assert_eq!(r.current(), 0);
    }

    #[test]
    fn registering_actions_for_a_tab_that_does_not_exist_is_ignored() {
        let mut r = ribbon();
        r.set_tab_actions(9, vec![Action::new("ghost", "i", "Ghost")]);
        assert!(!r.set_current(9));
        assert!(r.visible_actions().is_empty());
    }

    #[test]
    fn badges_behave_exactly_as_they_do_on_the_strip() {
        // The two bars share a visual language; disagreeing about what a large
        // count looks like would be visible on one screen.
        let mut r = ribbon();
        r.set_badge(1, 0);
        assert_eq!(r.badge_text(1), None, "zero removes the pill");
        r.set_badge(1, 7);
        assert_eq!(r.badge_text(1).as_deref(), Some("7"));
        r.set_badge(1, 100);
        assert_eq!(r.badge_text(1).as_deref(), Some("99+"));
    }

    #[test]
    fn labels_drop_on_a_narrow_bar_but_icons_stay() {
        // An elided label takes the space AND says nothing, which is worse than
        // an icon alone.
        let r = ribbon();
        assert!(r.show_labels(LABEL_HIDE_WIDTH));
        assert!(r.show_labels(600.0));
        assert!(!r.show_labels(LABEL_HIDE_WIDTH - 1.0));
    }

    #[test]
    fn segments_take_their_natural_width_not_an_equal_share() {
        // Equal segments are the strip's job. Natural widths are what leave the
        // right half room for actions.
        let r = SmartRibbon::new(&["All", "Unread"]);
        let w = r.tab_widths(180.0);
        assert_eq!(w.len(), 2);
        assert!(w[1] > w[0], "\"Unread\" is longer than \"All\"");
        assert!((w[0] + w[1] - 180.0).abs() < 0.01, "and they fill the half exactly");
    }

    #[test]
    fn an_empty_ribbon_does_not_divide_by_zero() {
        let r = SmartRibbon::default();
        assert!(r.tab_widths(180.0).is_empty());
        assert!(r.visible_actions().is_empty());
    }

    #[test]
    fn a_blank_label_still_gets_width() {
        // Otherwise a nameless tab collapses to nothing and cannot be tapped.
        let r = SmartRibbon::new(&["", "Sent"]);
        let w = r.tab_widths(180.0);
        assert!(w[0] > 0.0);
    }
}
