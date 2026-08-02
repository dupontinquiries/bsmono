//! Models tab — the model library as a reviewable list rather than a directory you hope is correct.
//!
//! Scans a models root, groups split checkpoints into single entries, and shows each one's status with the
//! action it needs. The organize action is deliberately two-step: pressing "Plan" computes the moves and
//! shows them, and only then does a separate confirm actually touch the disk. Reorganizing a model library
//! moves tens of gigabytes, and a mis-click that clobbers a shard costs a very long re-download — so the
//! plan is always visible before anything happens, and a destination that already exists is reported as a
//! conflict rather than overwritten.
//!
//! All of the grouping/planning logic lives in `logscope::modelorg` (pure and unit-tested); this file is
//! presentation and confirmation only.

use crate::theme;
use egui::RichText;
use logscope::modelorg::{self, ModelEntry, Move, Status};
use std::path::PathBuf;

pub struct Models {
    root: String,
    entries: Vec<ModelEntry>,
    scanned: bool,
    /// This machine's id, derived from its public key. Shown so it can be transcribed elsewhere.
    my_id: String,
    /// Trusted machines, loaded from disk and saved on every change.
    ///
    /// Held here rather than rebuilt per frame because a registry that is reloaded each frame would
    /// discard an edit before the save, and saved on every change rather than on exit because a
    /// crash between the two loses exactly the machine someone just added.
    trusted: logscope_fan::trustkit::Registry,
    /// What went wrong with the last add or save, if anything.
    trust_note: Option<String>,
    add_id: String,
    machines: Vec<String>,
    machine_note: String,
    /// Planned moves awaiting confirmation. Non-empty means "showing a plan, not yet applied".
    plan: Vec<Move>,
    conflicts: Vec<String>,
    note: String,
    repo_hint: String,
}

impl Default for Models {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        Models {
            root: format!("{home}/models"),
            entries: Vec::new(), scanned: false,
            // THE REAL KEY, not a placeholder. Same directory as the autosave state, so an operator
            // who backs up ~/.local/share/logscope keeps their machine identity with it — losing it
            // silently invalidates every registration other machines made against this one.
            //
            // A failure to load falls back to a visible marker rather than a plausible-looking id: an
            // id that parses and is nobody's is worse than one that obviously did not load, because
            // the first gets pasted somewhere.
            trusted: logscope_fan::trustkit::Registry::load(&trust_path()).0,
            trust_note: None,
            my_id: Some(state_dir().join(logscope_fan::clusternode::IDENTITY_FILE))
                .and_then(|p| logscope_fan::machineid::Identity::load_or_create(&p).ok())
                .map(|id| id.id())
                .unwrap_or_else(|| "— identity unavailable —".into()),
            add_id: String::new(), machines: Vec::new(), machine_note: String::new(),
            plan: Vec::new(), conflicts: Vec::new(),
            note: String::new(), repo_hint: String::new(),
        }
    }
}

impl Models {
    fn rescan(&mut self) {
        let root = PathBuf::from(&self.root);
        let files = modelorg::list_dir(&root);
        self.entries = modelorg::scan(&files);
        self.scanned = true;
        self.plan.clear();
        self.conflicts.clear();
        self.note = if self.entries.is_empty() {
            format!("no .gguf files directly in {} (already organized into subfolders?)", self.root)
        } else {
            format!("{} model(s) across {} file(s)", self.entries.len(),
                    self.entries.iter().map(|e| e.files.len()).sum::<usize>())
        };
    }

    fn build_plan(&mut self) {
        let root = PathBuf::from(&self.root);
        let (moves, conflicts) = modelorg::plan_moves(&root, &self.entries, &|p| p.exists());
        self.plan = moves;
        self.conflicts = conflicts;
        self.note = if self.plan.is_empty() && self.conflicts.is_empty() {
            "nothing to move — the library is already organized".into()
        } else {
            format!("{} file(s) would move, {} conflict(s) — review below, then Apply",
                    self.plan.len(), self.conflicts.len())
        };
    }

    fn apply(&mut self) {
        let (done, errs) = modelorg::apply_moves(&self.plan);
        self.note = format!("moved {} file(s){}", done.len(),
            if errs.is_empty() { String::new() } else { format!("; {} error(s): {}", errs.len(), errs.join("; ")) });
        self.plan.clear();
        self.conflicts.clear();
        self.rescan();
    }

    /// Trusted machines, buried under a collapsed header.
    ///
    /// Buried on purpose: this is set once per machine and then never touched, so it should not
    /// occupy space in the view someone uses every day. It lives in the model selector because that is
    /// where you already go to decide WHERE work runs — a machine and a model are the same decision
    /// from the operator's side.
    fn machines_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("trusted machines").default_open(false).show(ui, |ui| {
            // THIS machine's id, shown so it can be read off and typed into another. It is a
            // fingerprint of the public key, not a name — so pasting it IS the key exchange, and there
            // is no second step to forget.
            // NOT YET A REAL IDENTITY, and the UI must say so. The id derives from a fixed
            // placeholder string, so it is stable, well-formed, and THE SAME ON EVERY MACHINE — an
            // operator who copies it to a second device would register the first device's twin and
            // wonder why nothing connects.
            //
            // Showing a plausible id for an identity that does not exist is the same class as
            // "✓ verified" for a run with no verifier: a display that is affirmative by default.
            // The identity is REAL — generated once and persisted at
            // ~/.local/share/logscope/identity.key. I added a "placeholder identity" warning here
            // while the wiring was in flight and it outlived the thing it warned about, which is a
            // stale label claiming a defect that no longer exists — the same class as a stale gate,
            // pointed the other way.
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("this machine:").small().color(theme::DIM()));
                ui.label(egui::RichText::new(&self.my_id).monospace().small());
                if ui.small_button("copy").clicked() { ui.ctx().copy_text(self.my_id.clone()); }
            });
            ui.label(egui::RichText::new(
                "Type another machine's id here to trust it. The id is a fingerprint of its key, so \
there is nothing else to exchange.").small().color(theme::FAINT()));
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.add_id)
                    .hint_text("xxxxx-xxxxx-xxxxx-xx").desired_width(190.0));
                if ui.small_button("add").clicked() {
                    match logscope_fan::machineid::parse(&self.add_id) {
                        // The error is shown rather than swallowed: a mistyped id would otherwise be
                        // an id nobody holds, and the failure would surface later as "that machine is
                        // offline" — pointing at the network instead of at the typing.
                        Err(e) => self.machine_note = e.explain(),
                        Ok(id) => {
                            // INTO THE REGISTRY AND ONTO DISK. This pushed to a plain Vec that nothing read and
                            // nothing saved — a machine "added" in the UI was trusted by no code and gone on
                            // restart, which is worse than no button: the operator believes the step is done.
                            //
                            // Compute is the default level. It can run inference and cannot receive a workspace
                            // handoff — granting the strongest level from a typed id would make one keystroke the
                            // whole authorisation.
                            self.trusted.register(&id, logscope_fan::trustkit::Trust::Compute);
                            self.machines.push(id.clone());
                            self.add_id.clear();
                            // UNVERIFIED, not "added": an id alone carries no key, so the machine is listed and
                            // cannot yet be authenticated. "added" would overstate what just happened.
                            self.machine_note = match self.trusted.save(&trust_path()) {
                                Ok(()) => format!("added {id} — unverified until it announces itself"),
                                Err(e) => format!("added {id} FOR THIS SESSION ONLY — could not save: {e}"),
                            };
                        }
                    }
                }
            });
            if !self.machine_note.is_empty() {
                ui.label(egui::RichText::new(&self.machine_note).small().color(theme::DIM()));
            }
            // FROM THE REGISTRY, not from a session Vec — so the list survives a restart and shows
            // what is actually trusted rather than what was typed since launch.
            //
            // The verified/unverified distinction is the point of showing it at all: an entry with no
            // key cannot be authenticated, so it is listed and cannot be acted on. trustkit calls that
            // unverifiable(), and a list that hid it would imply a working link that does not exist.
            let mut revoke: Option<String> = None;
            for (id, level, verified) in self.trusted.entries_for_display() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("  {id}")).monospace().small());
                    ui.label(egui::RichText::new(if verified { "✓ key on file" }
                        else { "⚠ unverified — no key yet" })
                        .small().color(if verified { theme::GREEN() } else { theme::AMBER() }));
                    ui.label(egui::RichText::new(format!("{level:?}")).small().color(theme::DIM()));
                    if ui.small_button("revoke").clicked() { revoke = Some(id.clone()); }
                });
            }
            // Revoked outside the loop: mutating the registry while iterating it is the borrow the
            // compiler would refuse, and collecting first is also what makes the save happen once.
            if let Some(id) = revoke {
                self.trusted.revoke(&id);
                self.machine_note = match self.trusted.save(&trust_path()) {
                    Ok(()) => format!("revoked {id}"),
                    // A revocation that failed to save comes BACK on restart, which is the worst
                    // direction for this error to go — so it says so plainly.
                    Err(e) => format!("revoked {id} for this session only — could NOT save: {e}"),
                };
            }
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.machines_ui(ui);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Models root").color(theme::FAINT()));
            ui.add(egui::TextEdit::singleline(&mut self.root).desired_width(320.0));
            if theme::action_button(ui, "⟳ Scan", theme::CYAN(), true).clicked() { self.rescan(); }
            let can_plan = self.scanned && !self.entries.is_empty();
            if theme::action_button(ui, "Plan organize", theme::AMBER(), can_plan).clicked() { self.build_plan(); }
        });
        if !self.note.is_empty() {
            ui.add_space(theme::PAD_XS as f32);
            ui.label(RichText::new(&self.note).color(theme::FAINT()));
        }
        ui.add_space(theme::PAD_SM as f32);

        if !self.scanned {
            ui.label(RichText::new("Scan a models directory to see the library.").color(theme::FAINT()));
            return;
        }

        // The plan, when one exists — shown BEFORE anything is applied, with Apply/Cancel.
        if !self.plan.is_empty() || !self.conflicts.is_empty() {
            ui.group(|ui| {
                ui.label(RichText::new("Planned changes (nothing has moved yet)").strong());
                egui::ScrollArea::vertical().max_height(180.0).id_salt("plan").show(ui, |ui| {
                    for m in &self.plan {
                        ui.label(RichText::new(format!("{}  →  {}",
                            m.from.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                            m.to.parent().map(|p| p.display().to_string()).unwrap_or_default()))
                            .monospace().color(theme::CYAN()));
                    }
                    for c in &self.conflicts {
                        ui.label(RichText::new(format!("conflict: {c}")).monospace().color(theme::AMBER()));
                    }
                });
                ui.horizontal(|ui| {
                    if theme::action_button(ui, "✓ Apply moves", theme::CYAN(), !self.plan.is_empty()).clicked() {
                        self.apply();
                    }
                    if theme::action_button(ui, "Cancel", theme::FAINT(), true).clicked() {
                        self.plan.clear(); self.conflicts.clear();
                        self.note = "plan discarded — nothing moved".into();
                    }
                });
            });
            ui.add_space(theme::PAD_SM as f32);
        }

        // Optional repo for fetch commands: left blank we print a placeholder rather than guessing, since
        // a filename does not determine which repository it came from.
        ui.horizontal(|ui| {
            ui.label(RichText::new("HF repo (for fetch commands)").color(theme::FAINT()));
            ui.add(egui::TextEdit::singleline(&mut self.repo_hint)
                .hint_text("owner/repo — leave blank to print a placeholder")
                .desired_width(280.0));
        });
        ui.add_space(theme::PAD_XS as f32);

        let repo = if self.repo_hint.trim().is_empty() { None } else { Some(self.repo_hint.trim()) };
        egui::ScrollArea::vertical().id_salt("models").show(ui, |ui| {
            for e in &self.entries {
                let gib = e.bytes as f64 / (1u64 << 30) as f64;
                let (tag, colour) = match &e.status {
                    Status::Single => ("ok", theme::CYAN()),
                    Status::SplitComplete { .. } => ("ok · split", theme::CYAN()),
                    Status::SplitIncomplete { .. } => ("INCOMPLETE", theme::AMBER()),
                    Status::SplitInconsistent { .. } => ("BROKEN", theme::AMBER()),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("[{tag}]")).monospace().color(colour));
                    ui.label(RichText::new(&e.name).strong());
                    ui.label(RichText::new(format!("{gib:.1} GiB · {}", e.family())).color(theme::FAINT()));
                });
                // Each problem is printed with its remedy, as a copyable command.
                match &e.status {
                    Status::SplitComplete { total } => {
                        ui.label(RichText::new(format!("    {total} shards — merge into one file:")).color(theme::FAINT()));
                        if let Some(cmd) = modelorg::merge_plan(&modelorg::target_dir(&PathBuf::from(&self.root), e), e) {
                            ui.label(RichText::new(format!("    {cmd}")).monospace().color(theme::CYAN()));
                        }
                    }
                    Status::SplitIncomplete { have, missing, total } => {
                        ui.label(RichText::new(format!("    have {}/{total} shards, missing {missing:?} — do NOT merge or load until complete", have.len()))
                            .color(theme::AMBER()));
                        if let Some(cmds) = modelorg::fetch_plan(e, repo) {
                            for c in cmds { ui.label(RichText::new(format!("    {c}")).monospace().color(theme::CYAN())); }
                        }
                    }
                    Status::SplitInconsistent { detail } => {
                        ui.label(RichText::new(format!("    {detail} — the shard set is not coherent; re-download it"))
                            .color(theme::AMBER()));
                    }
                    Status::Single => {}
                }
                ui.add_space(theme::PAD_XS as f32);
            }
        });
    }
}

/// Where the trust list lives — beside the identity, because they are one setting.
/// Where the trusted-devices list lives.
///
/// The FILENAME comes from logscope_fan rather than being spelled again here. It was written out in
/// both places and they happened to agree — which is the state a path spends right up until one of
/// them is edited, and then a device list silently splits in two with neither half wrong.
fn trust_path() -> std::path::PathBuf {
    state_dir().join(logscope_fan::clusternode::TRUSTED_FILE)
}

/// The directory holding identity, trusted devices and the chat autosave.
///
/// One definition, because an operator backing up this directory should get all of it — and a second
/// spelling of the path is how one file ends up somewhere the backup does not reach.
pub fn state_dir() -> std::path::PathBuf {
    std::env::var("HOME").ok().map(std::path::PathBuf::from).unwrap_or_default()
        .join(".local/share/logscope")
}
