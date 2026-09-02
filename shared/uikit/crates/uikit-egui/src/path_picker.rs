//! path_picker — choose a directory by walking it, not by typing it.
//!
//! Generalised from `gitmind`'s `remote_browse`, which solved this for SSH
//! deploy targets and was the only browser in the tree. Three apps now need
//! one — gitmind for a remote destination, beatspace for a music library,
//! voiceterm for a notes root — and the alternative on offer was `rfd`, a
//! native dialog. That is the wrong answer for this fleet: a native file
//! dialog on a phone is somebody else's full-screen UI with its own idea of
//! touch targets, and on postmarketOS it may not be there at all.
//!
//! ## The lister is the seam
//!
//! The browser knows nothing about how directories are read. gitmind hands it
//! a closure that shells out over ssh; beatspace hands it one that calls
//! `read_dir`. That is the whole difference between a remote picker and a
//! local one, and it is why the original was worth generalising rather than
//! copying — `remote_browse`'s signature took `(user, host, path)` and baked
//! the transport into the model.
//!
//! ## The rule worth keeping
//!
//! `remote_browse`'s hard-won detail, preserved: **store what the lister
//! resolved, not what was asked for.** Entering `~` yields `/home/user`, and
//! recording the tilde means one string that two tools disagree about — its
//! comment cites `ssh` and `rsync` differing on `~/proj`. The same applies
//! locally to a relative path, which means something different once the
//! process changes directory.

use std::path::{Path, PathBuf};

/// One directory's contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    /// The path as the LISTER resolved it.
    pub path: String,
    /// Subdirectory names, not full paths.
    pub dirs: Vec<String>,
}

/// What the browser is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Idle,
    Loading,
    Ready(Listing),
    Failed(String),
}

/// A directory walk with history.
#[derive(Debug, Clone)]
pub struct PathPicker {
    stack: Vec<String>,
    pub state: State,
    /// Show entries beginning with a dot.
    ///
    /// Off by default and toggleable rather than absent: a music library does
    /// not need them and a config root is nothing but them.
    pub show_hidden: bool,
    /// Typed into the filter box. Subdirectories are ranked by
    /// `fuzzy::score` against this and non-matches dropped — the same
    /// subsequence matching voiceterm's navigation palette uses, so
    /// typing "dnl" narrows straight to "Downloads" instead of requiring
    /// an exact prefix. Empty matches (and ranks) everything, so a picker
    /// with nothing typed behaves exactly as it did before this field
    /// existed.
    pub filter: String,
}

impl Default for PathPicker {
    fn default() -> Self {
        PathPicker { stack: Vec::new(), state: State::Idle, show_hidden: false, filter: String::new() }
    }
}

impl PathPicker {
    pub fn new() -> PathPicker {
        PathPicker::default()
    }

    /// The directory currently shown.
    pub fn current(&self) -> Option<&str> {
        match &self.state {
            State::Ready(l) => Some(l.path.as_str()),
            _ => self.stack.last().map(String::as_str),
        }
    }

    pub fn can_go_up(&self) -> bool {
        self.stack.len() > 1
    }

    /// Walk into `path`.
    ///
    /// The lister returns the resolved path alongside the entries, so the
    /// browser records what exists rather than what was typed.
    pub fn open<F>(&mut self, path: &str, lister: F)
    where
        F: Fn(&str) -> Result<Listing, String>,
    {
        self.state = State::Loading;
        match lister(path) {
            Ok(mut l) => {
                if !self.show_hidden {
                    l.dirs.retain(|d| !d.starts_with('.'));
                }
                // Sorted here rather than trusting the lister: read_dir order is
                // filesystem order, which is arbitrary, and a picker whose entries
                // move between visits is one nobody can build muscle memory for.
                l.dirs.sort_by_key(|d| d.to_lowercase());
                if self.stack.last().map(String::as_str) != Some(l.path.as_str()) {
                    self.stack.push(l.path.clone());
                }
                self.state = State::Ready(l);
            }
            Err(e) => self.state = State::Failed(e),
        }
    }

    /// Back to the previous directory.
    ///
    /// Pops the CURRENT entry and re-lists the one beneath, rather than
    /// trimming the path string. A string trim gets `/` wrong, gets a
    /// trailing slash wrong, and cannot go back across a symlink the lister
    /// resolved.
    pub fn up<F>(&mut self, lister: F)
    where
        F: Fn(&str) -> Result<Listing, String>,
    {
        if !self.can_go_up() {
            return;
        }
        self.stack.pop();
        let target = self.stack.pop().unwrap_or_default();
        self.open(&target, lister);
    }

    /// Start somewhere, clearing any history.
    pub fn reset_to<F>(&mut self, path: &str, lister: F)
    where
        F: Fn(&str) -> Result<Listing, String>,
    {
        self.stack.clear();
        self.open(path, lister);
    }

    /// The breadcrumb trail, oldest first.
    pub fn trail(&self) -> &[String] {
        &self.stack
    }
}

/// List a local directory. The lister for anything not over a network.
///
/// Resolves the path before reporting it, so `~`, `.` and a symlink all record
/// what they point at. A failure names the path — "permission denied" without
/// one is a sentence the operator cannot act on.
pub fn list_local(path: &str) -> Result<Listing, String> {
    let expanded = expand_tilde(path);
    let resolved = expanded
        .canonicalize()
        .map_err(|e| format!("{}: {e}", expanded.display()))?;
    let mut dirs = Vec::new();
    for e in std::fs::read_dir(&resolved).map_err(|e| format!("{}: {e}", resolved.display()))? {
        let e = match e {
            Ok(e) => e,
            // One unreadable entry should not lose the other three hundred.
            Err(_) => continue,
        };
        if e.path().is_dir() {
            dirs.push(e.file_name().to_string_lossy().to_string());
        }
    }
    Ok(Listing { path: resolved.to_string_lossy().to_string(), dirs })
}

/// `~` and `~/x` to the home directory. Anything else is returned unchanged.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" || path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            if path == "~" {
                return PathBuf::from(home);
            }
            return Path::new(&home).join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

/// What the picker's UI reported back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Walk into this absolute path.
    Enter(String),
    Up,
    /// Accept the directory currently shown.
    Choose(String),
    ToggleHidden,
}

/// Draw the picker. Returns what was pressed; the caller supplies the lister.
///
/// Returns an action rather than taking the lister itself, so the render stays
/// free of I/O and the caller decides whether a directory read happens on this
/// thread. A local `read_dir` is fine inline; an ssh round trip is not, and a
/// widget that hid the difference would stall a frame on somebody's network.
pub fn picker_ui(
    ui: &mut egui::Ui,
    p: &mut PathPicker,
    c: &crate::PaletteColors,
    max_rows: usize,
) -> Option<Action> {
    let mut act = None;

    ui.horizontal(|ui| {
        if ui.add_enabled(p.can_go_up(), egui::Button::new("⬆ up")).clicked() {
            act = Some(Action::Up);
        }
        if let Some(cur) = p.current() {
            // Elided from the LEFT: the end of a path is the part that
            // identifies it, and truncating there leaves every sibling
            // directory looking identical.
            let shown = elide_left(cur, 48);
            ui.label(egui::RichText::new(shown).monospace().small());
        }
    });

    match &p.state {
        State::Idle => { ui.label(egui::RichText::new("nowhere yet").weak().small()); }
        State::Loading => { ui.spinner(); }
        State::Failed(e) => {
            ui.colored_label(c.warning, egui::RichText::new(e).small());
        }
        State::Ready(l) => {
            if l.dirs.is_empty() {
                // Distinguished from a failure: an empty directory is a fact
                // about the filesystem, and showing nothing at all reads as a
                // broken picker.
                ui.label(egui::RichText::new("no subfolders here").weak().small());
            } else {
                ui.add(egui::TextEdit::singleline(&mut p.filter)
                    .hint_text("filter…").id_salt("path_picker-filter"));
            }
            // Ranked, not just filtered: `crate::fuzzy::score` rewards an
            // early, contiguous match, so typing "dnl" on a directory with
            // both "Downloads" and "download-old" puts the more likely one
            // first instead of leaving list order (usually alphabetical,
            // sometimes filesystem-arbitrary) to decide.
            let mut ranked: Vec<(&String, i32)> = l.dirs.iter()
                .filter_map(|d| crate::fuzzy::score(&p.filter, d).map(|s| (d, s)))
                .collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1));
            if ranked.is_empty() && !p.filter.is_empty() {
                ui.label(egui::RichText::new("no folders match").weak().small());
            }
            egui::ScrollArea::vertical()
                .max_height(max_rows as f32 * 22.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (d, _) in ranked {
                        if ui.selectable_label(false, format!("📁 {d}")).clicked() {
                            let joined = Path::new(&l.path).join(d);
                            act = Some(Action::Enter(joined.to_string_lossy().to_string()));
                        }
                    }
                });
            ui.horizontal(|ui| {
                if ui.button("use this folder").clicked() {
                    act = Some(Action::Choose(l.path.clone()));
                }
                let mut hidden = p.show_hidden;
                if ui.checkbox(&mut hidden, "hidden").changed() {
                    act = Some(Action::ToggleHidden);
                }
            });
        }
    }
    // A filter typed in this directory means nothing once the listing
    // changes underneath it — clearing on navigation is what keeps a
    // leftover "dnl" from hiding every entry in the next folder shown.
    clear_filter_on_navigate(p, &act);
    act
}

/// Extracted so the navigate-clears-filter rule is testable without
/// simulating an egui click through `picker_ui`'s full render.
fn clear_filter_on_navigate(p: &mut PathPicker, act: &Option<Action>) {
    if matches!(act, Some(Action::Enter(_)) | Some(Action::Up)) {
        p.filter.clear();
    }
}

/// Keep the tail of a long path, with a leading ellipsis.
pub fn elide_left(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let tail: String = s.chars().skip(s.chars().count() - max + 1).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(path: &str) -> Result<Listing, String> {
        match path {
            "/music" => Ok(Listing {
                path: "/music".into(),
                dirs: vec!["Zed".into(), ".hidden".into(), "alpha".into(), "Beta".into()],
            }),
            "/music/alpha" => Ok(Listing { path: "/music/alpha".into(), dirs: vec![] }),
            "~" => Ok(Listing { path: "/home/u".into(), dirs: vec!["music".into()] }),
            other => Err(format!("{other}: no such directory")),
        }
    }

    #[test]
    fn it_records_what_the_lister_resolved_not_what_was_typed() {
        // remote_browse's hard-won rule. Entering `~` yields /home/u, and
        // recording the tilde means one string two tools disagree about — its
        // comment cites ssh and rsync differing on `~/proj`. The same applies
        // locally to a relative path once the process changes directory.
        let mut p = PathPicker::new();
        p.open("~", fake);
        assert_eq!(p.current(), Some("/home/u"));
        assert_eq!(p.trail(), &["/home/u".to_string()]);
    }

    #[test]
    fn entries_are_sorted_case_insensitively() {
        // read_dir order is filesystem order, which is arbitrary. A picker
        // whose entries move between visits is one nobody builds muscle
        // memory for.
        let mut p = PathPicker::new();
        p.open("/music", fake);
        match &p.state {
            State::Ready(l) => assert_eq!(l.dirs, vec!["alpha", "Beta", "Zed"]),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn hidden_entries_are_off_until_asked_for() {
        let mut p = PathPicker::new();
        p.open("/music", fake);
        match &p.state {
            State::Ready(l) => assert!(!l.dirs.iter().any(|d| d.starts_with('.'))),
            s => panic!("{s:?}"),
        }
        p.show_hidden = true;
        p.open("/music", fake);
        match &p.state {
            State::Ready(l) => assert!(l.dirs.iter().any(|d| d == ".hidden")),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn going_up_re_lists_rather_than_trimming_the_string() {
        // A string trim gets `/` wrong, gets a trailing slash wrong, and cannot
        // cross a symlink the lister resolved.
        let mut p = PathPicker::new();
        p.open("/music", fake);
        p.open("/music/alpha", fake);
        assert_eq!(p.current(), Some("/music/alpha"));
        assert!(p.can_go_up());
        p.up(fake);
        assert_eq!(p.current(), Some("/music"));
        // And at the root of the walk, up does nothing rather than emptying.
        assert!(!p.can_go_up());
        p.up(fake);
        assert_eq!(p.current(), Some("/music"));
    }

    #[test]
    fn a_failure_keeps_the_reason_and_the_path() {
        // "permission denied" without a path is a sentence nobody can act on.
        let mut p = PathPicker::new();
        p.open("/nope", fake);
        match &p.state {
            State::Failed(e) => assert!(e.contains("/nope"), "{e}"),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn an_empty_directory_is_not_a_failure() {
        let mut p = PathPicker::new();
        p.open("/music/alpha", fake);
        assert!(matches!(p.state, State::Ready(_)), "{:?}", p.state);
    }

    #[test]
    fn a_long_path_keeps_its_tail() {
        // The END identifies a directory. Truncating there leaves every
        // sibling looking identical, which is the one thing a picker must not
        // do.
        let long = "/home/kesslerd/Music/Musique/Music Library/My Creations/Beats";
        let out = elide_left(long, 24);
        assert!(out.ends_with("Beats"), "{out}");
        assert!(out.starts_with('…'));
        assert!(out.chars().count() <= 24);
        // A short path is untouched.
        assert_eq!(elide_left("/music", 24), "/music");
    }

    #[test]
    fn the_local_lister_reads_a_real_directory() {
        let root = std::env::temp_dir().join(format!("pp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a-file.txt"), b"x").unwrap();
        let l = list_local(root.to_str().unwrap()).unwrap();
        assert_eq!(l.dirs, vec!["sub"], "files must not appear as folders");
        assert!(!l.path.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    fn draw(p: &mut PathPicker) -> (Vec<String>, Option<Action>) {
        let ctx = egui::Context::default();
        let c = crate::palette_colors("midnight");
        let mut act = None;
        let run = |ctx: &egui::Context, p: &mut PathPicker, act: &mut Option<Action>| {
            ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,
                                                            egui::vec2(400.0, 600.0))),
                ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { *act = picker_ui(ui, p, &c, 8); });
            })
        };
        run(&ctx, p, &mut act);
        let out = run(&ctx, p, &mut act);
        let texts = out.shapes.iter().filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        }).collect();
        (texts, act)
    }

    #[test]
    fn an_empty_filter_shows_everything() {
        let mut p = PathPicker::new();
        p.open("/music", fake);
        let (texts, _) = draw(&mut p);
        let joined = texts.join(" | ");
        // "Beta" and "alpha" both show; ".hidden" is excluded by
        // show_hidden defaulting off, unrelated to the filter.
        assert!(joined.contains("Zed") && joined.contains("alpha") && joined.contains("Beta"),
                "{joined}");
    }

    #[test]
    fn typing_a_subsequence_narrows_and_ranks() {
        let mut p = PathPicker::new();
        p.open("/music", fake);
        p.filter = "eta".into();
        let (texts, _) = draw(&mut p);
        let joined = texts.join(" | ");
        assert!(joined.contains("Beta"), "'eta' is a subsequence of 'Beta'; {joined}");
        assert!(!joined.contains("Zed") && !joined.contains("alpha"),
                "neither 'Zed' nor 'alpha' contains 'eta' in order; {joined}");
    }

    #[test]
    fn a_filter_with_no_matches_says_so() {
        let mut p = PathPicker::new();
        p.open("/music", fake);
        p.filter = "xyz not a subsequence of anything".into();
        let (texts, _) = draw(&mut p);
        assert!(texts.join(" | ").contains("no folders match"), "{texts:?}");
    }

    #[test]
    fn entering_a_directory_clears_the_stale_filter() {
        let mut p = PathPicker::new();
        p.filter = "something from the parent directory".into();
        clear_filter_on_navigate(&mut p, &Some(Action::Enter("/music/alpha".into())));
        assert_eq!(p.filter, "");
    }

    #[test]
    fn going_up_also_clears_it() {
        let mut p = PathPicker::new();
        p.filter = "leftover".into();
        clear_filter_on_navigate(&mut p, &Some(Action::Up));
        assert_eq!(p.filter, "");
    }

    #[test]
    fn choosing_or_no_action_leaves_the_filter_alone() {
        let mut p = PathPicker::new();
        p.filter = "kept".into();
        clear_filter_on_navigate(&mut p, &Some(Action::Choose("/music".into())));
        assert_eq!(p.filter, "kept", "Choose exits the picker; nothing to re-filter");
        clear_filter_on_navigate(&mut p, &None);
        assert_eq!(p.filter, "kept");
    }
}

#[cfg(test)]
mod layer_id_hygiene {
    /// No kit widget may open an `Area` under an id a caller handed it.
    ///
    /// `Area::new(x)` creates a LAYER whose id is x, and egui registers the
    /// area as a widget under it. A caller's id comes from `ui.id().with(...)`
    /// and lives in the background layer's namespace, so passing it straight
    /// through makes one Id name both a background widget and a foreground
    /// layer. egui's debug build asserts on exactly that:
    ///
    ///   DEBUG ASSERT: Widget C79D changed layer_id during the frame from
    ///   LayerId { Background 5A3E } to LayerId { Foreground C79D }
    ///
    /// The layer id equalling the widget id is the signature. A literal
    /// `Id::new("...")` is fine — it is the kit's own namespace, not a
    /// caller's — and so is a derived `.with(...)`.
    ///
    /// A source check because reproducing it needs a display and a debug
    /// build, which is precisely why it reached a user rather than a test.
    #[test]
    fn an_area_never_takes_a_callers_id_unchanged() {
        for (name, src) in [
            ("slide_menu", include_str!("slide_menu.rs")),
            ("toggle_grid", include_str!("toggle_grid.rs")),
            ("confirm_dialog", include_str!("confirm_dialog.rs")),
            ("toast", include_str!("toast.rs")),
            ("repl_voice_glow", include_str!("repl_voice_glow.rs")),
        ] {
            for (i, _) in src.match_indices("Area::new(") {
                // Skip comment lines — the explanation of this very rule
                // quotes `Area::new(x)`, and matching prose would make the
                // guard fail on its own documentation.
                let line_start = src[..i].rfind('\n').map(|n| n + 1).unwrap_or(0);
                if src[line_start..i].trim_start().starts_with("//") { continue; }
                let arg: String = src[i + 10..].chars().take_while(|c| *c != ')').collect();
                let ok = arg.contains("Id::new(")     // the kit's own namespace
                    || arg.contains(".with(")          // derived from a caller's
                    || arg.trim().is_empty();
                assert!(ok,
                    "{name}: Area::new({arg}) uses an id verbatim — if a caller passed it, \
that Id now names both a background widget and this foreground layer");
            }
        }
    }
}
