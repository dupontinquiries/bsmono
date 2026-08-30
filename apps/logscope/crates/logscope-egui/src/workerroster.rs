//! workerroster — the callers `workergen` never had.
//!
//! ## What was missing
//!
//! `workergen::generate` builds a persona: a stance, habits, refusals,
//! and a `render()` that turns them into a system prompt. It has 400
//! lines of tests and no caller anywhere in the app.
//!
//! `promptroute` has a `Dest::Worker` that nothing can reach, because
//! the routing call hardcodes `workers: false` — there is no roster to
//! ask.
//!
//! This is the piece between them: hold the generated personas, pick one
//! for a prompt, and answer "are there any" so the matrix can route.
//!
//! ## Not the same thing as `logscope-fan`'s Roster
//!
//! There is already a `Roster` in `logscope-fan`, and it is a DIFFERENT
//! concept — worth stating because the names collide:
//!
//! - `logscope_fan::roster::Roster` holds `WorkerSpec`s: an operational
//!   contract. Which paths a worker may edit, which tools it may call,
//!   how anyone knows it succeeded.
//! - This holds `workergen::Worker`s: a persona. How it thinks, what it
//!   habitually does, what it refuses.
//!
//! A worker eventually wants both — a contract binding what it may touch
//! and a persona shaping how it answers — but they are separate
//! questions and merging them would make the operational one depend on a
//! corpus of book fragments, which is absurd.
//!
//! ## Why a seed per worker rather than a random one
//!
//! Regenerating the same worker must produce the same worker. A roster
//! that quietly reshuffles personalities between sessions would make
//! every answer unattributable — you could not tell a worker changing
//! its mind from the roster handing you a different worker.

use crate::workergen::{self, Corpus, Worker};

/// The generated personas available to answer a prompt.
#[derive(Debug, Default)]
pub struct WorkerRoster {
    workers: Vec<Worker>,
    /// The seed each worker was generated from, parallel to `workers`.
    /// Kept so a roster can be rebuilt exactly — see the module note.
    seeds: Vec<u64>,
}

impl WorkerRoster {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.workers.len()
    }

    pub fn all(&self) -> &[Worker] {
        &self.workers
    }

    pub fn seeds(&self) -> &[u64] {
        &self.seeds
    }

    /// Generate `n` workers from `corpus`, starting at `seed`.
    ///
    /// Returns how many were added. FEWER THAN `n` IS NORMAL and not an
    /// error: `generate` rejects incoherent combinations (a worker told
    /// both to cite sources and never to cite sources), so some seeds
    /// produce nothing. Retrying a rejected seed with a different one
    /// would hide how often the corpus contradicts itself, which is the
    /// signal that the corpus needs work.
    pub fn fill(&mut self, corpus: &Corpus, seed: u64, n: usize) -> usize {
        let mut added = 0;
        for i in 0..n as u64 {
            let s = seed.wrapping_add(i);
            if let Ok(w) = workergen::generate(corpus, s) {
                // Two seeds can land on the same stance. A roster with
                // the same worker twice reads as a bug in the generator
                // even when it is only a collision, so skip it.
                if self.workers.iter().any(|e| e.name == w.name) {
                    continue;
                }
                self.workers.push(w);
                self.seeds.push(s);
                added += 1;
            }
        }
        added
    }

    /// Pick a worker for `prompt`.
    ///
    /// Deterministic on the prompt text, NOT random and NOT round-robin.
    /// Asking the same question twice should reach the same worker, or a
    /// person comparing two answers cannot tell whether the worker
    /// changed its mind or they simply got a different worker.
    ///
    /// This is a rotation, not a match: nothing here understands which
    /// stance suits which question. Pretending otherwise would need a
    /// classifier, and `promptroute` already argues that a table beats a
    /// model call for decisions this small.
    pub fn pick(&self, prompt: &str) -> Option<&Worker> {
        if self.workers.is_empty() {
            return None;
        }
        let h = prompt
            .bytes()
            .fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
        self.workers.get((h % self.workers.len() as u64) as usize)
    }

    /// The system prompt for a worker, or `None` when the roster is
    /// empty — so a caller falls through to the engine rather than
    /// sending a prompt with no persona attached.
    pub fn system_prompt(&self, prompt: &str) -> Option<String> {
        self.pick(prompt).map(|w| w.render())
    }

    /// Forget everything. What a corpus change should call: personas
    /// built from the old corpus would otherwise sit alongside new ones
    /// with no way to tell them apart.
    pub fn clear(&mut self) {
        self.workers.clear();
        self.seeds.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Corpus {
        Corpus {
            roles: vec!["cartographer".into(), "archivist".into(), "auditor".into()],
            manners: vec!["from the margins".into(), "impatiently".into(), "slowly".into()],
            actions: vec![
                "name the assumption".into(),
                "measure before arguing".into(),
                "read the whole file".into(),
            ],
            taboos: vec![
                "guess at a number".into(),
                "answer before reading".into(),
                "invent a citation".into(),
            ],
        }
    }

    #[test]
    fn an_empty_roster_picks_nothing_rather_than_panicking() {
        // `pick` does `h % len`, and len 0 would divide by zero. The
        // empty roster is the NORMAL state at startup, not an edge case.
        let r = WorkerRoster::new();
        assert!(r.pick("anything").is_none());
        assert!(r.system_prompt("anything").is_none());
    }

    #[test]
    fn the_same_prompt_always_reaches_the_same_worker() {
        // The property that makes answers comparable. If this rotated or
        // randomised, a person asking twice could not tell a worker
        // changing its mind from the roster handing them a different
        // worker.
        let mut r = WorkerRoster::new();
        r.fill(&corpus(), 1, 6);
        assert!(!r.is_empty(), "the corpus produced no workers at all");
        let a = r.pick("should i use int8 here").map(|w| w.name.clone());
        let b = r.pick("should i use int8 here").map(|w| w.name.clone());
        assert_eq!(a, b);
    }

    #[test]
    fn different_prompts_can_reach_different_workers() {
        // Weak assertion on purpose: with a small roster two prompts may
        // legitimately collide, so this checks the roster is CAPABLE of
        // spreading rather than asserting a particular split.
        let mut r = WorkerRoster::new();
        r.fill(&corpus(), 100, 8);
        if r.len() < 2 {
            return; // nothing to spread across; not a failure of pick
        }
        let picks: std::collections::HashSet<String> = (0..40)
            .filter_map(|i| r.pick(&format!("question number {i}")).map(|w| w.name.clone()))
            .collect();
        assert!(picks.len() > 1, "every prompt landed on one worker: {picks:?}");
    }

    #[test]
    fn filling_is_reproducible_from_its_seed() {
        // A roster that reshuffles personalities between sessions makes
        // every answer unattributable.
        let mut a = WorkerRoster::new();
        let mut b = WorkerRoster::new();
        a.fill(&corpus(), 42, 5);
        b.fill(&corpus(), 42, 5);
        let names = |r: &WorkerRoster| r.all().iter().map(|w| w.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&a), names(&b));
        assert_eq!(a.seeds(), b.seeds());
    }

    #[test]
    fn fewer_workers_than_asked_for_is_normal() {
        // `generate` rejects incoherent combinations, so some seeds
        // produce nothing. Retrying with a different seed would hide how
        // often the corpus contradicts itself — which is the signal that
        // the corpus needs work, not something to paper over.
        let mut r = WorkerRoster::new();
        let asked = 10;
        let got = r.fill(&corpus(), 7, asked);
        assert_eq!(got, r.len(), "the return value must match what landed");
        assert!(got <= asked);
    }

    #[test]
    fn a_name_is_never_listed_twice() {
        // Two seeds can land on the same stance. The same worker twice
        // reads as a generator bug even when it is only a collision.
        let mut r = WorkerRoster::new();
        r.fill(&corpus(), 1, 30);
        let mut names: Vec<&str> = r.all().iter().map(|w| w.name.as_str()).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "a duplicate worker name got in");
        assert_eq!(r.seeds().len(), r.len(), "seeds must stay parallel to workers");
    }

    #[test]
    fn clearing_drops_the_seeds_too() {
        // Or a rebuild would append to stale seeds and the two arrays
        // would stop describing the same workers.
        let mut r = WorkerRoster::new();
        r.fill(&corpus(), 3, 4);
        r.clear();
        assert!(r.is_empty());
        assert!(r.seeds().is_empty());
    }
}
