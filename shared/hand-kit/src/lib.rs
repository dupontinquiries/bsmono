//! hand-kit — the gesture taxonomy, and a way to score a detector against a
//! real device's own captures.
//!
//! ## The dataset is the contribution, not the model
//!
//! A hand tracker is easy to write and hard to trust. What makes it trustable
//! is a labelled set from **the camera it will actually run on** — and the
//! `database-small` capture set is exactly that: 41 frames at 1920×1080 from
//! both Fairphone cameras, with the label encoded in the filename:
//!
//! ```text
//!   front--left-open-side-fingers3.jpeg
//!   │      │    │    │    │
//!   │      │    │    │    └── how many fingers are extended
//!   │      │    │    └─────── which way the hand faces the lens
//!   │      │    └──────────── aperture: open / closed / halfway
//!   │      └───────────────── which hand
//!   └──────────────────────── which camera
//! ```
//!
//! Parsing that convention into types is what turns a folder into a test
//! fixture. Every claim a detector makes can then be checked against a frame
//! somebody actually took, rather than against a benchmark shot on other
//! hardware in other light.
//!
//! ## Why the labels are parsed rather than retyped
//!
//! A hand-written table would drift from the folder the moment a capture is
//! added, and the drift would be invisible — the harness would keep passing on
//! the frames it knew about. Parsing means a new capture is either understood
//! or **loudly unparsed**, which is the failure worth having.
//!
//! ## What this crate deliberately does not do
//!
//! No image decoding, no inference, no model download. It is the taxonomy and
//! the scoring, so the interesting question — *does this detector work on my
//! phone's camera* — can be asked without one.

// ------------------------------------------------------------- taxonomy ---

/// Which camera took the frame.
///
/// Kept because it changes everything downstream: the front camera is fixed
/// focus, wider, and the hand is usually closer and partly out of frame, while
/// the rear is sharper with the hand further away. A detector that scores well
/// on one and badly on the other is not a detector that works.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Camera { Front, Rear }

/// Which hand.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hand { Left, Right, Both }

/// Which way the hand faces the lens.
///
/// `Side` is the hard case and the reason it is a separate value: edge-on, most
/// fingers occlude each other, and a finger count taken from a silhouette is
/// wrong in a way that looks confident.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Facing { Palm, Back, Side }

/// How open the hand is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Aperture { Closed, Halfway, Open }

/// A named gesture beyond the open/closed axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gesture { LPose, OkSymbol, IndexUp }

/// One labelled capture.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Capture {
    pub file: String,
    pub camera: Camera,
    pub hand: Option<Hand>,
    pub facing: Option<Facing>,
    pub aperture: Option<Aperture>,
    /// Extended fingers, when the label states a count.
    pub fingers: Option<u8>,
    pub gesture: Option<Gesture>,
    /// Captured in low light — the label says `dark`.
    pub dark: bool,
    /// Two hands in frame.
    pub two_hands: bool,
}

impl Capture {
    /// Is this frame a fair test of finger counting?
    ///
    /// Edge-on hands are excluded: fingers occlude each other, so a count
    /// scored against a side view measures whether the detector guessed, not
    /// whether it saw. Scoring them together hides a real weakness behind an
    /// average.
    pub fn counts_fingers_fairly(&self) -> bool {
        self.fingers.is_some() && self.facing != Some(Facing::Side) && !self.two_hands
    }
}

/// Parse a capture filename. `None` when the name carries no usable label —
/// which is the honest answer for the two `Photo from …` frames, whose names
/// say nothing about what is in them.
pub fn parse(file: &str) -> Option<Capture> {
    let stem = file
        .rsplit_once('.').map(|(a, _)| a).unwrap_or(file)
        // several files carry a doubled extension: `…-1-finger.jpg.jpg`
        .trim_end_matches(".jpg").trim_end_matches(".jpeg");
    let lower = stem.to_lowercase();

    // A name with no camera prefix is not a labelled capture.
    let camera = if lower.starts_with("front") { Camera::Front }
                 else if lower.starts_with("rear") { Camera::Rear }
                 else { return None };

    // The timestamp captures are named by the camera app, not by us.
    if lower.contains("photo from") { return None; }

    let has = |t: &str| lower.contains(t);

    let two_hands = has("2hands");
    let hand = if two_hands { Some(Hand::Both) }
               else if has("left") { Some(Hand::Left) }
               else if has("right") { Some(Hand::Right) }
               else { None };

    // "frontside"/"backside" describe the hand; the leading "front-" is the
    // camera. Checked as whole tokens so the camera prefix cannot be read as a
    // facing — the bug this ordering exists to prevent.
    let facing = if has("side") && !has("frontside") && !has("backside") { Some(Facing::Side) }
                 else if has("backside") || has("-back") { Some(Facing::Back) }
                 else if has("frontside") || has("-front-") { Some(Facing::Palm) }
                 else { None };

    let aperture = if has("halfway") || has("partialopen") { Some(Aperture::Halfway) }
                   else if has("closed") || has("fist") { Some(Aperture::Closed) }
                   else if has("open") { Some(Aperture::Open) }
                   else { None };

    let gesture = if has("lpose") || has("lshape") { Some(Gesture::LPose) }
                  else if has("oksymbol") { Some(Gesture::OkSymbol) }
                  else if has("index-up") { Some(Gesture::IndexUp) }
                  else { None };

    let fingers = finger_count(&lower);

    Some(Capture {
        file: file.to_string(), camera, hand, facing, aperture, fingers, gesture,
        dark: has("dark"), two_hands,
    })
}

/// Pull a finger count from a label.
///
/// Two conventions live in the set — `fingers3` and `3-finger`, plus
/// `0-finger-fist` and `5fingers`. Handling both is not tidiness: a parser that
/// knew only one would silently drop every rear-camera count, and the harness
/// would report a smaller, easier test set without saying so.
fn finger_count(lower: &str) -> Option<u8> {
    let b = lower.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if !c.is_ascii_digit() { continue; }
        let d = c - b'0';
        if d > 5 { continue; }
        let before = lower[..i].as_bytes();
        let after = &lower[i + 1..];
        let is_fingers_prefix = before.ends_with(b"fingers") || before.ends_with(b"finger");
        let is_count_prefix = after.starts_with("finger") || after.starts_with("-finger");
        if is_fingers_prefix || is_count_prefix { return Some(d); }
    }
    None
}

// ------------------------------------------------------------- scoring ---

/// What a detector claimed about one frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Claim {
    pub hand_present: bool,
    pub fingers: Option<u8>,
    pub aperture: Option<Aperture>,
}

/// How a detector did over a set.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Score {
    pub frames: usize,
    /// Frames where a hand was present and found.
    pub detected: usize,
    /// Frames scored for finger count, and how many were right.
    pub finger_scored: usize,
    pub finger_correct: usize,
    /// Aperture agreement.
    pub aperture_scored: usize,
    pub aperture_correct: usize,
}

impl Score {
    pub fn detection_rate(&self) -> Option<f32> {
        if self.frames == 0 { None } else { Some(self.detected as f32 / self.frames as f32) }
    }
    pub fn finger_accuracy(&self) -> Option<f32> {
        if self.finger_scored == 0 { None }
        else { Some(self.finger_correct as f32 / self.finger_scored as f32) }
    }
    pub fn aperture_accuracy(&self) -> Option<f32> {
        if self.aperture_scored == 0 { None }
        else { Some(self.aperture_correct as f32 / self.aperture_scored as f32) }
    }
}

/// Score claims against truth.
///
/// Rates are `Option`: a set with nothing to score returns `None` rather than
/// 0%, because zero reads as total failure and would condemn a detector nobody
/// has tested yet.
pub fn score(pairs: &[(&Capture, Claim)]) -> Score {
    let mut s = Score::default();
    for (truth, claim) in pairs {
        s.frames += 1;
        if claim.hand_present { s.detected += 1; }
        // A missed hand cannot be scored on fingers — counting it as a wrong
        // count would charge the same mistake twice and hide which stage
        // failed.
        if !claim.hand_present { continue; }
        if truth.counts_fingers_fairly() {
            s.finger_scored += 1;
            if claim.fingers == truth.fingers { s.finger_correct += 1; }
        }
        if let Some(a) = truth.aperture {
            s.aperture_scored += 1;
            if claim.aperture == Some(a) { s.aperture_correct += 1; }
        }
    }
    s
}

/// Split a score by a property, so an average cannot hide a weakness.
///
/// The reason this exists: a detector that is excellent on palm-forward frames
/// and useless edge-on averages to "quite good", and the average is the number
/// somebody would ship on.
pub fn partition<'a>(pairs: &[(&'a Capture, Claim)], pred: impl Fn(&Capture) -> bool)
    -> (Score, Score) {
    let yes: Vec<(&Capture, Claim)> =
        pairs.iter().filter(|(c, _)| pred(c)).map(|(c, m)| (*c, *m)).collect();
    let no: Vec<(&Capture, Claim)> =
        pairs.iter().filter(|(c, _)| !pred(c)).map(|(c, m)| (*c, *m)).collect();
    (score(&yes), score(&no))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real filenames from the capture set.
    const FILES: &[&str] = &[
        "front--front-closed.jpeg",
        "front--front-open-fingers1.jpeg",
        "front--front-open-fingers2.jpeg",
        "front--front-open-fingers3.jpeg",
        "front--front-open-fingers4.jpeg",
        "front--front-open.jpeg",
        "front--left-Lshape2-dark.jpeg",
        "front--left-open-side-closed.jpeg",
        "front--left-open-side-fingers2.jpeg",
        "front--left-open-side.jpeg",
        "front--left-partialopen.jpeg",
        "front-2hands-closed-back.jpeg",
        "front-2hands-index-up-backside.jpeg",
        "front-2hands-left-Lpose-frontside.jpeg",
        "rear--left-back-5fingers.jpg",
        "rear--left-front-0-finger-fist.jpg",
        "rear--left-front-1-finger.jpg.jpg",
        "rear--left-front-2-finger.jpg",
        "rear--left-front-4finger.jpg",
        "rear--left-front-halfway.jpg",
        "rear--left-front-oksymbol.jpg.jpg",
        "rear--left-side-closed.jpg",
    ];

    fn cap(f: &str) -> Capture { parse(f).expect(f) }

    #[test]
    fn every_labelled_capture_parses() {
        // A new capture is either understood or LOUDLY unparsed — the failure
        // worth having, versus a hand-written table that silently keeps passing
        // on the frames it already knew.
        for f in FILES { assert!(parse(f).is_some(), "unparsed: {f}"); }
    }

    #[test]
    fn an_unlabelled_photo_is_none_rather_than_a_guess() {
        // The camera app's own names say nothing about what is in them, and
        // inventing a label would put fiction in the ground truth.
        assert!(parse("Photo from 2026-08-10 19-50-36.788921.jpeg").is_none());
        assert!(parse("random.jpeg").is_none());
    }

    #[test]
    fn the_camera_prefix_is_not_read_as_a_facing() {
        // "front--front-open" is the FRONT CAMERA showing a PALM. Reading the
        // first token as facing would mislabel every front-camera frame.
        let c = cap("front--front-open.jpeg");
        assert_eq!(c.camera, Camera::Front);
        assert_eq!(c.facing, Some(Facing::Palm));
        let r = cap("rear--left-front-open.jpg");
        assert_eq!(r.camera, Camera::Rear);
        assert_eq!(r.facing, Some(Facing::Palm));
    }

    #[test]
    fn frontside_is_a_facing_and_side_alone_is_edge_on() {
        // The set uses both, and conflating them turns the hardest case into
        // the easiest.
        assert_eq!(cap("front-2hands-left-Lpose-frontside.jpeg").facing, Some(Facing::Palm));
        assert_eq!(cap("front--left-open-side.jpeg").facing, Some(Facing::Side));
        assert_eq!(cap("front-2hands-index-up-backside.jpeg").facing, Some(Facing::Back));
    }

    #[test]
    fn both_finger_count_conventions_are_read() {
        // `fingers3` on the front camera and `3-finger` on the rear. A parser
        // knowing only one would silently drop every rear count and report a
        // smaller, easier test set without saying so.
        assert_eq!(cap("front--front-open-fingers3.jpeg").fingers, Some(3));
        assert_eq!(cap("rear--left-front-2-finger.jpg").fingers, Some(2));
        assert_eq!(cap("rear--left-front-4finger.jpg").fingers, Some(4));
        assert_eq!(cap("rear--left-back-5fingers.jpg").fingers, Some(5));
        assert_eq!(cap("rear--left-front-0-finger-fist.jpg").fingers, Some(0));
    }

    #[test]
    fn a_doubled_extension_does_not_break_the_label() {
        // Several rear captures are named `….jpg.jpg`.
        let c = cap("rear--left-front-1-finger.jpg.jpg");
        assert_eq!(c.fingers, Some(1));
        assert_eq!(cap("rear--left-front-oksymbol.jpg.jpg").gesture, Some(Gesture::OkSymbol));
    }

    #[test]
    fn a_number_that_is_not_a_finger_count_is_not_read_as_one() {
        // "Lshape2" is the second L-pose capture, not two fingers. A loose
        // digit scan would put a wrong count in the ground truth, which is
        // worse than a missing one.
        let c = cap("front--left-Lshape2-dark.jpeg");
        assert_eq!(c.fingers, None);
        assert_eq!(c.gesture, Some(Gesture::LPose));
        assert!(c.dark);
    }

    #[test]
    fn a_fist_is_closed_and_partialopen_is_halfway() {
        assert_eq!(cap("rear--left-front-0-finger-fist.jpg").aperture, Some(Aperture::Closed));
        assert_eq!(cap("front--left-partialopen.jpeg").aperture, Some(Aperture::Halfway));
        assert_eq!(cap("rear--left-front-halfway.jpg").aperture, Some(Aperture::Halfway));
    }

    #[test]
    fn closed_wins_over_open_when_a_name_carries_both() {
        // "front--left-open-side-closed" is an open-side view of a CLOSED hand;
        // reading the first match would label a fist as an open hand.
        assert_eq!(cap("front--left-open-side-closed.jpeg").aperture, Some(Aperture::Closed));
    }

    #[test]
    fn two_hand_frames_are_marked_and_not_counted_for_fingers() {
        // A count label on a two-hand frame is ambiguous about whose fingers,
        // so those frames must not enter the finger score.
        let c = cap("front-2hands-closed-back.jpeg");
        assert!(c.two_hands);
        assert_eq!(c.hand, Some(Hand::Both));
        assert!(!c.counts_fingers_fairly());
    }

    #[test]
    fn edge_on_frames_are_excluded_from_finger_scoring() {
        // Fingers occlude each other side-on, so a count scored there measures
        // whether the detector guessed, not whether it saw.
        let side = cap("front--left-open-side-fingers2.jpeg");
        assert_eq!(side.facing, Some(Facing::Side));
        assert!(!side.counts_fingers_fairly());
        assert!(cap("front--front-open-fingers2.jpeg").counts_fingers_fairly());
    }

    #[test]
    fn a_missed_hand_is_not_also_charged_a_wrong_finger_count() {
        // Counting it twice hides which stage failed — detection or counting.
        let c = cap("front--front-open-fingers3.jpeg");
        let s = score(&[(&c, Claim { hand_present: false, fingers: None, aperture: None })]);
        assert_eq!(s.frames, 1);
        assert_eq!(s.detected, 0);
        assert_eq!(s.finger_scored, 0, "scored a count on a frame with no detection");
    }

    #[test]
    fn a_perfect_detector_scores_perfectly() {
        let caps: Vec<Capture> = FILES.iter().filter_map(|f| parse(f)).collect();
        let pairs: Vec<(&Capture, Claim)> = caps.iter()
            .map(|c| (c, Claim { hand_present: true, fingers: c.fingers, aperture: c.aperture }))
            .collect();
        let s = score(&pairs);
        assert_eq!(s.detection_rate(), Some(1.0));
        assert_eq!(s.finger_accuracy(), Some(1.0));
        assert_eq!(s.aperture_accuracy(), Some(1.0));
    }

    #[test]
    fn an_empty_set_reports_none_rather_than_zero_percent() {
        // Zero reads as total failure and would condemn a detector nobody has
        // tested yet.
        let s = score(&[]);
        assert_eq!(s.detection_rate(), None);
        assert_eq!(s.finger_accuracy(), None);
    }

    #[test]
    fn partitioning_stops_an_average_hiding_a_weakness() {
        // A detector excellent palm-forward and useless edge-on averages to
        // "quite good", and the average is the number somebody ships on.
        let caps: Vec<Capture> = FILES.iter().filter_map(|f| parse(f)).collect();
        let pairs: Vec<(&Capture, Claim)> = caps.iter()
            .map(|c| {
                // pretend: finds palms, misses edge-on hands entirely
                let ok = c.facing != Some(Facing::Side);
                (c, Claim { hand_present: ok, fingers: if ok { c.fingers } else { None },
                            aperture: if ok { c.aperture } else { None } })
            })
            .collect();
        let (palmish, edge) = partition(&pairs, |c| c.facing != Some(Facing::Side));
        assert_eq!(palmish.detection_rate(), Some(1.0));
        assert_eq!(edge.detection_rate(), Some(0.0), "the weakness is visible");
        assert!(score(&pairs).detection_rate().unwrap() > 0.6, "and the average hides it");
    }

    #[test]
    fn both_cameras_are_represented_so_one_cannot_be_tuned_for() {
        // The front camera is fixed-focus and wider with the hand closer; the
        // rear is sharper and further. A detector good on one only is not a
        // detector that works.
        let caps: Vec<Capture> = FILES.iter().filter_map(|f| parse(f)).collect();
        assert!(caps.iter().any(|c| c.camera == Camera::Front));
        assert!(caps.iter().any(|c| c.camera == Camera::Rear));
    }
}
