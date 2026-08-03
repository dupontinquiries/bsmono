//! export_reminder — `MonthlyExportReminder` and `ExportReminderCard`.
//!
//! Snaps are ephemeral by design; that is the app's defining characteristic.
//! But a user builds up a folder over a month or two, and if nothing ever
//! surfaces it those snaps stay invisible and there is no easy path to keep the
//! ones they liked. The card exists to hold the line the header draws:
//!
//! > ephemeral by default, but not LOSSY by default.
//!
//! ## Why monthly, and why that is the hard part
//!
//! Daily is a nag, and a nag gets switched off — after which the feature
//! protects nobody. Monthly is noticed but not irritating, and it matches how
//! people already think about photos ("photos from last month").
//!
//! Which means every rule here is about NOT showing it:
//!
//! * Already shown this month → no.
//! * User tapped "Maybe later" → no, for the rest of the month. Honouring that
//!   is the whole difference between a prompt and a nag; re-asking on the next
//!   launch is exactly what teaches people to disable things.
//! * An auto-sync path is configured → no, ever. That is the self-hosting /
//!   Syncthing escape hatch: those users already export continuously, know what
//!   they want, and a prompt offering to do it again is noise.
//! * Not enough recent snaps → no. A "remember when?" card with two thumbnails
//!   on it is not a memory, it is an advertisement.
//!
//! ## The month key
//!
//! State is stored as `YYYY-MM`, not as a date or a timestamp. Comparing months
//! by string is what makes "have we shown it this month" a single equality —
//! and it is why a clock that jumps backwards cannot un-show a reminder that
//! was already shown, which a day-count would allow.

/// A calendar month, as the reminder counts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Month { pub year: i32, pub month: u32 }

impl Month {
    /// `YYYY-MM`, the storage key. Zero-padded so string comparison and
    /// chronological order agree — "2026-9" would sort after "2026-10".
    pub fn key(&self) -> String { format!("{:04}-{:02}", self.year, self.month) }

    pub fn parse(s: &str) -> Option<Month> {
        let (y, m) = s.trim().split_once('-')?;
        Some(Month { year: y.parse().ok()?, month: m.parse().ok()? })
    }
}

/// Everything the decision depends on. Passed in rather than read from storage
/// so the rule is a pure function — `shouldShowThisMonth` is the piece worth
/// testing, and a version that reaches for QSettings and a clock cannot be.
#[derive(Debug, Clone, Default)]
pub struct ReminderState {
    /// `export/lastReminderShown`.
    pub last_shown: Option<String>,
    /// `export/snoozeUntil` — the month the user said "maybe later" in.
    pub snoozed_month: Option<String>,
    /// `export/autoSyncPath` — a path or URL. Set means never prompt.
    pub auto_sync_path: String,
}

/// How many recent snaps make the card worth showing. Below this a "remember
/// when?" strip has nothing to remember with.
pub const MIN_RECENT_SNAPS: usize = 3;

impl ReminderState {
    /// Whether the escape hatch is engaged.
    pub fn auto_sync(&self) -> bool { !self.auto_sync_path.trim().is_empty() }

    /// `shouldShowThisMonth`. Pure: every input is an argument.
    pub fn should_show(&self, now: Month, recent_snaps: usize) -> bool {
        if self.auto_sync() { return false; }
        if recent_snaps < MIN_RECENT_SNAPS { return false; }
        let key = now.key();
        if self.last_shown.as_deref() == Some(key.as_str()) { return false; }
        if self.snoozed_month.as_deref() == Some(key.as_str()) { return false; }
        true
    }

    /// `markShownThisMonth`.
    pub fn mark_shown(&mut self, now: Month) { self.last_shown = Some(now.key()); }

    /// `snoozeForThisMonth` — the "Maybe later" handler. Records the snooze AND
    /// the show, because a card the user dismissed was still shown, and leaving
    /// `last_shown` unset would let a later code path re-offer it.
    pub fn snooze(&mut self, now: Month) {
        self.snoozed_month = Some(now.key());
        self.last_shown = Some(now.key());
    }

    pub fn set_auto_sync(&mut self, path: &str) { self.auto_sync_path = path.trim().to_string(); }
}

/// What the card asks the host to do. The card runs neither the export nor the
/// bookkeeping — it reports, the host acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction { ExportRequested, Dismissed }

/// The card.
#[derive(Debug, Clone, Default)]
pub struct ExportCard {
    /// Thumbnail paths, newest first. Never the images themselves — the card
    /// shows what the host hands it.
    pub recent: Vec<String>,
    pub visible: bool,
}

/// Up to this many thumbnails on the strip; more is a contact sheet.
pub const MAX_THUMBS: usize = 5;

impl ExportCard {
    /// Build and decide visibility in one step, so a card cannot exist on
    /// screen without having passed the gate.
    pub fn for_month(state: &ReminderState, now: Month, recent: Vec<String>) -> Self {
        let visible = state.should_show(now, recent.len());
        ExportCard { recent, visible }
    }

    pub fn thumbs(&self) -> &[String] {
        &self.recent[..self.recent.len().min(MAX_THUMBS)]
    }

    /// The prompt line. Names the count, because "you have snaps" is not a
    /// reason to act and "you have 47 snaps from last month" is.
    pub fn prompt(&self) -> String {
        format!("{} snaps from the last month. Keep the ones you liked?", self.recent.len())
    }
}

/// Draw the card. Returns the action once one is taken.
pub fn export_card_ui(ui: &mut egui::Ui, card: &ExportCard,
                      c: &uikit_egui::PaletteColors) -> Option<CardAction> {
    if !card.visible { return None; }
    let mut out = None;
    ui.group(|ui| {
        ui.colored_label(c.text, card.prompt());
        ui.horizontal(|ui| {
            for _ in card.thumbs() {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 4.0, c.bg_raised);
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Export as zip").clicked() { out = Some(CardAction::ExportRequested); }
            // "Maybe later" is a plain button, not a dismiss X: an escape that
            // is hard to find is how a prompt becomes a nag.
            if ui.button("Maybe later").clicked() { out = Some(CardAction::Dismissed); }
        });
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(year: i32, month: u32) -> Month { Month { year, month } }
    fn snaps(n: usize) -> Vec<String> { (0..n).map(|i| format!("s{i}.jpg")).collect() }

    #[test]
    fn a_fresh_month_with_enough_snaps_shows_the_card() {
        let s = ReminderState::default();
        assert!(s.should_show(m(2026, 8), 10));
    }

    #[test]
    fn it_shows_once_a_month_and_not_again() {
        let mut s = ReminderState::default();
        assert!(s.should_show(m(2026, 8), 10));
        s.mark_shown(m(2026, 8));
        assert!(!s.should_show(m(2026, 8), 10));
        assert!(s.should_show(m(2026, 9), 10), "next month is a fresh prompt");
    }

    #[test]
    fn maybe_later_is_honoured_for_the_rest_of_the_month() {
        // Re-asking on the next launch is exactly what teaches people to
        // disable things.
        let mut s = ReminderState::default();
        s.snooze(m(2026, 8));
        assert!(!s.should_show(m(2026, 8), 100));
        assert!(s.should_show(m(2026, 9), 100));
    }

    #[test]
    fn a_snooze_also_counts_as_a_show() {
        // A card the user dismissed WAS shown. Leaving last_shown unset lets a
        // later code path re-offer it in the same month.
        let mut s = ReminderState::default();
        s.snooze(m(2026, 8));
        assert_eq!(s.last_shown.as_deref(), Some("2026-08"));
    }

    #[test]
    fn auto_sync_silences_the_reminder_permanently() {
        // The self-hosting escape hatch: those users already export
        // continuously and a prompt offering to do it again is noise.
        let mut s = ReminderState::default();
        s.set_auto_sync("/home/ada/Syncthing/snaps");
        for month in 1..=12 {
            assert!(!s.should_show(m(2026, month), 500), "month {month}");
        }
    }

    #[test]
    fn a_blank_auto_sync_path_is_not_configured() {
        // An empty setting must not silence the reminder, or clearing the field
        // leaves the feature switched off with nothing on screen to say so.
        let mut s = ReminderState::default();
        s.set_auto_sync("   ");
        assert!(!s.auto_sync());
        assert!(s.should_show(m(2026, 8), 10));
    }

    #[test]
    fn too_few_snaps_means_no_card() {
        // A "remember when?" strip with two thumbnails is not a memory, it is
        // an advertisement.
        let s = ReminderState::default();
        assert!(!s.should_show(m(2026, 8), 0));
        assert!(!s.should_show(m(2026, 8), MIN_RECENT_SNAPS - 1));
        assert!(s.should_show(m(2026, 8), MIN_RECENT_SNAPS));
    }

    #[test]
    fn the_month_key_is_zero_padded_so_order_and_string_order_agree() {
        // "2026-9" would sort after "2026-10", which turns any later
        // range comparison on this key into a silent bug.
        assert_eq!(m(2026, 9).key(), "2026-09");
        assert_eq!(m(2026, 12).key(), "2026-12");
        assert!(m(2026, 9).key() < m(2026, 10).key());
    }

    #[test]
    fn the_month_key_round_trips() {
        for month in 1..=12 {
            let x = m(2026, month);
            assert_eq!(Month::parse(&x.key()), Some(x));
        }
        assert_eq!(Month::parse("garbage"), None);
    }

    #[test]
    fn a_clock_that_jumps_backwards_cannot_un_show_a_reminder() {
        // Stored state is a month KEY, not a day count, so an equality decides
        // it. A day-count would let a backwards clock re-open the prompt.
        let mut s = ReminderState::default();
        s.mark_shown(m(2026, 8));
        assert!(!s.should_show(m(2026, 8), 50));
        // Even a jump into the past does not re-show THIS month.
        assert!(s.should_show(m(2026, 7), 50), "a genuinely different month does show");
        assert!(!s.should_show(m(2026, 8), 50), "and August is still marked");
    }

    #[test]
    fn a_card_cannot_be_visible_without_passing_the_gate() {
        let mut s = ReminderState::default();
        assert!(ExportCard::for_month(&s, m(2026, 8), snaps(10)).visible);
        s.snooze(m(2026, 8));
        assert!(!ExportCard::for_month(&s, m(2026, 8), snaps(10)).visible);
        let empty = ExportCard::for_month(&ReminderState::default(), m(2026, 8), snaps(1));
        assert!(!empty.visible, "too few snaps");
    }

    #[test]
    fn the_thumbnail_strip_is_capped() {
        // More than a handful is a contact sheet, not a prompt.
        let card = ExportCard::for_month(&ReminderState::default(), m(2026, 8), snaps(40));
        assert_eq!(card.thumbs().len(), MAX_THUMBS);
        let few = ExportCard::for_month(&ReminderState::default(), m(2026, 8), snaps(4));
        assert_eq!(few.thumbs().len(), 4, "and it does not pad");
    }

    #[test]
    fn the_prompt_names_the_count_because_a_number_is_the_reason_to_act() {
        let card = ExportCard::for_month(&ReminderState::default(), m(2026, 8), snaps(47));
        assert!(card.prompt().starts_with("47 snaps"));
    }
}
