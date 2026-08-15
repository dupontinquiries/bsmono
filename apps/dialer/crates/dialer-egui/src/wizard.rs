//! The dialer's setup wizard — same pattern as the kit's `setup_wizard`
//! (pure phase machine + thin egui render), but its own module because the
//! kit's wizard *is* voiceterm's model-download flow, not a framework.
//!
//! Two steps, both skippable — a phone that can't finish setup must still be
//! a phone:
//!
//! 1. **Carrier / APN** — suggests the carrier matching the modem's reported
//!    MCC+MNC (from mobile-broadband-provider-info via `providers`), with
//!    the full searchable list behind it. Fields are editable before Apply,
//!    because carriers rebrand APNs faster than any database tracks.
//! 2. **Matrix** — homeserver + password sign-in (discovery + login run on a
//!    worker thread; the UI never blocks). Signalling-only in v1: the step
//!    says so instead of implying calls will have audio.

use std::sync::mpsc::{channel, Receiver};

use dialer_core::link::ApnSetting;
use dialer_core::command_guard::GuardPolicy;
use dialer_core::matrix::Creds;
use dialer_core::providers::{self, Apn, Source};
use uikit_egui::PaletteColors;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Carrier,
    Matrix,
    Commands,
    Done,
}

/// What moves the wizard forward. Pure, unit-tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Next,
    Back,
    Finish,
}

pub fn next_phase(current: Phase, step: Step) -> Phase {
    match (current, step) {
        (Phase::Carrier, Step::Next) => Phase::Matrix,
        (Phase::Matrix, Step::Next) => Phase::Commands,
        (Phase::Commands, Step::Next) => Phase::Done,
        (Phase::Matrix, Step::Back) => Phase::Carrier,
        (Phase::Commands, Step::Back) => Phase::Matrix,
        (Phase::Done, Step::Back) => Phase::Commands,
        (_, Step::Finish) => Phase::Done,
        (p, _) => p,
    }
}

/// What the shell must act on. The wizard mutates only itself; applying an
/// APN, spawning the matrix link and closing the overlay are shell concerns.
#[derive(Debug)]
pub enum WizardOut {
    ApplyApn(ApnSetting),
    MatrixSignedIn(Creds),
    MatrixForgotten,
    SetGuardPolicy(GuardPolicy),
    Close,
}

/// First-run marker, next to the history file.
fn marker_path() -> std::path::PathBuf {
    dialer_core::history::HistoryStore::default_path()
        .parent()
        .map(|d| d.join("setup-done"))
        .unwrap_or_else(|| std::path::PathBuf::from("setup-done"))
}

pub fn setup_was_completed() -> bool {
    marker_path().exists()
}

pub fn mark_setup_completed() {
    if let Some(dir) = marker_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(marker_path(), b"");
}

pub struct Wizard {
    pub phase: Phase,
    // carrier step
    apns: Vec<Apn>,
    source: Source,
    search: String,
    /// Editable fields, filled by tapping a carrier.
    carrier: String,
    apn: String,
    username: String,
    password: String,
    apply_feedback: Option<String>,
    // matrix step
    mx_server: String,
    mx_user: String,
    mx_password: String,
    mx_status: String,
    mx_busy: Option<Receiver<Result<Creds, String>>>,
    mx_signed_in: Option<String>, // user_id
    // commands step
    policy: GuardPolicy,
    trusted_raw: String,
    prefixes_raw: String,
    roots_raw: String,
}

impl Wizard {
    pub fn new(matrix_user: Option<String>, policy: GuardPolicy) -> Wizard {
        let (apns, source) = providers::load();
        let trusted_raw = policy.trusted_senders.join("\n");
        let prefixes_raw = policy.allowed_prefixes.join("\n");
        let roots_raw = policy
            .view_roots
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        Wizard {
            policy,
            trusted_raw,
            prefixes_raw,
            roots_raw,
            phase: Phase::Carrier,
            apns,
            source,
            search: String::new(),
            carrier: String::new(),
            apn: String::new(),
            username: String::new(),
            password: String::new(),
            apply_feedback: None,
            mx_server: String::new(),
            mx_user: String::new(),
            mx_password: String::new(),
            mx_status: String::new(),
            mx_busy: None,
            mx_signed_in: matrix_user,
        }
    }

    /// Called by the shell when a backend reports the APN result, so the
    /// wizard shows it where the Apply button is (a toast alone scrolls away
    /// from the decision point).
    pub fn note_apply_result(&mut self, msg: &str) {
        self.apply_feedback = Some(msg.to_string());
    }

    fn pick(&mut self, a: &Apn) {
        self.carrier = a.carrier.clone();
        self.apn = a.apn.clone();
        self.username = a.username.clone();
        self.password = a.password.clone();
        self.apply_feedback = None;
    }

    fn carrier_ui(
        &mut self,
        ui: &mut egui::Ui,
        colors: &PaletteColors,
        operator: Option<&str>,
        out: &mut Vec<WizardOut>,
    ) {
        ui.heading("Cellular data (APN)");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!(
                "Carriers from the {} — mobile-broadband-provider-info, the same \
                 dataset NetworkManager uses.",
                self.source.label()
            ))
            .small()
            .color(colors.text_weak),
        );
        if self.source == Source::Builtin {
            ui.label(
                egui::RichText::new(format!("refresh: curl -o <data dir>/serviceproviders.xml {}",
                    providers::REFRESH_URL))
                .small()
                .monospace()
                .color(colors.text_weak),
            );
        }
        ui.add_space(8.0);

        // Suggestion by registered operator, when the modem told us one.
        let suggested: Vec<Apn> = operator
            .map(|code| providers::suggest(&self.apns, code).into_iter().cloned().collect())
            .unwrap_or_default();
        if let Some(code) = operator {
            if suggested.is_empty() {
                ui.label(
                    egui::RichText::new(format!("network {code} not in the database — pick manually"))
                        .color(colors.text_weak),
                );
            } else {
                ui.label(egui::RichText::new(format!("Detected network {code}:")).strong());
                let picks: Vec<Apn> = suggested.clone();
                for a in &picks {
                    if ui
                        .button(format!("★ {} — {} ({})", a.carrier, a.apn, a.country))
                        .clicked()
                    {
                        self.pick(a);
                    }
                }
            }
            ui.add_space(6.0);
        }

        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.search);
        });
        let hits: Vec<Apn> =
            providers::search(&self.apns, &self.search).into_iter().take(30).cloned().collect();
        egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
            for a in &hits {
                if ui.button(format!("{} — {} ({})", a.carrier, a.apn, a.country)).clicked() {
                    self.pick(a);
                }
            }
        });

        ui.add_space(8.0);
        egui::Grid::new("apn-fields").num_columns(2).show(ui, |ui| {
            ui.label("Carrier");
            ui.text_edit_singleline(&mut self.carrier);
            ui.end_row();
            ui.label("APN");
            ui.text_edit_singleline(&mut self.apn);
            ui.end_row();
            ui.label("Username");
            ui.text_edit_singleline(&mut self.username);
            ui.end_row();
            ui.label("Password");
            ui.text_edit_singleline(&mut self.password);
            ui.end_row();
        });
        if let Some(fb) = &self.apply_feedback {
            ui.label(egui::RichText::new(fb).small().color(colors.text_weak));
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ready = !self.apn.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Apply APN")).clicked() {
                self.apply_feedback = Some("applying…".into());
                out.push(WizardOut::ApplyApn(ApnSetting {
                    carrier: if self.carrier.trim().is_empty() {
                        "cellular".into()
                    } else {
                        self.carrier.trim().to_string()
                    },
                    apn: self.apn.trim().to_string(),
                    username: self.username.trim().to_string(),
                    password: self.password.trim().to_string(),
                }));
            }
        });
    }

    fn matrix_ui(&mut self, ui: &mut egui::Ui, colors: &PaletteColors, out: &mut Vec<WizardOut>) {
        ui.heading("Matrix");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "Incoming Matrix calls ring here and can be declined. \
                 Answering (audio) needs a media engine and isn't in yet.",
            )
            .small()
            .color(colors.text_weak),
        );
        ui.add_space(8.0);

        if let Some(user) = self.mx_signed_in.clone() {
            ui.label(format!("Signed in as {user}"));
            if ui.button("Sign out / forget session").clicked() {
                self.mx_signed_in = None;
                out.push(WizardOut::MatrixForgotten);
            }
            return;
        }

        // A login in flight?
        if let Some(rx) = &self.mx_busy {
            match rx.try_recv() {
                Ok(Ok(creds)) => {
                    self.mx_signed_in = Some(creds.user_id.clone());
                    self.mx_status.clear();
                    self.mx_busy = None;
                    self.mx_password.clear();
                    out.push(WizardOut::MatrixSignedIn(creds));
                }
                Ok(Err(e)) => {
                    self.mx_status = e;
                    self.mx_busy = None;
                }
                Err(_) => {
                    ui.spinner();
                    ui.label("signing in…");
                    return;
                }
            }
        }

        egui::Grid::new("mx-fields").num_columns(2).show(ui, |ui| {
            ui.label("Homeserver");
            ui.text_edit_singleline(&mut self.mx_server);
            ui.end_row();
            ui.label("User");
            ui.text_edit_singleline(&mut self.mx_user);
            ui.end_row();
            ui.label("Password");
            ui.add(egui::TextEdit::singleline(&mut self.mx_password).password(true));
            ui.end_row();
        });
        if !self.mx_status.is_empty() {
            ui.label(egui::RichText::new(&self.mx_status).color(colors.danger));
        }
        ui.add_space(6.0);

        #[cfg(feature = "matrix")]
        {
            let ready = !self.mx_server.trim().is_empty()
                && !self.mx_user.trim().is_empty()
                && !self.mx_password.is_empty();
            if ui.add_enabled(ready, egui::Button::new("Sign in")).clicked() {
                let server = self.mx_server.trim().to_string();
                let user = self.mx_user.trim().to_string();
                let pass = self.mx_password.clone();
                let (tx, rx) = channel();
                self.mx_busy = Some(rx);
                self.mx_status.clear();
                std::thread::spawn(move || {
                    // discover → login; both blocking, so: worker thread.
                    let base = dialer_core::matrix::discover(&server);
                    let _ = tx.send(dialer_core::matrix::login(&base, &user, &pass));
                });
            }
        }
        #[cfg(not(feature = "matrix"))]
        ui.label(
            egui::RichText::new("built without the `matrix` feature").color(colors.warning),
        );
    }

    fn commands_ui(&mut self, ui: &mut egui::Ui, colors: &PaletteColors, out: &mut Vec<WizardOut>) {
        ui.heading("Shared commands & files");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "A contact can send a suggested command or a file. Commands are                  NEVER run automatically — you see the exact text and confirm each one.",
            )
            .small()
            .color(colors.text_weak),
        );
        ui.add_space(8.0);

        ui.checkbox(&mut self.policy.allow_run, "Allow running received commands");
        if self.policy.allow_run {
            ui.label(
                egui::RichText::new(
                    "With this on, an inbound command shows a Run button that opens a                      confirmation. Only trusted senders + allowed prefixes skip the                      confirmation. Commands run as argv (no shell) — metacharacters are inert.",
                )
                .small()
                .color(colors.warning),
            );
            ui.add_space(6.0);
            ui.label("Trusted senders (one per line, e.g. matrix:@me:x.org, tel:+15551234):");
            ui.add(egui::TextEdit::multiline(&mut self.trusted_raw).desired_rows(2).desired_width(f32::INFINITY));
            ui.label("Auto-run prefixes for trusted senders (one per line, e.g. 'git ', 'cargo test'):");
            ui.add(egui::TextEdit::multiline(&mut self.prefixes_raw).desired_rows(2).desired_width(f32::INFINITY));
        }
        ui.add_space(6.0);
        ui.label("Folders a received 'view:' path may open (one per line; empty = deny all):");
        ui.add(egui::TextEdit::multiline(&mut self.roots_raw).desired_rows(2).desired_width(f32::INFINITY));

        ui.add_space(8.0);
        if ui.button("Save command policy").clicked() {
            self.policy.trusted_senders =
                self.trusted_raw.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
            self.policy.allowed_prefixes =
                self.prefixes_raw.lines().map(|l| l.to_string()).filter(|l| !l.trim().is_empty()).collect();
            self.policy.view_roots = self
                .roots_raw
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(std::path::PathBuf::from)
                .collect();
            out.push(WizardOut::SetGuardPolicy(self.policy.clone()));
        }
    }

    /// Render the current phase; returns actions for the shell. `operator`
    /// is the modem-reported MCC+MNC, when known.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        colors: &PaletteColors,
        operator: Option<&str>,
    ) -> Vec<WizardOut> {
        let mut out = Vec::new();
        let steps = ["1 · Carrier", "2 · Matrix", "3 · Commands", "4 · Done"];
        let idx = match self.phase {
            Phase::Carrier => 0,
            Phase::Matrix => 1,
            Phase::Commands => 2,
            Phase::Done => 3,
        };
        ui.horizontal(|ui| {
            for (i, s) in steps.iter().enumerate() {
                let c = if i == idx { colors.accent } else { colors.text_weak };
                ui.label(egui::RichText::new(*s).small().color(c));
            }
        });
        ui.separator();

        match self.phase {
            Phase::Carrier => self.carrier_ui(ui, colors, operator, &mut out),
            Phase::Matrix => self.matrix_ui(ui, colors, &mut out),
            Phase::Commands => self.commands_ui(ui, colors, &mut out),
            Phase::Done => {
                ui.heading("All set");
                ui.add_space(6.0);
                ui.label("You can reopen setup any time from the status bar.");
            }
        }

        ui.add_space(10.0);
        ui.separator();
        ui.horizontal(|ui| {
            if self.phase != Phase::Carrier && ui.button("‹ Back").clicked() {
                self.phase = next_phase(self.phase, Step::Back);
            }
            match self.phase {
                Phase::Done => {
                    if ui.button("Finish").clicked() {
                        mark_setup_completed();
                        out.push(WizardOut::Close);
                    }
                }
                _ => {
                    let next = ui.button("Next ›").clicked();
                    let skip = ui.button("Skip").clicked();
                    if next || skip {
                        self.phase = next_phase(self.phase, Step::Next);
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("✕").clicked() {
                    // Closing without finishing still counts as "seen" — a
                    // wizard that re-ambushes every launch teaches people to
                    // dismiss it without reading.
                    mark_setup_completed();
                    out.push(WizardOut::Close);
                }
            });
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_walk_forward_back_and_finish_jumps_to_done() {
        assert_eq!(next_phase(Phase::Carrier, Step::Next), Phase::Matrix);
        // Matrix -> Commands -> Done. A Commands phase was inserted between
        // them by the iOS-model commit; this test still asserted the older
        // two-step chain and failed on the merge.
        //
        // Updated rather than relaxed: walking the WHOLE chain in both
        // directions is what makes an inserted phase fail here, and that is
        // exactly the failure that just did its job.
        assert_eq!(next_phase(Phase::Matrix, Step::Next), Phase::Commands);
        assert_eq!(next_phase(Phase::Commands, Step::Next), Phase::Done);
        assert_eq!(next_phase(Phase::Matrix, Step::Back), Phase::Carrier);
        assert_eq!(next_phase(Phase::Commands, Step::Back), Phase::Matrix);
        assert_eq!(next_phase(Phase::Done, Step::Back), Phase::Commands);
        assert_eq!(next_phase(Phase::Carrier, Step::Finish), Phase::Done);
        // no-ops keep their phase
        assert_eq!(next_phase(Phase::Carrier, Step::Back), Phase::Carrier);
        assert_eq!(next_phase(Phase::Done, Step::Next), Phase::Done);
    }
}
