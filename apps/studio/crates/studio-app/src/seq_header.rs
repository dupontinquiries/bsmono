//! seq_header — "where you are", drawn the same on both views of a pattern.
//!
//! The Rack tab (the step grid) and the Program tab (the piano roll) edit one
//! pattern's one channel from two sides. Until c20 they shared that selection
//! but did not SHOW it the same way: the pattern chooser lived only on the
//! Rack, the Program tab never named the pattern it was editing, neither said
//! which view you were in, and the roll's columns sat 52 px left of the
//! grid's. Switching felt like going somewhere else.
//!
//! This strip is on both, above their scroll areas so it never scrolls away:
//! the pattern (◀ name ▶), the channel pills (each with what it holds in BOTH
//! views -- `4|2` is four steps and two notes, so you can see there is
//! something on the other side before you go looking), and the Steps / Notes
//! switch, which `Tab` also flips. The scroll offset is shared so the same
//! steps stay under your eye across a switch.

use studio_egui::touch::Panel;
use uikit_egui::PaletteColors;

use crate::Studio;

/// Steps set and piano-roll notes on channel `ch` of `p` -- the pill's `4|2`.
/// Only what is inside the pattern's current length counts: a step hidden by
/// a shortened pattern is not something you would find by switching views.
pub fn channel_summary(p: &studio_live::Pattern, ch: usize) -> (u32, usize) {
    let steps = (0..p.steps).filter(|s| p.is_set(ch, *s)).count() as u32;
    let notes = p.roll(ch).iter().filter(|n| n.start < p.steps).count();
    (steps, notes)
}

/// The other view of the pattern, for `Tab`; `None` off the two sequencer
/// tabs, where Tab means nothing here.
pub fn other_view(p: Panel) -> Option<Panel> {
    match p {
        Panel::Rack => Some(Panel::Program),
        Panel::Program => Some(Panel::Rack),
        _ => None,
    }
}

/// Take every plain Tab key event out of `events`; true when one was a press.
///
/// Called from eframe's `raw_input_hook`, BEFORE egui sees the frame: egui
/// moves keyboard focus on Tab as the frame begins, so consuming the key
/// later in `ui()` still let it walk focus onto the top bar's buttons -- the
/// second Tab then found a focused widget and did not flip (seen under Xvfb,
/// c20). Shift-Tab and Tab with other modifiers are left alone.
pub fn take_tab(events: &mut Vec<egui::Event>) -> bool {
    let mut pressed = false;
    events.retain(|e| match e {
        egui::Event::Key { key: egui::Key::Tab, pressed: p, modifiers, .. } if modifiers.is_none() => {
            pressed |= *p;
            false
        }
        _ => true,
    });
    pressed
}

/// The name a view goes by in the switch.
pub fn view_name(p: Panel) -> &'static str {
    match p { Panel::Program => "Notes", _ => "Steps" }
}

/// Draw the strip for `here` (Rack or Program).
pub fn show(ui: &mut egui::Ui, s: &mut Studio, c: &PaletteColors, here: Panel) {
    ui.horizontal_wrapped(|ui| {
        // ── the view switch, first: it is the "where" ───────────────────
        for v in [Panel::Rack, Panel::Program] {
            let r = ui.add(egui::Button::selectable(here == v, egui::RichText::new(view_name(v)).strong()))
                .on_hover_text("Tab switches between Steps and Notes");
            if r.clicked() && here != v { s.panel = v; }
        }
        ui.separator();
        // ── the pattern ─────────────────────────────────────────────────
        let n = s.rack.patterns.len();
        if ui.small_button("◀").on_hover_text("previous pattern").clicked() && n > 0 {
            s.current_pattern = (s.current_pattern + n - 1) % n;
        }
        let name = s.rack.patterns.get(s.current_pattern).map(|p| p.name.clone()).unwrap_or_else(|| "—".into());
        ui.label(egui::RichText::new(name).strong());
        ui.colored_label(c.text_weak, format!("{}/{}", (s.current_pattern + 1).min(n), n));
        if ui.small_button("▶").on_hover_text("next pattern").clicked() && n > 0 {
            s.current_pattern = (s.current_pattern + 1) % n;
        }
        ui.separator();
        // ── the channel ─────────────────────────────────────────────────
        let Some(p) = s.rack.patterns.get(s.current_pattern) else { return };
        let mut selected = s.selected_channel;
        for (i, ch) in s.rack.channels.iter().enumerate() {
            let (st, nt) = channel_summary(p, i);
            let label = if st == 0 && nt == 0 { ch.name.clone() } else { format!("{} {st}|{nt}", ch.name) };
            let r = ui.add(egui::Button::selectable(selected == i, label))
                .on_hover_text(format!("{st} step{} in Steps · {nt} note{} in Notes",
                    if st == 1 { "" } else { "s" }, if nt == 1 { "" } else { "s" }));
            if r.clicked() { selected = i; }
        }
        if selected != s.selected_channel {
            s.selected_channel = selected;
            s.roll.selected = None; // a selected note belongs to the channel it was on
        }
        s.selected_channel = s.selected_channel.min(s.rack.channels.len().saturating_sub(1));
    });
}

/// The horizontal offset `here`'s scroll area should adopt this frame:
/// `Some` only on the frame a view takes over from the other one, so a view
/// scrolled by hand is never fought.
pub fn adopt_scroll(s: &Studio, here: Panel) -> Option<f32> {
    (s.seq_scroll_owner.is_some() && s.seq_scroll_owner != Some(here)).then_some(s.seq_scroll_x)
}

/// Record `here`'s offset after it drew.
pub fn record_scroll(s: &mut Studio, here: Panel, x: f32) {
    s.seq_scroll_x = x;
    s.seq_scroll_owner = Some(here);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_summary_counts_both_views_inside_the_pattern_only() {
        let mut p = studio_live::Pattern::new("a", 8, 2);
        p.set(0, 1, true);
        p.set(0, 5, true);
        p.add_note(0, studio_live::Note { key: 0, start: 2, len: 1, vel: 1.0, slide: false, nudge: 0.0 });
        p.add_note(0, studio_live::Note { key: 0, start: 12, len: 1, vel: 1.0, slide: false, nudge: 0.0 }); // past the end
        assert_eq!(channel_summary(&p, 0), (2, 1));
        assert_eq!(channel_summary(&p, 1), (0, 0));
        p.steps = 4;
        assert_eq!(channel_summary(&p, 0), (1, 1), "a step hidden by shortening is not counted");
    }

    #[test]
    fn tab_is_taken_out_of_the_input_whole_and_only_plain_tab() {
        let key = |key, pressed, modifiers| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        let mut ev = vec![key(egui::Key::Tab, true, egui::Modifiers::NONE), key(egui::Key::Tab, false, egui::Modifiers::NONE),
                          key(egui::Key::Tab, true, egui::Modifiers::SHIFT), key(egui::Key::A, true, egui::Modifiers::NONE)];
        assert!(take_tab(&mut ev));
        assert_eq!(ev.len(), 2, "press and release both gone; shift-Tab and A stay: {ev:?}");
        assert!(!take_tab(&mut ev), "nothing left to take");
    }

    #[test]
    fn tab_flips_only_between_the_two_views_of_a_pattern() {
        assert_eq!(other_view(Panel::Rack), Some(Panel::Program));
        assert_eq!(other_view(Panel::Program), Some(Panel::Rack));
        assert_eq!(other_view(Panel::Timeline), None);
        assert_eq!(other_view(Panel::Mixer), None);
    }

    #[test]
    fn the_scroll_is_adopted_once_on_a_switch_and_never_fought() {
        let mut s = Studio::demo();
        assert_eq!(adopt_scroll(&s, Panel::Rack), None, "nothing recorded yet");
        record_scroll(&mut s, Panel::Rack, 300.0);
        assert_eq!(adopt_scroll(&s, Panel::Rack), None, "the Rack scrolled itself: leave it");
        assert_eq!(adopt_scroll(&s, Panel::Program), Some(300.0), "the Program tab takes it over");
        record_scroll(&mut s, Panel::Program, 300.0);
        assert_eq!(adopt_scroll(&s, Panel::Program), None, "adopted once, then its own");
    }
}
