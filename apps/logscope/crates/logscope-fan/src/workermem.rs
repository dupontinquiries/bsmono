//! workermem — a worker bee's notes, as a trust boundary rather than a log.
//!
//! C11-20c. `docs/12-queen-and-workers.md` specifies this in two sentences that
//! do most of the design work:
//!
//! > **Memory is model-authored text that re-enters a prompt.** That is the same
//! > trust boundary as a git branch name or a file the agent read, and it gets
//! > the same treatment: sanitized and fenced.
//!
//! > **Memory is append-only with provenance, and capped.** Each entry records
//! > whether it came from an observation, a decision, or an inference. Without
//! > that distinction a worker's guess becomes its established fact by turn 200.
//!
//! Pure: text in, text out. No I/O, no model calls, no clock.
//!
//! ## Why the fencer is injected rather than called
//!
//! The sanitiser the doc names — `read_harness::fence` — lives in
//! `logscope-egui`, which DEPENDS on this crate. Reaching for it here is
//! impossible, and copying it is the mistake this tree already made once:
//! `voiceprint-kit` is a straight copy of voiceterm's fingerprint code and now
//! needs a dedicated checker to notice when the two drift apart.
//!
//! So [`WorkerMemory::render`] takes the fencer as a parameter. Two things fall
//! out of that, both wanted: this crate keeps one implementation of the trust
//! boundary rather than a second copy, and the fencing becomes visible at the
//! call site instead of buried in a method someone can forget to use. There is
//! deliberately **no** `render_raw` — the only way to get these strings out in
//! prompt shape is through a fencer.
//!
//! ## Why the cap refuses instead of evicting
//!
//! The doc is explicit that a full memory is "summarized by the Queen at
//! retirement — not silently truncated". A ring buffer would be the obvious
//! implementation and the wrong one: the oldest entries are the established
//! decisions, so evicting them keeps the worker's recent guesses and discards
//! the operator's instructions. [`WorkerMemory::append`] therefore fails when
//! full, naming what has to happen next.

/// Where an entry came from. The whole point of the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provenance {
    /// A command's output — something that was observed to be true.
    Observation,
    /// The operator's instruction. The only kind the worker did not author.
    Decision,
    /// The worker's own conclusion. Load-bearing to keep separate: this is the
    /// kind that must not silently become fact.
    Inference,
}

impl Provenance {
    /// The label a prompt shows. Spelled out rather than abbreviated, because
    /// the reader is a small model and `INF` is not self-describing.
    pub fn label(self) -> &'static str {
        match self {
            Provenance::Observation => "observation",
            Provenance::Decision => "decision (from the operator)",
            Provenance::Inference => "inference (this worker's own conclusion, not established)",
        }
    }

    /// Every variant, in the order a prompt should present them: what was seen,
    /// then what was instructed, then what was guessed.
    pub fn all() -> [Provenance; 3] {
        [Provenance::Observation, Provenance::Decision, Provenance::Inference]
    }
}

/// One note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub provenance: Provenance,
    pub text: String,
}

/// Why an append was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppendError {
    /// The cap is reached. Carries the sentence a caller should act on: the
    /// Queen summarizes at retirement, and until then nothing more is recorded.
    Full { cap: usize },
    /// Nothing worth recording. Refused rather than stored so an empty note
    /// cannot consume budget that a real one needs.
    Empty,
}

impl AppendError {
    pub fn message(&self) -> String {
        match self {
            AppendError::Full { cap } => format!(
                "worker memory is full at {cap} entries. It is NOT truncated automatically: the \
                 Queen summarizes it at retirement (see WorkerMemory::summarize_with), because the \
                 oldest entries are the established decisions and evicting them would keep this \
                 worker's recent guesses while discarding the operator's instructions"
            ),
            AppendError::Empty => "an empty note is refused rather than stored".to_string(),
        }
    }
}

/// A worker's append-only notes.
#[derive(Clone, Debug)]
pub struct WorkerMemory {
    worker: String,
    entries: Vec<Entry>,
    cap: usize,
}

impl WorkerMemory {
    /// `cap` is in entries. A cap of 0 is clamped to 1 — a memory that can hold
    /// nothing is a configuration mistake that would present as a worker which
    /// silently never learns.
    pub fn new(worker: &str, cap: usize) -> Self {
        Self { worker: worker.to_string(), entries: Vec::new(), cap: cap.max(1) }
    }

    /// Record a note. Append-only: there is no method that edits or removes one,
    /// which is the property that makes provenance meaningful later.
    pub fn append(&mut self, provenance: Provenance, text: &str) -> Result<(), AppendError> {
        if text.trim().is_empty() {
            return Err(AppendError::Empty);
        }
        if self.entries.len() >= self.cap {
            return Err(AppendError::Full { cap: self.cap });
        }
        self.entries.push(Entry { provenance, text: text.to_string() });
        Ok(())
    }

    pub fn worker(&self) -> &str { &self.worker }
    pub fn entries(&self) -> &[Entry] { &self.entries }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn is_full(&self) -> bool { self.entries.len() >= self.cap }
    pub fn remaining(&self) -> usize { self.cap.saturating_sub(self.entries.len()) }

    /// Notes of one kind, in the order they were recorded.
    pub fn of(&self, p: Provenance) -> Vec<&str> {
        self.entries.iter().filter(|e| e.provenance == p).map(|e| e.text.as_str()).collect()
    }

    /// What this worker may treat as established: observations and the
    /// operator's decisions, never its own inferences.
    ///
    /// This is the reader half of the doc's turn-200 argument. Storing the
    /// distinction and then handing every consumer one undifferentiated list
    /// would record provenance without ever acting on it.
    pub fn established(&self) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.provenance != Provenance::Inference).collect()
    }

    /// Render for a prompt, through `fencer`.
    ///
    /// `fencer(label, provenance, body)` is `read_harness::fence` with its
    /// budget already bound. EVERY entry's text goes through it — the store
    /// never emits a note's body any other way, which is what makes "memory is
    /// sanitized and fenced" a property of the type rather than of the caller
    /// remembering.
    ///
    /// One fenced block per provenance rather than per entry: a worker with
    /// forty notes would otherwise produce forty fence headers, and the header
    /// is there to mark a trust boundary, not to separate bullet points.
    pub fn render(&self, fencer: impl Fn(&str, &str, &str) -> String) -> String {
        let mut out = String::new();
        for p in Provenance::all() {
            let notes = self.of(p);
            if notes.is_empty() { continue; }
            let body = notes.iter().map(|n| format!("- {n}")).collect::<Vec<_>>().join("\n");
            out.push_str(&fencer(&format!("{} memory", self.worker), p.label(), &body));
        }
        out
    }

    /// Collapse to a summary, PER PROVENANCE.
    ///
    /// `f(provenance, notes)` is the Queen's summarization of one kind's notes.
    ///
    /// Summarizing all notes into one blob would be the obvious signature and
    /// would destroy the thing the cap exists to protect: after such a collapse
    /// the worker's inferences and the operator's decisions are one
    /// indistinguishable paragraph, which is precisely the turn-200 drift the
    /// provenance tagging was introduced to prevent. Retirement is exactly when
    /// that distinction is most load-bearing, since the summary is what a
    /// revived worker reads first.
    ///
    /// Kinds with no notes produce no entry, so a memory holding only
    /// observations does not gain an empty "inference" section. A summary that
    /// comes back blank is dropped for the same reason `append` refuses one.
    pub fn summarize_with(&mut self, f: impl Fn(Provenance, &[&str]) -> String) {
        let mut next = Vec::new();
        for p in Provenance::all() {
            let notes = self.of(p);
            if notes.is_empty() { continue; }
            let s = f(p, &notes);
            if s.trim().is_empty() { continue; }
            next.push(Entry { provenance: p, text: s });
        }
        self.entries = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fencer with the shape of `read_harness::fence`, standing in for it so
    /// this crate does not need the real one (which lives downstream). It marks
    /// its body unmistakably, so a test can prove a string went THROUGH it.
    fn spy_fence(label: &str, provenance: &str, body: &str) -> String {
        format!("<<FENCE {label} ({provenance})>>\n{body}\n<<END>>\n")
    }

    fn filled(cap: usize) -> WorkerMemory {
        let mut m = WorkerMemory::new("terminal-ui", cap);
        m.append(Provenance::Decision, "the operator wants tabs, not windows").unwrap();
        m.append(Provenance::Observation, "cargo test passes 40/40").unwrap();
        m.append(Provenance::Inference, "the resize bug is probably in layout.rs").unwrap();
        m
    }

    #[test]
    fn notes_are_append_only_and_keep_their_provenance() {
        let m = filled(10);
        assert_eq!(m.len(), 3);
        assert_eq!(m.of(Provenance::Decision), vec!["the operator wants tabs, not windows"]);
        assert_eq!(m.of(Provenance::Inference), vec!["the resize bug is probably in layout.rs"]);
        // Order within a kind is the order recorded.
        let mut m2 = WorkerMemory::new("w", 10);
        m2.append(Provenance::Observation, "first").unwrap();
        m2.append(Provenance::Observation, "second").unwrap();
        assert_eq!(m2.of(Provenance::Observation), vec!["first", "second"]);
    }

    #[test]
    fn an_inference_is_never_established() {
        // The turn-200 rule, as a property rather than a comment: a worker's own
        // conclusion must not be readable as something that was observed or
        // instructed, no matter how long it has been sitting in memory.
        let m = filled(10);
        let est: Vec<&str> = m.established().iter().map(|e| e.text.as_str()).collect();
        assert_eq!(est.len(), 2, "only the observation and the decision are established: {est:?}");
        assert!(!est.iter().any(|t| t.contains("probably")),
            "an inference leaked into established(): {est:?}");
        // And the complement: established() is not merely empty or a filter that
        // has stopped discriminating.
        assert!(est.contains(&"cargo test passes 40/40"));
        assert!(est.contains(&"the operator wants tabs, not windows"));
    }

    #[test]
    fn a_full_memory_refuses_rather_than_evicting_the_oldest() {
        // A ring buffer here would keep the worker's newest guesses and discard
        // the operator's oldest instructions, which is the exact inversion of
        // what matters. Pin the refusal AND the survival of entry one.
        let mut m = filled(3);
        assert!(m.is_full());
        let e = m.append(Provenance::Observation, "one more").unwrap_err();
        assert_eq!(e, AppendError::Full { cap: 3 });
        assert!(e.message().contains("summarize"), "the refusal must name the way out: {}", e.message());
        assert_eq!(m.len(), 3, "a refused append must not change the store");
        assert_eq!(m.of(Provenance::Decision), vec!["the operator wants tabs, not windows"],
            "the oldest entry — the operator's decision — must survive a full memory");
    }

    #[test]
    fn an_empty_note_cannot_consume_budget() {
        let mut m = WorkerMemory::new("w", 2);
        assert_eq!(m.append(Provenance::Observation, "   \n ").unwrap_err(), AppendError::Empty);
        assert_eq!(m.remaining(), 2);
    }

    #[test]
    fn every_rendered_body_passes_through_the_fencer() {
        // The trust boundary, asserted structurally: no note may appear in the
        // output except inside a fenced block. This is what makes injecting the
        // fencer safe — there is no path around it.
        let m = filled(10);
        let out = m.render(spy_fence);
        for e in m.entries() {
            let pos = out.find(e.text.as_str()).unwrap_or_else(|| panic!("missing note: {e:?}"));
            let open = out[..pos].rfind("<<FENCE").expect("note appeared before any fence opened");
            let closed_before = out[..pos].rfind("<<END>>").map(|c| c > open).unwrap_or(false);
            assert!(!closed_before, "note {:?} sits outside a fenced block", e.text);
        }
        // Each kind present gets exactly one block — not one per entry.
        assert_eq!(out.matches("<<FENCE").count(), 3);
    }

    #[test]
    fn a_kind_with_no_notes_produces_no_block() {
        let mut m = WorkerMemory::new("w", 4);
        m.append(Provenance::Observation, "only an observation").unwrap();
        let out = m.render(spy_fence);
        assert_eq!(out.matches("<<FENCE").count(), 1);
        assert!(!out.contains("inference"), "an absent kind must not get an empty section: {out}");
    }

    #[test]
    fn summarizing_preserves_the_provenance_split() {
        // The failure this guards: collapsing to ONE summary blob would make the
        // worker's guess and the operator's instruction indistinguishable at
        // exactly the moment a revived worker reads them first.
        let mut m = filled(3);
        m.summarize_with(|p, notes| format!("[{} notes summarized as {:?}]", notes.len(), p));
        assert_eq!(m.len(), 3, "one summary per kind present");
        for p in Provenance::all() {
            assert_eq!(m.of(p).len(), 1, "{p:?} must keep its own summary");
        }
        assert!(m.of(Provenance::Inference)[0].contains("Inference"));
        // And the collapse frees budget, which is the point of doing it.
        let mut big = WorkerMemory::new("w", 6);
        for i in 0..6 { big.append(Provenance::Observation, &format!("note {i}")).unwrap(); }
        assert!(big.is_full());
        big.summarize_with(|_, n| format!("{} observations", n.len()));
        assert_eq!(big.len(), 1);
        assert!(!big.is_full(), "a summarized memory must have room to keep working");
        assert!(big.append(Provenance::Observation, "after summary").is_ok());
    }

    #[test]
    fn a_blank_summary_is_dropped_rather_than_stored() {
        let mut m = filled(3);
        m.summarize_with(|_, _| "  ".to_string());
        assert!(m.is_empty(), "a summarizer that returned nothing must not leave blank entries");
    }

    #[test]
    fn a_zero_cap_is_clamped_so_a_worker_cannot_silently_never_learn() {
        let mut m = WorkerMemory::new("w", 0);
        assert!(m.append(Provenance::Observation, "something").is_ok());
    }
}
