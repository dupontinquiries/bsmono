//! snap_export — `SnapExporter.cpp`, ported.
//!
//! Archives the media folder so a user can take their snaps somewhere else.
//! The shell runs the command; everything here is the **decision** — which
//! tool, which path, and what counts as success.
//!
//! ## An empty folder is a success, not an error
//!
//! The C++ short-circuits so the caller can say *"nothing to export"* rather
//! than show a failure. A user who has received no snaps has not hit a bug, and
//! an error dialog there teaches them to distrust the ones that matter.
//!
//! ## The extension follows the tool, not the request
//!
//! If the user asks for `snaps.zip` and only `tar` is present, the output is
//! `snaps.tar.gz` — because a file named `.zip` that is a tarball is worse than
//! a correctly-named one. The C++ rebuilds the path from the base name for
//! exactly this reason, and the result reports the path actually written rather
//! than the one requested.
//!
//! ## Contents, not the folder
//!
//! `zip -j` junks paths and `tar -C <dir> .` archives relative to it, so the
//! archive opens directly to the files rather than to an `AppData/media/`
//! prefix nobody wants to click through.

/// Which archiver to use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool { Zip, TarGz }

impl Tool {
    pub fn program(self) -> &'static str {
        match self { Tool::Zip => "zip", Tool::TarGz => "tar" }
    }

    /// Extension including the dot, as it appears on the written file.
    pub fn extension(self) -> &'static str {
        match self { Tool::Zip => ".zip", Tool::TarGz => ".tar.gz" }
    }
}

/// Choose an archiver from what is installed.
///
/// zip first because it *"opens natively everywhere"* — a phone or a Windows
/// machine on the other end of the transfer can open it without installing
/// anything, which is the point of exporting at all.
pub fn choose_tool(has_zip: bool, has_tar: bool) -> Option<Tool> {
    if has_zip { Some(Tool::Zip) } else if has_tar { Some(Tool::TarGz) } else { None }
}

/// Strip every known archive extension from a path.
///
/// `.tar.gz` is two extensions, so trimming one leaves `snaps.tar` and the
/// output becomes `snaps.tar.tar.gz`. Looped until nothing more matches.
pub fn base_without_extension(path: &str) -> String {
    let mut s = path.to_string();
    loop {
        let lower = s.to_lowercase();
        let hit = [".zip", ".gz", ".tar", ".tgz", ".7z"]
            .iter()
            .find(|e| lower.ends_with(*e))
            .copied();
        match hit {
            Some(e) => { let n = s.len() - e.len(); s.truncate(n); }
            None => break,
        }
    }
    s
}

/// The path that will actually be written.
pub fn output_path(requested: &str, tool: Tool) -> String {
    format!("{}{}", base_without_extension(requested), tool.extension())
}

/// Arguments for the chosen tool, run from inside the media directory.
///
/// Returned as an argv rather than a shell string: a media path containing a
/// space or a quote would otherwise be a command-injection bug in a feature
/// whose whole job is handling the user's own files.
pub fn argv(tool: Tool, out: &str) -> Vec<String> {
    match tool {
        // -q quiet, -j junk paths, -r recurse. `.` because we run from inside
        // the media dir.
        Tool::Zip => vec!["-q".into(), "-j".into(), "-r".into(), out.into(), ".".into()],
        // czf create-gzip-file, -C to archive the directory's contents.
        Tool::TarGz => vec!["-czf".into(), out.into(), "-C".into(), ".".into(), ".".into()],
    }
}

/// What an export attempt produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Files were archived.
    Wrote { path: String, files: usize },
    /// Nothing to export — a success with a different message.
    Empty,
    /// Neither archiver is installed.
    NoTool,
    Failed(String),
}

impl Outcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, Outcome::Wrote { .. } | Outcome::Empty)
    }

    /// The line a user reads.
    pub fn message(&self) -> String {
        match self {
            Outcome::Wrote { path, files } => {
                let n = if *files == 1 { "1 snap".to_string() } else { format!("{files} snaps") };
                format!("Exported {n} to {path}")
            }
            Outcome::Empty => "Nothing to export yet.".into(),
            // Names the fix rather than the failure: a missing tool is
            // something the user can act on, unlike most errors.
            Outcome::NoTool => "Install `zip` or `tar` to export.".into(),
            Outcome::Failed(e) => format!("Export failed: {e}"),
        }
    }
}

/// Plan an export. `None` for the plan means nothing should run.
pub fn plan(file_count: usize, requested: &str, has_zip: bool, has_tar: bool)
    -> Result<(Tool, String), Outcome> {
    // Checked BEFORE tool selection: telling a user to install zip when they
    // have nothing to export would send them to fix the wrong thing.
    if file_count == 0 { return Err(Outcome::Empty); }
    match choose_tool(has_zip, has_tar) {
        Some(t) => Ok((t, output_path(requested, t))),
        None => Err(Outcome::NoTool),
    }
}

/// The archive path to ask for: `<dest>/snapclone-snaps-<epoch>.zip`, where
/// `dest` is the chosen export folder or, when none is set, `home`. The epoch
/// keeps two exports on one day from overwriting each other; the extension is
/// corrected to the tool that actually runs by [`output_path`].
///
/// The destination must not be the snaps directory itself: the archiver runs
/// from inside it and would try to add the archive to itself.
pub fn requested_path(dest_dir: &str, home: &str, snaps_dir: &str, now_secs: u64) -> Result<String, Outcome> {
    let dest = if dest_dir.trim().is_empty() { home } else { dest_dir.trim() };
    let dest = dest.trim_end_matches('/');
    if dest.is_empty() {
        return Err(Outcome::Failed("no export folder and no home directory".into()));
    }
    if dest == snaps_dir.trim_end_matches('/') {
        return Err(Outcome::Failed("the export folder is the snaps folder itself — choose another".into()));
    }
    Ok(format!("{dest}/snapclone-snaps-{now_secs}.zip"))
}

/// Run a whole export: count the app's snap files in `snaps`, plan, run the
/// archiver from inside `snaps`, report. Blocking -- the host calls it on its own
/// thread. `has_zip` / `has_tar` are the host's PATH probe, passed in so a test
/// can choose the tool.
///
/// Only files the app wrote (`snap_expiry::stored_snap`) are COUNTED; the
/// archivers take the directory, which also holds the code SVG -- harmless in a
/// backup, and filtering the argv per file would hit argument limits on a year
/// of snaps.
pub fn run(snaps: &std::path::Path, dest_dir: &str, home: &str, now_secs: u64,
           has_zip: bool, has_tar: bool) -> Outcome {
    let go = || -> Result<Outcome, Outcome> {
        let files = std::fs::read_dir(snaps).map(|it| it.filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|n| crate::snap_expiry::stored_snap(n).is_some()))
            .count()).unwrap_or(0);
        let requested = requested_path(dest_dir, home, &snaps.display().to_string(), now_secs)?;
        let (tool, out) = plan(files, &requested, has_zip, has_tar)?;
        if let Some(parent) = std::path::Path::new(&out).parent() {
            std::fs::create_dir_all(parent).map_err(|e| Outcome::Failed(format!("{}: {e}", parent.display())))?;
        }
        let status = std::process::Command::new(tool.program())
            .args(argv(tool, &out))
            .current_dir(snaps)
            .status()
            .map_err(|e| Outcome::Failed(format!("{}: {e}", tool.program())))?;
        if status.success() {
            Ok(Outcome::Wrote { path: out, files })
        } else {
            Err(Outcome::Failed(format!("{} exited with {status}", tool.program())))
        }
    };
    go().unwrap_or_else(|e| e)
}

#[cfg(test)]
mod tests {
    /// End to end with the real `tar`: two snaps in, an archive out in the chosen
    /// folder holding them; an empty folder is Empty, not an archive.
    #[test]
    fn an_export_really_writes_an_archive_with_the_snaps_in_it() {
        let has_tar = std::process::Command::new("tar").arg("--version").output().is_ok();
        assert!(has_tar, "this test needs tar on PATH");
        let root = std::env::temp_dir().join(format!("snapclone-export-{}", std::process::id()));
        let snaps = root.join("snaps");
        let dest = root.join("out");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&snaps).unwrap();
        assert_eq!(run(&snaps, dest.to_str().unwrap(), "/nonexistent", 5, false, true), Outcome::Empty);
        std::fs::write(snaps.join("snap-1000.jpg"), b"one").unwrap();
        std::fs::write(snaps.join("snap-2000.jpg"), b"two").unwrap();
        match run(&snaps, dest.to_str().unwrap(), "/nonexistent", 5, false, true) {
            Outcome::Wrote { path, files } => {
                assert_eq!(files, 2);
                assert!(path.ends_with("snapclone-snaps-5.tar.gz"), "{path}");
                let list = std::process::Command::new("tar").args(["-tzf", &path]).output().unwrap();
                let list = String::from_utf8_lossy(&list.stdout);
                assert!(list.contains("snap-1000.jpg") && list.contains("snap-2000.jpg"), "archive holds: {list}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(run(&snaps, dest.to_str().unwrap(), "/x", 5, false, false), Outcome::NoTool);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_archive_goes_to_the_chosen_folder_or_home_never_into_itself() {
        assert_eq!(requested_path("/media/sd/", "/home/kd", "/home/kd/.local/share/snapclone/snaps", 7).unwrap(),
                   "/media/sd/snapclone-snaps-7.zip");
        assert_eq!(requested_path("  ", "/home/kd", "/x/snaps", 7).unwrap(), "/home/kd/snapclone-snaps-7.zip");
        assert!(requested_path("/x/snaps/", "/home/kd", "/x/snaps", 7).is_err(), "archiving into the source folder");
        assert!(requested_path("", "", "/x/snaps", 7).is_err());
    }

    use super::*;

    #[test]
    fn an_empty_folder_is_a_success_with_its_own_message() {
        // A user who has received no snaps has not hit a bug, and an error
        // dialog there teaches them to distrust the ones that matter.
        let e = plan(0, "/home/kd/snaps.zip", true, true).unwrap_err();
        assert_eq!(e, Outcome::Empty);
        assert!(e.is_ok());
        assert!(e.message().contains("Nothing to export"));
    }

    #[test]
    fn an_empty_folder_is_checked_before_the_tool_is() {
        // Telling a user to install zip when they have nothing to export sends
        // them to fix the wrong thing.
        assert_eq!(plan(0, "/x/snaps.zip", false, false).unwrap_err(), Outcome::Empty);
    }

    #[test]
    fn zip_is_preferred_because_it_opens_everywhere() {
        // The receiving end may be a phone or a Windows machine, and needing to
        // install something defeats the point of exporting.
        assert_eq!(choose_tool(true, true), Some(Tool::Zip));
        assert_eq!(choose_tool(false, true), Some(Tool::TarGz));
        assert_eq!(choose_tool(true, false), Some(Tool::Zip));
        assert_eq!(choose_tool(false, false), None);
    }

    #[test]
    fn the_extension_follows_the_tool_that_will_actually_run() {
        // A file named .zip that is really a tarball is worse than a
        // correctly-named one: it fails to open with no explanation.
        let (tool, path) = plan(5, "/home/kd/snaps.zip", false, true).unwrap();
        assert_eq!(tool, Tool::TarGz);
        assert_eq!(path, "/home/kd/snaps.tar.gz");
    }

    #[test]
    fn a_double_extension_is_stripped_completely() {
        // Trimming one leaves "snaps.tar" and the output becomes
        // "snaps.tar.tar.gz".
        assert_eq!(base_without_extension("/x/snaps.tar.gz"), "/x/snaps");
        assert_eq!(output_path("/x/snaps.tar.gz", Tool::TarGz), "/x/snaps.tar.gz");
        assert_eq!(output_path("/x/snaps.tgz", Tool::Zip), "/x/snaps.zip");
    }

    #[test]
    fn a_path_with_no_extension_gains_one() {
        assert_eq!(output_path("/home/kd/my snaps", Tool::Zip), "/home/kd/my snaps.zip");
    }

    #[test]
    fn a_dot_in_a_directory_name_is_not_treated_as_an_extension() {
        // ~/.local/share/exports/snaps — stripping there would rewrite the
        // directory path and write somewhere else entirely.
        assert_eq!(output_path("/home/kd/.local/share/snaps", Tool::Zip),
                   "/home/kd/.local/share/snaps.zip");
    }

    #[test]
    fn arguments_are_an_argv_rather_than_a_shell_string() {
        // A media path containing a space or a quote would otherwise be a
        // command-injection bug in a feature whose whole job is handling the
        // user's own files.
        let a = argv(Tool::Zip, "/home/kd/my snaps.zip");
        assert!(a.contains(&"/home/kd/my snaps.zip".to_string()),
                "the path is one argument: {a:?}");
        assert!(a.iter().all(|s| !s.contains('&') && !s.contains(';')));
    }

    #[test]
    fn the_archive_holds_the_contents_not_the_folder() {
        // -j junks paths for zip; -C archives relative for tar. Otherwise the
        // archive opens to an AppData/media/ prefix nobody wants to click
        // through.
        assert!(argv(Tool::Zip, "o.zip").contains(&"-j".to_string()));
        assert!(argv(Tool::TarGz, "o.tar.gz").contains(&"-C".to_string()));
    }

    #[test]
    fn a_missing_tool_names_the_fix_rather_than_the_failure() {
        // A missing tool is something the user can act on, unlike most errors.
        let e = plan(5, "/x/snaps.zip", false, false).unwrap_err();
        assert_eq!(e, Outcome::NoTool);
        assert!(!e.is_ok());
        assert!(e.message().contains("Install"));
    }

    #[test]
    fn the_success_message_counts_in_english() {
        // "1 snaps" is the tell that nobody exported a single photo.
        assert!(Outcome::Wrote { path: "/x/a.zip".into(), files: 1 }
                .message().contains("1 snap to"));
        assert!(Outcome::Wrote { path: "/x/a.zip".into(), files: 7 }
                .message().contains("7 snaps"));
    }

    #[test]
    fn the_message_reports_the_path_written_not_the_one_requested() {
        // The user needs to find the file, and it may not be where they asked.
        let (_, path) = plan(3, "/home/kd/snaps.zip", false, true).unwrap();
        let msg = Outcome::Wrote { path: path.clone(), files: 3 }.message();
        assert!(msg.contains(".tar.gz"), "{msg}");
    }

    #[test]
    fn a_failure_carries_the_reason_through() {
        let f = Outcome::Failed("zip exited 12".into());
        assert!(!f.is_ok());
        assert!(f.message().contains("zip exited 12"));
    }
}
