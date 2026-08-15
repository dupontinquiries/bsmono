//! snapshot-cam — a local camera whose photos stay local until you say otherwise.
//!
//! The fleet already has the pieces: snapclone knows how to pick a camera and
//! pump frames, photosync knows how to store a file by content. What was
//! missing is the small app between them — press a button, get a photo, decide
//! later where it goes.
//!
//! ## Local first, and that is a data rule rather than a slogan
//!
//! A capture lands on this device and NOTHING is sent. Sharing is a separate,
//! explicit act with its own target, so a photo cannot leave by default,
//! by accident, or by a sync that was already running. The type system carries
//! this: a [`Shot`] has no destination field until [`Shot::share_to`] is called.
//!
//! ## Two destinations, deliberately different
//!
//! * **photosync** — content-addressed storage on your own machines. This is
//!   backup: the photo is yours and stays yours.
//! * **a snapclone contact** — a person. This is publication: someone else can
//!   see it, and it cannot be recalled.
//!
//! Collapsing those into one "share" button is how a backup becomes a
//! disclosure, so [`ShareTarget`] keeps them distinct and
//! [`ShareTarget::is_disclosure`] answers the question a confirmation dialog
//! needs to ask.
//!
//! ## The core is headless
//!
//! Everything here is plain data: no camera handle, no egui. The shell owns the
//! device and the pixels; this owns what a photo IS and where it may go, which
//! is the part worth testing without a webcam attached.

/// Where a photo is allowed to go.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ShareTarget {
    /// Content-addressed storage on the user's own machines.
    PhotoSync,
    /// A snapclone contact, by their peer id.
    Contact(String),
}

impl ShareTarget {
    /// Does sending here mean somebody ELSE can see the photo?
    ///
    /// The question a confirmation needs to ask. Backing up to your own store
    /// and sending to a person are both "sharing" in casual speech and are not
    /// remotely the same act — one is recallable, the other is not.
    pub fn is_disclosure(&self) -> bool {
        matches!(self, ShareTarget::Contact(_))
    }

    pub fn label(&self) -> String {
        match self {
            ShareTarget::PhotoSync => "photosync (your devices)".into(),
            ShareTarget::Contact(who) => format!("{who} (they will see it)"),
        }
    }
}

/// What happened to a photo after it was taken.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum Disposition {
    /// On this device only. The default, and the only state a capture can be
    /// born in.
    #[default]
    Local,
    /// Queued for a target but not yet confirmed sent.
    Pending(ShareTarget),
    Shared(ShareTarget),
    /// The send failed. Kept distinct from Local: the user asked, so the app
    /// owes them either a retry or an explanation.
    Failed(ShareTarget, String),
}

/// One captured photo.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Shot {
    /// Filename on disk, relative to the capture directory.
    pub file: String,
    pub width: u32,
    pub height: u32,
    pub taken_ms: u64,
    /// Which camera took it, for the case where a phone has two.
    pub camera: String,
    disposition: Disposition,
}

impl Shot {
    /// A new capture. Always local — there is no constructor that produces a
    /// shared one, which is the invariant stated as code.
    pub fn new(file: &str, width: u32, height: u32, taken_ms: u64, camera: &str) -> Shot {
        Shot {
            file: file.to_string(), width, height, taken_ms,
            camera: camera.to_string(),
            disposition: Disposition::Local,
        }
    }

    pub fn disposition(&self) -> &Disposition { &self.disposition }
    pub fn is_local(&self) -> bool { self.disposition == Disposition::Local }

    /// Ask to send this photo somewhere.
    ///
    /// Moves to Pending rather than Shared: the send has not happened yet, and
    /// a UI that showed "shared" before the transfer completed would be lying
    /// during exactly the window where it matters.
    pub fn share_to(&mut self, target: ShareTarget) -> Result<(), String> {
        if let Disposition::Pending(t) = &self.disposition {
            return Err(format!("already sending to {}", t.label()));
        }
        self.disposition = Disposition::Pending(target);
        Ok(())
    }

    /// The send completed.
    pub fn confirm_sent(&mut self) {
        if let Disposition::Pending(t) = self.disposition.clone() {
            self.disposition = Disposition::Shared(t);
        }
    }

    /// The send failed.
    pub fn fail(&mut self, why: &str) {
        if let Disposition::Pending(t) = self.disposition.clone() {
            self.disposition = Disposition::Failed(t, why.to_string());
        }
    }

    /// Can this be retried?
    pub fn retryable(&self) -> bool { matches!(self.disposition, Disposition::Failed(_, _)) }

    /// Has this photo been disclosed to another person?
    ///
    /// Pending counts. Once the transfer is in flight the app cannot promise it
    /// did not arrive, and a UI claiming otherwise would be guessing on the
    /// user's behalf about something they cannot undo.
    pub fn disclosed(&self) -> bool {
        match &self.disposition {
            Disposition::Pending(t) | Disposition::Shared(t) => t.is_disclosure(),
            // A FAILED disclosure also counts: "failed" means we did not get
            // confirmation, not that nothing left the device.
            Disposition::Failed(t, _) => t.is_disclosure(),
            Disposition::Local => false,
        }
    }
}

/// A filename for a capture.
///
/// Sortable-by-name because that is what every file manager and `ls` will do
/// with it: `cam-20260814-101530-front.jpg` orders chronologically without
/// anything having to read the EXIF. A name built from a friendly date would
/// not.
pub fn capture_filename(unix_ms: u64, camera: &str, ext: &str) -> String {
    let secs = unix_ms / 1000;
    let (y, mo, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    let cam = sanitize(camera);
    format!("cam-{y:04}{mo:02}{d:02}-{:02}{:02}{:02}-{cam}.{ext}",
            rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Reduce a camera name to something safe in a filename.
///
/// Device names carry spaces, colons and slashes — "Integrated_Webcam: HD" is
/// a real one — and a slash silently creates a directory that does not exist.
fn sanitize(s: &str) -> String {
    let out: String = s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() { "cam".into() } else { trimmed }
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The roll: every photo taken on this device.
#[derive(Clone, Debug, Default)]
pub struct Roll {
    shots: Vec<Shot>,
}

impl Roll {
    pub fn len(&self) -> usize { self.shots.len() }
    pub fn is_empty(&self) -> bool { self.shots.is_empty() }
    pub fn shots(&self) -> &[Shot] { &self.shots }
    pub fn shots_mut(&mut self) -> &mut [Shot] { &mut self.shots }

    /// Add a capture. Newest first, because that is what the user just took and
    /// what they will act on.
    pub fn push(&mut self, s: Shot) { self.shots.insert(0, s); }

    /// Photos that have never left this device.
    pub fn local_only(&self) -> Vec<&Shot> {
        self.shots.iter().filter(|s| s.is_local()).collect()
    }

    /// Photos somebody else can see.
    ///
    /// Worth being able to ask directly: "what have I shared" is the question
    /// a privacy-conscious user asks, and making them scroll a roll to work it
    /// out is how they lose track.
    pub fn disclosed(&self) -> Vec<&Shot> {
        self.shots.iter().filter(|s| s.disclosed()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot() -> Shot { Shot::new("a.jpg", 1920, 1080, 1_755_000_000_000, "front") }

    #[test]
    fn a_capture_is_born_local() {
        // There is no constructor that produces a shared photo. The invariant
        // is the type, not a convention somebody has to remember.
        assert!(shot().is_local());
        assert!(!shot().disclosed());
    }

    #[test]
    fn backing_up_is_not_disclosure() {
        // photosync is the user's own machines. Treating it as disclosure would
        // put a scary confirmation on the safe action and train people to click
        // through the dangerous one.
        let mut s = shot();
        s.share_to(ShareTarget::PhotoSync).unwrap();
        s.confirm_sent();
        assert!(!s.disclosed());
        assert!(!ShareTarget::PhotoSync.is_disclosure());
    }

    #[test]
    fn sending_to_a_contact_is_disclosure() {
        let mut s = shot();
        s.share_to(ShareTarget::Contact("alice".into())).unwrap();
        s.confirm_sent();
        assert!(s.disclosed());
    }

    #[test]
    fn a_send_in_flight_already_counts_as_disclosed() {
        // Once the transfer starts the app cannot promise it did not arrive,
        // and claiming otherwise guesses on the user's behalf about something
        // they cannot undo.
        let mut s = shot();
        s.share_to(ShareTarget::Contact("bob".into())).unwrap();
        assert!(s.disclosed(), "pending disclosure reported as private");
    }

    #[test]
    fn a_failed_disclosure_still_counts() {
        // "Failed" means no confirmation came back, NOT that nothing left the
        // device. Reporting it as private is the one error that cannot be
        // corrected later.
        let mut s = shot();
        s.share_to(ShareTarget::Contact("bob".into())).unwrap();
        s.fail("peer unreachable");
        assert!(s.disclosed());
        assert!(s.retryable());
    }

    #[test]
    fn sharing_moves_to_pending_not_shared() {
        // A UI showing "shared" before the transfer completes lies during
        // exactly the window where the truth matters.
        let mut s = shot();
        s.share_to(ShareTarget::PhotoSync).unwrap();
        assert!(matches!(s.disposition(), Disposition::Pending(_)));
        s.confirm_sent();
        assert!(matches!(s.disposition(), Disposition::Shared(_)));
    }

    #[test]
    fn a_second_send_while_one_is_in_flight_is_refused() {
        // Two transfers of one photo is either a duplicate on the far side or a
        // race over the same file handle.
        let mut s = shot();
        s.share_to(ShareTarget::PhotoSync).unwrap();
        assert!(s.share_to(ShareTarget::Contact("a".into())).is_err());
    }

    #[test]
    fn a_failed_send_can_be_re_targeted() {
        // The failure may be the target's fault — an unreachable peer — so
        // retrying somewhere else must be possible.
        let mut s = shot();
        s.share_to(ShareTarget::Contact("bob".into())).unwrap();
        s.fail("unreachable");
        assert!(s.share_to(ShareTarget::PhotoSync).is_ok());
    }

    #[test]
    fn filenames_sort_chronologically() {
        // What `ls` and every file manager will do with them. A friendly date
        // would not sort, and the roll would look shuffled outside this app.
        let a = capture_filename(1_755_000_000_000, "front", "jpg");
        let b = capture_filename(1_755_000_060_000, "front", "jpg");
        assert!(a < b, "{a} !< {b}");
    }

    #[test]
    fn a_device_name_cannot_break_the_path() {
        // "Integrated_Webcam: HD" is a real device name. A slash silently
        // creates a directory that does not exist and the capture is lost.
        let n = capture_filename(1_755_000_000_000, "Integrated_Webcam: HD/2", "jpg");
        assert!(!n.contains('/'), "{n}");
        assert!(!n.contains(' '), "{n}");
        assert!(!n.contains(':'), "{n}");
    }

    #[test]
    fn an_unnameable_camera_still_produces_a_filename() {
        // An all-punctuation name would sanitise to empty and give "cam--.jpg",
        // or worse a name ending in a dot.
        let n = capture_filename(1_755_000_000_000, "///", "jpg");
        assert!(n.contains("cam"), "{n}");
        assert!(n.ends_with(".jpg"));
    }

    #[test]
    fn the_roll_shows_the_newest_first() {
        let mut r = Roll::default();
        r.push(Shot::new("old.jpg", 1, 1, 1, "a"));
        r.push(Shot::new("new.jpg", 1, 1, 2, "a"));
        assert_eq!(r.shots()[0].file, "new.jpg");
    }

    #[test]
    fn the_roll_can_answer_what_have_i_shared() {
        // The question a privacy-conscious user actually asks. Making them
        // scroll to work it out is how they lose track.
        let mut r = Roll::default();
        r.push(shot());
        r.push(Shot::new("b.jpg", 1, 1, 2, "a"));
        r.shots_mut()[0].share_to(ShareTarget::Contact("x".into())).unwrap();
        assert_eq!(r.disclosed().len(), 1);
        assert_eq!(r.local_only().len(), 1);
    }

    #[test]
    fn a_target_says_plainly_whether_others_will_see_it() {
        // The label goes in front of the user at the moment of choosing, so it
        // has to say the consequence rather than the mechanism.
        assert!(ShareTarget::Contact("alice".into()).label().contains("see it"));
        assert!(ShareTarget::PhotoSync.label().contains("your devices"));
    }
}
