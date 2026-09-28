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
    ///
    /// No longer the only possibility: `sync.rs::export_database` and
    /// `import_database` DO exist, are GTK-free, and are substantial
    /// (gzip, signed packets, trusted-device verification on import). The
    /// Qt page's note — "the GTK UI has full sync support today" — was
    /// accurate, and this page reported `NotImplemented` for a backend
    /// that had already landed.
    NotImplemented,
    /// The export would silently omit part of the database.
    ///
    /// `export_database` builds its packet with `locations: Vec::new()` and
    /// a `// stub - would need to add to Database` comment, so an export
    /// taken today carries calibrations and no locations. That is worth
    /// stating rather than either hiding (a button that quietly drops data
    /// is this module's stated worst case — "sync that half works loses
    /// data quietly") or blocking (calibrations ARE the expensive thing to
    /// recreate, and an export that saves them is worth having).
    ///
    /// So: the action is offered, and the view says what it will not carry.
    Partial { missing: &'static str },
    /// No device has been paired, so `import_database` would reject the
    /// packet at the trust check regardless. Distinct from the above
    /// because the fix is a user action, not a code change.
    NoTrustedDevice,
}

impl Unavailable {
    /// Whether the action can still be performed despite the caveat.
    ///
    /// `Partial` is the only reason that does not BLOCK — it annotates.
    /// Folding it into a bool at the call site is what would turn "exports
    /// without locations" into "exports", which is the failure this whole
    /// enum exists to prevent.
    pub fn blocks(&self) -> bool {
        !matches!(self, Unavailable::Partial { .. })
    }

    /// One line for the view, beneath the button.
    pub fn describe(&self) -> String {
        match self {
            Unavailable::NotImplemented => SYNC_NOTE.to_string(),
            Unavailable::Partial { missing } => {
                format!("Exports calibrations. Does not yet include {missing}.")
            }
            Unavailable::NoTrustedDevice => {
                "Pair a device first — an incoming database from an unknown \
                 device is rejected."
                    .to_string()
            }
        }
    }
}

/// The note the Qt page shows beneath the disabled buttons.
pub const SYNC_NOTE: &str =
    "Export / import need a few more backend methods. The GTK UI has full sync \
     support today.";

/// The page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncPage {
    pub db: DbSummary,
    /// What the HOST knows about the transfer backend.
    ///
    /// Defaults to `None`, which reads as "this frontend has not been wired
    /// to a backend" and reproduces the original ported behaviour exactly —
    /// so a host that says nothing still gets the disabled buttons and the
    /// Qt note, and nothing regresses for one that has not been updated.
    ///
    /// Set by the host rather than probed here because this module has no
    /// backend types in it at all, which is what lets the whole page be
    /// tested without a database, a device or a frame. Probing
    /// `sync.rs::SyncManager` from inside the view would trade that away
    /// for a capability check the host already has cheaply.
    pub backend: Option<TransferBackend>,
    /// The path typed into the import row. The GTK page opens a file
    /// chooser; egui has no native one without another dependency, and a
    /// phone's file chooser is a portal this build does not talk to yet,
    /// so the path is typed (or pasted) instead.
    pub import_path: String,
    /// The last export or import's outcome, from the host.
    pub note: Option<String>,
    /// Where exports go, so the note can say it before the first one.
    pub export_dir: String,
}

/// What the page asks the host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    /// Snapshot the live database to the export directory.
    Export,
    /// Merge calibrations from the database at this path.
    Import(String),
}

/// What the host's transfer backend can currently do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransferBackend {
    /// `sync.rs::export_database` is reachable.
    pub can_export: bool,
    /// `sync.rs::import_database` is reachable AND at least one device is
    /// paired — import verifies the sender against the trusted list before
    /// doing anything, so an unpaired device cannot import at all.
    pub can_import: bool,
    /// Whether the export still omits locations (the `Vec::new()` stub in
    /// `export_database`). The host owns this because it is a fact about
    /// the backend build, not about the view.
    pub exports_locations: bool,
}

impl SyncPage {
    /// Can the database be exported?
    ///
    /// Returns a REASON rather than false so the view can explain itself.
    /// `Ok` and `Err(Partial)` are both actionable — see `Unavailable::blocks`.
    pub fn export(&self) -> Result<(), Unavailable> {
        match self.backend {
            None => Err(Unavailable::NotImplemented),
            Some(b) if !b.can_export => Err(Unavailable::NotImplemented),
            Some(b) if !b.exports_locations => {
                Err(Unavailable::Partial { missing: "saved locations" })
            }
            Some(_) => Ok(()),
        }
    }

    pub fn import(&self) -> Result<(), Unavailable> {
        match self.backend {
            None => Err(Unavailable::NotImplemented),
            Some(b) if !b.can_import => Err(Unavailable::NoTrustedDevice),
            Some(_) => Ok(()),
        }
    }

    /// Whether the transfer buttons should be clickable.
    ///
    /// `Partial` counts as available: an export that carries every
    /// calibration and no locations is worth taking, and the caveat is
    /// shown beneath rather than used to grey the button out. Calibrations
    /// are the expensive thing to recreate.
    pub fn transfer_available(&self) -> bool {
        match self.export() {
            Ok(()) => true,
            Err(u) => !u.blocks(),
        }
    }

    /// Whether each transfer button should be clickable, and the notes to
    /// show beneath them — deduplicated, so two actions blocked for the
    /// same reason produce one line rather than the same sentence twice.
    ///
    /// Lives here rather than inside `sync_ui` because it is a DECISION,
    /// not drawing, and a decision inside a `&mut Ui` function is one no
    /// test reaches. Not hypothetical: a break-test that regressed import
    /// to being gated on export — the exact bug separating the two buttons
    /// was meant to fix — passed every test in this module while that
    /// logic was inline in the view.
    pub fn button_state(&self) -> ButtonState {
        let exp = self.export();
        let imp = self.import();
        let ok = |r: &Result<(), Unavailable>| r.as_ref().err().map_or(true, |u| !u.blocks());
        let mut notes: Vec<String> = Vec::new();
        for r in [exp.as_ref().err(), imp.as_ref().err()].into_iter().flatten() {
            let d = r.describe();
            if !notes.contains(&d) {
                notes.push(d);
            }
        }
        ButtonState { export: ok(&exp), import: ok(&imp), notes }
    }
}

/// What the view draws: two enable flags and the notes under them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonState {
    pub export: bool,
    pub import: bool,
    pub notes: Vec<String>,
}

/// Draw the page. Returns what was pressed; the host performs it.
pub fn sync_ui(ui: &mut egui::Ui, p: &mut SyncPage, c: &uikit_egui::PaletteColors) -> Option<SyncAction> {
    let mut out = None;
    crate::ui::egui::chrome::card_heading(ui, "📍 Local database");
    ui.add_space(6.0);
    egui::Grid::new("sync_db").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        for (label, value) in p.db.rows() {
            ui.colored_label(c.text_weak, label);
            ui.colored_label(c.text, value);
            ui.end_row();
        }
    });

    ui.add_space(12.0);
    // Each button asks its OWN question. They were both gated on
    // `transfer_available()` (i.e. on export) when both were permanently
    // disabled and the distinction could not show. It can now: import is
    // blocked by having no paired device, which export does not care
    // about, and greying export because nothing is paired would be wrong.
    // Every decision here comes from `button_state`; this function only
    // draws. See its doc for why.
    let st = p.button_state();
    // Disabled rather than hidden: a greyed button with a reason underneath
    // tells the user the capability exists and is coming. A missing button
    // tells them nothing, and they go looking for it in settings.
    if ui.add_enabled(st.export, egui::Button::new("📤 Export database")).clicked() {
        out = Some(SyncAction::Export);
    }
    if !p.export_dir.is_empty() {
        ui.colored_label(c.text_weak, egui::RichText::new(format!("to {}", p.export_dir)).small());
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_enabled(st.import, egui::TextEdit::singleline(&mut p.import_path)
            .hint_text("path to a sensors_*.db file").desired_width(220.0));
        let can = st.import && !p.import_path.trim().is_empty();
        if ui.add_enabled(can, egui::Button::new("📥 Import database")).clicked() {
            out = Some(SyncAction::Import(p.import_path.trim().to_string()));
        }
    });
    if let Some(n) = &p.note {
        ui.colored_label(c.text, egui::RichText::new(n).small());
    }

    let notes = st.notes;
    if !notes.is_empty() {
        ui.add_space(10.0);
        crate::ui::egui::chrome::card_heading(ui, "ℹ️  Note");
        for n in &notes {
            ui.colored_label(c.text_weak, n);
        }
    }
    out
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
    fn a_wired_backend_enables_export_and_a_stubbed_one_annotates_it() {
        // `sync.rs::export_database` builds its packet with
        // `locations: Vec::new()` and a "would need to add to Database"
        // comment. An export taken against that build carries every
        // calibration and no locations.
        //
        // Offered anyway, with the omission stated: calibrations are the
        // expensive thing to recreate, so an export that saves them is worth
        // having. What must NOT happen is offering it silently — this
        // module's own doc names quiet data loss as the worst outcome, and
        // a bool return would have collapsed "exports everything" and
        // "exports most things" into one value.
        let stub = SyncPage {
            db: db(3, 3, 0, "linear"),
            backend: Some(TransferBackend {
                can_export: true,
                can_import: true,
                exports_locations: false,
            }),
            ..Default::default()
        };
        assert_eq!(
            stub.export(),
            Err(Unavailable::Partial { missing: "saved locations" })
        );
        assert!(
            stub.transfer_available(),
            "a partial export is still worth offering"
        );
        assert!(
            stub.export().unwrap_err().describe().contains("locations"),
            "the caveat must name what is missing"
        );

        let complete = SyncPage {
            backend: Some(TransferBackend {
                can_export: true,
                can_import: true,
                exports_locations: true,
            }),
            ..Default::default()
        };
        assert_eq!(complete.export(), Ok(()));
    }

    #[test]
    fn import_is_blocked_by_pairing_not_by_export() {
        // The two buttons ask different questions, which could not show while
        // both were permanently disabled. `import_database` verifies the
        // sender against the trusted-device list before doing anything, so
        // an unpaired device cannot import — but that has no bearing on
        // whether export works, and greying export for it would be wrong.
        let p = SyncPage {
            backend: Some(TransferBackend {
                can_export: true,
                can_import: false,
                exports_locations: true,
            }),
            ..Default::default()
        };
        assert_eq!(p.export(), Ok(()), "export does not care about pairing");
        assert_eq!(p.import(), Err(Unavailable::NoTrustedDevice));
        assert!(p.import().unwrap_err().blocks(), "unpaired import must be blocked");
    }

    #[test]
    fn only_partial_is_non_blocking() {
        // The distinction the view relies on. If `Partial` ever started
        // blocking, the stubbed export would silently go back to being a
        // greyed button; if anything else stopped blocking, a broken action
        // would become clickable.
        assert!(!Unavailable::Partial { missing: "x" }.blocks());
        assert!(Unavailable::NotImplemented.blocks());
        assert!(Unavailable::NoTrustedDevice.blocks());
    }

    #[test]
    fn the_two_buttons_are_enabled_independently() {
        // THE case a break-test escaped. Regressing import to `imp_ok =
        // exp_ok` — reintroducing the exact coupling that separating them
        // was meant to remove — failed nothing, because the enable logic
        // was inline in `sync_ui` where no test could reach it. Now it is
        // `button_state`, and this asserts on it.
        let p = SyncPage {
            backend: Some(TransferBackend {
                can_export: true,
                can_import: false,
                exports_locations: true,
            }),
            ..Default::default()
        };
        let st = p.button_state();
        assert!(st.export, "export works with no device paired");
        assert!(!st.import, "import does not");
        assert_eq!(st.notes.len(), 1, "only the blocked action explains itself");
        assert!(st.notes[0].contains("Pair a device"));
    }

    #[test]
    fn one_reason_shared_by_both_actions_prints_once() {
        // The default (unwired) case: both blocked by NotImplemented. Two
        // identical sentences stacked under the buttons reads as a bug.
        let st = SyncPage::default().button_state();
        assert!(!st.export && !st.import);
        assert_eq!(st.notes, vec![SYNC_NOTE.to_string()]);
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
