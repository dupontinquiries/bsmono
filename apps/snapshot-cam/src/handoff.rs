//! handoff — the two ways a photo leaves this device, made real.
//!
//! `main.rs` used to queue a share and leave it Pending forever ("the
//! transport is not wired, and claiming success here would be the exact
//! lie the core is written to avoid"). Both targets have a transport now:
//!
//! * **photosync** is a copy into the photosync library folder — the one
//!   photosync-egui scans, read from its own settings so the two apps agree
//!   without either linking the other. The copy is confirmed when the
//!   bytes are on disk, which is what "backed up to your own machine"
//!   means here.
//! * **a contact** is a request in snapclone's outbox (`snap-handoff`).
//!   snapclone sends the file through its ordinary snap path and writes
//!   a marker; [`poll`] reads the markers back into the roll. Until a
//!   marker arrives the shot is Pending, and Pending already counts as
//!   disclosed — the request names the file and somebody else's app has
//!   it.

use crate::{Disposition, Roll, ShareTarget};
use std::path::{Path, PathBuf};

/// photosync's library root, from its settings; else the pictures folder
/// photosync itself defaults to.
pub fn photosync_root() -> PathBuf {
    if let Some(text) = config_kit::load("photosync", "settings.conf") {
        let ini = config_kit::Ini::parse(&text);
        if let Some(r) = ini.get("", "root").or_else(|| ini.get("general", "root")) {
            let r = r.trim();
            if !r.is_empty() { return PathBuf::from(r); }
        }
    }
    if let Some(d) = std::env::var_os("XDG_PICTURES_DIR") { return PathBuf::from(d); }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Pictures")
}

/// The subfolder this app's photos land in inside the library, so they
/// are recognisable there and photosync's scanner groups them.
pub const LIBRARY_SUBDIR: &str = "snapshot-cam";

/// Copy a capture into the library. Returns where it went.
pub fn back_up(capture_dir: &Path, file: &str, library_root: &Path) -> Result<PathBuf, String> {
    let src = capture_dir.join(file);
    if !src.is_file() { return Err(format!("{} is not on disk", src.display())); }
    let dir = library_root.join(LIBRARY_SUBDIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let dst = dir.join(file);
    // Through a temp name so photosync's scanner never sees half a JPEG.
    let tmp = dir.join(format!(".{file}.part"));
    std::fs::copy(&src, &tmp).map_err(|e| format!("copy failed: {e}"))?;
    std::fs::rename(&tmp, &dst).map_err(|e| format!("rename failed: {e}"))?;
    Ok(dst)
}

/// Queue a photo for a contact: a request in snapclone's outbox. Returns
/// the request id, which the roll keeps until the marker comes back.
pub fn request_send(capture_dir: &Path, file: &str, contact: &str, taken_ms: u64, outbox: &Path, now_ms: u64) -> Result<String, String> {
    let path = capture_dir.join(file);
    if !path.is_file() { return Err(format!("{} is not on disk", path.display())); }
    let abs = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
    let req = snap_handoff::Request {
        id: snap_handoff::new_id(now_ms), to: contact.to_string(), file: abs, taken_ms, from: crate::store::APP.into(),
    };
    snap_handoff::write_request(outbox, &req).map_err(|e| format!("cannot write to the outbox: {e}"))?;
    Ok(req.id)
}

/// Perform every Pending share on the roll that has not been started, and
/// read back the outcomes of the ones that have. Returns the notes worth
/// showing, newest last. Called each frame; cheap when nothing is pending.
pub fn poll(roll: &mut Roll, dir: &Path, outbox: &Path, library_root: &Path, now_ms: u64) -> Vec<String> {
    let mut notes = Vec::new();
    for s in roll.shots_mut() {
        let Disposition::Pending(target) = s.disposition().clone() else { continue };
        match target {
            ShareTarget::PhotoSync => match back_up(dir, &s.file, library_root) {
                Ok(dst) => { s.confirm_sent(); notes.push(format!("{} backed up to {}", s.file, dst.display())); }
                Err(e) => { s.fail(&e); notes.push(format!("{}: back-up failed: {e}", s.file)); }
            },
            ShareTarget::Contact(who) => match &s.handoff {
                None => match request_send(dir, &s.file, &who, s.taken_ms, outbox, now_ms) {
                    Ok(id) => { s.handoff = Some(id); notes.push(format!("{} queued for {who} (snapclone will send it)", s.file)); }
                    Err(e) => { s.fail(&e); notes.push(format!("{}: {e}", s.file)); }
                },
                Some(id) => match snap_handoff::outcome(outbox, id) {
                    Some(snap_handoff::Outcome::Sent) => {
                        snap_handoff::finish(outbox, id);
                        s.handoff = None;
                        s.confirm_sent();
                        notes.push(format!("{} sent to {who}", s.file));
                    }
                    Some(snap_handoff::Outcome::Failed(why)) => {
                        snap_handoff::finish(outbox, id);
                        s.handoff = None;
                        s.fail(&why);
                        notes.push(format!("{}: send to {who} failed: {why}", s.file));
                    }
                    None => {}
                },
            },
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Shot;

    fn capture(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), b"\xff\xd8not really a jpeg\xff\xd9").unwrap();
    }

    #[test]
    fn backing_up_copies_into_the_library_subfolder_atomically() {
        let d = tempfile::tempdir().unwrap();
        let cap = d.path().join("roll");
        capture(&cap, "cam-1.jpg");
        let lib = d.path().join("Pictures");
        let dst = back_up(&cap, "cam-1.jpg", &lib).unwrap();
        assert_eq!(dst, lib.join(LIBRARY_SUBDIR).join("cam-1.jpg"));
        assert_eq!(std::fs::read(&dst).unwrap(), std::fs::read(cap.join("cam-1.jpg")).unwrap());
        assert!(!lib.join(LIBRARY_SUBDIR).join(".cam-1.jpg.part").exists(), "no temp file left");
        assert!(back_up(&cap, "missing.jpg", &lib).is_err());
    }

    #[test]
    fn a_contact_send_becomes_an_outbox_request_naming_the_absolute_file() {
        let d = tempfile::tempdir().unwrap();
        let cap = d.path().join("roll");
        capture(&cap, "cam-2.jpg");
        let outbox = d.path().join("outbox");
        let id = request_send(&cap, "cam-2.jpg", "sc-2dd464a4", 1_700_000_000_000, &outbox, 5).unwrap();
        let pending = snap_handoff::pending(&outbox);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);
        assert_eq!(pending[0].to, "sc-2dd464a4");
        assert!(pending[0].file.is_absolute() && pending[0].file.ends_with("cam-2.jpg"));
        assert_eq!(pending[0].from, "snapshot-cam");
    }

    #[test]
    fn the_poll_walks_a_send_from_pending_to_shared_through_the_marker() {
        let d = tempfile::tempdir().unwrap();
        let cap = d.path().join("roll");
        capture(&cap, "cam-3.jpg");
        capture(&cap, "cam-4.jpg");
        let outbox = d.path().join("outbox");
        let lib = d.path().join("Pictures");
        let mut roll = Roll::default();
        roll.push(Shot::new("cam-3.jpg", 4, 4, 1, "front"));
        roll.push(Shot::new("cam-4.jpg", 4, 4, 2, "front"));
        roll.shots_mut()[0].share_to(ShareTarget::Contact("sc-bee".into())).unwrap();
        roll.shots_mut()[1].share_to(ShareTarget::PhotoSync).unwrap();

        // First poll: the back-up completes, the request is written.
        let notes = poll(&mut roll, &cap, &outbox, &lib, 10);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(matches!(roll.shots()[1].disposition(), Disposition::Shared(ShareTarget::PhotoSync)));
        assert!(lib.join(LIBRARY_SUBDIR).join("cam-3.jpg").exists());
        let id = roll.shots()[0].handoff.clone().expect("the request id is on the shot");
        assert!(matches!(roll.shots()[0].disposition(), Disposition::Pending(_)), "still pending until snapclone answers");
        assert!(roll.shots()[0].disclosed(), "and already counts as disclosed");

        // Nothing answered: nothing changes, nothing is said.
        assert!(poll(&mut roll, &cap, &outbox, &lib, 11).is_empty());

        // snapclone answers.
        snap_handoff::mark(&outbox, &id, &snap_handoff::Outcome::Sent).unwrap();
        let notes = poll(&mut roll, &cap, &outbox, &lib, 12);
        assert_eq!(notes, vec!["cam-4.jpg sent to sc-bee"]);
        assert!(matches!(roll.shots()[0].disposition(), Disposition::Shared(ShareTarget::Contact(_))));
        assert!(roll.shots()[0].handoff.is_none());
        assert!(snap_handoff::pending(&outbox).is_empty(), "the request was cleaned up");

        // A failure comes back as Failed with the reason, retryable.
        roll.push(Shot::new("cam-5.jpg", 4, 4, 3, "front"));
        capture(&cap, "cam-5.jpg");
        roll.shots_mut()[0].share_to(ShareTarget::Contact("sc-bee".into())).unwrap();
        poll(&mut roll, &cap, &outbox, &lib, 13);
        let id = roll.shots()[0].handoff.clone().unwrap();
        snap_handoff::mark(&outbox, &id, &snap_handoff::Outcome::Failed("no known address".into())).unwrap();
        poll(&mut roll, &cap, &outbox, &lib, 14);
        assert!(matches!(roll.shots()[0].disposition(), Disposition::Failed(_, ref why) if why == "no known address"));
        assert!(roll.shots()[0].retryable());
    }

    #[test]
    fn a_photo_missing_from_disk_fails_its_share_rather_than_hanging() {
        let d = tempfile::tempdir().unwrap();
        let mut roll = Roll::default();
        roll.push(Shot::new("never-saved.jpg", 4, 4, 1, "front"));
        roll.shots_mut()[0].share_to(ShareTarget::PhotoSync).unwrap();
        let notes = poll(&mut roll, &d.path().join("roll"), &d.path().join("outbox"), &d.path().join("lib"), 1);
        assert!(notes[0].contains("failed"), "{notes:?}");
        assert!(roll.shots()[0].retryable());
    }
}
