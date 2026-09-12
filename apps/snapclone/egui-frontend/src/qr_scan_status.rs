//! qr_scan_status — why the scanner has not found anything yet.
//!
//! A QR scanner that is looking and a QR scanner that is broken produce the same screen: a
//! camera preview and no result. `TODO_end_to_end.md` names it — *"a scanner that silently
//! does nothing is indistinguishable from a camera that is not running"* — and the person
//! holding the phone has no way to tell which, so they hold the code closer, then at a
//! different angle, then give up on a feature that was never running.
//!
//! This is the state that tells them apart. Four questions, in the order they stop being the
//! problem: are frames arriving, do they contain anything, is the decoder being asked, and has
//! it found something.
//!
//! Pure — counters in, a sentence out. The camera thread and the decoder both just increment.

/// What the scanner has seen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Frames handed to the scanner.
    pub frames: u64,
    /// Frames that looked like an image rather than a black or torn buffer.
    pub plausible: u64,
    /// Frames actually put through the decoder — every Nth, not every one.
    pub attempts: u64,
    /// Successful decodes.
    pub hits: u64,
}

/// What to tell the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Not scanning.
    Idle,
    /// No frames at all — the camera is not delivering.
    NoFrames,
    /// Frames arriving, all of them blank or torn.
    NoPicture,
    /// Looking, and nothing found yet.
    Looking { attempts: u64 },
    /// Found one.
    Found(String),
}

/// How many frames may arrive with nothing in them before that IS the answer.
///
/// A few black frames at startup are normal — a sensor takes a moment to expose. Thirty is
/// about a second at 30fps, long enough not to cry wolf on a cold camera and short enough that
/// someone pointing a phone at a code is not left guessing.
pub const BLANK_GRACE: u64 = 30;

/// How many frames may arrive before "no frames" stops being the explanation.
pub const NO_FRAME_GRACE: u64 = 0;

/// Diagnose.
///
/// The order matters and is the order the faults stop mattering in: a camera delivering
/// nothing cannot produce a blank frame, and blank frames cannot produce a decode attempt.
/// Reporting the later fault first sends someone to fix a decoder when the camera is off.
pub fn status(scanning: bool, c: Counts, last_hit: Option<&str>) -> Status {
    if let Some(id) = last_hit {
        return Status::Found(id.to_string());
    }
    if !scanning {
        return Status::Idle;
    }
    if c.frames <= NO_FRAME_GRACE {
        return Status::NoFrames;
    }
    if c.plausible == 0 && c.frames > BLANK_GRACE {
        return Status::NoPicture;
    }
    Status::Looking { attempts: c.attempts }
}

impl Status {
    /// One line for the scanner's status area.
    ///
    /// Each says what to DO. "No frames from the camera" with no suggestion is only marginally
    /// better than silence — the user still does not know whether to check a permission, a
    /// cable or their aim.
    pub fn line(&self) -> String {
        match self {
            Status::Idle => "scanner off".into(),
            Status::NoFrames => {
                "no frames from the camera — check the camera permission, or that another app \
                 does not have it open"
                    .into()
            }
            Status::NoPicture => {
                "frames are arriving but they are blank — check the lens cover and the light"
                    .into()
            }
            Status::Looking { attempts } => {
                format!("looking… {attempts} frames checked. Fill more of the view with the code")
            }
            Status::Found(id) => format!("found {id}"),
        }
    }

    /// Is this a fault the user can act on, as opposed to normal progress?
    ///
    /// For deciding whether to show the line prominently. "Looking" every second in red teaches
    /// people to ignore the status area, which costs exactly when there IS a fault.
    pub fn is_fault(&self) -> bool {
        matches!(self, Status::NoFrames | Status::NoPicture)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(frames: u64, plausible: u64, attempts: u64) -> Counts {
        Counts { frames, plausible, attempts, hits: 0 }
    }

    /// The order is the order the faults stop mattering in: a camera delivering nothing cannot
    /// produce a blank frame, and blank frames cannot produce a decode attempt. Reporting the
    /// later fault first sends someone to fix a decoder when the camera is off.
    #[test]
    fn the_first_thing_that_is_wrong_is_the_thing_reported() {
        // Nothing at all.
        assert_eq!(status(true, c(0, 0, 0), None), Status::NoFrames);
        // Frames, all blank, past the grace.
        assert_eq!(status(true, c(100, 0, 0), None), Status::NoPicture);
        // Frames with pictures in them, no code yet.
        assert_eq!(status(true, c(100, 100, 25), None), Status::Looking { attempts: 25 });
        // A hit outranks everything, including "not scanning" — the scanner closes on a hit
        // and the result must survive that.
        assert_eq!(status(false, c(0, 0, 0), Some("abc")), Status::Found("abc".into()));
    }

    /// A sensor takes a moment to expose. Reporting "blank" on the first black frame cries
    /// wolf on every cold camera.
    #[test]
    fn a_few_black_frames_at_startup_are_not_a_fault() {
        assert_eq!(status(true, c(5, 0, 0), None), Status::Looking { attempts: 0 });
        assert_eq!(status(true, c(BLANK_GRACE, 0, 0), None), Status::Looking { attempts: 0 });
        assert_eq!(status(true, c(BLANK_GRACE + 1, 0, 0), None), Status::NoPicture);
        // And one good frame among them clears it — the camera plainly works.
        assert_eq!(
            status(true, c(500, 1, 1), None),
            Status::Looking { attempts: 1 },
            "one good frame should stop the blank complaint"
        );
    }

    /// "No frames from the camera" with no suggestion is only marginally better than silence:
    /// the user still does not know whether to check a permission, a cover or their aim.
    #[test]
    fn every_line_says_what_to_do_about_it() {
        assert!(Status::NoFrames.line().contains("permission"));
        assert!(Status::NoPicture.line().contains("lens"));
        assert!(Status::Looking { attempts: 3 }.line().contains("Fill more"));
        assert!(Status::Found("x".into()).line().contains('x'));
    }

    /// "Looking" in red every second teaches people to ignore the status area, which costs
    /// exactly when there IS a fault.
    #[test]
    fn only_actionable_faults_are_marked_as_faults() {
        assert!(Status::NoFrames.is_fault());
        assert!(Status::NoPicture.is_fault());
        assert!(!Status::Looking { attempts: 9 }.is_fault());
        assert!(!Status::Found("x".into()).is_fault());
        assert!(!Status::Idle.is_fault());
    }
}
