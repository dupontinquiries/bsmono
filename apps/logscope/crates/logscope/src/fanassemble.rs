//! fanassemble.rs — split one answer into per-function units, generate them in parallel, assemble live.
//!
//! A big model writing a 400-line file emits it serially: measured at 4.88 tok/s, that is 75 seconds of
//! watching a cursor. The same file is a dozen mostly-independent functions, and a dozen small models can
//! write them at once. The wall time then approaches the SLOWEST unit rather than the sum.
//!
//! ## What makes the parallelism sound
//!
//! Units can be written independently only if each knows the others' SIGNATURES without needing their
//! bodies. So the plan carries signatures, and a unit's prompt contains every signature it may call. That
//! is the whole trick: an interface is small and cheap to agree on up front, and a body is large and
//! expensive — so agree on interfaces serially, then write bodies in parallel.
//!
//! ## What the assembly must never do
//!
//! **Show a partial file as if it were whole.** Pieces land out of order, and a reader who scrolls past a
//! silently-absent function will believe the file is complete and wrong rather than incomplete. Every
//! unit therefore occupies its slot from the beginning — as a placeholder that says what it is waiting
//! for — and the assembly reports `is_complete()` separately from having text to show.

use std::collections::BTreeMap;

/// One independently-generatable piece.
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    /// Stable id, used to slot a result back into place regardless of arrival order.
    pub id: String,
    /// Destination file.
    pub path: String,
    /// Position within the file. Assembly orders by this, NOT by completion time.
    pub order: usize,
    /// The signature other units may rely on. Agreed before any body is written.
    pub signature: String,
    /// One line on what it should do.
    pub intent: String,
}

/// State of one unit.
#[derive(Clone, Debug, PartialEq)]
pub enum UnitState {
    Pending,
    Running,
    /// Body produced. `by` records which model, because provenance matters when reviewing a mixed file.
    Done { body: String, by: String },
    /// Generation or validation failed. Kept distinct from Pending: "not yet" and "will not" call for
    /// different actions, and collapsing them makes a stuck run look busy.
    Failed { why: String },
}

#[derive(Clone, Debug, Default)]
pub struct Assembly {
    units: Vec<Unit>,
    state: BTreeMap<String, UnitState>,
}

impl Assembly {
    pub fn new(units: Vec<Unit>) -> Self {
        let state = units.iter().map(|u| (u.id.clone(), UnitState::Pending)).collect();
        Assembly { units, state }
    }

    pub fn start(&mut self, id: &str) { self.set(id, UnitState::Running); }
    pub fn complete(&mut self, id: &str, body: impl Into<String>, by: impl Into<String>) {
        self.set(id, UnitState::Done { body: body.into(), by: by.into() });
    }
    pub fn fail(&mut self, id: &str, why: impl Into<String>) {
        self.set(id, UnitState::Failed { why: why.into() });
    }
    fn set(&mut self, id: &str, s: UnitState) {
        if let Some(slot) = self.state.get_mut(id) { *slot = s; }
    }

    pub fn state_of(&self, id: &str) -> Option<&UnitState> { self.state.get(id) }
    pub fn units(&self) -> &[Unit] { &self.units }

    /// Every unit finished successfully.
    pub fn is_complete(&self) -> bool {
        self.units.iter().all(|u| matches!(self.state.get(&u.id), Some(UnitState::Done { .. })))
    }

    /// (done, failed, total) — for a progress line that distinguishes stuck from slow.
    pub fn progress(&self) -> (usize, usize, usize) {
        let mut done = 0; let mut failed = 0;
        for u in &self.units {
            match self.state.get(&u.id) {
                Some(UnitState::Done { .. }) => done += 1,
                Some(UnitState::Failed { .. }) => failed += 1,
                _ => {}
            }
        }
        (done, failed, self.units.len())
    }

    /// The units still worth dispatching, in file order.
    pub fn pending(&self) -> Vec<&Unit> {
        self.units.iter().filter(|u| matches!(self.state.get(&u.id), Some(UnitState::Pending))).collect()
    }

    /// The file as it stands, in DECLARED order with placeholders for anything not yet done.
    ///
    /// Ordered by `Unit::order`, never by arrival: a unit that finished first must not jump to the top,
    /// or the reader watches the document reshuffle itself and cannot follow it.
    pub fn render(&self, path: &str) -> String {
        let mut units: Vec<&Unit> = self.units.iter().filter(|u| u.path == path).collect();
        units.sort_by_key(|u| u.order);
        let mut out = String::new();
        for u in units {
            match self.state.get(&u.id) {
                Some(UnitState::Done { body, .. }) => { out.push_str(body.trim_end()); out.push_str("\n\n"); }
                Some(UnitState::Failed { why }) => {
                    // A failure is louder than a placeholder and says what to do, because a silent gap
                    // reads as "nothing was needed here".
                    out.push_str(&format!("// FAILED: {} — {why}\n{} {{ todo!() }}\n\n", u.id, u.signature));
                }
                _ => {
                    out.push_str(&format!("// pending: {}\n{} {{ todo!() }}\n\n", u.intent, u.signature));
                }
            }
        }
        out.trim_end().to_string()
    }

    pub fn paths(&self) -> Vec<String> {
        let mut p: Vec<String> = self.units.iter().map(|u| u.path.clone()).collect();
        p.sort(); p.dedup(); p
    }
}

/// The prompt for one unit.
///
/// Carries every OTHER unit's signature so this one can call them without waiting for their bodies —
/// which is what allows the units to run at the same time. It also states the boundary explicitly,
/// because a model given context tends to helpfully write the neighbours too, and duplicate definitions
/// are worse than missing ones.
pub fn unit_prompt(u: &Unit, all: &[Unit]) -> String {
    let mut s = String::new();
    let others: Vec<&Unit> = all.iter().filter(|o| o.id != u.id).collect();
    if !others.is_empty() {
        s.push_str("These already exist and you may call them. Do NOT write them:\n");
        for o in others { s.push_str(&format!("    {}   // {}\n", o.signature, o.intent)); }
        s.push('\n');
    }
    s.push_str(&format!("Write EXACTLY ONE item, in {}:\n\n    {}\n\nIt should: {}\n\n", u.path, u.signature, u.intent));
    s.push_str("Reply with a single fenced code block containing only that item — no prose before or \
after, no other functions, no imports unless the item cannot compile without them. If you cannot write \
it, reply with the block containing only `todo!()` and a comment saying why.");
    s
}

/// Asked of the big model ONCE, before any body is written.
///
/// This is the only serial step, and it is deliberately cheap: signatures and one-line intents, not
/// implementations. Getting interfaces right is the part that needs the strongest model and the whole
/// picture; filling them in is the part that parallelises.
pub const PLAN_PROMPT: &str = "\
Before writing any code, decompose the task into INDEPENDENT units.

Emit one line per unit, in this exact form, and nothing else:

    UNIT <id> | <file path> | <order> | <signature> | <one-line intent>

Rules:
- <order> is the position within its file, counting from 0.
- <signature> must be complete enough that another unit could CALL this one without seeing its body — that is what lets them be written at the same time.
- A unit should be one function, one type, or one impl block. If two units cannot be written without seeing each other's bodies, they are one unit; merge them.
- Put types before the functions that use them.
Emit no code in this step.";

/// Parse the planner's reply into units. Malformed lines are skipped rather than guessed at.
///
/// Skipping is deliberate: a half-understood unit line would produce a unit with a wrong signature, and
/// every body written against it would then be wrong too. A missing unit is visible as a gap; a corrupt
/// one is not.
pub fn parse_plan(reply: &str) -> Vec<Unit> {
    let mut out = Vec::new();
    for line in reply.lines() {
        let l = line.trim();
        let Some(rest) = l.strip_prefix("UNIT ") else { continue };
        let f: Vec<&str> = rest.split('|').map(str::trim).collect();
        if f.len() < 5 { continue; }
        let Ok(order) = f[2].parse::<usize>() else { continue };
        if f[0].is_empty() || f[1].is_empty() || f[3].is_empty() { continue; }
        out.push(Unit { id: f[0].into(), path: f[1].into(), order,
                        signature: f[3].into(), intent: f[4..].join(" | ") });
    }
    // Duplicate ids would collide in the state map and silently overwrite each other.
    let mut seen = std::collections::HashSet::new();
    out.retain(|u| seen.insert(u.id.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units() -> Vec<Unit> {
        vec![
            Unit { id: "parse".into(), path: "src/log.rs".into(), order: 1,
                   signature: "pub fn parse(line: &[u8]) -> Option<Entry>".into(),
                   intent: "split a log line into fields".into() },
            Unit { id: "entry".into(), path: "src/log.rs".into(), order: 0,
                   signature: "pub struct Entry".into(), intent: "the parsed record".into() },
            Unit { id: "scan".into(), path: "src/scan.rs".into(), order: 0,
                   signature: "pub fn scan(buf: &[u8]) -> usize".into(),
                   intent: "count entries in a buffer".into() },
        ]
    }

    #[test]
    fn arrival_order_does_not_reorder_the_document() {
        // The unit that finishes first must not jump to the top, or the reader watches the file
        // reshuffle itself while they are trying to read it.
        let mut a = Assembly::new(units());
        a.complete("parse", "pub fn parse(line: &[u8]) -> Option<Entry> { None }", "small-a");
        a.complete("entry", "pub struct Entry { pub ts: u64 }", "small-b");
        let r = a.render("src/log.rs");
        assert!(r.find("struct Entry") < r.find("fn parse"), "declared order wins:\n{r}");
    }

    #[test]
    fn a_partial_file_is_never_shown_as_whole() {
        // A silently-absent function reads as "nothing was needed here", and the reader believes the
        // file is complete and wrong rather than incomplete.
        let mut a = Assembly::new(units());
        a.complete("entry", "pub struct Entry {}", "small-a");
        let r = a.render("src/log.rs");
        assert!(r.contains("// pending: split a log line into fields"));
        assert!(r.contains("todo!()"), "the placeholder must not compile silently into nothing");
        assert!(!a.is_complete());
        assert_eq!(a.progress(), (1, 0, 3));
    }

    #[test]
    fn failed_is_distinguishable_from_pending() {
        // "not yet" and "will not" call for different actions; collapsing them makes a stuck run look
        // busy forever.
        let mut a = Assembly::new(units());
        a.fail("parse", "3 compile errors after 2 repair rounds");
        let r = a.render("src/log.rs");
        assert!(r.contains("// FAILED: parse — 3 compile errors"));
        assert!(!r.contains("// pending: split a log line"));
        assert_eq!(a.progress(), (0, 1, 3));
        assert!(!a.is_complete(), "a failed unit must never count as done");
        assert!(!a.pending().iter().any(|u| u.id == "parse"), "and must not be redispatched as pending");
    }

    #[test]
    fn a_unit_prompt_carries_the_others_signatures_but_forbids_writing_them() {
        // Signatures are what make the units independent: a body can call a neighbour it has never seen.
        // But a model handed context will helpfully write the neighbours too, and duplicate definitions
        // are worse than missing ones.
        let u = units();
        let p = unit_prompt(&u[0], &u);
        assert!(p.contains("pub struct Entry"), "the neighbour's signature is available");
        assert!(p.contains("Do NOT write them"));
        assert!(p.contains("EXACTLY ONE item"));
        assert!(!p.contains("// split a log line into fields\n    pub fn parse"), "not listed as its own neighbour");
        assert!(p.contains("todo!()"), "a unit that cannot be written must say so, not invent");
    }

    #[test]
    fn completion_records_which_model_wrote_it() {
        // Provenance matters when reviewing a file written by several models at once.
        let mut a = Assembly::new(units());
        a.complete("scan", "pub fn scan(buf: &[u8]) -> usize { 0 }", "qwen-1b");
        match a.state_of("scan") {
            Some(UnitState::Done { by, .. }) => assert_eq!(by, "qwen-1b"),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn paths_are_separated_so_one_file_does_not_leak_into_another() {
        let mut a = Assembly::new(units());
        a.complete("scan", "pub fn scan(buf: &[u8]) -> usize { 1 }", "m");
        assert_eq!(a.paths(), vec!["src/log.rs".to_string(), "src/scan.rs".to_string()]);
        assert!(!a.render("src/log.rs").contains("fn scan"));
        assert!(a.render("src/scan.rs").contains("fn scan"));
    }

    #[test]
    fn the_planner_reply_parses_and_bad_lines_are_skipped_not_guessed() {
        let reply = "Here is the plan:\n\
            UNIT entry | src/log.rs | 0 | pub struct Entry | the parsed record\n\
            UNIT parse | src/log.rs | 1 | pub fn parse(l: &[u8]) -> Option<Entry> | split a line\n\
            UNIT broken | src/log.rs | notanumber | pub fn x() | bad order\n\
            UNIT short | only three fields\n\
            some trailing prose\n";
        let u = parse_plan(reply);
        assert_eq!(u.len(), 2, "malformed lines are skipped: {u:?}");
        assert_eq!(u[0].id, "entry");
        assert_eq!(u[1].order, 1);
        assert_eq!(u[1].intent, "split a line");
    }

    #[test]
    fn duplicate_unit_ids_are_dropped_not_merged() {
        // Two units with one id would collide in the state map and silently overwrite each other, so the
        // second body would replace the first with no sign anything was lost.
        let u = parse_plan("UNIT a | f.rs | 0 | fn a() | first\nUNIT a | f.rs | 1 | fn a2() | second\n");
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].signature, "fn a()", "the first wins; the duplicate is discarded");
    }

    #[test]
    fn the_plan_prompt_demands_callable_signatures_and_no_code() {
        assert!(PLAN_PROMPT.contains("without seeing its body"),
            "the reason must be stated or a model will emit stubs instead of real signatures");
        assert!(PLAN_PROMPT.contains("Emit no code in this step"));
        assert!(PLAN_PROMPT.contains("merge them"), "inseparable units must not be split");
    }

    #[test]
    fn everything_done_is_complete_and_placeholder_free() {
        let mut a = Assembly::new(units());
        for (id, body) in [("entry", "pub struct Entry {}"), ("parse", "pub fn parse() {}"),
                           ("scan", "pub fn scan() {}")] {
            a.complete(id, body, "m");
        }
        assert!(a.is_complete());
        assert_eq!(a.progress(), (3, 0, 3));
        assert!(!a.render("src/log.rs").contains("pending"));
        assert!(a.pending().is_empty());
    }
}
