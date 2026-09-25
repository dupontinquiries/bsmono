//! ux — what drivemind SAYS to the person using it, decided without egui.
//!
//! The GUI was a set of bare buttons whose results appeared only in the log:
//! nothing confirmed an action had worked, nothing warned before a manifest was
//! replaced, nothing said where to start. These are the words and the
//! decisions; `lib.rs` only draws them.

use drivemind_core::{human_bytes, integrity::VerifyReport, smart};
use std::path::Path;

/// How a banner reads: done and fine, neutral, or something needs attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Info,
    Bad,
}

/// Seconds a banner stays up (it can be closed sooner).
pub const BANNER_SECS: u64 = 8;

/// After "Create manifest".
pub fn created_banner(files: u64, manifest: &Path) -> (Tone, String) {
    (Tone::Good, format!(
        "Manifest written: {files} file(s) recorded in {}. Keep it -- Verify compares against it later.",
        manifest.display()))
}

/// After "Verify".
pub fn verify_banner(rep: &VerifyReport) -> (Tone, String) {
    if rep.clean() {
        return (Tone::Good, format!("Verified: all {} file(s) unchanged since the manifest.", rep.ok));
    }
    let mut parts = Vec::new();
    if !rep.changed.is_empty() { parts.push(format!("{} changed", rep.changed.len())); }
    if !rep.missing.is_empty() { parts.push(format!("{} missing", rep.missing.len())); }
    if !rep.read_errors.is_empty() { parts.push(format!("{} unreadable", rep.read_errors.len())); }
    (Tone::Bad, format!("Verify found problems: {} ({} fine). The list is below.", parts.join(", "), rep.ok))
}

/// After a SMART probe.
pub fn smart_banner(rep: &smart::SmartReport) -> (Tone, String) {
    match rep.worst() {
        s if s >= smart::Severity::Warning => (Tone::Bad, format!(
            "{}: the drive reports a problem ({s:?}). Back up what matters; the Advise tab says what next.", rep.device)),
        _ => (Tone::Good, format!("{}: SMART looks healthy.", rep.device)),
    }
}

/// After a cleanup scan.
pub fn cleanup_banner(suggestions: usize, bytes: u64) -> (Tone, String) {
    if suggestions == 0 {
        return (Tone::Info, "Nothing to clean up here.".into());
    }
    (Tone::Good, format!(
        "{suggestions} suggestion(s), {} reclaimable. drivemind deletes nothing itself: copy a command to run it.",
        human_bytes(bytes)))
}

/// After a video scan.
pub fn videos_banner(candidates: usize, savings: u64) -> (Tone, String) {
    if candidates == 0 {
        return (Tone::Info, "No videos worth re-encoding here.".into());
    }
    (Tone::Good, format!("{candidates} video(s) worth re-encoding, about {} to gain.", human_bytes(savings)))
}

/// Why creating a manifest at `path` needs a yes first -- it would REPLACE the
/// record of what the files were -- or None if nothing is there to lose.
pub fn overwrite_warning(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let age = meta.modified().ok()
        .and_then(|m| m.elapsed().ok())
        .map(|d| match d.as_secs() {
            s if s < 3600 => "less than an hour ago".to_string(),
            s if s < 86_400 => format!("{} hour(s) ago", s / 3600),
            s => format!("{} day(s) ago", s / 86_400),
        })
        .unwrap_or_else(|| "at an unknown time".into());
    Some(format!(
        "{} already exists ({}, written {age}). Creating a manifest replaces it: \
         changes made since then can no longer be detected against it. \
         To check the files instead, use Verify.",
        path.display(), human_bytes(meta.len())))
}

/// One step of the getting-started checklist.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub label: &'static str,
    pub done: bool,
    /// What to do, shown for the first step not yet done.
    pub hint: &'static str,
}

/// The first-run path, ticked off from what has actually happened.
pub fn getting_started(device_selected: bool, probed: bool, manifest_exists: bool, verified: bool) -> Vec<Step> {
    vec![
        Step { label: "Pick a drive", done: device_selected, hint: "Tap a drive in the row above (↻ looks again)." },
        Step { label: "Check its health", done: probed, hint: "Press Probe SMART: the drive's own error counters, read-only." },
        Step { label: "Record your files", done: manifest_exists, hint: "Verify tab: choose a folder and press Create manifest." },
        Step { label: "Check them later", done: verified, hint: "Verify tab: press Verify to find anything changed or unreadable." },
        Step { label: "Read the advice", done: verified && probed, hint: "The Advise tab turns the results into next steps." },
    ]
}

/// The step to point at: the first not yet done.
pub fn next_step(steps: &[Step]) -> Option<&Step> {
    steps.iter().find(|s| !s.done)
}

/// Tooltip for each tab, in tab order.
pub fn tab_help(i: usize) -> &'static str {
    match i {
        0 => "Health: the selected drive's SMART report -- temperature, hours, and anything it flags.",
        1 => "Verify: record a folder's files (a manifest of hashes), then check later that nothing changed or rotted.",
        2 => "Cleanup: find what can be reclaimed under a folder. Suggestions only -- nothing is deleted for you.",
        3 => "Videos: find videos that would shrink a lot if re-encoded, and hand them to av1tuner.",
        4 => "Advise: next steps drawn from the health, verify and cleanup results.",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(ok: u64, changed: usize, missing: usize, errs: usize) -> VerifyReport {
        VerifyReport {
            ok,
            changed: (0..changed).map(|i| (format!("c{i}"), drivemind_core::integrity::FileStatus::Missing)).collect(),
            missing: (0..missing).map(|i| format!("m{i}")).collect(),
            new: Vec::new(),
            read_errors: (0..errs).map(|i| (format!("e{i}"), "EIO".into())).collect(),
            bytes_hashed: 3 << 30,
        }
    }

    #[test]
    fn a_result_is_affirmed_in_plain_words_and_problems_are_named() {
        let (t, s) = created_banner(1204, Path::new("/data/manifest.json"));
        assert_eq!(t, Tone::Good);
        assert!(s.starts_with("Manifest written: 1204 file(s) recorded in /data/manifest.json"), "{s}");
        let (t, s) = verify_banner(&rep(1204, 0, 0, 0));
        assert_eq!((t, s.as_str()), (Tone::Good, "Verified: all 1204 file(s) unchanged since the manifest."));
        let (t, s) = verify_banner(&rep(1200, 2, 1, 1));
        assert_eq!(t, Tone::Bad);
        assert!(s.contains("2 changed, 1 missing, 1 unreadable") && s.contains("1200 fine"), "{s}");
        assert_eq!(cleanup_banner(0, 0).0, Tone::Info);
        assert!(cleanup_banner(3, 5 << 30).1.contains("deletes nothing itself"));
        assert_eq!(videos_banner(0, 0).0, Tone::Info);
    }

    #[test]
    fn replacing_a_manifest_asks_first_and_says_what_is_lost() {
        let dir = std::env::temp_dir().join(format!("drivemind-ux-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = dir.join("manifest.json");
        assert_eq!(overwrite_warning(&m), None, "nothing there: no question");
        std::fs::write(&m, vec![b'x'; 2048]).unwrap();
        let w = overwrite_warning(&m).expect("an existing manifest needs a yes");
        assert!(w.contains("already exists") && w.contains("less than an hour ago") && w.contains("Verify"), "{w}");
        assert_eq!(overwrite_warning(&dir), None, "a directory is not a manifest");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_checklist_follows_what_has_happened() {
        let s = getting_started(false, false, false, false);
        assert_eq!(next_step(&s).unwrap().label, "Pick a drive");
        let s = getting_started(true, true, false, false);
        assert_eq!(next_step(&s).unwrap().label, "Record your files");
        assert!(next_step(&getting_started(true, true, true, true)).is_none(), "all done: the guide can go");
        for i in 0..5 { assert!(!tab_help(i).is_empty()); }
    }
}
