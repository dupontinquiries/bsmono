//! Branches page — the gitmind native store's branches. Opens the repo's chunk
//! store, lists branches with HEAD marked, and creates or deletes them.
//!
//! ## The read-only gap
//!
//! This page said "read-only list" while the Qt GUI had `branch_create`,
//! `branch_delete` and `branch_switch`. `gitmind_core::native` has carried
//! `branch_create` and `branch_delete` the whole time, so the port was one page
//! away from parity rather than one feature away.
//!
//! ## The guards live in core, and this page must not duplicate them
//!
//! `branch_create` refuses a duplicate name; `branch_delete` refuses the branch
//! HEAD points at. Re-checking those here would mean two implementations of one
//! rule, and the UI copy would drift from the one that actually runs — so the
//! page DISABLES the obviously-wrong action (delete on the current branch, an
//! empty name) and lets core reject everything else with its own message.
//!
//! ## Delete asks first
//!
//! A branch is a pointer, and deleting one in a content-addressed store loses
//! no chunks — but it can lose the only name for a line of work, and the user
//! cannot tell by looking which branches those are. One confirm click is cheap
//! against that.
use crate::app::App;
use gitmind_core::store::ChunkStore;

#[derive(Default)]
pub struct State {
    pub loaded: bool,
    pub branches: Vec<(String, String, bool)>, // (name, oid, is_head)
    pub err: String,
    /// New-branch name being typed.
    pub new_name: String,
    /// Branch awaiting a delete confirmation, if any.
    pub confirm_delete: Option<String>,
    /// Last successful action, shown until the next one.
    pub note: String,
}

/// Is this a name the store will accept?
///
/// Only the checks core cannot make for us: core rejects duplicates and
/// unknown names with its own messages, but it never sees a name the user has
/// not finished typing. Whitespace and ref-path characters are refused here
/// because `refs/heads/{name}` with a slash or a space produces a ref nobody
/// can type back.
pub fn name_is_usable(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty()
        && !n.contains(char::is_whitespace)
        && !n.contains('/')
        && !n.starts_with('-')
        && !n.contains("..")
}

fn open_store() -> Result<ChunkStore, String> {
    let root = cockpit::ops::repo_root();
    let dir = gitmind_core::portage::native_store_dir(&root);
    std::fs::create_dir_all(&dir).map_err(|_| "cannot create .gitmind store dir".to_string())?;
    ChunkStore::open(dir.join("store.redb"))
}

fn load(state: &mut State) {
    state.loaded = true;
    state.err.clear();
    match open_store() {
        Ok(store) => match gitmind_core::native::branch_list(&store) {
            Ok(bs) => {
                state.branches = bs.iter().map(|b| (b.name.clone(), b.oid.clone(), b.is_head)).collect();
            }
            Err(e) => state.err = e,
        },
        Err(e) => state.err = e,
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Branches");
    ui.label(egui::RichText::new("gitmind native-store branches (● = HEAD)").weak().small());
    ui.separator();

    if ui.button("↻ refresh").clicked() || !app.branches.loaded {
        load(&mut app.branches);
    }
    if !app.branches.err.is_empty() {
        ui.colored_label(egui::Color32::from_rgb(0xf7, 0x76, 0x8e), &app.branches.err);
    }
    if app.branches.branches.is_empty() && app.branches.err.is_empty() {
        ui.label(egui::RichText::new("no branches in the native store yet").weak());
    }
    if !app.branches.note.is_empty() {
        ui.label(egui::RichText::new(&app.branches.note).weak());
    }

    // ── create ──────────────────────────────────────────────────────────
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("New branch:");
        ui.text_edit_singleline(&mut app.branches.new_name);
        let usable = name_is_usable(&app.branches.new_name);
        // Disabled rather than hidden, so the button shows the feature exists
        // and the name field shows why it is not available yet.
        if ui.add_enabled(usable, egui::Button::new("create")).clicked() {
            let name = app.branches.new_name.trim().to_string();
            // Branches from HEAD — `start: None`. A start-point picker is a
            // different feature and pretending to offer one by taking a second
            // text field would invite a typo'd oid rather than a choice.
            match open_store().and_then(|s| gitmind_core::native::branch_create(&s, &name, None)) {
                Ok(oid) => {
                    app.branches.note = format!("created {name} at {}", &oid[..oid.len().min(8)]);
                    app.branches.new_name.clear();
                    app.branches.err.clear();
                    load(&mut app.branches);
                }
                // Core's message, not ours: it knows why (duplicate name, no
                // HEAD to branch from) and a paraphrase here would drift.
                Err(e) => app.branches.err = e,
            }
        }
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    // ── list, with delete ───────────────────────────────────────────────
    let mut pending: Option<String> = None;
    let mut confirmed: Option<String> = None;
    uikit_egui::scroll::area(ui, |ui| {
        for (name, oid, head) in &app.branches.branches {
            ui.horizontal(|ui| {
                ui.label(if *head { "●" } else { " " });
                ui.label(egui::RichText::new(name).strong());
                let short = &oid[..oid.len().min(8)];
                ui.label(egui::RichText::new(short).monospace().weak());

                if app.branches.confirm_delete.as_deref() == Some(name.as_str()) {
                    ui.label(egui::RichText::new("delete?").weak());
                    if ui.button("yes").clicked() { confirmed = Some(name.clone()); }
                    if ui.button("no").clicked() { pending = Some(String::new()); }
                } else if !*head {
                    // The current branch has no delete button at all: core
                    // would refuse it, and offering an action that always
                    // fails teaches people to distrust the buttons.
                    if ui.button("delete").clicked() { pending = Some(name.clone()); }
                }
            });
        }
    });

    if let Some(n) = pending {
        app.branches.confirm_delete = if n.is_empty() { None } else { Some(n) };
    }
    if let Some(n) = confirmed {
        match open_store().and_then(|s| gitmind_core::native::branch_delete(&s, &n)) {
            Ok(()) => {
                app.branches.note = format!("deleted {n}");
                app.branches.err.clear();
            }
            Err(e) => app.branches.err = e,
        }
        app.branches.confirm_delete = None;
        load(&mut app.branches);
    }
}

#[cfg(test)]
mod tests {
    use super::name_is_usable;

    #[test]
    fn an_ordinary_name_is_usable() {
        assert!(name_is_usable("feature-x"));
        assert!(name_is_usable("v2"));
        assert!(name_is_usable("  trimmed  "), "surrounding space is trimmed, not rejected");
    }

    #[test]
    fn an_empty_or_blank_name_is_refused_before_core_sees_it() {
        // Core never sees a name the user has not finished typing, so this is
        // the one check it cannot make for us.
        assert!(!name_is_usable(""));
        assert!(!name_is_usable("   "));
    }

    #[test]
    fn a_slash_is_refused_because_the_ref_path_already_has_one() {
        // `refs/heads/{name}` with a slash produces a ref nobody can type back,
        // and the store will happily create it.
        assert!(!name_is_usable("feature/x"));
    }

    #[test]
    fn whitespace_inside_a_name_is_refused() {
        // Same reason: it round-trips through a ref path and a shell.
        assert!(!name_is_usable("my branch"));
        assert!(!name_is_usable("tab\there"));
    }

    #[test]
    fn a_leading_dash_is_refused() {
        // It reads as a flag to every command that later takes the name.
        assert!(!name_is_usable("-f"));
    }

    #[test]
    fn a_double_dot_is_refused() {
        // `a..b` is range syntax; a branch named that is unusable in exactly
        // the commands somebody would use it in.
        assert!(!name_is_usable("a..b"));
    }

    #[test]
    fn the_page_does_not_re_implement_cores_duplicate_check() {
        // A duplicate name IS usable as far as this page is concerned — core
        // owns that rule and returns its own message. Two implementations of
        // one rule is how UI copy drifts from what actually runs.
        assert!(name_is_usable("main"), "the page pre-judged a duplicate");
    }
}
