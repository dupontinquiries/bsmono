//! cleanup-kit — "clean up with AI", for voiceterm, logscope and tadpole.
//!
//! All three produce **rough text that a person then has to read**: a
//! transcript, a log excerpt, an OCR page. All three want the same thing —
//! better formatting and a flag on anything that looks wrong — and none of them
//! should grow its own prompt, its own model discovery, or its own idea of what
//! the model is allowed to change.
//!
//! ## A cleanup is a proposal, never a rewrite
//!
//! This is the load-bearing decision. A small model asked to "clean up" a
//! transcript will happily invent a word it could not read, tidy a log line
//! into a different error code, or correct a name it has never seen. Applying
//! that silently turns a rough-but-true document into a smooth-and-wrong one,
//! and **the user cannot tell which they have**.
//!
//! So [`Proposal`] carries the original beside the suggestion, per span, and
//! nothing is applied until accepted. The expensive property is not that the
//! model is good; it is that being wrong is *visible and reversible*.
//!
//! ## What the model may and may not do
//!
//! [`Task`] is deliberately narrow. Reflowing, punctuation and casing are safe
//! because the words survive. Summarising is offered but marked as **lossy**,
//! so a caller cannot mistake it for a cleanup. There is no "improve this"
//! task, because a model with permission to improve has permission to change
//! anything.
//!
//! ## No inference here
//!
//! Discovery, task shape, proposal shape, and the accept/reject machinery. The
//! backend is a trait, which is what lets llama.cpp and logscope's wgpu engine
//! be alternatives rather than a fork.

use std::path::{Path, PathBuf};

// ── model discovery ─────────────────────────────────────────────────────────

/// A model this kit knows how to ask for, in preference order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KnownModel {
    /// Substring matched case-insensitively against a filename.
    pub matches: &'static str,
    pub label: &'static str,
    /// Why it is ranked where it is — shown in the picker so a choice is
    /// explainable rather than magic.
    pub why: &'static str,
}

/// Preference order, best first.
///
/// Researched rather than guessed, and the ordering is about **this job** — a
/// short, structured, on-device edit — not about general capability:
///
/// * **Gemma 4 E2B/E4B** first: Apache 2.0, built for phones and 8GB machines,
///   with native structured JSON output. Structured output is what makes a
///   *proposal* checkable rather than a wall of prose to re-parse, and this
///   monorepo targets PinePhone and Fairphone, where the E-series is aimed.
/// * **Qwen3.5 4B** next: the strongest general small model for CPU-only use,
///   and the better pick when the text is multilingual.
/// * **Phi-4-mini** when RAM is tight — MIT, ~2.5GB at Q4.
/// * **Llama 3.2 3B/1B** last, not because it is bad but because it is no
///   longer the newest; it keeps its place for the widest runner support, which
///   matters when a backend is fussy.
pub const KNOWN: &[KnownModel] = &[
    KnownModel { matches: "gemma-4-e", label: "Gemma 4 E-series",
                 why: "Apache 2.0, phone-sized, native structured output" },
    KnownModel { matches: "gemma4", label: "Gemma 4",
                 why: "Apache 2.0, structured output" },
    KnownModel { matches: "qwen3.5", label: "Qwen3.5 4B",
                 why: "strongest small CPU model; best for multilingual text" },
    KnownModel { matches: "qwen3", label: "Qwen3",
                 why: "strong small model, wide runner support" },
    KnownModel { matches: "phi-4-mini", label: "Phi-4-mini",
                 why: "MIT, ~2.5GB at Q4 — the tight-RAM pick" },
    KnownModel { matches: "phi4-mini", label: "Phi-4-mini",
                 why: "MIT, ~2.5GB at Q4 — the tight-RAM pick" },
    KnownModel { matches: "llama-3.2", label: "Llama 3.2",
                 why: "widest runner support; safe when a backend is fussy" },
    KnownModel { matches: "llama3.2", label: "Llama 3.2",
                 why: "widest runner support; safe when a backend is fussy" },
];

/// A model file on disk.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Found {
    pub path: PathBuf,
    pub label: String,
    pub why: String,
    /// Index into [`KNOWN`]; lower is preferred.
    pub rank: usize,
}

/// Which engine can run a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    /// GGUF via llama.cpp.
    LlamaCpp,
    /// GGUF via logscope's wgpu engine — no C toolchain, runs where llama.cpp
    /// currently does not build.
    Wgpu,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self { Backend::LlamaCpp => "llama.cpp", Backend::Wgpu => "wgpu (logscope)" }
    }
}

/// Pick a backend.
///
/// Both read GGUF, so this is not a capability question — it is an
/// **availability** one. llama.cpp is preferred when it is actually present
/// because it is the more exercised path; the wgpu engine is the fallback that
/// makes the feature work at all on machines where the C toolchain does not
/// cooperate, which is the whole reason for having two.
pub fn choose_backend(llama_cpp_available: bool, wgpu_available: bool) -> Option<Backend> {
    match (llama_cpp_available, wgpu_available) {
        (true, _) => Some(Backend::LlamaCpp),
        (false, true) => Some(Backend::Wgpu),
        (false, false) => None,
    }
}

/// Scan a directory listing for models this kit recognises.
///
/// Takes the listing rather than reading the directory, so it is testable and
/// so a caller can feed it whatever it already has.
pub fn recognise(files: &[(PathBuf, u64)]) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for (path, _size) in files {
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !name.ends_with(".gguf") { continue; }
        if let Some((rank, k)) = KNOWN.iter().enumerate().find(|(_, k)| name.contains(k.matches)) {
            out.push(Found { path: path.clone(), label: k.label.into(),
                             why: k.why.into(), rank });
        }
    }
    // Rank first, then path, so a directory holding two copies of the same
    // family picks deterministically rather than by filesystem order.
    out.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.path.cmp(&b.path)));
    out
}

/// The model that should be used, if any.
pub fn best(files: &[(PathBuf, u64)]) -> Option<Found> { recognise(files).into_iter().next() }

/// Where to look, mirroring the app-level convention.
pub fn search_paths(home: &Path) -> Vec<PathBuf> {
    vec![home.join("models"), home.join(".local/share/models"),
         PathBuf::from("/usr/share/models")]
}

// ── the job ─────────────────────────────────────────────────────────────────

/// What the model is being asked to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Task {
    /// Line breaks, paragraphs, list structure. Words unchanged.
    Reflow,
    /// Punctuation and sentence casing — what a transcript most needs.
    Punctuate,
    /// Flag spans that look wrong: a garbled OCR word, an impossible timestamp,
    /// a log line that contradicts its neighbours. Changes nothing.
    Vet,
    /// Condense. **Lossy**, and marked so, because a caller must not offer it
    /// beside the others as though it were the same kind of operation.
    Summarise,
}

impl Task {
    /// Does this task discard information?
    pub fn is_lossy(self) -> bool { matches!(self, Task::Summarise) }

    /// May this task change the words themselves?
    pub fn may_alter_words(self) -> bool {
        matches!(self, Task::Punctuate | Task::Summarise)
    }

    pub fn label(self) -> &'static str {
        match self {
            Task::Reflow => "Fix formatting",
            Task::Punctuate => "Add punctuation",
            Task::Vet => "Check for problems",
            Task::Summarise => "Summarise (lossy)",
        }
    }
}

/// One suggested change.
#[derive(Clone, PartialEq, Debug)]
pub struct Span {
    /// Index into the caller's units — segments, regions, lines.
    pub at: usize,
    pub original: String,
    pub suggested: String,
    /// Why, in the model's words. Shown, because an unexplained change is one a
    /// user must diff by eye.
    pub note: String,
    pub accepted: bool,
}

impl Span {
    /// Did the model actually change anything?
    ///
    /// A no-op suggestion is noise in a review list, and a model that returns
    /// the input unchanged for half the spans is common.
    pub fn is_change(&self) -> bool { self.original.trim() != self.suggested.trim() }

    /// Words added or removed, ignoring punctuation and case.
    ///
    /// The measure that matters for trust: a `Reflow` or `Punctuate` that
    /// changes the word list has exceeded its brief, whatever it claims.
    pub fn word_delta(&self) -> isize {
        fn words(s: &str) -> Vec<String> {
            s.split_whitespace()
                .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
                .filter(|w| !w.is_empty())
                .collect()
        }
        words(&self.suggested).len() as isize - words(&self.original).len() as isize
    }
}

/// What a backend returns.
#[derive(Clone, PartialEq, Debug)]
pub struct Proposal {
    pub task: Task,
    pub model: String,
    pub backend: Backend,
    pub spans: Vec<Span>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CleanupError {
    /// Nothing recognised in the model directories.
    NoModel,
    /// A model is present but no engine can run it.
    NoBackend,
    Failed(String),
}

/// A backend that can run a cleanup.
pub trait Engine {
    fn run(&self, task: Task, units: &[String]) -> Result<Vec<Span>, CleanupError>;
    fn model_label(&self) -> String;
    fn backend(&self) -> Backend;
}

impl Proposal {
    pub fn from_engine(engine: &dyn Engine, task: Task, units: &[String])
        -> Result<Proposal, CleanupError> {
        let spans = engine.run(task, units)?;
        Ok(Proposal { task, model: engine.model_label(), backend: engine.backend(), spans })
    }

    /// Suggestions that actually change something.
    pub fn changes(&self) -> Vec<&Span> {
        self.spans.iter().filter(|s| s.is_change()).collect()
    }

    /// Suggestions that broke the task's contract.
    ///
    /// A `Reflow` or `Vet` that alters the words did something it was not asked
    /// to do. Rather than trusting the task label, this checks — because the
    /// model's compliance is exactly the thing that cannot be assumed.
    pub fn overreach(&self) -> Vec<&Span> {
        if self.task.may_alter_words() { return Vec::new(); }
        self.spans.iter().filter(|s| s.is_change() && s.word_delta() != 0).collect()
    }

    /// Apply the accepted spans to the caller's units.
    ///
    /// Out-of-range indices are skipped rather than panicking: a model that
    /// hallucinates a span index is misbehaving, not a reason to take the app
    /// down.
    pub fn apply(&self, units: &[String]) -> Vec<String> {
        let mut out = units.to_vec();
        for s in self.spans.iter().filter(|s| s.accepted && s.is_change()) {
            if let Some(slot) = out.get_mut(s.at) { *slot = s.suggested.clone(); }
        }
        out
    }

    pub fn accept_all(&mut self) {
        for s in self.spans.iter_mut() { s.accepted = true; }
    }

    /// Accept everything that stayed inside the task's contract.
    ///
    /// The useful middle setting: a user who trusts reflowing but not invention
    /// can take the safe changes in one click and review the rest.
    pub fn accept_compliant(&mut self) {
        let strict = !self.task.may_alter_words();
        for s in self.spans.iter_mut() {
            s.accepted = !strict || s.word_delta() == 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str) -> (PathBuf, u64) { (PathBuf::from(format!("/home/kd/models/{name}")), 1) }

    fn span(at: usize, orig: &str, sug: &str) -> Span {
        Span { at, original: orig.into(), suggested: sug.into(),
               note: "test".into(), accepted: false }
    }

    #[test]
    fn gemma_4_e_is_preferred_for_this_job() {
        // Not "best model overall" — best for a short, structured, on-device
        // edit on a phone-class device, which is what this monorepo targets.
        let found = best(&[f("llama-3.2-3b-instruct-q4.gguf"),
                           f("gemma-4-e2b-it-q4_k_m.gguf"),
                           f("qwen3.5-4b-instruct-q4.gguf")]).expect("something");
        assert_eq!(found.label, "Gemma 4 E-series");
        assert!(found.why.contains("structured output"),
                "the ranking is explainable: {}", found.why);
    }

    #[test]
    fn the_order_falls_back_through_the_researched_list() {
        assert_eq!(best(&[f("llama-3.2-1b.gguf"), f("qwen3.5-4b.gguf")]).unwrap().label,
                   "Qwen3.5 4B");
        assert_eq!(best(&[f("llama-3.2-1b.gguf"), f("phi-4-mini-q4.gguf")]).unwrap().label,
                   "Phi-4-mini");
        assert_eq!(best(&[f("llama-3.2-1b.gguf")]).unwrap().label, "Llama 3.2");
    }

    #[test]
    fn only_gguf_files_are_considered() {
        // A README named after a model is not a model.
        assert!(best(&[f("gemma-4-e2b-readme.txt"), f("qwen3.5-notes.md")]).is_none());
    }

    #[test]
    fn two_copies_of_one_family_pick_deterministically() {
        // Filesystem order is not stable, and a picker that changed its mind
        // between launches would be indistinguishable from a bug.
        let files = vec![f("gemma-4-e4b-q4.gguf"), f("gemma-4-e2b-q4.gguf")];
        let a = best(&files).unwrap();
        let mut rev = files.clone();
        rev.reverse();
        assert_eq!(a, best(&rev).unwrap());
    }

    #[test]
    fn nothing_recognised_is_none_rather_than_a_wrong_guess() {
        assert!(best(&[f("some-random-model.gguf")]).is_none());
        assert!(best(&[]).is_none());
    }

    #[test]
    fn llama_cpp_is_preferred_but_wgpu_makes_the_feature_exist_at_all() {
        // Both read GGUF, so this is availability rather than capability — and
        // the wgpu path is why the feature works where the C toolchain does not.
        assert_eq!(choose_backend(true, true), Some(Backend::LlamaCpp));
        assert_eq!(choose_backend(false, true), Some(Backend::Wgpu));
        assert_eq!(choose_backend(true, false), Some(Backend::LlamaCpp));
        assert_eq!(choose_backend(false, false), None);
    }

    #[test]
    fn a_no_op_suggestion_is_not_a_change() {
        // Models return the input unchanged for a large share of spans, and a
        // review list full of those is a review list nobody reads.
        assert!(!span(0, "hello world", " hello world ").is_change());
        assert!(span(0, "hello world", "Hello, world.").is_change());
    }

    #[test]
    fn word_delta_ignores_punctuation_and_case() {
        // Punctuating must read as zero drift, or every legitimate cleanup
        // would look like overreach.
        assert_eq!(span(0, "so the thing is it broke", "So, the thing is: it broke.").word_delta(), 0);
        assert_eq!(span(0, "the thing", "the entire thing").word_delta(), 1);
        assert_eq!(span(0, "the entire thing", "the thing").word_delta(), -1);
    }

    #[test]
    fn a_reflow_that_changes_words_is_flagged_as_overreach() {
        // The property the whole design exists for: a model asked only to
        // reformat, inventing a word. Trusting the task label would hide it.
        let p = Proposal {
            task: Task::Reflow, model: "m".into(), backend: Backend::Wgpu,
            spans: vec![
                span(0, "line one line two", "line one\nline two"),
                span(1, "the routing tbl", "the routing table entry"),
            ],
        };
        let over = p.overreach();
        assert_eq!(over.len(), 1, "only the word-adding span");
        assert_eq!(over[0].at, 1);
    }

    #[test]
    fn punctuating_is_allowed_to_touch_words_so_nothing_is_flagged() {
        let p = Proposal {
            task: Task::Punctuate, model: "m".into(), backend: Backend::Wgpu,
            spans: vec![span(0, "hello there", "Hello there, friend.")],
        };
        assert!(p.overreach().is_empty(), "the task permits it");
    }

    #[test]
    fn summarise_is_marked_lossy_and_reflow_is_not() {
        // A caller must not offer them side by side as though they were the
        // same kind of operation.
        assert!(Task::Summarise.is_lossy());
        for t in [Task::Reflow, Task::Punctuate, Task::Vet] {
            assert!(!t.is_lossy(), "{t:?}");
        }
    }

    #[test]
    fn vet_changes_nothing_by_contract() {
        assert!(!Task::Vet.may_alter_words());
        assert!(!Task::Reflow.may_alter_words());
    }

    #[test]
    fn nothing_is_applied_until_accepted() {
        // The load-bearing rule. A cleanup that applied itself would turn a
        // rough-but-true document into a smooth-and-wrong one with no way for
        // the user to tell which they have.
        let units = vec!["line one line two".to_string(), "second".to_string()];
        let p = Proposal {
            task: Task::Reflow, model: "m".into(), backend: Backend::Wgpu,
            spans: vec![span(0, "line one line two", "line one\nline two")],
        };
        assert_eq!(p.apply(&units), units, "unaccepted spans change nothing");
    }

    #[test]
    fn accept_compliant_takes_the_safe_changes_and_leaves_the_rest() {
        // The useful middle setting for someone who trusts reflowing but not
        // invention.
        let units = vec!["a b".to_string(), "c d".to_string()];
        let mut p = Proposal {
            task: Task::Reflow, model: "m".into(), backend: Backend::Wgpu,
            spans: vec![span(0, "a b", "a\nb"), span(1, "c d", "c d e")],
        };
        p.accept_compliant();
        let out = p.apply(&units);
        assert_eq!(out[0], "a\nb", "the safe change landed");
        assert_eq!(out[1], "c d", "the word-adding one did not");
    }

    #[test]
    fn a_hallucinated_span_index_is_skipped_rather_than_panicking() {
        // A model that invents an index is misbehaving, not a reason to take
        // the app down.
        let units = vec!["only one".to_string()];
        let mut p = Proposal {
            task: Task::Reflow, model: "m".into(), backend: Backend::Wgpu,
            spans: vec![span(99, "nothing", "something")],
        };
        p.accept_all();
        assert_eq!(p.apply(&units), units);
    }

    #[test]
    fn changes_lists_only_real_edits() {
        let p = Proposal {
            task: Task::Punctuate, model: "m".into(), backend: Backend::Wgpu,
            spans: vec![span(0, "same", "same"), span(1, "x", "X.")],
        };
        assert_eq!(p.changes().len(), 1);
    }
}
