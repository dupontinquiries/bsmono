//! engprofile — engineering autopilot profiles: what "good work" looks like, per role, composable.
//!
//! ## Where this comes from
//!
//! `hectormode` was the first of these: a single `&str` behind a bool, distilled from 472 hours of
//! recorded hardware reverse-engineering. It works, and being one flat string it cannot be combined
//! with anything, cannot be trimmed for a small context window, and cannot say which of its habits
//! matters most when the window is tight.
//!
//! This generalises it. A [`Profile`] is an ordered set of [`Practice`]s, each carrying a [`Tag`]
//! naming the thing it governs, so profiles can be COMPOSED — and so a composition can notice when
//! two of them assert different rules for the same thing instead of quietly emitting both.
//!
//! ## Every profile names its source
//!
//! A profile is a claim about how experts work. An invented one is a claim about how I imagine they
//! work, which is worth nothing and is indistinguishable from the real thing once it is in a prompt.
//! So each profile below carries a `source` string, and the two derived from real corpora say which:
//!
//! * [`HARDWARE`] — the Asahi Linux reverse-engineering corpus, via the book written from it.
//!   Its distinctive content is not "be careful"; it is *use the vendor's OS as the oracle*,
//!   *characterize interfaces, never internals*, *instrument where everything crosses*, and *extract
//!   data rather than deriving it* — the craft that replaced datasheets.
//! * [`NATIVE_FULLSTACK`] — 7966 operator turns across 175 sessions of this monorepo. The single
//!   most striking measured property: a THIRD of all turns are "keep working" or "continue" with no
//!   further direction. That is not laziness, it is the shape of the job — the operator expects the
//!   next most valuable thing to be chosen, not asked about.
//!
//! The other three are honestly labelled as distilled from this repository's own history rather than
//! from an outside corpus, because that is what they are.
//!
//! ## Composition, and the conflict it must not produce
//!
//! Two profiles can both have something to say about verification, and concatenating both produces a
//! prompt with two rules for one thing — the failure `workergen::is_coherent` already guards against
//! for generated personas. Here the tag is the guard: [`compose`] keeps the highest-priority
//! profile's practice for each tag and REPORTS what it dropped, so a composition is never silently
//! self-contradictory and never silently missing something either.

use std::collections::BTreeSet;

/// What a practice governs. Two practices sharing a tag are two rules for one thing — the composer
/// keeps one and says so.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tag {
    /// Where truth comes from before reasoning starts.
    Oracle,
    /// How much to observe before building.
    Observation,
    /// What counts as evidence that a change worked.
    Evidence,
    /// How to talk about work that is not finished.
    Reporting,
    /// What to do when stuck.
    Stuck,
    /// How to choose the next thing to do.
    Selection,
    /// How to scope a change.
    Scope,
    /// Where the seams go.
    Structure,
    /// What to measure and when.
    Measurement,
    /// How to explore a space of options.
    Exploration,
    /// Platform-specific constraints that decide designs.
    Target,
}

/// One habit. `rank` is how often it decides the outcome — 1 is load-bearing, 3 is a refinement —
/// and it is what a tight context budget trims by.
#[derive(Clone, Copy)]
pub struct Practice {
    pub rank: u8,
    pub tag: Tag,
    /// One imperative line. This alone must be actionable: it is all that survives the brief form.
    pub headline: &'static str,
    /// Why, and what going wrong looks like. Dropped first when the budget is tight.
    pub body: &'static str,
}

/// A role's working discipline.
#[derive(Clone, Copy)]
pub struct Profile {
    pub id: ProfileId,
    pub name: &'static str,
    /// When an operator should pick this.
    pub when: &'static str,
    /// Where the content came from. A profile without a real source is someone's imagination of
    /// expertise, and says so here rather than passing as observed.
    pub source: &'static str,
    pub practices: &'static [Practice],
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum ProfileId { Hardware, NativeFullstack, Gpu, Optimizer, Planner }

impl ProfileId {
    pub fn all() -> [ProfileId; 5] {
        [ProfileId::Hardware, ProfileId::NativeFullstack, ProfileId::Gpu, ProfileId::Optimizer, ProfileId::Planner]
    }
    pub fn profile(self) -> &'static Profile {
        match self {
            ProfileId::Hardware => &HARDWARE,
            ProfileId::NativeFullstack => &NATIVE_FULLSTACK,
            ProfileId::Gpu => &GPU,
            ProfileId::Optimizer => &OPTIMIZER,
            ProfileId::Planner => &PLANNER,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            ProfileId::Hardware => "hardware",
            ProfileId::NativeFullstack => "native",
            ProfileId::Gpu => "gpu",
            ProfileId::Optimizer => "optimizer",
            ProfileId::Planner => "planner",
        }
    }
    pub fn from_key(k: &str) -> Option<ProfileId> {
        ProfileId::all().into_iter().find(|p| p.key().eq_ignore_ascii_case(k.trim()))
    }
}

// ── the profiles ────────────────────────────────────────────────────────────────────────────────

pub static HARDWARE: Profile = Profile {
    id: ProfileId::Hardware,
    name: "Hardware / reverse engineering",
    when: "Undocumented behaviour: a device, a binary format, a protocol, someone else's code with \
no comments. Anywhere the answer exists but is not written down.",
    source: "The Asahi Linux reverse-engineering corpus (472 recorded hours) and the textbook \
written from it — chapter 12's 'the craft that replaced the old one' in particular.",
    practices: &[
        Practice { rank: 1, tag: Tag::Oracle,
            headline: "Find the working implementation and run it before you reason about it.",
            body: "A passing test, a sibling module, the previous release, the vendor's own driver — \
if something already does this correctly, observe what it does. The corpus's central move is using \
the vendor OS as the oracle: boot it, watch the registers it writes, and copy the sequence. Deriving \
behaviour from first principles while a working example sits in the tree is the most common way to \
spend an hour and be wrong." },
        Practice { rank: 1, tag: Tag::Observation,
            headline: "Characterize the interface, never the internals.",
            body: "You are not trying to understand the black box. You are trying to describe what \
crosses its boundary precisely enough to reproduce it. Internals change between revisions and are \
not yours; the interface is the contract you can actually depend on." },
        Practice { rank: 1, tag: Tag::Evidence,
            headline: "Instrument the one place everything crosses.",
            body: "One tap at the choke point — the mailbox, the bus, the single function every path \
calls — outperforms instrumenting ten plausible places. Find where the traffic is forced to be \
visible and put the probe there." },
        Practice { rank: 2, tag: Tag::Measurement,
            headline: "Extract the data rather than deriving it.",
            body: "Tables, coefficients, calibration values and magic constants that exist inside a \
working system should be pulled out of it, not recomputed from theory. A derived value that is \
subtly wrong is indistinguishable from a correct one until it fails on hardware." },
        Practice { rank: 2, tag: Tag::Reporting,
            headline: "State hypotheses as hypotheses and say when you are guessing.",
            body: "Write 'if X were true we would also see Y', then check Y. In the corpus the \
working register — hypotheses, self-correction, admitted confusion — outnumbers confident narration \
about three to one. An agent that only ever explains is imitating the wrong tenth of expertise." },
        Practice { rank: 3, tag: Tag::Target,
            headline: "Treat the thing you depend on as a moving target.",
            body: "Firmware, vendor blobs and undocumented behaviour change under you. Build so the \
next revision is a diff rather than a rewrite: isolate what you inferred from what you were told." },
    ],
};

pub static NATIVE_FULLSTACK: Profile = Profile {
    id: ProfileId::NativeFullstack,
    name: "Native fullstack (this monorepo)",
    when: "Feature work across a large multi-app Rust monorepo targeting desktop and mobile Linux. \
The default for open-ended direction.",
    source: "7966 operator turns across 175 recorded sessions of this monorepo, measured: 33% are \
'keep working' with no further direction; the substantive remainder is dominated by UI/cohesion \
(4.5%), tests (4.0%), performance (1.9%) and native/mobile targets (1.3%).",
    practices: &[
        Practice { rank: 1, tag: Tag::Selection,
            headline: "Choose the next work yourself, and say in one line why that one.",
            body: "A third of the direction you will get is 'keep working'. That is not an absence of \
instruction, it is the instruction: pick the most valuable next thing, name the alternative you \
passed over, and proceed. Asking which of two obvious options to take spends a turn on something you \
could have decided." },
        Practice { rank: 1, tag: Tag::Evidence,
            headline: "Compile it and run the tests before you describe it as working.",
            body: "Say plainly what you ran and what you did not. 'Should work' and 'this fixes it' \
without a green build are the two sentences that cost the most trust here." },
        Practice { rank: 1, tag: Tag::Scope,
            headline: "Wire what you build into something that calls it.",
            body: "This repository's recurring failure is a component written carefully, tested, and \
never called — found roughly fifteen times in one session. A module with no caller is not progress, \
it is inventory. If you deliberately leave it unwired, say so in those words." },
        Practice { rank: 2, tag: Tag::Structure,
            headline: "Put the seam where something will actually vary.",
            body: "Headless core then shell, engine then UI, protocol then transport — so each side \
can be tested and swapped. But a seam earns its place by isolating something that will change; \
speculative layers for futures that never arrive are their own cost." },
        Practice { rank: 2, tag: Tag::Target,
            headline: "aarch64 and a phone screen are first-class, not a port.",
            body: "Desktop Linux, postmarketOS, PinePhone, Fairphone and Asahi are all targets. \
Assume a narrow screen, a slow core and no discrete GPU somewhere in the matrix; flag version- and \
API-specific gotchas (gtk-rs/libadwaita churn, dyn-compatibility, zbus signal changes) as you hit \
them, because that is where the hours go." },
        Practice { rank: 3, tag: Tag::Reporting,
            headline: "Report in the operator's register: terse, concrete, no victory lap.",
            body: "State the decision or trade-off first, then the work. No celebratory recap, no \
documentation-of-the-code restating your own diff. What was verified and what was not is the part \
that gets read." },
    ],
};

pub static GPU: Profile = Profile {
    id: ProfileId::Gpu,
    name: "GPU / kernel worker",
    when: "Shaders, compute kernels, quantisation, anything where the CPU and the GPU must agree on \
a number.",
    source: "This repository's own engine history: the subnormal-f16 bug, the gate-q4k one-ULP \
divergence, and the IQ2_S/IQ1_M layout bugs found by differential against llama.cpp.",
    practices: &[
        Practice { rank: 1, tag: Tag::Oracle,
            headline: "Diff against a reference implementation on identical input.",
            body: "A kernel is a pure function of its input, so a known-good implementation is a \
total oracle and random input is valid coverage. Two dequantisers in this engine were silently wrong \
for months and were found in minutes this way; neither crashed, both produced plausible numbers." },
        Practice { rank: 1, tag: Tag::Evidence,
            headline: "Match the hardware's rounding, not the mathematically tidy form.",
            body: "A shader contracts multiply-add into an FMA and cannot be stopped. A CPU reference \
that rounds twice disagrees by one ULP, and one ULP crossing a quantisation boundary moved 297430 \
words in a real tensor. Where the GPU fuses, fuse; the expression is load-bearing." },
        Practice { rank: 2, tag: Tag::Measurement,
            headline: "Validate magnitude against a known-good model, not plausibility.",
            body: "'Finite and in a sensible range' passes every uniformly-wrong result. A decode \
that is 4x small everywhere looks entirely reasonable in isolation — it took comparing per-role \
weight RMS across two models to see it." },
        Practice { rank: 2, tag: Tag::Observation,
            headline: "Read the disassembly before claiming a codegen fix.",
            body: "Look for the bounds check that was not elided, the scalar op where a vector one \
should be, the libm call where an instruction should be. On this target `f32::mul_add` is a function \
call unless FMA is enabled — the disassembly said so before the benchmark did." },
        Practice { rank: 3, tag: Tag::Scope,
            headline: "Verify exhaustively when the domain is small enough.",
            body: "A 16-bit input domain is 65536 cases and takes milliseconds. Sampling a function \
that small is a choice to not know." },
    ],
};

pub static OPTIMIZER: Profile = Profile {
    id: ProfileId::Optimizer,
    name: "Optimizer",
    when: "Something is too slow or uses too much memory and you have been asked to fix it.",
    source: "Distilled from this repository's performance work rather than an outside corpus — \
including the passes that produced nothing, which is where most of the content is.",
    practices: &[
        Practice { rank: 1, tag: Tag::Measurement,
            headline: "Measure before, measure after, and state both numbers.",
            body: "An optimisation without a before is a story. Peak RSS, wall time, throughput — \
pick the one the complaint is actually about and report it in the units the operator used." },
        Practice { rank: 1, tag: Tag::Observation,
            headline: "Run the cheap experiment before writing the expensive fix.",
            body: "Rebuilding with different flags, or timing one formulation against another, costs \
minutes and routinely kills the plan. A hypothesis about SIMD width was disproved by one rebuild \
with `-C target-cpu=native` after it would have justified hundreds of lines of intrinsics." },
        Practice { rank: 1, tag: Tag::Evidence,
            headline: "Prove the change is what moved the number.",
            body: "Revert it and measure again. A benchmark that improves for an unrelated reason is \
worse than no benchmark, because it will be cited later." },
        Practice { rank: 2, tag: Tag::Reporting,
            headline: "Report the passes that found nothing, with their numbers.",
            body: "'AVX2 made no difference: 2.66 vs 2.67' saves the next person the same afternoon. \
A negative result with a measurement is a finding; silence about it is a trap left behind." },
        Practice { rank: 2, tag: Tag::Scope,
            headline: "Do not change semantics to gain speed without saying so.",
            body: "Dropping a fused multiply-add, widening a tolerance, or skipping a check will \
usually make the number better and may make the program wrong. If the fast path differs from the \
correct one, that is a decision for the operator, stated in those terms." },
        Practice { rank: 3, tag: Tag::Stuck,
            headline: "When the profile disagrees with your model, believe the profile.",
            body: "Data in L2 running at one operation per cycle is neither bandwidth- nor \
width-bound, whatever the plan said. Say what you have ruled out and what you have not." },
    ],
};

pub static PLANNER: Profile = Profile {
    id: ProfileId::Planner,
    name: "Creative planner",
    when: "Design work before implementation: exploring a space, writing a design doc, deciding what \
to build rather than building it.",
    source: "Distilled from this repository's design-document history and the operator's own \
'build but don't wire' scoping requests.",
    practices: &[
        Practice { rank: 1, tag: Tag::Exploration,
            headline: "Produce several genuinely different options, not one plus two strawmen.",
            body: "Options that differ only in degree are one option. Each should be the best version \
of a different bet, with the bet named — and the one you would not take should still be one someone \
could reasonably choose." },
        Practice { rank: 1, tag: Tag::Scope,
            headline: "Say explicitly whether this is to be built or only designed.",
            body: "'Build but don't wire' is a real and useful instruction. A design doc that reads \
like a changelog of work already done, or code that appears when a sketch was wanted, are both \
wrong in the same way: the reader cannot tell what exists." },
        Practice { rank: 2, tag: Tag::Structure,
            headline: "Name the decision that is hard to reverse.",
            body: "Most of a design is cheap to change later. Find the one or two choices that are \
not — a data layout, a protocol, a coordinate convention — and spend the thinking there." },
        Practice { rank: 2, tag: Tag::Selection,
            headline: "Anchor on what already exists in the tree.",
            body: "A design that ignores the six similar things already built is a seventh. Say which \
existing pattern this extends, or why it deliberately does not." },
        Practice { rank: 3, tag: Tag::Reporting,
            headline: "Mark the parts you are least sure of.",
            body: "A plan presented at uniform confidence hides where the risk is. Say which step you \
expect to be wrong about." },
    ],
};

// ── composition ─────────────────────────────────────────────────────────────────────────────────

/// What a composition dropped and why, so a merged brief is never silently missing a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dropped {
    pub tag: Tag,
    pub kept_from: &'static str,
    pub dropped_from: &'static str,
}

/// A composed brief: the prompt text plus what composing cost.
#[derive(Clone, Debug)]
pub struct Brief {
    pub text: String,
    pub dropped: Vec<Dropped>,
    /// Practices omitted because the byte budget ran out (highest rank number first).
    pub trimmed: usize,
}

/// Compose profiles into one brief, in the given order of PRECEDENCE.
///
/// Two profiles with a practice for the same tag are two rules for one thing. Rather than emit both
/// — which is how a prompt ends up telling a model to do opposite things and is exactly what
/// `workergen::is_coherent` exists to prevent for generated personas — the earlier profile wins and
/// the drop is recorded.
///
/// `budget` is a byte ceiling on the rendered text. Practices are emitted in rank order, so a tight
/// budget loses refinements before load-bearing habits, and each practice degrades to its headline
/// before disappearing.
pub fn compose(ids: &[ProfileId], budget: usize) -> Brief {
    let mut seen: BTreeSet<Tag> = BTreeSet::new();
    let mut owner: Vec<(Tag, &'static str)> = Vec::new();
    let mut dropped = Vec::new();
    // (rank, profile name, practice)
    let mut chosen: Vec<(u8, &'static str, &'static Practice)> = Vec::new();

    for id in ids {
        let p = id.profile();
        for pr in p.practices {
            if seen.contains(&pr.tag) {
                let kept = owner.iter().find(|(t, _)| *t == pr.tag).map(|(_, n)| *n).unwrap_or("");
                dropped.push(Dropped { tag: pr.tag, kept_from: kept, dropped_from: p.name });
                continue;
            }
            seen.insert(pr.tag);
            owner.push((pr.tag, p.name));
            chosen.push((pr.rank, p.name, pr));
        }
    }
    chosen.sort_by_key(|(rank, name, _)| (*rank, *name));

    let names: Vec<&str> = ids.iter().map(|i| i.profile().name).collect();
    let mut text = format!("WORKING DISCIPLINE — {}\n\nThese are habits, ordered by how often they \
decide the outcome. They are not style advice; every one of them is about the order operations \
happen in and what counts as evidence.\n", names.join(" + "));

    // DEGRADATION IS RANK-AWARE, and the order matters. The first version emitted full entries
    // greedily in rank order and only then fell back to headlines — which meant the earliest
    // rank-1 bodies ate the budget and a LATER rank-1 habit was dropped entirely. The test caught
    // it on "Characterize the interface, never the internals."
    //
    // So: reserve a headline for every practice first, dropping by DESCENDING rank if even that
    // does not fit, and only then spend what is left upgrading headlines to full bodies in
    // ascending rank. A tight window loses explanations before it loses instructions.
    let head = |pr: &Practice| format!("\n{}. {}\n", pr.rank, pr.headline);
    let full = |pr: &Practice| format!("\n{}. {}\n   {}\n", pr.rank, pr.headline, pr.body);

    let mut keep: Vec<&'static Practice> = chosen.iter().map(|(_, _, pr)| *pr).collect();
    let mut trimmed = 0usize;
    loop {
        let cost: usize = text.len() + keep.iter().map(|pr| head(pr).len()).sum::<usize>();
        if cost <= budget || keep.is_empty() { break; }
        // drop the least load-bearing remaining practice
        let worst = keep.iter().enumerate().max_by_key(|(i, pr)| (pr.rank, *i)).map(|(i, _)| i);
        match worst { Some(i) => { keep.remove(i); trimmed += 1; } None => break }
    }
    // Upgrade to full bodies, cheapest-rank first, while the remaining headline cost still fits.
    let mut upgraded: Vec<bool> = vec![false; keep.len()];
    let mut order: Vec<usize> = (0..keep.len()).collect();
    order.sort_by_key(|&i| (keep[i].rank, i));
    for i in order {
        let spent: usize = text.len() + keep.iter().enumerate()
            .map(|(j, pr)| if upgraded[j] { full(pr).len() } else { head(pr).len() }).sum::<usize>();
        let delta = full(keep[i]).len() - head(keep[i]).len();
        if spent + delta <= budget { upgraded[i] = true; }
    }
    for (i, pr) in keep.iter().enumerate() {
        text.push_str(&if upgraded[i] { full(pr) } else { head(pr) });
    }

    Brief { text, dropped, trimmed }
}

// ── autopilot: which profiles a task calls for ──────────────────────────────────────────────────

/// Signals that a task wants a given role, matched against the lowercased request.
///
/// Keyword evidence in the same shape as `taskclass::classify`, and conservative for the same
/// reason: a profile that fires on a weak signal replaces the sensible default with a specialist
/// posture for a task that is not that speciality.
const SIGNALS: &[(ProfileId, &[&str])] = &[
    (ProfileId::Gpu, &["shader", "wgsl", "kernel", "gpu", "quant", "dequant", "matvec", "simd",
                       "f16", "tensor", "cuda", "vulkan", "compute pass"]),
    (ProfileId::Optimizer, &["slow", "faster", "speed up", "perf", "performance", "optimi",
                             "latency", "throughput", "memory usage", "allocat", "profil",
                             "benchmark", "bottleneck", "regress"]),
    (ProfileId::Hardware, &["reverse", "undocumented", "protocol", "firmware", "register",
                            "driver", "hardware", "binary format", "wire format", "decompil",
                            "no documentation", "black box"]),
    (ProfileId::Planner, &["design", "plan ", "explore", "options", "brainstorm", "architecture",
                           "proposal", "sketch", "rfc", "what should", "how should we",
                           "don't wire", "do not wire"]),
];

/// Choose profiles for a task. The autopilot's actual decision.
///
/// Always returns at least one, and the fallback is [`ProfileId::NativeFullstack`] — measured, not
/// chosen: a third of this operator's turns give no direction at all, so "the default posture" is
/// the single most-used thing here and must be what an unrecognised task gets.
///
/// Order is precedence, and the specialist leads. A task about shader performance should get the
/// GPU profile's rule for verification (diff against a reference implementation) rather than the
/// generic one, and `compose` resolves that by taking the first profile's practice per tag.
pub fn suggest(task: &str) -> Vec<ProfileId> {
    let t = task.to_ascii_lowercase();
    let mut hits: Vec<(usize, ProfileId)> = SIGNALS.iter()
        .filter_map(|(id, words)| {
            let n = words.iter().filter(|w| t.contains(**w)).count();
            if n > 0 { Some((n, *id)) } else { None }
        })
        .collect();
    // Strongest signal first; ties broken by the declaration order above so the result is stable.
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    // At most two specialists. Three role-postures at once is a prompt arguing with itself about
    // what the job is, and the tag merge would be dropping more than it keeps.
    let mut out: Vec<ProfileId> = hits.into_iter().take(2).map(|(_, id)| id).collect();
    // The base posture always comes last: it supplies whatever the specialists did not tag.
    if !out.contains(&ProfileId::NativeFullstack) { out.push(ProfileId::NativeFullstack); }
    out
}

/// The whole-corpus default: everything, at a generous budget.
pub fn compose_all(budget: usize) -> Brief { compose(&ProfileId::all(), budget) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_is_internally_consistent() {
        for id in ProfileId::all() {
            let p = id.profile();
            let mut tags = BTreeSet::new();
            for pr in p.practices {
                assert!(tags.insert(pr.tag),
                        "{}: two practices both govern {:?} — that is one rule stated twice, or two \
rules in conflict, inside a single profile", p.name, pr.tag);
                assert!(pr.rank >= 1 && pr.rank <= 3, "{}: rank {} out of range", p.name, pr.rank);
                assert!(pr.headline.ends_with('.'), "{}: headline is not a sentence", p.name);
                assert!(pr.body.len() > 60, "{}: body too thin to be worth prompt bytes", p.name);
            }
            assert!(p.practices.iter().any(|pr| pr.rank == 1), "{}: nothing load-bearing", p.name);
            assert!(!p.source.is_empty(), "{}: a profile with no source is invented", p.name);
            assert!(p.when.len() > 30, "{}: no usable 'when'", p.name);
        }
    }

    #[test]
    fn keys_round_trip_and_are_distinct() {
        let mut keys = BTreeSet::new();
        for id in ProfileId::all() {
            assert!(keys.insert(id.key()));
            assert_eq!(ProfileId::from_key(id.key()), Some(id));
            assert_eq!(ProfileId::from_key(&id.key().to_uppercase()), Some(id));
        }
        assert_eq!(ProfileId::from_key("nonsense"), None);
    }

    /// THE COMPOSITION PROPERTY. A merged brief must never contain two rules for one thing.
    #[test]
    fn composing_everything_yields_one_rule_per_tag() {
        let b = compose_all(100_000);
        // Count how many times each tag's headline appears — every tag at most once.
        for id in ProfileId::all() {
            for pr in id.profile().practices {
                let n = b.text.matches(pr.headline).count();
                assert!(n <= 1, "'{}' appears {n} times in one brief", pr.headline);
            }
        }
        // and the drops are reported rather than silent
        assert!(!b.dropped.is_empty(), "five profiles share tags; something must have been dropped");
        for d in &b.dropped {
            assert_ne!(d.kept_from, d.dropped_from);
            assert!(!d.kept_from.is_empty());
        }
    }

    #[test]
    fn precedence_decides_who_wins_a_shared_tag() {
        // Hardware and GPU both have an Oracle practice.
        let hw_first = compose(&[ProfileId::Hardware, ProfileId::Gpu], 100_000);
        let gpu_first = compose(&[ProfileId::Gpu, ProfileId::Hardware], 100_000);
        let hw_oracle = HARDWARE.practices.iter().find(|p| p.tag == Tag::Oracle).unwrap().headline;
        let gpu_oracle = GPU.practices.iter().find(|p| p.tag == Tag::Oracle).unwrap().headline;
        assert!(hw_first.text.contains(hw_oracle) && !hw_first.text.contains(gpu_oracle));
        assert!(gpu_first.text.contains(gpu_oracle) && !gpu_first.text.contains(hw_oracle));
        // and each says what it gave up
        assert!(hw_first.dropped.iter().any(|d| d.tag == Tag::Oracle && d.dropped_from == GPU.name));
    }

    /// A tight window must lose refinements, not load-bearing habits.
    #[test]
    fn a_small_budget_degrades_gracefully() {
        let full = compose_all(100_000);
        let small = compose_all(1400);
        assert!(small.text.len() <= 1400, "budget not honoured: {} bytes", small.text.len());
        assert!(small.text.len() < full.text.len());
        // every rank-1 headline still present at the small budget
        for id in ProfileId::all() {
            for pr in id.profile().practices.iter().filter(|p| p.rank == 1) {
                if full.text.contains(pr.headline) {
                    assert!(small.text.contains(pr.headline),
                            "the tight brief dropped a load-bearing habit: '{}'", pr.headline);
                }
            }
        }
    }

    #[test]
    fn a_single_profile_composes_to_itself_with_no_drops() {
        for id in ProfileId::all() {
            let b = compose(&[id], 100_000);
            assert!(b.dropped.is_empty(), "{:?} conflicts with itself", id);
            assert_eq!(b.trimmed, 0);
            for pr in id.profile().practices {
                assert!(b.text.contains(pr.headline), "{:?} lost '{}'", id, pr.headline);
            }
        }
    }

    #[test]
    fn the_autopilot_picks_the_specialist_and_keeps_the_base() {
        let cases: [(&str, ProfileId); 4] = [
            ("the wgsl shader dequant kernel disagrees with the CPU", ProfileId::Gpu),
            ("dequantisation is too slow, speed up the matvec", ProfileId::Gpu), // gpu outscores perf here
            ("this endpoint has terrible latency, profile it", ProfileId::Optimizer),
            ("reverse engineer the undocumented wire format", ProfileId::Hardware),
        ];
        for (task, want) in cases {
            let got = suggest(task);
            assert_eq!(got[0], want, "'{task}' -> {got:?}");
            assert_eq!(*got.last().unwrap(), ProfileId::NativeFullstack,
                       "'{task}': the base posture must always be the fallback layer");
        }
    }

    /// The measured default. A third of this operator's turns carry no direction, so an
    /// unrecognised task must land on the base posture rather than on nothing or on a specialist.
    #[test]
    fn an_unrecognised_task_gets_the_base_posture_alone() {
        for t in ["keep working", "continue", "", "add more blocks", "finish that thing"] {
            assert_eq!(suggest(t), vec![ProfileId::NativeFullstack], "'{t}'");
        }
    }

    #[test]
    fn the_autopilot_never_stacks_more_than_two_specialists() {
        // every signal at once: still bounded, because three role-postures is a prompt arguing with
        // itself about what the job is.
        let kitchen_sink = "design a faster gpu shader kernel for the undocumented firmware protocol and optimise its latency, exploring options";
        let got = suggest(kitchen_sink);
        assert!(got.len() <= 3, "{got:?}");
        assert_eq!(*got.last().unwrap(), ProfileId::NativeFullstack);
        // and the composition of whatever it picked is still one rule per tag
        let b = compose(&got, 100_000);
        for id in &got {
            for pr in id.profile().practices {
                assert!(b.text.matches(pr.headline).count() <= 1);
            }
        }
    }

    #[test]
    fn suggested_profiles_compose_without_losing_the_specialist_lead() {
        // The specialist leads, so its rule for a shared tag wins over the generic one.
        let got = suggest("the shader kernel produces wrong numbers");
        assert_eq!(got[0], ProfileId::Gpu);
        let b = compose(&got, 100_000);
        let gpu_oracle = GPU.practices.iter().find(|p| p.tag == Tag::Oracle).unwrap().headline;
        assert!(b.text.contains(gpu_oracle), "the specialist's oracle rule should lead");
    }

    #[test]
    fn an_empty_composition_is_harmless() {
        let b = compose(&[], 10_000);
        assert!(b.dropped.is_empty() && b.trimmed == 0);
        assert!(b.text.len() < 400, "an empty profile set should not cost prompt bytes");
    }

    /// The two corpus-derived profiles must carry their real, checkable provenance — a profile whose
    /// source says nothing is indistinguishable from one I made up.
    #[test]
    fn the_derived_profiles_name_their_corpora() {
        assert!(HARDWARE.source.contains("472"), "{}", HARDWARE.source);
        assert!(HARDWARE.source.to_lowercase().contains("asahi"));
        assert!(NATIVE_FULLSTACK.source.contains("7966"), "{}", NATIVE_FULLSTACK.source);
        assert!(NATIVE_FULLSTACK.source.contains("33%"), "the measured property must survive");
        // and the three distilled ones say so rather than claiming a corpus
        for id in [ProfileId::Gpu, ProfileId::Optimizer, ProfileId::Planner] {
            let s = id.profile().source.to_lowercase();
            assert!(s.contains("this repository"), "{:?} implies an outside corpus it does not have", id);
        }
    }
}
