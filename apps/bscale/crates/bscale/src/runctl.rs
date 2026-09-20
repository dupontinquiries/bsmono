//! runctl — what Stop, Pause and Resume mean, and which of them is offered.
//!
//! The shell has one control for ending a run: **Stop**. Its log line is
//! honest about the mechanism — *"stopping after the current frame; the
//! encoder finalises a partial file"* — and says nothing about what happens
//! next, because until now nothing did. `d2x_job::checkpoint` can resume a
//! segmented upscale from the segments already on disk, and it has **no
//! callers anywhere**: `Checkpoint::resumable`, `path_for` and `fingerprint`
//! are all written, tested and unreachable.
//!
//! So a person who stops a four-hour upscale at three hours loses three hours,
//! and the code to not lose them was already there.
//!
//! ## Stop and Pause are different promises, not different buttons
//!
//! Offering both without distinguishing them is worse than offering one.
//!
//! * **Stop** ends the run and keeps the partial output. The file is playable.
//!   Nothing is promised about continuing, and for a non-segmented run nothing
//!   can be.
//! * **Pause** ends the run and promises it can be continued from where it
//!   got to. That promise is only keepable when the work is durable at some
//!   granularity, which for this pipeline means segments — a segment ends with
//!   a complete, independently decodable file, and the encoder is not
//!   resumable mid-stream.
//!
//! A Pause button on a run that cannot resume is a lie the user finds out
//! about an hour later, which is why [`Controls::for_run`] does not offer one.
//!
//! ## Resume is offered by the OUTPUT, not by memory
//!
//! The obvious design keeps "the paused job" in the app. That is wrong the
//! moment the app closes, which is most of why anyone wanted a pause — a
//! laptop lid, a reboot, a crash. The checkpoint is a file beside the output,
//! so resumability is a question you ask about a PATH, and it survives
//! everything the app does not.

use std::collections::BTreeSet;

/// What the run is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunState {
    /// Nothing running.
    #[default]
    Idle,
    /// Running normally.
    Running,
    /// Stop requested, still finishing the current frame and finalising.
    ///
    /// A real state rather than a transition: finalising a large file takes
    /// visible time, and a UI that jumps straight to Idle invites the user to
    /// start another run into the same output while the encoder still has it.
    Stopping,
    /// Pause requested, finishing the current segment so it is resumable.
    ///
    /// Distinct from `Stopping` because the wait is for a different thing —
    /// a segment boundary rather than the current frame — and because the
    /// user is owed a different sentence about it.
    Pausing,
    /// Ended, with a partial output kept.
    Stopped,
    /// Ended at a segment boundary, resumable.
    Paused,
    /// Ran to completion.
    Done,
}

impl RunState {
    /// Is work still happening?
    ///
    /// `Stopping` and `Pausing` count: the thread is alive, the output file is
    /// open, and starting another run now would write into it.
    pub fn is_busy(self) -> bool {
        matches!(self, RunState::Running | RunState::Stopping | RunState::Pausing)
    }

    /// The sentence to show. Each names what is being waited for, because
    /// "stopping…" alone reads as a hang on a run that takes a minute to
    /// finalise.
    pub fn status(self) -> &'static str {
        match self {
            RunState::Idle => "",
            RunState::Running => "running",
            RunState::Stopping => "finishing the current frame, then finalising a playable partial file",
            RunState::Pausing => "finishing the current segment so the run can be continued",
            RunState::Stopped => "stopped — the partial output is playable",
            RunState::Paused => "paused — continue from the last finished segment",
            RunState::Done => "finished",
        }
    }
}

/// Whether a run's work is durable partway through.
///
/// The one fact that decides whether Pause can be offered at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// Segmented: each segment lands as its own decodable file, so the run can
    /// stop between segments and continue later.
    Segmented { total: usize },
    /// One continuous encode. Stopping keeps what was written; there is no
    /// boundary to continue from, because the encoder cannot resume mid-stream.
    Continuous,
}

/// Which controls to show, and which to enable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controls {
    pub start_enabled: bool,
    pub stop_enabled: bool,
    /// Shown at all. A Pause button that is merely disabled on a continuous
    /// run still suggests the feature exists for it.
    pub pause_shown: bool,
    pub pause_enabled: bool,
    /// Shown when a checkpoint beside the output says there is something to
    /// continue.
    pub resume_shown: bool,
}

impl Controls {
    /// The control set for a given state, durability and checkpoint.
    ///
    /// `resumable` is the set of finished segments found on disk — from
    /// `Checkpoint::resumable`, which is the function this whole module exists
    /// to give a caller.
    pub fn for_run(state: RunState, dur: Durability, resumable: &BTreeSet<usize>) -> Controls {
        let busy = state.is_busy();
        let segmented = matches!(dur, Durability::Segmented { .. });
        // A run that finished every segment is complete, not resumable —
        // offering Resume there would re-run nothing and look broken.
        let partial = match dur {
            Durability::Segmented { total } => !resumable.is_empty() && resumable.len() < total,
            Durability::Continuous => false,
        };
        Controls {
            start_enabled: !busy,
            stop_enabled: busy,
            // Only where the promise is keepable, and never while already
            // winding down — a second press should not switch a Stopping run
            // into a Pausing one, because the frame it is finishing is not a
            // segment boundary.
            pause_shown: segmented,
            pause_enabled: segmented && state == RunState::Running,
            resume_shown: !busy && partial,
        }
    }
}

/// What happens when the user asks to end a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndRequest {
    /// Cut the source now; finalise whatever is written.
    Stop,
    /// Cut at the next segment boundary so the work is resumable.
    Pause,
}

impl EndRequest {
    /// The state the run moves to.
    pub fn next_state(self) -> RunState {
        match self {
            EndRequest::Stop => RunState::Stopping,
            EndRequest::Pause => RunState::Pausing,
        }
    }

    /// The log line. Says what is being waited for and what will be true
    /// afterwards, because the old one said only the mechanism.
    pub fn log(self) -> &'static str {
        match self {
            EndRequest::Stop =>
                "stopping after the current frame; the encoder finalises a playable partial file. \
                 This run cannot be continued — use Pause for that.",
            EndRequest::Pause =>
                "pausing at the end of the current segment; finished segments are kept and the \
                 run can be continued from there.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> BTreeSet<usize> {
        BTreeSet::new()
    }

    fn some(ids: &[usize]) -> BTreeSet<usize> {
        ids.iter().copied().collect()
    }

    /// PAUSE IS NOT OFFERED WHERE IT CANNOT BE KEPT. A Pause button on a
    /// continuous encode is a lie the user discovers an hour later.
    #[test]
    fn a_continuous_run_has_no_pause_at_all() {
        let c = Controls::for_run(RunState::Running, Durability::Continuous, &none());
        assert!(!c.pause_shown, "not merely disabled — absent");
        assert!(!c.pause_enabled);
        assert!(c.stop_enabled, "stopping still works and still keeps the partial");
    }

    #[test]
    fn a_segmented_run_offers_pause_while_running() {
        let c = Controls::for_run(RunState::Running, Durability::Segmented { total: 10 }, &none());
        assert!(c.pause_shown && c.pause_enabled);
    }

    /// A second press must not switch a Stopping run into a Pausing one: the
    /// frame it is finishing is not a segment boundary, so the promise could
    /// not be kept.
    #[test]
    fn pause_is_disabled_once_a_run_is_already_winding_down() {
        for s in [RunState::Stopping, RunState::Pausing] {
            let c = Controls::for_run(s, Durability::Segmented { total: 10 }, &none());
            assert!(c.pause_shown, "the button stays visible so it does not jump about");
            assert!(!c.pause_enabled, "{s:?} must not accept a pause");
        }
    }

    /// STOPPING AND PAUSING ARE BUSY. The thread is alive and the output file
    /// is open; starting another run now would write into it.
    #[test]
    fn a_winding_down_run_still_blocks_a_new_start() {
        for s in [RunState::Running, RunState::Stopping, RunState::Pausing] {
            assert!(s.is_busy(), "{s:?}");
            let c = Controls::for_run(s, Durability::Segmented { total: 4 }, &none());
            assert!(!c.start_enabled, "{s:?} must not allow a second run");
        }
    }

    /// RESUME IS OFFERED BY THE OUTPUT. Some segments done, not all.
    #[test]
    fn resume_appears_when_a_checkpoint_has_partial_work() {
        let c = Controls::for_run(
            RunState::Paused, Durability::Segmented { total: 10 }, &some(&[0, 1, 2]));
        assert!(c.resume_shown);
    }

    /// A run with EVERY segment done is finished, not resumable — Resume there
    /// would re-run nothing and look broken.
    #[test]
    fn a_complete_checkpoint_does_not_offer_resume() {
        let all = some(&[0, 1, 2, 3]);
        let c = Controls::for_run(RunState::Done, Durability::Segmented { total: 4 }, &all);
        assert!(!c.resume_shown);
    }

    #[test]
    fn an_empty_checkpoint_does_not_offer_resume() {
        let c = Controls::for_run(RunState::Idle, Durability::Segmented { total: 4 }, &none());
        assert!(!c.resume_shown, "nothing finished means nothing to continue");
    }

    /// Resume must not appear mid-run: the checkpoint is being written, and
    /// continuing into an output the encoder still holds is the one thing this
    /// model exists to prevent.
    #[test]
    fn resume_is_hidden_while_anything_is_running() {
        for s in [RunState::Running, RunState::Stopping, RunState::Pausing] {
            let c = Controls::for_run(s, Durability::Segmented { total: 9 }, &some(&[0, 1]));
            assert!(!c.resume_shown, "{s:?}");
        }
    }

    /// Every state says what it is WAITING FOR. "stopping…" alone reads as a
    /// hang on a run that takes a minute to finalise a large file.
    #[test]
    fn the_transient_states_explain_the_wait() {
        assert!(RunState::Stopping.status().contains("finalis"));
        assert!(RunState::Pausing.status().contains("segment"));
        assert_ne!(RunState::Stopping.status(), RunState::Pausing.status(),
                   "the two waits are for different things");
    }

    /// The end-of-run states say what is TRUE now, which is the part the old
    /// single log line never covered.
    #[test]
    fn the_end_states_say_what_you_are_left_with() {
        assert!(RunState::Stopped.status().contains("playable"));
        assert!(RunState::Paused.status().contains("continue"));
    }

    /// Stop's log names its own limitation and points at the alternative,
    /// because the moment a person wants to continue is after they stopped.
    #[test]
    fn stopping_says_it_cannot_be_continued_and_names_pause() {
        let l = EndRequest::Stop.log();
        assert!(l.contains("cannot be continued"), "{l}");
        assert!(l.contains("Pause"), "and points at what can: {l}");
        assert!(EndRequest::Pause.log().contains("continued"));
    }

    #[test]
    fn each_request_moves_to_its_own_transient_state() {
        assert_eq!(EndRequest::Stop.next_state(), RunState::Stopping);
        assert_eq!(EndRequest::Pause.next_state(), RunState::Pausing);
    }
}
