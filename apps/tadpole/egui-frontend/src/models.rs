//! models — find the OCR weights, and say what to do when they are not there.
//!
//! The weights are 28MB of MNN and cannot live in this repository, so the app
//! has to cope with not having them. That is not an error state to report once
//! and forget: it is the **first thing a new user meets**, and an OCR app that
//! says "failed" when it means "download this file" sends people to inspect
//! their image instead of their install.
//!
//! ## Detection is by file, not by directory
//!
//! `~/models` existing tells you nothing. What matters is whether the three
//! files a PP-OCRv5 pipeline needs are present:
//!
//! * `det.mnn` — the detection net that finds text boxes
//! * `rec.mnn` — the recognition net that reads each one
//! * `dict.txt` — maps recognition class indices to characters
//!
//! **A partial set is its own state**, and the most useful one to report
//! precisely. Someone who fetched two of three files has a specific, fixable
//! problem, and "no model found" would send them to re-download all of it.
//!
//! ## The phases come from the kit
//!
//! `uikit_egui::setup_wizard` already has the four-phase machine and
//! `initial_phase`. This adds what it does not have: a *discovery* step for
//! weights the user fetches themselves, rather than a picker that downloads a
//! GGUF. Reusing the phases means an app that later gains a downloader does not
//! grow a second wizard beside this one.

use std::path::{Path, PathBuf};

/// A file the OCR pipeline needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Needed {
    pub file: &'static str,
    /// What it is for, in the message a user actually reads.
    pub purpose: &'static str,
}

/// The PP-OCRv5 set, as rusto-rs ships it.
pub const REQUIRED: [Needed; 3] = [
    Needed { file: "det.mnn", purpose: "finds the text on the page" },
    Needed { file: "rec.mnn", purpose: "reads each piece of text" },
    Needed { file: "dict.txt", purpose: "maps the model's output to characters" },
];

/// Where the weights were found, or why they were not.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ModelState {
    /// Every file present, in this directory.
    Ready(PathBuf),
    /// Some files present. Names what is missing, because a user who fetched
    /// two of three has a specific problem and being told "nothing found"
    /// sends them to re-download all of it.
    Partial { dir: PathBuf, missing: Vec<&'static str> },
    /// Nothing found anywhere that was searched.
    Missing,
}

impl ModelState {
    pub fn is_ready(&self) -> bool { matches!(self, ModelState::Ready(_)) }

    pub fn dir(&self) -> Option<&Path> {
        match self {
            ModelState::Ready(d) => Some(d),
            ModelState::Partial { dir, .. } => Some(dir),
            ModelState::Missing => None,
        }
    }
}

/// Directories searched, in order.
///
/// `~/models` first because that is where the user was told to put them, then
/// the conventional data locations. Order matters: a user who deliberately
/// dropped files in `~/models` should not be silently served a stale copy from
/// a package directory they have forgotten about.
pub fn search_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("models"),
        home.join("models/PPOCR_v5"),
        home.join(".local/share/tadpole/models"),
        PathBuf::from("/usr/share/tadpole/models"),
    ]
}

/// Look for the weights.
///
/// `exists` is injected so this is testable without a filesystem — the whole
/// point of the function is the *decision*, and a test that had to create
/// twelve directories would be testing `std::fs`.
pub fn discover<F>(home: &Path, exists: F) -> ModelState
where F: Fn(&Path) -> bool {
    let mut best_partial: Option<(PathBuf, Vec<&'static str>)> = None;
    for dir in search_paths(home) {
        let missing: Vec<&'static str> = REQUIRED.iter()
            .filter(|n| !exists(&dir.join(n.file)))
            .map(|n| n.file)
            .collect();
        if missing.is_empty() {
            return ModelState::Ready(dir);
        }
        if missing.len() < REQUIRED.len() {
            // Keep the most complete partial rather than the first: a directory
            // with two of three files is a better thing to tell the user about
            // than one with one.
            let better = best_partial.as_ref().map(|(_, m)| missing.len() < m.len()).unwrap_or(true);
            if better { best_partial = Some((dir, missing)); }
        }
    }
    match best_partial {
        Some((dir, missing)) => ModelState::Partial { dir, missing },
        None => ModelState::Missing,
    }
}

/// What the wizard tells the user, as lines to render.
///
/// Returned as data rather than drawn, so the message is testable and so the
/// same words can appear in a CLI, a log, or a tooltip without being rewritten.
pub fn guidance(state: &ModelState, home: &Path) -> Vec<String> {
    match state {
        ModelState::Ready(d) => vec![
            format!("Models found in {}", d.display()),
        ],
        ModelState::Partial { dir, missing } => {
            let mut v = vec![
                format!("Found part of the model set in {}", dir.display()),
                format!("Still needed: {}", missing.join(", ")),
            ];
            for n in REQUIRED.iter().filter(|n| missing.contains(&n.file)) {
                v.push(format!("  {} — {}", n.file, n.purpose));
            }
            // The likeliest cause named explicitly, because a half-finished
            // download looks identical to a wrong directory and the two have
            // different fixes.
            v.push("A partial set usually means an interrupted download, or files \
                    placed one level deeper than expected.".into());
            v
        }
        ModelState::Missing => vec![
            "No OCR model found yet.".into(),
            format!("Put the PP-OCRv5 files in {}", home.join("models").display()),
            format!("  {}", REQUIRED.iter().map(|n| n.file).collect::<Vec<_>>().join(", ")),
            "They ship with rusto-rs under models/PPOCR_v5.".into(),
            "Tadpole reads the folder each time you open this page — no restart \
             needed.".into(),
        ],
    }
}

/// The kit phase this state opens on.
///
/// `Partial` opens on `Intro` rather than `Done`: some files present is not a
/// working install, and opening on Done would show a Scan button that fails.
pub fn phase(state: &ModelState) -> uikit_egui::setup_wizard::Phase {
    uikit_egui::setup_wizard::initial_phase(state.is_ready())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf { PathBuf::from("/home/kd") }

    /// A fake filesystem: the set of paths that exist.
    fn fs(paths: &[&str]) -> impl Fn(&Path) -> bool {
        let owned: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        move |p: &Path| owned.iter().any(|o| o == &p.display().to_string())
    }

    #[test]
    fn a_complete_set_in_the_models_folder_is_ready() {
        let state = discover(&home(), fs(&[
            "/home/kd/models/det.mnn",
            "/home/kd/models/rec.mnn",
            "/home/kd/models/dict.txt",
        ]));
        assert_eq!(state, ModelState::Ready(home().join("models")));
        assert!(state.is_ready());
    }

    #[test]
    fn the_nested_ppocr_directory_is_found_too() {
        // rusto-rs ships them under models/PPOCR_v5, and a user who copies the
        // folder wholesale rather than its contents has done nothing wrong.
        let state = discover(&home(), fs(&[
            "/home/kd/models/PPOCR_v5/det.mnn",
            "/home/kd/models/PPOCR_v5/rec.mnn",
            "/home/kd/models/PPOCR_v5/dict.txt",
        ]));
        assert_eq!(state, ModelState::Ready(home().join("models/PPOCR_v5")));
    }

    #[test]
    fn a_partial_set_names_exactly_what_is_missing() {
        // Someone who fetched two of three files has a specific, fixable
        // problem; "no model found" would send them to re-download all of it.
        let state = discover(&home(), fs(&[
            "/home/kd/models/det.mnn",
            "/home/kd/models/rec.mnn",
        ]));
        match &state {
            ModelState::Partial { dir, missing } => {
                assert_eq!(dir, &home().join("models"));
                assert_eq!(missing, &vec!["dict.txt"]);
            }
            other => panic!("expected Partial, got {other:?}"),
        }
        let text = guidance(&state, &home()).join("\n");
        assert!(text.contains("dict.txt"));
        assert!(text.contains("maps the model's output to characters"),
                "the message says what the missing file is FOR");
    }

    #[test]
    fn the_most_complete_partial_is_the_one_reported() {
        // A directory with two of three files is a better thing to tell the
        // user about than one with one.
        let state = discover(&home(), fs(&[
            "/home/kd/models/det.mnn",
            "/home/kd/models/PPOCR_v5/det.mnn",
            "/home/kd/models/PPOCR_v5/rec.mnn",
        ]));
        match state {
            ModelState::Partial { dir, missing } => {
                assert_eq!(dir, home().join("models/PPOCR_v5"));
                assert_eq!(missing, vec!["dict.txt"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn nothing_anywhere_is_missing_not_partial() {
        let state = discover(&home(), fs(&[]));
        assert_eq!(state, ModelState::Missing);
        let text = guidance(&state, &home()).join("\n");
        assert!(text.contains("/home/kd/models"), "it says WHERE to put them");
        assert!(text.contains("det.mnn") && text.contains("rec.mnn") && text.contains("dict.txt"),
                "and exactly which files");
        assert!(text.contains("rusto-rs"), "and where to get them");
        assert!(text.contains("no restart"), "and that it will notice");
    }

    #[test]
    fn the_models_folder_is_searched_before_the_system_one() {
        // A user who deliberately dropped files in ~/models must not be
        // silently served a stale copy from a package directory.
        let state = discover(&home(), fs(&[
            "/home/kd/models/det.mnn", "/home/kd/models/rec.mnn", "/home/kd/models/dict.txt",
            "/usr/share/tadpole/models/det.mnn", "/usr/share/tadpole/models/rec.mnn",
            "/usr/share/tadpole/models/dict.txt",
        ]));
        assert_eq!(state, ModelState::Ready(home().join("models")));
    }

    #[test]
    fn a_partial_install_does_not_open_on_done() {
        // Some files present is not a working install, and opening on Done
        // would show a Scan button that fails.
        use uikit_egui::setup_wizard::Phase;
        let partial = ModelState::Partial { dir: home(), missing: vec!["rec.mnn"] };
        assert_eq!(phase(&partial), Phase::Intro);
        assert_eq!(phase(&ModelState::Missing), Phase::Intro);
        assert_eq!(phase(&ModelState::Ready(home())), Phase::Done);
    }

    #[test]
    fn a_partial_message_names_the_likeliest_cause() {
        // A half-finished download looks identical to a wrong directory, and
        // the two have different fixes.
        let state = ModelState::Partial { dir: home(), missing: vec!["rec.mnn"] };
        let text = guidance(&state, &home()).join("\n");
        assert!(text.contains("interrupted download"));
        assert!(text.contains("one level deeper"));
    }

    #[test]
    fn guidance_is_data_rather_than_drawing() {
        // So the same words can appear in a CLI, a log or a tooltip without
        // being rewritten — and so they are testable at all.
        assert!(!guidance(&ModelState::Missing, &home()).is_empty());
        assert!(guidance(&ModelState::Ready(home()), &home())[0].contains("/home/kd"));
    }
}
