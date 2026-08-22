//! workergen — stop the queen shipping the same worker with a different name.
//!
//! The observed failure is monotony: ask for a worker and get the same
//! competent, characterless instruction set every time. That is not a
//! creativity problem, it is a SAMPLING problem — the same prompt through the
//! same model lands in the same place, and asking it to "be more varied" moves
//! the mean rather than the spread.
//!
//! So variety comes from outside the model. Two sources:
//!
//! **Commons are recombined from a corpus.** Words and phrases from books and
//! a dictionary, glued into instruction sets that are strange but structurally
//! sound. Most are ordinary; a few are odd in a way nobody would have written
//! deliberately, and those are the point.
//!
//! **Legendaries are hand-tuned.** A worker that thinks like a storyboarder, a
//! combinatorialist, a condensed-matter physicist. These are not generated,
//! because the value in them is exactly the thing generation cannot supply —
//! someone who knows what a storyboarder actually does wrote it down.
//!
//! ## Strange is the goal; incoherent is not
//!
//! A worker whose instructions contradict each other is not a rare drop, it is
//! a broken one, and it will burn a whole task before anyone notices. So
//! [`Worker::is_coherent`] is a real gate: a generated worker that fails it is
//! rerolled, not shipped. The test for this module is not "does it produce
//! variety" — that is easy and useless — it is "does it produce variety that
//! still parses as a job".

/// How rare, and therefore where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rarity {
    /// Recombined from the corpus. Most workers.
    Common,
    /// Recombined, but with an unusual pairing that survived the coherence
    /// gate.
    Rare,
    /// Hand-written. Never generated, because the value is the human judgement
    /// in it.
    Legendary,
}

impl Rarity {
    pub fn label(self) -> &'static str {
        match self {
            Rarity::Common => "common",
            Rarity::Rare => "rare",
            Rarity::Legendary => "legendary",
        }
    }
}

/// A worker the queen can dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worker {
    pub name: String,
    pub rarity: Rarity,
    /// The lens it thinks through — "as a storyboarder", "as a
    /// combinatorialist". The single most load-bearing line.
    pub stance: String,
    /// What it always does, in order.
    pub habits: Vec<String>,
    /// What it refuses. A worker with no refusals will do anything asked,
    /// which is the same as having no stance at all.
    pub refusals: Vec<String>,
}

impl Worker {
    /// Is this a job, or word salad?
    ///
    /// Cheap structural checks, because the expensive ones need a model and
    /// this gate runs on every reroll. It catches the failures that actually
    /// occur in recombination: nothing to do, nothing refused, a habit that is
    /// also a refusal, and duplicate habits — which is what a naive shuffle
    /// produces most often.
    pub fn is_coherent(&self) -> Result<(), String> {
        if self.stance.trim().is_empty() {
            return Err("no stance — the lens is the worker".into());
        }
        if self.habits.is_empty() {
            return Err("no habits — nothing it always does".into());
        }
        if self.refusals.is_empty() {
            return Err("no refusals — a worker that will do anything has no stance".into());
        }
        for h in &self.habits {
            if self.refusals.iter().any(|r| overlaps(h, r)) {
                return Err(format!("habit and refusal collide: {h:?}"));
            }
        }
        let mut seen: Vec<&str> = Vec::new();
        for h in &self.habits {
            let k = h.trim();
            if seen.contains(&k) {
                return Err(format!("duplicate habit: {k:?}"));
            }
            seen.push(k);
        }
        Ok(())
    }

    /// The instruction block a runtime would hand a model.
    pub fn render(&self) -> String {
        let mut s = format!("You work {}.\n\nAlways:\n", self.stance);
        for h in &self.habits { s.push_str(&format!("- {h}\n")); }
        s.push_str("\nNever:\n");
        for r in &self.refusals { s.push_str(&format!("- {r}\n")); }
        s
    }
}

/// Do two instructions plausibly contradict?
///
/// A word-overlap heuristic, not comprehension. It catches "always cite
/// sources" against "never cite sources", which is what recombination actually
/// produces; it will not catch a subtle contradiction, and pretending
/// otherwise would be worse than the honest limit.
fn overlaps(a: &str, b: &str) -> bool {
    let key = |s: &str| -> Vec<String> {
        s.to_lowercase().split_whitespace()
            .filter(|w| w.len() > 3)
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
            .filter(|w| !w.is_empty())
            .collect()
    };
    let (ka, kb) = (key(a), key(b));
    if ka.is_empty() || kb.is_empty() { return false; }
    let shared = ka.iter().filter(|w| kb.contains(w)).count();
    // Two of the content words in common is a collision; one is a coincidence
    // ("the code" appears everywhere).
    shared >= 2
}

/// The corpus a common worker is built from.
///
/// Fragments rather than whole sentences, because whole sentences from a book
/// read as quotation and fragments read as instruction. The caller supplies
/// them — this module does not ship a dictionary.
#[derive(Debug, Clone, Default)]
pub struct Corpus {
    /// Nouns for a stance: "cartographer", "archivist".
    pub roles: Vec<String>,
    /// Manners: "impatiently", "from the margins".
    pub manners: Vec<String>,
    /// Verb phrases for habits: "name the assumption", "measure before
    /// arguing".
    pub actions: Vec<String>,
    /// Things to refuse: "guess at a number", "answer before reading".
    pub taboos: Vec<String>,
}

impl Corpus {
    pub fn is_usable(&self) -> Result<(), String> {
        for (n, v) in [("roles", &self.roles), ("manners", &self.manners),
                       ("actions", &self.actions), ("taboos", &self.taboos)] {
            if v.len() < 2 {
                return Err(format!(
                    "corpus needs at least 2 {n}; with fewer, every worker is the same one \
and the generator is theatre"));
            }
        }
        Ok(())
    }
}

/// Deterministic, seedable mixing. Not cryptographic — reproducibility is the
/// requirement, because a worker that cannot be regenerated from its seed
/// cannot be reported in a bug.
fn mix(seed: u64, salt: u64) -> u64 {
    let mut h = seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 30; h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27; h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

fn pick<'a>(v: &'a [String], seed: u64, salt: u64) -> &'a str {
    &v[(mix(seed, salt) % v.len() as u64) as usize]
}

/// Generate a worker from a seed.
///
/// The seed IS the worker's identity: the same seed gives the same worker
/// forever, so one can be referred to, reported and reproduced. A generator
/// whose output cannot be named again is one nobody can file a bug against.
///
/// Rerolls internally on an incoherent draw — up to a bound, because a corpus
/// that cannot produce a coherent worker should say so rather than spin.
pub fn generate(c: &Corpus, seed: u64) -> Result<Worker, String> {
    c.is_usable()?;
    for attempt in 0..8u64 {
        let s = mix(seed, attempt);
        let role = pick(&c.roles, s, 1);
        let manner = pick(&c.manners, s, 2);
        let habits: Vec<String> = (0..3)
            .map(|i| pick(&c.actions, s, 10 + i).to_string())
            .collect();
        let refusals: Vec<String> = (0..2)
            .map(|i| pick(&c.taboos, s, 20 + i).to_string())
            .collect();
        // Dedupe rather than reroll on it: a repeated pick is the common case
        // with a small corpus and rerolling the whole worker for it would make
        // small corpora fail far more often than they need to.
        let habits = dedupe(habits);
        let refusals = dedupe(refusals);

        // RARE is an unusual PAIRING, not a rarer dice roll. A stance whose
        // role and manner come from distant parts of the corpus is the odd
        // combination worth keeping — "an archivist, impatiently" is a
        // different worker from "an archivist, thoroughly".
        let distance = (mix(s, 1) % c.roles.len() as u64)
            .abs_diff(mix(s, 2) % c.manners.len() as u64);
        let rarity = if distance >= (c.roles.len().max(c.manners.len()) as u64 * 2 / 3).max(1) {
            Rarity::Rare
        } else {
            Rarity::Common
        };

        let w = Worker {
            name: format!("{}-{:04x}", role.split_whitespace().next().unwrap_or("worker"),
                          (s & 0xffff) as u16),
            rarity,
            stance: format!("as {role}, {manner}"),
            habits,
            refusals,
        };
        if w.is_coherent().is_ok() { return Ok(w); }
    }
    Err("no coherent worker in 8 draws — the corpus contradicts itself".into())
}

fn dedupe(v: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in v { if !out.contains(&x) { out.push(x); } }
    out
}

/// The hand-tuned roster.
///
/// Not generated, and that is the whole point: what makes these worth having
/// is that someone who knows what the job involves wrote down how it thinks.
/// A recombinator cannot produce "block the sequence before rendering a
/// frame", because that is knowledge about storyboarding rather than about
/// words.
pub fn legendaries() -> Vec<Worker> {
    vec![
        Worker {
            name: "storyboarder".into(), rarity: Rarity::Legendary,
            stance: "as a storyboarder".into(),
            habits: vec![
                "block the whole sequence in thumbnails before rendering any single frame".into(),
                "ask what the viewer knows at this moment, and what they learn next".into(),
                "cut to the shot that carries information, not the shot that looks best".into(),
            ],
            refusals: vec![
                "polish one panel while the sequence is unresolved".into(),
                "describe a beat you cannot draw".into(),
            ],
        },
        Worker {
            name: "art-lead".into(), rarity: Rarity::Legendary,
            stance: "as an art lead".into(),
            habits: vec![
                "state the visual rule first, then apply it — a style is a constraint, not a mood"
                    .into(),
                "name the one thing that must read at a glance, and protect it".into(),
                "check the worst-case surface: the smallest screen, the darkest theme".into(),
            ],
            refusals: vec![
                "approve a look you cannot describe as a rule someone else could follow".into(),
                "let consistency lose to a single striking exception".into(),
            ],
        },
        Worker {
            name: "combinatorialist".into(), rarity: Rarity::Legendary,
            stance: "as a combinatorialist".into(),
            habits: vec![
                "count the cases before reasoning about them, and say which are symmetric".into(),
                "look for the bijection: a hard count is usually an easy one in disguise".into(),
                "check the smallest nontrivial case by hand before trusting the general argument"
                    .into(),
            ],
            refusals: vec![
                "assert a bound without the construction or the proof it is tight".into(),
                "reason about an average when the tail is what matters".into(),
            ],
        },
        Worker {
            name: "condensed-matter".into(), rarity: Rarity::Legendary,
            stance: "as a condensed-matter physicist".into(),
            habits: vec![
                "identify the order parameter and what breaks the symmetry".into(),
                "check the limits — zero temperature, infinite size — before the middle".into(),
                "estimate the scale first; a factor of a thousand is a different problem".into(),
            ],
            refusals: vec![
                "trust a simulation whose limits you have not checked against theory".into(),
                "call an effect emergent before ruling out the boring explanation".into(),
            ],
        },
        Worker {
            name: "graphic-designer".into(), rarity: Rarity::Legendary,
            stance: "as a graphic designer".into(),
            habits: vec![
                "set the hierarchy before the palette: what is read first, second, not at all"
                    .into(),
                "remove one element and check whether anything was lost".into(),
                "align to a grid you can state, so a stranger can extend the work".into(),
            ],
            refusals: vec![
                "add an element without saying what it does for the reader".into(),
                "use motion or colour to carry meaning that has no fallback".into(),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Corpus {
        Corpus {
            roles: ["cartographer", "archivist", "surveyor", "printer", "glassblower"]
                .iter().map(|s| s.to_string()).collect(),
            manners: ["impatiently", "from the margins", "in the open", "very slowly"]
                .iter().map(|s| s.to_string()).collect(),
            actions: ["name the assumption before using it",
                      "measure before arguing",
                      "write the failing case down first",
                      "say which part you are least sure of",
                      "read the whole thing once before touching any of it"]
                .iter().map(|s| s.to_string()).collect(),
            taboos: ["guess at a number you could look up",
                     "answer before reading the input",
                     "call something done without checking it"]
                .iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn the_same_seed_is_always_the_same_worker() {
        // The seed IS the identity. A generator whose output cannot be named
        // again is one nobody can file a bug against.
        let c = corpus();
        let a = generate(&c, 12345).unwrap();
        let b = generate(&c, 12345).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_are_actually_different_workers() {
        // The failure this module exists for: the same competent,
        // characterless worker every time. Variety measured on the STANCE,
        // which is the load-bearing line — varying only the name would be the
        // same monotony with a new label.
        let c = corpus();
        let stances: std::collections::HashSet<String> =
            (0..60u64).filter_map(|s| generate(&c, s).ok()).map(|w| w.stance).collect();
        assert!(stances.len() >= 10, "only {} distinct stances in 60 draws", stances.len());
    }

    #[test]
    fn every_generated_worker_is_a_job_and_not_word_salad() {
        // Strange is the goal; incoherent is not. A worker whose instructions
        // contradict each other burns a whole task before anyone notices.
        let c = corpus();
        for s in 0..200u64 {
            let w = generate(&c, s).unwrap();
            w.is_coherent().unwrap_or_else(|e| panic!("seed {s}: {e}\n{}", w.render()));
        }
    }

    #[test]
    fn a_habit_that_is_also_a_refusal_is_caught() {
        // What recombination actually produces, as opposed to subtle
        // contradiction: the same instruction on both lists.
        let w = Worker {
            name: "x".into(), rarity: Rarity::Common, stance: "as a tester".into(),
            habits: vec!["measure before arguing".into()],
            refusals: vec!["measure before arguing".into()],
        };
        let e = w.is_coherent().unwrap_err();
        assert!(e.contains("collide"), "{e}");
    }

    #[test]
    fn a_worker_with_no_refusals_is_refused() {
        // A worker that will do anything asked has no stance, which makes it
        // the generic worker this module exists to stop shipping.
        let w = Worker {
            name: "x".into(), rarity: Rarity::Common, stance: "as a tester".into(),
            habits: vec!["do the thing".into()], refusals: vec![],
        };
        assert!(w.is_coherent().unwrap_err().contains("no refusals"));
    }

    #[test]
    fn a_thin_corpus_says_so_instead_of_producing_one_worker_forever() {
        // With one role and one manner every draw is identical and the
        // generator is theatre. Better to refuse than to look like it works.
        let thin = Corpus {
            roles: vec!["archivist".into()], manners: vec!["slowly".into()],
            actions: vec!["read".into(), "write".into()],
            taboos: vec!["guess".into(), "assume".into()],
        };
        let e = generate(&thin, 1).unwrap_err();
        assert!(e.contains("at least 2 roles"), "{e}");
        assert!(e.contains("theatre"), "the message should say why: {e}");
    }

    #[test]
    fn rare_is_an_unusual_pairing_not_a_rarer_dice_roll() {
        // A rarity tier that is just a lower probability adds nothing — the
        // worker is the same, drawn less often. Rare here means the role and
        // the manner came from distant parts of the corpus, which is what
        // makes "an archivist, impatiently" worth keeping.
        let c = corpus();
        let ws: Vec<Worker> = (0..200u64).filter_map(|s| generate(&c, s).ok()).collect();
        let rare = ws.iter().filter(|w| w.rarity == Rarity::Rare).count();
        assert!(rare > 0, "no rare workers in 200 draws");
        assert!(rare < ws.len(), "everything was rare, so the tier means nothing");
    }

    #[test]
    fn the_legendaries_carry_knowledge_a_recombinator_could_not_invent() {
        // The reason they are hand-written. "Block the sequence before
        // rendering a frame" is knowledge about storyboarding, not about
        // words, and no shuffle of a dictionary produces it.
        let l = legendaries();
        assert!(l.len() >= 5);
        for w in &l {
            assert_eq!(w.rarity, Rarity::Legendary);
            w.is_coherent().unwrap_or_else(|e| panic!("{}: {e}", w.name));
            assert!(w.habits.len() >= 3, "{} is too thin to be legendary", w.name);
        }
        let sb = l.iter().find(|w| w.name == "storyboarder").unwrap();
        assert!(sb.render().contains("thumbnails"));
        // And they are never produced by the generator.
        let c = corpus();
        let names: Vec<String> = (0..200u64).filter_map(|s| generate(&c, s).ok())
            .map(|w| w.name).collect();
        for w in &l {
            assert!(!names.contains(&w.name), "{} was generated", w.name);
        }
    }

    #[test]
    fn the_rendered_block_reads_as_instructions() {
        let c = corpus();
        let w = generate(&c, 7).unwrap();
        let r = w.render();
        assert!(r.starts_with("You work as "), "{r}");
        assert!(r.contains("Always:") && r.contains("Never:"), "{r}");
    }
}
