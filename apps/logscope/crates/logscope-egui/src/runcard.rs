//! runcard — the ACTIVE THREAD card: what the agent is doing, at a glance.
//!
//! The reference panels pin a small card to the nav rail carrying the current task and its age —
//! "Investigate anomalous latency in payment pipeline / 12m ago" — so the thing being worked on is
//! visible from anywhere in the interface. (Those references are generated design studies, not
//! captures of this app; they say what the interface should feel like and nothing about how it
//! behaves.)
//!
//! Ours had no equivalent. A run's state was legible only in the status line, which is a single
//! sentence that the next event overwrites, and in the HUD of whichever panel happened to be open.
//! For the mode this app is actually used in — an unattended run left going for an hour — "what is
//! it doing and how long has it been doing it" is the question, and it had no fixed place to be
//! answered.
//!
//! ## Rows, not a sentence
//!
//! The formatting is a list of label/value pairs rather than prose because the values change at
//! different rates: the phase every few turns, the elapsed time every second, the plan progress
//! rarely. A sentence rebuilt every frame is unreadable while it churns; a table lets the eye rest
//! on the row it cares about.
//!
//! Pure: `RunState` is plain data and `rows` is a function of it, so every case — no plan, blocked,
//! nothing verified, a task too long for the card — is a test rather than a run to sit through.

/// What the card needs to know. Filled from `AgentRun`; plain data so the formatting is testable
/// without starting anything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunState {
    pub task: String,
    /// The phase machine's position, when the run has one.
    pub phase: Option<&'static str>,
    pub turns: usize,
    pub max_turns: usize,
    /// Turns dropped by transcript compaction. Surfaced because "the model forgot" deserves an
    /// answer other than a guess.
    pub compacted: usize,
    pub plan_done: usize,
    pub plan_total: usize,
    pub verified: bool,
    pub blocked: bool,
    pub elapsed_secs: u64,
}

/// Label/value rows for the card, in reading order.
///
/// Order is by how often a reader needs it, not by how often it changes: the task first because it
/// is what the card is for, then how far along, then the cost.
pub fn rows(s: &RunState) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    out.push(("task", elide_task(&s.task, 64)));

    // STATE BEFORE PROGRESS. A blocked run that has done nine of ten steps is blocked, and a reader
    // who sees "9/10" first has been told the opposite of what matters.
    let state = if s.blocked { "blocked".to_string() }
        else if s.verified { "verified".to_string() }
        else { s.phase.unwrap_or("working").to_string() };
    out.push(("state", state));

    if s.plan_total > 0 {
        out.push(("plan", format!("{}/{} steps", s.plan_done, s.plan_total)));
    }
    out.push(("turns", match s.max_turns {
        0 => format!("{}", s.turns),
        m => format!("{}/{m}", s.turns),
    }));
    if s.compacted > 0 {
        out.push(("elided", format!("{} turn(s) compacted", s.compacted)));
    }
    out.push(("running", logscope_fan::sessiontime::compact(
        std::time::Duration::from_secs(s.elapsed_secs))));
    out
}

/// What one fan branch is doing RIGHT NOW.
///
/// The reference's ACTIVITY panel gives every worker a line saying what it is up to — "Analyzing
/// logs", "Processing traces". Those are its worked example's words; the honest version here is the
/// branch's actual scheduler state, which the fan already knows and never showed. A branch was
/// legible only as `finished` plus a file count, so a fan mid-run looked the same whether a branch
/// was generating, queued behind the single worker, or already done.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Doing {
    /// Holding the inference worker, producing an agent turn.
    Generating,
    /// Holding the worker for its review pass over its own diff.
    Reviewing,
    /// Ready, waiting its turn. There is ONE local model, so "parallel" means round-robin and most
    /// branches are here most of the time — saying so is the point.
    Waiting,
    /// Agent loop over, review not started or not applicable.
    Finished,
}

impl Doing {
    pub fn label(self) -> &'static str {
        match self {
            Doing::Generating => "generating",
            Doing::Reviewing => "reviewing",
            Doing::Waiting => "waiting for the worker",
            Doing::Finished => "finished",
        }
    }
    /// Whether this branch currently holds the single inference worker. At most one can.
    pub fn holds_worker(self) -> bool { matches!(self, Doing::Generating | Doing::Reviewing) }
}

/// One row per branch: label, what it is doing, and what it has produced.
///
/// `(label, doing, detail)`. The detail is deliberately last and deliberately short — a review's
/// notes can run to paragraphs and do not belong in a status list.
pub fn branch_rows(rows: &[(String, Doing, usize, bool, Option<bool>)]) -> Vec<(String, String, String)> {
    rows.iter().map(|(label, doing, files, verified, approved)| {
        let mut detail = format!("{files} file(s)");
        if *verified { detail.push_str(" · verified"); }
        // A review verdict outranks the file count: a branch that touched nine files and was
        // rejected is rejected, and reading the nine first is reading it backwards.
        match approved {
            Some(true) => detail = format!("approved · {detail}"),
            Some(false) => detail = format!("revise · {detail}"),
            None => {}
        }
        (elide_task(label, 28), doing.label().to_string(), detail)
    }).collect()
}

/// A task line for a narrow card: one line, cut on a word.
///
/// Reuses `nodelabel::elide_words` rather than repeating the rule. Three functions named `elide`
/// already exist in this tree with three different behaviours, and a fourth would be the mistake
/// `dupscan` caught last time.
pub fn elide_task(task: &str, max: usize) -> String {
    let t = crate::nodelabel::elide_words(task, max);
    if t.is_empty() { "(no task)".to_string() } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(rows: &'a [(&'static str, String)], k: &str) -> Option<&'a str> {
        rows.iter().find(|(l, _)| *l == k).map(|(_, v)| v.as_str())
    }

    fn running() -> RunState {
        RunState { task: "wire the oracle into the loop".into(), phase: Some("patch"),
                   turns: 4, max_turns: 40, compacted: 0, plan_done: 1, plan_total: 3,
                   verified: false, blocked: false, elapsed_secs: 12 * 60 }
    }

    #[test]
    fn a_running_task_reports_what_it_is_doing() {
        let r = rows(&running());
        assert_eq!(get(&r, "task"), Some("wire the oracle into the loop"));
        assert_eq!(get(&r, "state"), Some("patch"));
        assert_eq!(get(&r, "plan"), Some("1/3 steps"));
        assert_eq!(get(&r, "turns"), Some("4/40"));
        assert_eq!(get(&r, "running"), Some("12m"));
        assert_eq!(get(&r, "elided"), None, "nothing compacted, nothing to say");
    }

    /// A blocked run that has done nine of ten steps is BLOCKED. A reader who sees the progress
    /// first has been told the opposite of what matters.
    #[test]
    fn blocked_outranks_progress() {
        let s = RunState { blocked: true, plan_done: 9, plan_total: 10, ..running() };
        let r = rows(&s);
        assert_eq!(get(&r, "state"), Some("blocked"));
        let state_at = r.iter().position(|(l, _)| *l == "state").unwrap();
        let plan_at = r.iter().position(|(l, _)| *l == "plan").unwrap();
        assert!(state_at < plan_at, "progress must not be read before the state");
    }

    #[test]
    fn a_verified_run_says_so_rather_than_naming_a_phase() {
        let s = RunState { verified: true, phase: Some("done"), ..running() };
        assert_eq!(get(&rows(&s), "state"), Some("verified"));
    }

    /// A run without the phase machine still has a state to report.
    #[test]
    fn no_phase_machine_still_reports_working() {
        let s = RunState { phase: None, ..running() };
        assert_eq!(get(&rows(&s), "state"), Some("working"));
    }

    #[test]
    fn a_run_with_no_plan_omits_the_row_rather_than_showing_zero_of_zero() {
        let s = RunState { plan_done: 0, plan_total: 0, ..running() };
        assert_eq!(get(&rows(&s), "plan"), None, "'0/0 steps' is worse than silence");
    }

    /// "The model forgot" deserves an answer other than a guess, and only when it happened.
    #[test]
    fn compaction_is_reported_only_when_it_occurred() {
        assert_eq!(get(&rows(&running()), "elided"), None);
        let s = RunState { compacted: 7, ..running() };
        assert_eq!(get(&rows(&s), "elided"), Some("7 turn(s) compacted"));
    }

    #[test]
    fn an_unbounded_run_shows_a_count_not_a_ratio_against_zero() {
        let s = RunState { max_turns: 0, turns: 9, ..running() };
        assert_eq!(get(&rows(&s), "turns"), Some("9"), "'9/0' is nonsense");
    }

    #[test]
    fn a_long_task_is_cut_on_a_word_and_an_empty_one_says_so() {
        let long = "investigate the anomalous latency in the payment pipeline and report back";
        let out = elide_task(long, 40);
        assert!(out.chars().count() <= 40, "{out:?}");
        assert!(long.starts_with(out.trim_end_matches('…')), "{out:?} is not a prefix");
        assert_eq!(elide_task("   ", 40), "(no task)");
        assert_eq!(elide_task("", 40), "(no task)");
    }

    #[test]
    fn a_branch_row_says_what_the_branch_is_doing() {
        let r = branch_rows(&[("planrun.rs #1".into(), Doing::Generating, 3, false, None)]);
        assert_eq!(r[0].0, "planrun.rs #1");
        assert_eq!(r[0].1, "generating");
        assert_eq!(r[0].2, "3 file(s)");
    }

    /// One local model means round-robin, so most branches are waiting most of the time. Saying
    /// "waiting for the worker" rather than leaving them blank is the honest description of a fan.
    #[test]
    fn waiting_is_a_state_worth_naming() {
        assert_eq!(Doing::Waiting.label(), "waiting for the worker");
        assert!(!Doing::Waiting.holds_worker() && !Doing::Finished.holds_worker());
        assert!(Doing::Generating.holds_worker() && Doing::Reviewing.holds_worker());
    }

    /// A branch that touched nine files and was rejected is rejected. Reading the nine first is
    /// reading it backwards.
    #[test]
    fn a_review_verdict_outranks_the_file_count() {
        let r = branch_rows(&[("a".into(), Doing::Finished, 9, true, Some(false))]);
        assert!(r[0].2.starts_with("revise"), "{}", r[0].2);
        let ok = branch_rows(&[("a".into(), Doing::Finished, 1, true, Some(true))]);
        assert!(ok[0].2.starts_with("approved"), "{}", ok[0].2);
        let none = branch_rows(&[("a".into(), Doing::Finished, 1, true, None)]);
        assert!(none[0].2.starts_with("1 file"), "unreviewed must not imply a verdict: {}", none[0].2);
    }

    #[test]
    fn a_long_branch_label_is_cut_and_no_field_is_blank() {
        let r = branch_rows(&[("a-very-long-branch-label-that-will-not-fit-in-the-column".into(),
                               Doing::Waiting, 0, false, None)]);
        assert!(r[0].0.chars().count() <= 28, "{:?}", r[0].0);
        for (i, f) in [&r[0].0, &r[0].1, &r[0].2].iter().enumerate() {
            assert!(!f.trim().is_empty(), "field {i} blank");
        }
    }

    /// Every row must carry a value; a card with a blank beside a label reads as broken rather than
    /// as empty.
    #[test]
    fn no_row_is_ever_blank() {
        for s in [running(), RunState::default(),
                  RunState { blocked: true, ..Default::default() },
                  RunState { compacted: 3, max_turns: 0, ..Default::default() }] {
            for (label, value) in rows(&s) {
                assert!(!value.trim().is_empty(), "row '{label}' is blank");
            }
        }
    }
}
