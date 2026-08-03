//! menu — the action menu that was missing everywhere.
//!
//! An audit of the fleet found no `secondary_clicked`, no `context_menu` and no
//! shared long-press in any app or kit. There was no way to right-click a thing
//! and no way to press-and-hold it, on any surface, in any of the ports. Every
//! action had to be a visible button, which is why several pages grew rows of
//! them.
//!
//! ## One menu, two ways in, and that is the whole point
//!
//! A desktop user right-clicks; a phone user presses and holds. Those are the
//! same intent on different hardware, and a codebase that treats them as two
//! features grows two menus that drift — one gets a new action and the other
//! does not. `MenuTrigger` collapses them: the row asks "was I invoked?", not
//! "was I right-clicked?".
//!
//! Long-press has a rule right-click does not: **movement cancels it**. A finger
//! that drifts more than a few pixels is scrolling, and a menu that opens under
//! a scroll makes the list feel like it is grabbing at you. The slop is
//! generous, because a thumb resting on a phone is never perfectly still.
//!
//! ## Keyboard, because a menu you can only click is half a menu
//!
//! Up/Down move, Enter chooses, Escape dismisses. Disabled entries are SKIPPED
//! rather than landed on — arrowing onto something you cannot pick, twice,
//! reads as the keyboard being broken. Separators are skipped for the same
//! reason.
//!
//! ## Copy, which the fleet also could not do
//!
//! `copy_to_clipboard` exists here because nothing in any app wrote to the
//! clipboard — voiceterm had a `TermAction::Copy` that named the intent and
//! never performed it. A context menu without Copy is most of a context menu
//! missing.

use std::time::Duration;

/// `kHoldMs` — how long a press must rest before it is a hold.
///
/// 450ms, matching snapclone's snap tiles, which was the only place in the
/// fleet that had ever tuned this.
pub const HOLD_MS: f32 = 450.0;

/// How far a finger may drift and still count as a hold.
///
/// Generous on purpose: a thumb resting on a phone is never perfectly still,
/// and cancelling at two pixels makes long-press feel broken on exactly the
/// devices that need it most.
pub const HOLD_SLOP_PX: f32 = 12.0;

/// Tracks a press that might become a hold.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HoldTracker {
    down_at: Option<(f32, f32)>,
    held_ms: f32,
    fired: bool,
}

impl HoldTracker {
    pub fn press(&mut self, x: f32, y: f32) {
        self.down_at = Some((x, y));
        self.held_ms = 0.0;
        self.fired = false;
    }

    /// Returns true on the single frame the hold fires.
    ///
    /// Fires ONCE per press: a hold that kept firing would reopen the menu
    /// under the user's finger every frame they kept holding.
    pub fn tick(&mut self, dt_ms: f32) -> bool {
        if self.down_at.is_none() || self.fired { return false; }
        self.held_ms += dt_ms;
        if self.held_ms >= HOLD_MS { self.fired = true; return true; }
        false
    }

    /// Movement while held. Past the slop this is a scroll, not a hold.
    pub fn moved(&mut self, x: f32, y: f32) {
        let Some((ox, oy)) = self.down_at else { return };
        if (x - ox).hypot(y - oy) > HOLD_SLOP_PX { self.cancel(); }
    }

    /// Lift. Returns true if this was a plain tap — a press that ended before
    /// the hold fired, which is how a row distinguishes "open me" from "show me
    /// my options".
    pub fn release(&mut self) -> bool {
        let tap = self.down_at.is_some() && !self.fired;
        self.down_at = None;
        tap
    }

    pub fn cancel(&mut self) {
        self.down_at = None;
        self.held_ms = 0.0;
        self.fired = false;
    }

    pub fn is_down(&self) -> bool { self.down_at.is_some() }
    pub fn progress(&self) -> f32 { (self.held_ms / HOLD_MS).clamp(0.0, 1.0) }
}

/// One entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    /// A separator draws a line and can never be selected.
    pub separator: bool,
    /// Marks a destructive action so the view can colour it.
    pub danger: bool,
}

impl MenuItem {
    pub fn new(id: &str, label: &str) -> Self {
        MenuItem { id: id.into(), label: label.into(), enabled: true,
                   separator: false, danger: false }
    }
    pub fn disabled(mut self) -> Self { self.enabled = false; self }
    pub fn danger(mut self) -> Self { self.danger = true; self }
    pub fn separator() -> Self {
        MenuItem { id: String::new(), label: String::new(), enabled: false,
                   separator: true, danger: false }
    }
    /// Can the cursor land here?
    pub fn selectable(&self) -> bool { self.enabled && !self.separator }
}

/// The menu.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContextMenu {
    items: Vec<MenuItem>,
    open: bool,
    /// Where it was invoked, in screen coordinates.
    at: (f32, f32),
    cursor: Option<usize>,
}

impl ContextMenu {
    /// Open at a point. Refuses if nothing is selectable — a menu of greyed-out
    /// entries tells the user nothing they could not see already.
    pub fn open(&mut self, items: Vec<MenuItem>, at: (f32, f32)) -> bool {
        if !items.iter().any(|i| i.selectable()) { return false; }
        self.items = items;
        self.at = at;
        self.open = true;
        // No pre-selection: a highlighted first entry invites Enter, and the
        // first entry is often the destructive one.
        self.cursor = None;
        true
    }

    pub fn close(&mut self) { self.open = false; self.items.clear(); self.cursor = None; }
    pub fn is_open(&self) -> bool { self.open }
    pub fn items(&self) -> &[MenuItem] { &self.items }
    pub fn at(&self) -> (f32, f32) { self.at }
    pub fn cursor(&self) -> Option<usize> { self.cursor }

    /// Move the keyboard cursor, skipping separators and disabled entries.
    ///
    /// Arrowing onto something you cannot pick — twice in a row, past two
    /// separators — reads as the keyboard being broken.
    pub fn move_cursor(&mut self, delta: i32) {
        if !self.open || self.items.is_empty() { return; }
        let n = self.items.len() as i32;
        let start = match self.cursor {
            Some(c) => c as i32,
            // First press picks the first selectable in the direction of travel.
            None => if delta > 0 { -1 } else { n },
        };
        let mut i = start;
        for _ in 0..n {
            i += delta;
            // Wraps: a menu is a ring, and stopping dead at the bottom makes
            // the last item hard to reach from the top.
            if i < 0 { i = n - 1; }
            if i >= n { i = 0; }
            if self.items[i as usize].selectable() {
                self.cursor = Some(i as usize);
                return;
            }
        }
    }

    /// Enter. Returns the chosen id and closes.
    pub fn activate(&mut self) -> Option<String> {
        let i = self.cursor?;
        let item = self.items.get(i)?;
        if !item.selectable() { return None; }
        let id = item.id.clone();
        self.close();
        Some(id)
    }

    /// Click an entry directly.
    pub fn choose(&mut self, id: &str) -> Option<String> {
        let item = self.items.iter().find(|i| i.id == id)?;
        if !item.selectable() { return None; }
        let id = item.id.clone();
        self.close();
        Some(id)
    }

    /// Where to draw, kept on screen.
    ///
    /// A menu invoked near the right or bottom edge would otherwise open partly
    /// off it — which on a phone means the actions you wanted are the ones you
    /// cannot see.
    pub fn placement(&self, size: (f32, f32), screen: (f32, f32)) -> (f32, f32) {
        let (mut x, mut y) = self.at;
        if x + size.0 > screen.0 { x = (screen.0 - size.0).max(0.0); }
        if y + size.1 > screen.1 { y = (screen.1 - size.1).max(0.0); }
        (x.max(0.0), y.max(0.0))
    }
}

/// Put text on the clipboard.
///
/// Here because nothing in the fleet could: voiceterm had a `TermAction::Copy`
/// that named the intent and never performed it, and no app wrote to the
/// clipboard at all.
pub fn copy_to_clipboard(ctx: &egui::Context, text: impl Into<String>) {
    ctx.copy_text(text.into());
}

/// Was this response invoked as a context menu — by right-click OR by hold?
///
/// The row asks "was I invoked?", not "was I right-clicked?", so a desktop and
/// a phone cannot drift into two different menus.
pub fn invoked(resp: &egui::Response, hold: &mut HoldTracker, dt_ms: f32) -> Option<(f32, f32)> {
    if resp.secondary_clicked() {
        if let Some(p) = resp.interact_pointer_pos() { return Some((p.x, p.y)); }
    }
    if resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            if !hold.is_down() { hold.press(p.x, p.y); } else { hold.moved(p.x, p.y); }
            if hold.tick(dt_ms) { return Some((p.x, p.y)); }
        }
    } else if hold.is_down() {
        hold.release();
    }
    None
}

/// Draw an open menu. Returns the chosen id.
pub fn menu_ui(ui: &mut egui::Ui, m: &mut ContextMenu, colors: &crate::PaletteColors)
    -> Option<String> {
    if !m.is_open() { return None; }
    let mut chosen = None;

    ui.input(|i| {
        if i.key_pressed(egui::Key::ArrowDown) { 1 } else { 0 }
    });
    let (down, up, enter, esc) = ui.input(|i| (
        i.key_pressed(egui::Key::ArrowDown),
        i.key_pressed(egui::Key::ArrowUp),
        i.key_pressed(egui::Key::Enter),
        i.key_pressed(egui::Key::Escape),
    ));
    if down { m.move_cursor(1); }
    if up { m.move_cursor(-1); }
    if esc { m.close(); return None; }
    if enter { if let Some(id) = m.activate() { return Some(id); } }

    let items = m.items().to_vec();
    let cursor = m.cursor();
    egui::Area::new(egui::Id::new("uikit_context_menu"))
        .fixed_pos(egui::pos2(m.at().0, m.at().1))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                for (i, item) in items.iter().enumerate() {
                    if item.separator { ui.separator(); continue; }
                    let colour = if !item.enabled { colors.text_weak }
                                 else if item.danger { colors.danger }
                                 else { colors.text };
                    let text = egui::RichText::new(&item.label).color(colour);
                    let r = ui.add_enabled(item.enabled,
                        egui::SelectableLabel::new(cursor == Some(i), text));
                    if r.clicked() { chosen = Some(item.id.clone()); }
                }
            });
        });
    if let Some(id) = &chosen { m.choose(id); }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<MenuItem> {
        vec![MenuItem::new("open", "Open"),
             MenuItem::new("rename", "Rename").disabled(),
             MenuItem::separator(),
             MenuItem::new("delete", "Delete").danger()]
    }

    fn open_menu() -> ContextMenu {
        let mut m = ContextMenu::default();
        assert!(m.open(items(), (10.0, 10.0)));
        m
    }

    #[test]
    fn a_hold_fires_once_and_only_once() {
        // A hold that kept firing would reopen the menu under the finger every
        // frame the user kept holding.
        let mut h = HoldTracker::default();
        h.press(5.0, 5.0);
        assert!(!h.tick(HOLD_MS - 1.0));
        assert!(h.tick(2.0));
        assert!(!h.tick(HOLD_MS * 3.0), "not again");
    }

    #[test]
    fn movement_past_the_slop_cancels_the_hold() {
        // A finger that drifts is scrolling, and a menu opening under a scroll
        // makes the list feel like it is grabbing at you.
        let mut h = HoldTracker::default();
        h.press(5.0, 5.0);
        h.moved(5.0, 5.0 + HOLD_SLOP_PX + 1.0);
        assert!(!h.is_down());
        assert!(!h.tick(HOLD_MS * 2.0));
    }

    #[test]
    fn a_small_drift_does_not_cancel_because_thumbs_are_not_still() {
        let mut h = HoldTracker::default();
        h.press(5.0, 5.0);
        h.moved(6.0, 8.0);
        assert!(h.is_down());
        assert!(h.tick(HOLD_MS));
    }

    #[test]
    fn a_short_press_is_a_tap_and_a_long_one_is_not() {
        // How a row tells "open me" from "show me my options".
        let mut h = HoldTracker::default();
        h.press(0.0, 0.0);
        h.tick(100.0);
        assert!(h.release(), "short press is a tap");

        h.press(0.0, 0.0);
        h.tick(HOLD_MS);
        assert!(!h.release(), "the hold already fired; this is not also a tap");
    }

    #[test]
    fn a_menu_with_nothing_selectable_does_not_open() {
        // A menu of greyed-out entries tells the user nothing they could not
        // already see.
        let mut m = ContextMenu::default();
        assert!(!m.open(vec![MenuItem::new("x", "X").disabled(),
                             MenuItem::separator()], (0.0, 0.0)));
        assert!(!m.is_open());
    }

    #[test]
    fn a_fresh_menu_has_nothing_selected() {
        // A highlighted first entry invites Enter, and the first entry is often
        // the destructive one.
        let m = open_menu();
        assert_eq!(m.cursor(), None);
    }

    #[test]
    fn arrowing_skips_separators_and_disabled_entries() {
        // Landing on something you cannot pick reads as the keyboard being
        // broken.
        let mut m = open_menu();
        m.move_cursor(1);
        assert_eq!(m.cursor(), Some(0), "Open");
        m.move_cursor(1);
        assert_eq!(m.cursor(), Some(3), "past Rename and the separator, to Delete");
    }

    #[test]
    fn arrowing_up_from_nothing_starts_at_the_bottom() {
        let mut m = open_menu();
        m.move_cursor(-1);
        assert_eq!(m.cursor(), Some(3));
    }

    #[test]
    fn the_cursor_wraps_because_a_menu_is_a_ring() {
        // Stopping dead at the bottom makes the last item hard to reach from
        // the top.
        let mut m = open_menu();
        m.move_cursor(1);
        m.move_cursor(1);
        m.move_cursor(1);
        assert_eq!(m.cursor(), Some(0), "wrapped round to Open");
    }

    #[test]
    fn a_menu_of_one_selectable_entry_does_not_hang_the_cursor() {
        // The scan is bounded, so a single-entry menu cannot loop forever.
        let mut m = ContextMenu::default();
        m.open(vec![MenuItem::new("only", "Only"), MenuItem::separator()], (0.0, 0.0));
        m.move_cursor(1);
        assert_eq!(m.cursor(), Some(0));
        m.move_cursor(1);
        assert_eq!(m.cursor(), Some(0));
    }

    #[test]
    fn enter_activates_the_cursor_and_closes() {
        let mut m = open_menu();
        m.move_cursor(1);
        assert_eq!(m.activate().as_deref(), Some("open"));
        assert!(!m.is_open());
    }

    #[test]
    fn enter_with_no_selection_does_nothing() {
        let mut m = open_menu();
        assert_eq!(m.activate(), None);
        assert!(m.is_open(), "and the menu stays up");
    }

    #[test]
    fn a_disabled_entry_cannot_be_chosen_by_any_route() {
        // Clicking it is already blocked by the widget; this blocks the rest.
        let mut m = open_menu();
        assert_eq!(m.choose("rename"), None);
        assert!(m.is_open());
    }

    #[test]
    fn choosing_an_unknown_id_does_nothing() {
        let mut m = open_menu();
        assert_eq!(m.choose("nonsense"), None);
    }

    #[test]
    fn a_menu_near_an_edge_is_pulled_back_on_screen() {
        // On a phone, opening partly off-screen means the actions you wanted
        // are the ones you cannot see.
        let mut m = ContextMenu::default();
        m.open(items(), (380.0, 760.0));
        let (x, y) = m.placement((160.0, 120.0), (390.0, 780.0));
        assert!(x + 160.0 <= 390.0);
        assert!(y + 120.0 <= 780.0);
    }

    #[test]
    fn a_menu_bigger_than_the_screen_still_starts_on_it() {
        let mut m = ContextMenu::default();
        m.open(items(), (10.0, 10.0));
        let (x, y) = m.placement((500.0, 900.0), (390.0, 780.0));
        assert_eq!((x, y), (0.0, 0.0), "clamped rather than pushed negative");
    }
}
