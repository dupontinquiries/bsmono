//! reaction.rs — steer or stop an in-flight generation from live speech.
//!
//! While the model is generating, the speaker stack keeps transcribing. This module classifies those
//! fragments into a [`Reaction`] — `Interrupt` (stop), `Steer` (stop and redirect), `AddContext` (let it
//! run, queue an addendum), `Affirm` (keep going), or `Ignore` (backchannel/noise) — and debounces them
//! through a [`Reactor`] so a jittery whisper partial can't fire a false cancel. When the Reactor commits
//! a cancelling reaction it raises a [`CancelFlag`]; the `cancel: &dyn Fn() -> bool` closure already on
//! `backend::Backend::generate` reads that flag, so generation stops mid-token.
//!
//! `classify` and `Reactor` are pure and unit-tested. The seams are upstream (the same whisper + speaker
//! attribution that feeds `voice_ingest`) and the one-line `cancel` closure handed to `generate`. Feed the
//! Reactor only the *driving* speaker's fragments (gate on `speakers::is_allowed`) so a bystander can't
//! hijack the generation. Classification is heuristic — cues, not NLU.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// What the live speech is asking of the in-flight generation.
#[derive(Clone, Debug, PartialEq)]
pub enum Reaction {
    Ignore,
    Affirm,
    AddContext(String), // continue, but fold this in afterward
    Steer(String),      // stop and redirect to this
    Interrupt,          // stop
}

impl Reaction {
    fn kind(&self) -> u8 {
        match self { Reaction::Ignore => 0, Reaction::Affirm => 1, Reaction::AddContext(_) => 2,
                     Reaction::Steer(_) => 3, Reaction::Interrupt => 4 }
    }
    /// Does this reaction stop the current generation?
    pub fn is_cancel(&self) -> bool { matches!(self, Reaction::Interrupt | Reaction::Steer(_)) }
}

/// Classify one transcript fragment. Order of precedence: interrupt → steer → add-context → affirm →
/// ignore (so "stop" cancels, but "no, use postgres" redirects rather than merely stopping).
pub fn classify(fragment: &str) -> Reaction {
    let f = fragment.trim().to_lowercase();
    if f.is_empty() { return Reaction::Ignore; }

    if is_interrupt(&f) { return Reaction::Interrupt; }

    // strip leading discourse fillers ("wait, …", "no …", "okay …") to find the real intent, remembering
    // whether a negation was among them (negation + restated content = a steer).
    let (core, negated) = strip_leading_fillers(&f);
    if core.is_empty() {
        return if is_affirm(&f) { Reaction::Affirm } else if negated { Reaction::Interrupt } else { Reaction::Ignore };
    }
    if negated && is_backchannel(&core) { return Reaction::Ignore; } // "no problem", "no worries"

    if let Some(t) = match_prefix(&core, STEER_PREFIXES) { if !t.is_empty() { return Reaction::Steer(t); } }
    if negated { return Reaction::Steer(core); } // "no — use the other dataset"
    if let Some(t) = match_prefix(&core, ADD_PREFIXES) { if !t.is_empty() { return Reaction::AddContext(t); } }
    if is_affirm(&core) { return Reaction::Affirm; }
    Reaction::Ignore
}

const STEER_PREFIXES: &[&str] = &["actually", "instead", "rather", "i meant", "make it", "change it to",
                                  "change that to", "switch to", "let's do", "do it as"];
const ADD_PREFIXES: &[&str] = &["also", "and also", "plus", "make sure", "don't forget", "dont forget",
                                "oh and", "and make", "include", "add", "as well as"];

fn is_interrupt(f: &str) -> bool {
    matches!(f, "no" | "no no" | "nope" | "stop" | "cancel" | "wait" | "halt" | "abort" | "no stop")
        || f.starts_with("never mind") || f.starts_with("nevermind")
        || f.starts_with("scratch that") || f.starts_with("hold on") || f.starts_with("stop stop")
}

fn strip_leading_fillers(f: &str) -> (String, bool) {
    const FILLERS: &[&str] = &["no", "nope", "not that", "wait", "okay", "ok", "um", "uh", "so",
                               "well", "hey", "yeah", "like", "hmm", "i mean"];
    let mut s = f.trim();
    let mut negated = false;
    loop {
        let mut advanced = false;
        for w in FILLERS {
            if s == *w || s.starts_with(&format!("{w} ")) || s.starts_with(&format!("{w},")) {
                if matches!(*w, "no" | "nope" | "not that") { negated = true; }
                s = s[w.len()..].trim_start_matches([',', ' ']);
                advanced = true;
                break;
            }
        }
        if !advanced { return (s.to_string(), negated); }
    }
}

fn match_prefix(core: &str, prefixes: &[&str]) -> Option<String> {
    for p in prefixes {
        if core == *p { return Some(String::new()); }
        if let Some(rest) = core.strip_prefix(&format!("{p} ")) { return Some(rest.trim().to_string()); }
    }
    None
}

fn is_affirm(f: &str) -> bool {
    const AFFIRM: &[&str] = &["yes", "yeah", "yep", "good", "perfect", "great", "exactly", "right",
                              "correct", "continue", "keep going", "go on", "sounds good", "looks good",
                              "nice", "cool", "okay", "ok"];
    AFFIRM.iter().any(|a| f == *a || f.starts_with(&format!("{a} ")) || f.starts_with(&format!("{a},")))
}

fn is_backchannel(f: &str) -> bool {
    const BC: &[&str] = &["problem", "worries", "thanks", "thank you", "idea", "big deal", "biggie"];
    BC.iter().any(|b| f == *b || f.starts_with(&format!("{b} ")))
}

/// Debounces classified fragments into one committed reaction. A `stable` (endpoint) fragment commits
/// immediately; volatile partials must agree `threshold` times in a row first. Only active between
/// [`begin`](Reactor::begin) and [`end`](Reactor::end) (i.e. while a generation is in flight).
pub struct Reactor {
    armed: bool,
    threshold: u32,
    pending_kind: u8,
    pending_count: u32,
    committed: Option<Reaction>,
    flag: CancelFlag,
}

impl Reactor {
    pub fn new(threshold: u32) -> Self {
        Self { armed: false, threshold: threshold.max(1), pending_kind: 0, pending_count: 0,
               committed: None, flag: CancelFlag::new() }
    }
    /// A shareable cancel flag to hand to `Backend::generate` (`let f = reactor.cancel_flag(); … cancel:
    /// &|| f.is_cancelled()`). Raised automatically when a cancelling reaction commits.
    pub fn cancel_flag(&self) -> CancelFlag { self.flag.clone() }

    /// Generation started — arm and clear prior state.
    pub fn begin(&mut self) { self.armed = true; self.reset(); }
    /// Generation finished/aborted — disarm and clear (also lowers the cancel flag for the next run).
    pub fn end(&mut self) { self.armed = false; self.reset(); }
    fn reset(&mut self) { self.pending_kind = 0; self.pending_count = 0; self.committed = None; self.flag.lower(); }

    /// Observe a fragment. Returns the reaction if this observation *commits* one.
    pub fn observe(&mut self, fragment: &str, stable: bool) -> Option<Reaction> {
        if !self.armed || self.committed.is_some() { return None; }
        let r = classify(fragment);
        if matches!(r, Reaction::Ignore) { return None; } // backchannel doesn't disturb the pending tally
        if stable { return self.commit(r); }
        if r.kind() == self.pending_kind { self.pending_count += 1; }
        else { self.pending_kind = r.kind(); self.pending_count = 1; }
        if self.pending_count >= self.threshold { self.commit(r) } else { None }
    }
    fn commit(&mut self, r: Reaction) -> Option<Reaction> {
        if r.is_cancel() { self.flag.cancel(); }
        self.committed = Some(r.clone());
        Some(r)
    }

    pub fn committed(&self) -> Option<&Reaction> { self.committed.as_ref() }
    /// True once a cancelling reaction has committed (mirrors the raised cancel flag).
    pub fn should_cancel(&self) -> bool { self.committed.as_ref().is_some_and(|r| r.is_cancel()) }
    /// Consume the committed reaction (re-opens observation for the rest of the turn).
    pub fn take(&mut self) -> Option<Reaction> {
        self.pending_kind = 0; self.pending_count = 0;
        self.committed.take()
    }
}

/// A shareable, thread-safe cancel flag. The `cancel` closure on `Backend::generate` reads
/// [`is_cancelled`](CancelFlag::is_cancelled); the voice side raises it via the [`Reactor`].
#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);
impl CancelFlag {
    pub fn new() -> Self { Self::default() }
    pub fn cancel(&self) { self.0.store(true, Ordering::SeqCst); }
    pub fn lower(&self) { self.0.store(false, Ordering::SeqCst); }
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::SeqCst) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_buckets_common_cues() {
        assert_eq!(classify("stop"), Reaction::Interrupt);
        assert_eq!(classify("no"), Reaction::Interrupt);
        assert_eq!(classify("never mind that"), Reaction::Interrupt);
        assert_eq!(classify("actually use postgres instead"), Reaction::Steer("use postgres instead".into()));
        assert_eq!(classify("no, use the other dataset"), Reaction::Steer("use the other dataset".into()));
        assert_eq!(classify("wait, make it async"), Reaction::Steer("async".into()));
        assert_eq!(classify("also include unit tests"), Reaction::AddContext("include unit tests".into()));
        assert_eq!(classify("make sure it handles empty input"), Reaction::AddContext("it handles empty input".into()));
        assert_eq!(classify("yeah perfect"), Reaction::Affirm);
        assert_eq!(classify("keep going"), Reaction::Affirm);
        assert_eq!(classify("no problem"), Reaction::Ignore);     // backchannel, not a steer
        assert_eq!(classify("the weather is nice today"), Reaction::Ignore);
    }

    #[test]
    fn steer_and_interrupt_cancel_addcontext_and_affirm_do_not() {
        assert!(classify("stop").is_cancel());
        assert!(classify("no, switch to the cached path").is_cancel());
        assert!(!classify("also add logging").is_cancel());
        assert!(!classify("yes good").is_cancel());
    }

    #[test]
    fn reactor_debounces_volatile_partials() {
        let mut r = Reactor::new(2);
        r.begin();
        assert_eq!(r.observe("stop", false), None, "one jittery partial isn't enough");
        assert_eq!(r.observe("stop", false), Some(Reaction::Interrupt), "two in a row commits");
        assert!(r.should_cancel());
    }

    #[test]
    fn reactor_commits_immediately_on_stable_endpoint() {
        let mut r = Reactor::new(3);
        r.begin();
        assert_eq!(r.observe("no, use the parquet files", true), Some(Reaction::Steer("use the parquet files".into())));
        assert!(r.should_cancel());
    }

    #[test]
    fn reactor_inert_until_armed_and_after_end() {
        let mut r = Reactor::new(1);
        assert_eq!(r.observe("stop", true), None, "not armed → ignored");
        r.begin();
        assert_eq!(r.observe("stop", true), Some(Reaction::Interrupt));
        r.end();
        assert!(!r.should_cancel(), "end() clears the decision and lowers the flag");
    }

    #[test]
    fn cancel_flag_drives_the_generate_hook() {
        let mut r = Reactor::new(1);
        let flag = r.cancel_flag();
        let cancel = || flag.is_cancelled(); // this is what Backend::generate polls
        assert!(!cancel());
        r.begin();
        r.observe("stop", true);
        assert!(cancel(), "voice interrupt raises the flag generate() is watching");
        r.end();
        assert!(!cancel(), "next generation starts clean");
    }

    #[test]
    fn addcontext_commits_without_cancelling() {
        let mut r = Reactor::new(1);
        r.begin();
        let got = r.observe("also make sure to handle timezones", true);
        assert_eq!(got, Some(Reaction::AddContext("make sure to handle timezones".into())));
        assert!(!r.should_cancel(), "additive context lets generation continue");
        assert!(!r.cancel_flag().is_cancelled());
    }
}
