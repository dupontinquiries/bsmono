//! directives.rs — a library of DIRECTIVE prompts for the fan-out / fuzz mode. A model's behaviour
//! changes a lot with its instructions, so instead of fanning N branches that differ only by sampling
//! temperature (output diversity), we fan branches that differ by DIRECTIVE (behavioural diversity):
//! each branch gets the same task but a different strategy/persona/constraint, so the spatial view shows
//! genuinely different approaches side by side, not N rewordings of one.
//!
//! This is the pure library + selection logic. The fan-out path (`:fuzz N`) selects `n` directives
//! spread across categories, composes each onto the base system prompt, and spawns one branch per
//! composed prompt (temperature still varies within, for a second axis of diversity).

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tag { Strategy, Process, Constraint, Persona, Risk }

#[derive(Clone, Copy, Debug)]
pub struct Directive { pub name: &'static str, pub tag: Tag, pub prompt: &'static str }

/// The curated library. Spread across categories so a fan-out explores different AXES of behaviour
/// (how to approach it, how to work, what to optimize, who you are, how bold to be) rather than one.
pub const LIBRARY: &[Directive] = &[
    // Strategy — how to approach the whole problem
    Directive { name: "incremental",  tag: Tag::Strategy,   prompt: "Your context is limited. Land small, certain wins first and defer the big, risky pieces — but do not stop until the target is fully met." },
    Directive { name: "hardest-first", tag: Tag::Strategy,  prompt: "Attempt the hardest, most uncertain part first. If it works the rest is easy; if it fails, fail fast and say so." },
    Directive { name: "breadth",      tag: Tag::Strategy,   prompt: "Sketch two or three different approaches briefly and name their tradeoffs before committing to one." },
    Directive { name: "depth",        tag: Tag::Strategy,   prompt: "Commit to the single most obvious approach and drive it all the way to a complete, working solution." },
    Directive { name: "target-driven", tag: Tag::Strategy,  prompt: "Treat this as a benchmark to beat. Keep iterating and measuring against the goal until it is unambiguously hit." },
    // Process — how to work
    Directive { name: "tests-first",  tag: Tag::Process,    prompt: "Write the tests first, then the implementation that makes them pass." },
    Directive { name: "outline",      tag: Tag::Process,    prompt: "Write the solution as a plan or commented skeleton first, then fill it in." },
    Directive { name: "minimal-iter", tag: Tag::Process,    prompt: "Write the smallest thing that could possibly work, then improve it in a single focused pass." },
    Directive { name: "types-first",  tag: Tag::Process,    prompt: "Design the data types and function signatures first; let the types drive the implementation." },
    // Constraint — what to optimize for
    Directive { name: "shortest",     tag: Tag::Constraint, prompt: "Produce the shortest correct solution. No ceremony, no extra abstraction." },
    Directive { name: "no-deps",      tag: Tag::Constraint, prompt: "Use only the standard library — no external crates or dependencies." },
    Directive { name: "readable",     tag: Tag::Constraint, prompt: "Optimize for readability above all: clear names, small functions, obvious control flow." },
    Directive { name: "fast",         tag: Tag::Constraint, prompt: "Optimize for runtime performance. Prefer the algorithmically and mechanically fastest approach." },
    Directive { name: "defensive",    tag: Tag::Constraint, prompt: "Handle every error and edge case explicitly. Assume hostile input and degrade gracefully." },
    // Persona — who you are
    Directive { name: "prod-reviewer", tag: Tag::Persona,   prompt: "You are a senior engineer shipping this to production. Be rigorous about correctness and maintainability." },
    Directive { name: "prototyper",   tag: Tag::Persona,    prompt: "You are prototyping fast to test an idea. Favor speed of writing over polish." },
    Directive { name: "teacher",      tag: Tag::Persona,    prompt: "You are explaining to a learner. Make each step clear and motivated." },
    Directive { name: "skeptic",      tag: Tag::Persona,    prompt: "You are a skeptic. Surface the edge cases and failure modes others miss, and handle them." },
    // Risk — how bold to be
    Directive { name: "bold",         tag: Tag::Risk,       prompt: "Be ambitious. Reach for the elegant or clever solution even if it is harder to get right." },
    Directive { name: "safe",         tag: Tag::Risk,       prompt: "Be conservative. Prefer boring, well-trodden solutions that are obviously correct." },
];

const TAGS: [Tag; 5] = [Tag::Strategy, Tag::Process, Tag::Constraint, Tag::Persona, Tag::Risk];

/// Select up to `n` DISTINCT directives, spread across tags so the fan-out covers different axes before
/// repeating an axis. Deterministic in `seed` (reproducible fan-outs). Round-robins the tags, picking a
/// not-yet-used directive from each (seed rotates which one), until `n` are chosen or the library is
/// exhausted. Returns at most LIBRARY.len() (there can't be more distinct than exist).
pub fn select(n: usize, seed: u64) -> Vec<&'static Directive> {
    let n = n.min(LIBRARY.len());
    let mut chosen: Vec<usize> = Vec::with_capacity(n);
    let mut tag_round = 0usize;
    while chosen.len() < n {
        let tag = TAGS[(tag_round + seed as usize) % TAGS.len()];
        // candidates of this tag not yet chosen
        let cands: Vec<usize> = LIBRARY.iter().enumerate()
            .filter(|(i, d)| d.tag == tag && !chosen.contains(i)).map(|(i, _)| i).collect();
        if let Some(&pick) = cands.get((seed as usize + tag_round) % cands.len().max(1)).filter(|_| !cands.is_empty()) {
            chosen.push(pick);
        }
        tag_round += 1;
        // safety: if a full tag cycle adds nothing new, fall back to any remaining directive
        if tag_round % TAGS.len() == 0 && chosen.len() < n {
            if let Some(i) = (0..LIBRARY.len()).find(|i| !chosen.contains(i)) { chosen.push(i); } else { break; }
        }
    }
    chosen.into_iter().map(|i| &LIBRARY[i]).collect()
}

/// Compose `n` fan-out system prompts: each is the base system prompt followed by one directive. Cycles
/// the selected directives if `n` exceeds the distinct count (so you always get exactly `n` prompts, the
/// repeats getting their diversity from sampling temperature). Pairs each prompt with the directive name
/// so branches can be labelled in the UI.
pub fn fanout_prompts(base: &str, n: usize, seed: u64) -> Vec<(String, &'static str)> {
    let picks = select(n, seed);
    if picks.is_empty() { return (0..n).map(|_| (base.to_string(), "base")).collect(); }
    (0..n).map(|i| {
        let d = picks[i % picks.len()];
        let base = base.trim_end();
        let composed = if base.is_empty() { d.prompt.to_string() } else { format!("{base}\n\n{}", d.prompt) };
        (composed, d.name)
    }).collect()
}

/// Compose `n` fan-out USER prompts: each is one directive followed by the task. For the queue-based
/// fan-out (`:fuzzd`), which perturbs the user turn — the system prompt is built separately, so the
/// directive rides in the user message as an explicit "approach" the model will follow. Cycles
/// directives if `n` exceeds the distinct count. Pairs each with the directive name for UI labelling.
pub fn fanout_user(task: &str, n: usize, seed: u64) -> Vec<(String, &'static str)> {
    let picks = select(n, seed);
    let task = task.trim();
    if picks.is_empty() { return (0..n).map(|_| (task.to_string(), "base")).collect(); }
    (0..n).map(|i| {
        let d = picks[i % picks.len()];
        (format!("[Approach — {}]\n\n{task}", d.prompt), d.name)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_covers_every_tag() {
        for t in TAGS { assert!(LIBRARY.iter().any(|d| d.tag == t), "no directive for {t:?}"); }
        assert!(LIBRARY.len() >= 15);
    }

    #[test]
    fn select_returns_n_distinct() {
        let s = select(6, 0);
        assert_eq!(s.len(), 6);
        let names: std::collections::HashSet<_> = s.iter().map(|d| d.name).collect();
        assert_eq!(names.len(), 6, "directives must be distinct");
    }

    #[test]
    fn small_select_spreads_across_tags() {
        // 5 picks should hit all 5 tags (one axis each) before repeating an axis.
        let tags: std::collections::HashSet<_> = select(5, 3).iter().map(|d| d.tag).collect();
        assert_eq!(tags.len(), 5, "first 5 should cover all axes");
    }

    #[test]
    fn select_is_deterministic_and_seed_varies() {
        let a: Vec<_> = select(5, 7).iter().map(|d| d.name).collect();
        let b: Vec<_> = select(5, 7).iter().map(|d| d.name).collect();
        assert_eq!(a, b, "same seed → same selection");
        let c: Vec<_> = select(5, 8).iter().map(|d| d.name).collect();
        assert_ne!(a, c, "different seed → different selection");
    }

    #[test]
    fn select_caps_at_library_size() {
        assert_eq!(select(1000, 0).len(), LIBRARY.len());
    }

    #[test]
    fn fanout_prompts_compose_base_and_cycle() {
        let out = fanout_prompts("BASE SYS", 4, 1);
        assert_eq!(out.len(), 4);
        for (p, name) in &out {
            assert!(p.starts_with("BASE SYS\n\n"), "prompt must extend the base: {p}");
            assert!(!name.is_empty());
        }
        // distinct directives across the 4 (4 <= library size)
        let names: std::collections::HashSet<_> = out.iter().map(|(_, n)| *n).collect();
        assert_eq!(names.len(), 4);
        // more than the library → cycles, still returns exactly n
        assert_eq!(fanout_prompts("B", LIBRARY.len() + 5, 0).len(), LIBRARY.len() + 5);
    }

    #[test]
    fn empty_base_yields_bare_directive() {
        let out = fanout_prompts("", 1, 0);
        assert_eq!(out.len(), 1);
        assert!(!out[0].0.starts_with("\n"));
        assert!(LIBRARY.iter().any(|d| d.prompt == out[0].0));
    }

    #[test]
    fn fanout_user_carries_task_and_directive() {
        let out = fanout_user("write a sorter", 3, 2);
        assert_eq!(out.len(), 3);
        for (p, name) in &out {
            assert!(p.contains("write a sorter"), "task must be present: {p}");
            assert!(p.starts_with("[Approach"), "directive must lead: {p}");
            assert!(!name.is_empty());
        }
        let names: std::collections::HashSet<_> = out.iter().map(|(_, n)| *n).collect();
        assert_eq!(names.len(), 3, "distinct directives");
        assert_eq!(fanout_user("t", LIBRARY.len() + 3, 0).len(), LIBRARY.len() + 3);
    }
}
