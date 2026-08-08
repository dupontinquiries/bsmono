//! sync — `SyncPage`, ported.
//!
//! What the local database holds, and — eventually — moving it between devices.
//!
//! ## The transfer buttons are ported DISABLED, on purpose
//!
//! The Qt page has `enabled: false` on both, an `onClicked` that logs "not yet
//! wired up", and a note saying export/import need more backend methods and
//! that the GTK UI has full sync support today.
//!
//! It would have been easy to "finish" this during the port — invent a file
//! picker, write a serialiser, ship something. That is not porting, it is
//! writing a new feature under cover of a port, and it would have been the
//! most dangerous kind: sync that half works loses data quietly. So the state
//! comes across exactly as it is, including the note, because a disabled button
//! with a reason is information and a missing button is not. Someone reading
//! this screen learns that the capability exists elsewhere and where to get it.
//!
//! ## The counts are a breakdown, not a total
//!
//! `calibration_count` alongside `linear` and `neural` is not redundant. The
//! two kinds are not interchangeable — a neural calibration needs far more
//! samples to be worth anything — so "48 calibrations" answers a different
//! question from "46 linear, 2 neural". Showing only the total would hide the
//! case that actually matters: plenty of data, almost none of it the kind the
//! active model uses.

/// What the local database holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DbSummary {
    pub calibrations: u32,
    pub linear: u32,
    pub neural: u32,
    /// The model currently in use, as the backend names it.
    pub model_type: String,
}

impl DbSummary {
    /// The rows shown, in the Qt page's order.
    pub fn rows(&self) -> [(&'static str, String); 4] {
        [("Calibrations:", self.calibrations.to_string()),
         ("Linear:", self.linear.to_string()),
         ("Neural:", self.neural.to_string()),
         // Never blank: a model that has not reported its type would leave the
         // row looking like a failed read rather than an early one.
         ("Active model:", if self.model_type.trim().is_empty() {
             "—".into() } else { self.model_type.clone() })]
    }

    /// Does the active model have calibrations of its own kind?
    ///
    /// The case worth surfacing: a neural model with two neural calibrations
    /// and forty-six linear ones has plenty of data and almost none it can use.
    /// The total alone hides that completely.
    pub fn active_kind_count(&self) -> Option<u32> {
        match self.model_type.trim().to_lowercase().as_str() {
            "linear" => Some(self.linear),
            "neural" => Some(self.neural),
            _ => None,
        }
    }

    /// True when the numbers do not add up.
    ///
    /// Not an error to block on — the backend may count kinds this page does
    /// not know — but worth not asserting equality anywhere, because a future
    /// third kind would make a strict check fire on every screen.
    pub fn has_other_kinds(&self) -> bool {
        self.linear + self.neural < self.calibrations
    }
}

/// Why a transfer action is unavailable.
///
/// A reason rather than a bool, so the view can say WHICH thing is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// The backend methods do not exist yet. Ported from the Qt page's own
    /// note, verbatim in spirit: this is a statement about the app, not a
    /// runtime failure.
    NotImplemented,
}

/// The note the Qt page shows beneath the disabled buttons.
pub const SYNC_NOTE: &str =
    "Export / import need a few more backend methods. The GTK UI has full sync \
     support today.";

/// The page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncPage {
    pub db: DbSummary,
}

impl SyncPage {
    /// Can the database be exported?
    ///
    /// Always `Err` today. Returning a REASON rather than false means the view
    /// can explain itself, and means the day the backend lands there is exactly
    /// one place to change.
    pub fn export(&self) -> Result<(), Unavailable> { Err(Unavailable::NotImplemented) }

    pub fn import(&self) -> Result<(), Unavailable> { Err(Unavailable::NotImplemented) }

    pub fn transfer_available(&self) -> bool { self.export().is_ok() }
}

/// Draw the page.
pub fn sync_ui(ui: &mut egui::Ui, p: &SyncPage, c: &uikit_egui::PaletteColors) {
    ui.label(egui::RichText::new("📍 Local database").strong());
    ui.add_space(6.0);
    egui::Grid::new("sync_db").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        for (label, value) in p.db.rows() {
            ui.colored_label(c.text_weak, label);
            ui.colored_label(c.text, value);
            ui.end_row();
        }
    });

    ui.add_space(12.0);
    let can = p.transfer_available();
    // Disabled rather than hidden: a greyed button with a reason underneath
    // tells the user the capability exists and is coming. A missing button
    // tells them nothing, and they go looking for it in settings.
    ui.add_enabled(can, egui::Button::new("📤 Export database"));
    ui.add_enabled(can, egui::Button::new("📥 Import database"));

    if !can {
        ui.add_space(10.0);
        ui.label(egui::RichText::new("ℹ️  Note").strong());
        ui.colored_label(c.text_weak, SYNC_NOTE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(cal: u32, lin: u32, neu: u32, model: &str) -> DbSummary {
        DbSummary { calibrations: cal, linear: lin, neural: neu,
                    model_type: model.into() }
    }

    #[test]
    fn transfer_is_ported_unavailable_rather_than_invented() {
        // The Qt page has enabled:false on both buttons and an onClicked that
        // logs "not yet wired up". Finishing it during a port would be writing
        // a new feature under cover of porting one — and the most dangerous
        // kind, because sync that half works loses data quietly.
        let p = SyncPage::default();
        assert_eq!(p.export(), Err(Unavailable::NotImplemented));
        assert_eq!(p.import(), Err(Unavailable::NotImplemented));
        assert!(!p.transfer_available());
    }

    #[test]
    fn the_note_says_where_the_capability_does_exist() {
        // A disabled button with a reason is information; a missing button is
        // not, and sends the user hunting through settings.
        assert!(SYNC_NOTE.contains("GTK"));
    }

    #[test]
    fn the_counts_are_a_breakdown_rather_than_a_total() {
        // "48 calibrations" answers a different question from "46 linear, 2
        // neural", and the two kinds are not interchangeable.
        let rows = db(48, 46, 2, "neural").rows();
        assert_eq!(rows[0], ("Calibrations:", "48".to_string()));
        assert_eq!(rows[1], ("Linear:", "46".to_string()));
        assert_eq!(rows[2], ("Neural:", "2".to_string()));
    }

    #[test]
    fn the_active_model_kind_can_be_counted_separately() {
        // The case the total hides: plenty of data, almost none of it the kind
        // the active model can use.
        assert_eq!(db(48, 46, 2, "neural").active_kind_count(), Some(2));
        assert_eq!(db(48, 46, 2, "linear").active_kind_count(), Some(46));
    }

    #[test]
    fn an_unknown_model_kind_counts_nothing_rather_than_guessing() {
        // A third kind added later must not be silently attributed to one of
        // these two.
        assert_eq!(db(48, 46, 2, "gaussian").active_kind_count(), None);
        assert_eq!(db(48, 46, 2, "").active_kind_count(), None);
    }

    #[test]
    fn a_missing_model_type_shows_a_dash_not_a_blank_row() {
        // A blank looks like a failed read rather than an early one.
        let rows = db(0, 0, 0, "  ").rows();
        assert_eq!(rows[3], ("Active model:", "—".to_string()));
    }

    #[test]
    fn kinds_that_do_not_sum_to_the_total_are_noticed_not_asserted() {
        // The backend may count kinds this page does not know. Flagging it is
        // useful; asserting equality would fire on every screen the day a third
        // kind is added.
        assert!(db(48, 46, 2, "neural").has_other_kinds() == false);
        assert!(db(50, 46, 2, "neural").has_other_kinds());
    }

    #[test]
    fn an_empty_database_renders_zeroes_rather_than_dashes() {
        // Zero calibrations is a FACT — the user has not calibrated yet — and
        // differs from "we could not read the database". Only the model type,
        // which has no zero, uses a dash.
        let rows = db(0, 0, 0, "linear").rows();
        assert_eq!(rows[0].1, "0");
        assert_eq!(rows[3].1, "linear");
    }
}
