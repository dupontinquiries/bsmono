//! summary_queue — the scheduling policy from `log_summary.rs`.
//!
//! Plain-language summaries above raw command output, so a terminal is
//! *"friendly to non-power users"*. The LLM call belongs elsewhere; **when to
//! ask, when to wait, and when to give up** is the policy, and it is where all
//! the reasoning lives.
//!
//! ## Voice always wins ties, and the lock is a courtesy
//!
//! The C++ is unusually clear that this is not a correctness mechanism: voice
//! uses whisper in its own process space and summaries use ollama as a
//! subprocess, so *"they CAN technically run concurrently. The lock is a
//! COURTESY mechanism: on phone-class hardware, both running at once results in
//! slow whisper + slow ollama + bad UX. So we serialize."*
//!
//! That framing decides the shape. A courtesy lock must be **checked before
//! each attempt and never held across one**, because a summary worker that
//! blocked voice would invert exactly the priority it exists to protect.
//!
//! ## Debounce, then back off, then give up
//!
//! 500ms debounce *"so rapid-fire commands don't spam the LLM"*. Failures
//! retry on a doubling curve, capped at twelve attempts — and then the request
//! is **dropped**, not retried forever: a user with no ollama installed should
//! not accumulate an unbounded queue of work that can never succeed.
//!
//! ## A total budget across models, not per model
//!
//! Model selection walks a priority list, and the 45-second budget is measured
//! *"overall"* rather than per candidate. Per-model timeouts would let a list
//! of four slow models spend three minutes before reporting failure — long past
//! the point the output stopped being interesting.

/// How long to wait after a command completes before asking.
pub const DEBOUNCE_MS: u64 = 500;
/// Attempts before a request is dropped.
pub const MAX_ATTEMPTS: u32 = 12;
/// Total seconds allowed across every candidate model.
pub const TOTAL_BUDGET_SECS: u64 = 45;

/// Backoff for attempt `n`, in seconds.
///
/// Doubling from 2 through 32, then flat at 60 — the original's exact curve. I
/// first wrote a power-of-two cap at 64, which is close enough to look right
/// and is not what the app does; a retry schedule two apps disagree about is a
/// support call nobody can reproduce.
///
/// The cap matters as much as the doubling: a user who starts ollama an hour in
/// should see summaries resume without restarting, and unbounded doubling would
/// put the next attempt days out.
pub fn retry_backoff_secs(attempts: u32) -> u64 {
    match attempts {
        0 => 0,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        5 => 32,
        _ => 60,
    }
}

/// Why the worker is not running a request right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wait {
    /// Nothing queued.
    Idle,
    /// Debouncing, or backing off after a failure.
    NotYet,
    /// Voice is active — it wins.
    YieldToVoice,
}

/// What the worker should do.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    Run { block_id: u64 },
    Wait(Wait),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub block_id: u64,
    pub attempts: u32,
    /// Earliest time this may run.
    pub not_before_ms: u64,
}

impl Request {
    pub fn new(block_id: u64, now_ms: u64) -> Request {
        Request { block_id, attempts: 0, not_before_ms: now_ms + DEBOUNCE_MS }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SummaryQueue {
    queue: Vec<Request>,
}

impl SummaryQueue {
    pub fn len(&self) -> usize { self.queue.len() }
    pub fn is_empty(&self) -> bool { self.queue.is_empty() }

    /// Queue a block, or restart its debounce if already queued.
    ///
    /// Restart rather than duplicate: a command re-run before its summary
    /// arrives should produce one summary of the newer output, not two of
    /// which the first describes output already scrolled away.
    pub fn request(&mut self, block_id: u64, now_ms: u64) {
        if let Some(r) = self.queue.iter_mut().find(|r| r.block_id == block_id) {
            r.not_before_ms = now_ms + DEBOUNCE_MS;
            return;
        }
        self.queue.push(Request::new(block_id, now_ms));
    }

    /// What to do now.
    ///
    /// `voice_active` is checked here, before picking work, rather than being
    /// held across a run — a summary worker that blocked voice would invert the
    /// priority the courtesy lock exists to protect.
    pub fn next_action(&self, now_ms: u64, voice_active: bool) -> Action {
        if self.queue.is_empty() { return Action::Wait(Wait::Idle); }
        if voice_active { return Action::Wait(Wait::YieldToVoice); }
        match self.queue.iter().find(|r| now_ms >= r.not_before_ms) {
            Some(r) => Action::Run { block_id: r.block_id },
            None => Action::Wait(Wait::NotYet),
        }
    }

    /// A request succeeded.
    pub fn complete(&mut self, block_id: u64) {
        self.queue.retain(|r| r.block_id != block_id);
    }

    /// A request failed. Returns false when it was dropped for good.
    ///
    /// Dropping matters: a user with no ollama installed must not accumulate an
    /// unbounded queue of work that can never succeed.
    pub fn fail(&mut self, block_id: u64, now_ms: u64) -> bool {
        let Some(r) = self.queue.iter_mut().find(|r| r.block_id == block_id) else {
            return false;
        };
        r.attempts += 1;
        if r.attempts >= MAX_ATTEMPTS {
            self.queue.retain(|x| x.block_id != block_id);
            return false;
        }
        r.not_before_ms = now_ms + retry_backoff_secs(r.attempts) * 1_000;
        true
    }
}

/// Should model selection stop trying candidates?
///
/// The budget is measured across ALL models rather than per model: four slow
/// candidates at fifteen seconds each would otherwise spend a minute before
/// reporting failure, long past the point the output stopped being interesting.
pub fn budget_exhausted(elapsed_secs: u64) -> bool { elapsed_secs >= TOTAL_BUDGET_SECS }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_waits_out_the_debounce() {
        // "So rapid-fire commands don't spam the LLM."
        let mut q = SummaryQueue::default();
        q.request(1, 0);
        assert_eq!(q.next_action(DEBOUNCE_MS - 1, false), Action::Wait(Wait::NotYet));
        assert_eq!(q.next_action(DEBOUNCE_MS, false), Action::Run { block_id: 1 });
    }

    #[test]
    fn re_running_a_command_restarts_its_debounce_rather_than_queueing_twice() {
        // One summary of the newer output, not two of which the first describes
        // output already scrolled away.
        let mut q = SummaryQueue::default();
        q.request(1, 0);
        q.request(1, 400);
        assert_eq!(q.len(), 1);
        assert_eq!(q.next_action(DEBOUNCE_MS, false), Action::Wait(Wait::NotYet));
        assert_eq!(q.next_action(400 + DEBOUNCE_MS, false), Action::Run { block_id: 1 });
    }

    #[test]
    fn voice_always_wins_ties() {
        // The stated rule. Both can technically run at once; on phone-class
        // hardware that is "slow whisper + slow ollama + bad UX".
        let mut q = SummaryQueue::default();
        q.request(1, 0);
        assert_eq!(q.next_action(DEBOUNCE_MS, true), Action::Wait(Wait::YieldToVoice));
        assert_eq!(q.next_action(DEBOUNCE_MS, false), Action::Run { block_id: 1 });
    }

    #[test]
    fn voice_is_checked_before_work_is_picked_not_held_across_it() {
        // A worker that held the lock across a run would block voice — the
        // exact inversion the courtesy mechanism exists to prevent. The check
        // is a pure function of the current state, so it cannot be stale.
        let mut q = SummaryQueue::default();
        q.request(1, 0);
        assert_eq!(q.next_action(DEBOUNCE_MS, false), Action::Run { block_id: 1 });
        assert_eq!(q.next_action(DEBOUNCE_MS, true), Action::Wait(Wait::YieldToVoice),
                   "voice starting mid-wait is respected immediately");
    }

    #[test]
    fn an_empty_queue_is_idle_rather_than_waiting_on_voice() {
        // The distinction matters to a status line: "nothing to do" and
        // "deferring to voice" are different states.
        let q = SummaryQueue::default();
        assert_eq!(q.next_action(0, true), Action::Wait(Wait::Idle));
    }

    #[test]
    fn failures_back_off_on_a_doubling_curve() {
        assert_eq!(retry_backoff_secs(0), 0);
        assert_eq!(retry_backoff_secs(1), 2);
        assert_eq!(retry_backoff_secs(2), 4);
        assert_eq!(retry_backoff_secs(3), 8);
        assert_eq!(retry_backoff_secs(5), 32);
    }

    #[test]
    fn the_backoff_caps_so_a_late_start_still_recovers() {
        // A user who starts ollama an hour in should see summaries resume
        // without restarting the app — unbounded doubling would put the next
        // attempt days out.
        assert_eq!(retry_backoff_secs(6), 60, "flat at 60, matching the original");
        assert_eq!(retry_backoff_secs(100), 60);
    }

    #[test]
    fn a_request_is_dropped_after_the_attempt_cap() {
        // A user with no ollama installed must not accumulate an unbounded
        // queue of work that can never succeed.
        let mut q = SummaryQueue::default();
        q.request(1, 0);
        let mut t = DEBOUNCE_MS;
        for _ in 0..(MAX_ATTEMPTS - 1) {
            assert!(q.fail(1, t), "still retrying");
            t += 100_000;
        }
        assert!(!q.fail(1, t), "dropped at the cap");
        assert!(q.is_empty());
    }

    #[test]
    fn a_failed_request_waits_before_retrying() {
        let mut q = SummaryQueue::default();
        q.request(1, 0);
        q.fail(1, DEBOUNCE_MS);
        assert_eq!(q.next_action(DEBOUNCE_MS + 1_000, false), Action::Wait(Wait::NotYet));
        assert_eq!(q.next_action(DEBOUNCE_MS + 2_000, false), Action::Run { block_id: 1 });
    }

    #[test]
    fn completing_a_request_removes_it() {
        let mut q = SummaryQueue::default();
        q.request(1, 0);
        q.complete(1);
        assert!(q.is_empty());
        assert_eq!(q.next_action(DEBOUNCE_MS, false), Action::Wait(Wait::Idle));
    }

    #[test]
    fn failing_an_unknown_block_is_harmless() {
        // A late failure for a block already completed or dropped.
        let mut q = SummaryQueue::default();
        assert!(!q.fail(99, 0));
    }

    #[test]
    fn a_blocked_request_does_not_stall_a_ready_one_behind_it() {
        // Otherwise one failing command freezes summaries for everything after
        // it, and the feature appears broken rather than degraded.
        let mut q = SummaryQueue::default();
        q.request(1, 0);
        q.fail(1, DEBOUNCE_MS);
        q.request(2, DEBOUNCE_MS);
        assert_eq!(q.next_action(DEBOUNCE_MS * 2 + 100, false), Action::Run { block_id: 2 });
    }

    #[test]
    fn the_budget_is_total_rather_than_per_model() {
        // Four slow candidates at fifteen seconds each would otherwise spend a
        // minute before reporting failure — long past the point the output
        // stopped being interesting.
        assert!(!budget_exhausted(TOTAL_BUDGET_SECS - 1));
        assert!(budget_exhausted(TOTAL_BUDGET_SECS));
    }
}
