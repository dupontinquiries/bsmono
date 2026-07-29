//! modelroles.rs — who does what, in words a person can hold in their head.
//!
//! The engine had three different vocabularies for the same three jobs: `draft`/`target` from the
//! speculative-decoding literature, `primary`/`delegate` in the selector, and `liaison` in the reporting
//! path. Nobody can keep three mappings straight, and "target" in particular is actively misleading —
//! it sounds like a destination rather than the model doing the actual thinking.
//!
//! One set of names, chosen so the relationship is obvious from the word:
//!
//! * **Liaison** — small and fast. Talks to the person about what the machine is doing. Never decides
//!   anything; if it has nothing observed, it says nothing.
//! * **Disciple** — small. Proposes work ahead of the Thinker, which then accepts or rejects it. This is
//!   the speculative `draft` model, and "disciple" says the important part: it is *learning what the
//!   thinker would say*, and being wrong is expected and cheap.
//! * **Thinker** — large. Does the real reasoning and is the authority on every output. The
//!   speculative `target`.
//!
//! The analogy has one job: to make it obvious that the Disciple's output is PROVISIONAL and the
//! Thinker's is not. That is the distinction people actually get wrong.

/// What a configured model is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role { Liaison, Disciple, Thinker }

impl Role {
    pub fn label(self) -> &'static str {
        match self { Role::Liaison => "Liaison", Role::Disciple => "Disciple", Role::Thinker => "Thinker" }
    }

    /// One line for a tooltip — what it does, not what it is called elsewhere.
    pub fn describe(self) -> &'static str {
        match self {
            Role::Liaison => "Small and fast. Reports what the backend is doing while you wait. \
Never decides anything.",
            Role::Disciple => "Small. Proposes tokens ahead of the Thinker, which accepts or rejects \
them. Being wrong is expected and cheap — a rejected guess costs one verification.",
            Role::Thinker => "Large. Does the reasoning and is the authority on every output. Nothing \
reaches you that it did not accept.",
        }
    }

    /// The term this replaces, so old logs and papers remain findable.
    pub fn legacy_name(self) -> &'static str {
        match self { Role::Liaison => "narrator", Role::Disciple => "draft", Role::Thinker => "target" }
    }

    /// Is this role's output provisional — subject to another model's approval?
    ///
    /// The one distinction the naming exists to make obvious.
    pub fn is_provisional(self) -> bool { matches!(self, Role::Disciple) }
}

/// A rough capability/cost score for a model, inferred from its NAME.
///
/// Names are not a reliable source of truth and this does not pretend otherwise — it is a starting
/// point that a measurement should replace. It exists because the alternative is an empty configuration
/// on first run, and an empty configuration means the user gets one model doing all three jobs badly.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Score {
    /// Billions of parameters, as advertised by the name. 0 when the name says nothing.
    pub params_b: f32,
    /// Approximate GiB resident at 4-bit.
    pub gib_q4: f32,
}

/// Parse a size from a model name: `qwen3-30b-a3b`, `Llama-4-Scout-17B-16E`, `gemma-3-12b-it`.
///
/// For MoE names of the form `<total>b-a<active>b`, the ACTIVE count is what predicts speed and the
/// TOTAL is what predicts memory — so both are returned rather than collapsing them, which is the
/// mistake that makes a 30B-A3B look like a 30B dense model.
pub fn score_from_name(name: &str) -> Score {
    let low = name.to_ascii_lowercase();
    let bytes = low.as_bytes();
    let mut sizes: Vec<f32> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') { i += 1; }
            if i < bytes.len() && bytes[i] == b'b' {
                // must not be part of a longer word: `7b-instruct` yes, `7bit` no
                let next_ok = i + 1 >= bytes.len() || !bytes[i + 1].is_ascii_alphanumeric();
                if next_ok {
                    if let Ok(v) = low[start..i].parse::<f32>() {
                        if (0.1..=2000.0).contains(&v) { sizes.push(v); }
                    }
                }
            }
            continue;
        }
        i += 1;
    }
    // `30b-a3b`: the largest is the total, the smallest the active.
    let total = sizes.iter().cloned().fold(0.0f32, f32::max);
    Score { params_b: total, gib_q4: total * 0.55 }
}

/// Assign roles automatically from a candidate list and a memory budget.
///
/// Deliberately conservative: the Thinker is the largest model that FITS, not the largest available, and
/// the Liaison is the smallest thing present. Getting this wrong in the generous direction means an OOM
/// on first run, which is a worse first impression than a smaller model.
///
/// Returns None for a role when no candidate suits it — an explicit gap rather than a bad assignment.
pub fn auto_assign(models: &[String], budget_gib: f32) -> [(Role, Option<String>); 3] {
    if std::env::var_os("LOGSCOPE_NO_MODEL_DEFAULTS").is_some() {
        // Escape hatch for debugging: assign nothing and let the caller/user choose. Named negatively so
        // the helpful behaviour is what you get by not thinking about it.
        return [(Role::Liaison, None), (Role::Disciple, None), (Role::Thinker, None)];
    }
    let mut scored: Vec<(f32, &String)> = models.iter().map(|m| (score_from_name(m).gib_q4, m)).collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let thinker = scored.iter().rev().find(|(g, _)| *g > 0.0 && *g <= budget_gib).map(|(_, m)| (*m).clone());
    // The liaison must FIT too. Picking "the smallest present" without a budget check makes a 109B model
    // the liaison when it is the only one available — which then reports chat_ready long before anything
    // can actually load. Smallest-that-fits, or none.
    let liaison = scored.iter().find(|(g, _)| *g > 0.0 && *g <= budget_gib).map(|(_, m)| (*m).clone());
    // The Disciple must be STRICTLY smaller than the Thinker: a speculative pair where both are the same
    // size costs more than running the Thinker alone, since every draft is as expensive as verifying it.
    // No Thinker means no Disciple: a disciple with nothing to draft FOR is pointless, and using
    // f32::MAX as the bound made every model — including ones that do not fit — pass the size test.
    let disciple = match thinker.as_ref() {
        None => None,
        Some(t) => {
            let t_gib = score_from_name(t).gib_q4;
            scored.iter().rev()
                .find(|(g, _)| *g > 0.0 && *g <= budget_gib && *g < t_gib * 0.5)
                .map(|(_, m)| (*m).clone())
        }
    };
    [(Role::Liaison, liaison), (Role::Disciple, disciple), (Role::Thinker, thinker)]
}

/// The GPU-memory reality a role plan has to fit inside.
///
/// Not RAM. On Asahi/Honeykrisp the binding limit is the BO-VMA ceiling — roughly 24 GiB regardless of
/// how much memory the machine has — while Metal's unified budget is RAM-derived and far larger. A plan
/// built from system RAM would put Scout on both and OOM on one.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Budget {
    pub gpu_gib: f32,
    /// True on drivers whose allocator caps far below system RAM (Asahi Vulkan).
    pub constrained: bool,
}

impl Budget {
    /// Honeykrisp: the ceiling is a Mesa policy, not the machine's memory.
    pub fn asahi() -> Self { Budget { gpu_gib: 24.0, constrained: true } }
    /// Metal-class unified memory: governed by RAM.
    pub fn unified(ram_gib: f32) -> Self { Budget { gpu_gib: ram_gib * 0.5, constrained: false } }
}

/// Which model fills which role, and in what ORDER to load them.
#[derive(Clone, PartialEq, Debug)]
pub struct RolePlan {
    pub liaison: Option<String>,
    pub disciple: Option<String>,
    pub thinker: Option<String>,
    /// Why these, for the startup line.
    pub why: String,
}

impl RolePlan {
    /// Load order: the LIAISON first, always.
    ///
    /// It is the smallest model and the only one usable before the others arrive — so loading it first
    /// means the operator can ask "what is it doing?" during the ninety seconds the Thinker takes, rather
    /// than watching a progress bar. Loading the Thinker first optimises for the moment everything is
    /// ready and ignores the entire interval before it.
    pub fn load_order(&self) -> Vec<(Role, String)> {
        let mut v = Vec::new();
        if let Some(m) = &self.liaison { v.push((Role::Liaison, m.clone())); }
        if let Some(m) = &self.disciple { v.push((Role::Disciple, m.clone())); }
        if let Some(m) = &self.thinker { v.push((Role::Thinker, m.clone())); }
        v
    }

    /// Can the operator hold a conversation yet?
    ///
    /// True as soon as the Liaison is up, regardless of the Thinker. This is what makes "chat while the
    /// big model loads" possible at all — the coordinator does not need every role present to be useful,
    /// only the one that talks.
    pub fn chat_ready(&self, loaded: &[String]) -> bool {
        self.liaison.as_ref().is_some_and(|l| loaded.iter().any(|m| m == l))
    }
}

/// Build a plan for the platform actually present.
pub fn plan_for_platform(models: &[String], budget: Budget) -> RolePlan {
    let a = auto_assign(models, budget.gpu_gib);
    let pick = |r: Role| a.iter().find(|(x, _)| *x == r).and_then(|(_, m)| m.clone());
    let (liaison, disciple, thinker) = (pick(Role::Liaison), pick(Role::Disciple), pick(Role::Thinker));
    let why = match (&thinker, budget.constrained) {
        (Some(t), true) => format!(
            "{t} as Thinker — the GPU allocator caps at {:.0} GiB here regardless of system RAM, so a \
larger model would not fit however much memory the machine has", budget.gpu_gib),
        (Some(t), false) => format!("{t} as Thinker, within a {:.0} GiB unified budget", budget.gpu_gib),
        (None, _) => format!("no model fits the {:.0} GiB budget", budget.gpu_gib),
    };
    RolePlan { liaison, disciple, thinker, why }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_naming_makes_provisionality_obvious() {
        // The one thing the analogy exists to convey: a Disciple's output is a proposal, a Thinker's is
        // the answer. If a reader cannot tell that from the names, the rename bought nothing.
        assert!(Role::Disciple.is_provisional());
        assert!(!Role::Thinker.is_provisional());
        assert!(!Role::Liaison.is_provisional(), "the liaison does not propose OUTPUT, it reports");
        assert!(Role::Disciple.describe().contains("accepts or rejects"));
        assert!(Role::Thinker.describe().contains("authority"));
    }

    #[test]
    fn the_old_words_stay_findable() {
        // Renaming without a mapping orphans every prior log, paper and commit message.
        assert_eq!(Role::Disciple.legacy_name(), "draft");
        assert_eq!(Role::Thinker.legacy_name(), "target");
    }

    #[test]
    fn a_moe_name_is_sized_by_its_TOTAL_not_its_active_count() {
        // `qwen3-30b-a3b` needs 30B of memory and runs at 3B of speed. Reading the 3 would promise a
        // model that does not fit; the memory figure has to win when choosing what to load.
        assert_eq!(score_from_name("qwen3-coder-30b-a3b-instruct").params_b, 30.0);
        assert_eq!(score_from_name("Llama-4-Scout-17B-16E-Instruct").params_b, 17.0);
        assert_eq!(score_from_name("gemma-3-12b-it").params_b, 12.0);
        assert_eq!(score_from_name("DeepSeek-R1-Distill-Llama-8B").params_b, 8.0);
    }

    #[test]
    fn a_name_with_no_size_scores_zero_rather_than_guessing() {
        // A guess here becomes a load decision. Zero means "unknown" and keeps it out of auto-assignment.
        assert_eq!(score_from_name("mystery-model").params_b, 0.0);
        assert_eq!(score_from_name("model-7bit-quant").params_b, 0.0, "7bit is not 7B");
    }

    #[test]
    fn auto_assign_picks_the_largest_that_FITS_not_the_largest() {
        // Being generous here means an OOM on first run, which is a worse first impression than a
        // smaller model that works.
        let models: Vec<String> = ["tiny-1b", "small-8b", "big-70b"].iter().map(|s| s.to_string()).collect();
        let a = auto_assign(&models, 20.0);   // 70b needs ~38 GiB and must be rejected
        let thinker = a.iter().find(|(r, _)| *r == Role::Thinker).unwrap().1.clone();
        assert_eq!(thinker.as_deref(), Some("small-8b"));
        let liaison = a.iter().find(|(r, _)| *r == Role::Liaison).unwrap().1.clone();
        assert_eq!(liaison.as_deref(), Some("tiny-1b"), "the liaison is the smallest thing present");
    }

    #[test]
    fn a_disciple_must_be_much_smaller_than_its_thinker() {
        // A speculative pair of equal size costs MORE than the thinker alone: every draft is as
        // expensive as the verification it saves.
        let same: Vec<String> = ["a-8b", "b-8b"].iter().map(|s| s.to_string()).collect();
        let a = auto_assign(&same, 100.0);
        assert!(a.iter().find(|(r, _)| *r == Role::Disciple).unwrap().1.is_none(),
            "no disciple is better than a useless one");
        let good: Vec<String> = ["a-1b", "b-8b"].iter().map(|s| s.to_string()).collect();
        assert_eq!(auto_assign(&good, 100.0).iter().find(|(r, _)| *r == Role::Disciple).unwrap().1.as_deref(),
            Some("a-1b"));
    }

    fn ms(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn the_same_model_list_plans_DIFFERENTLY_on_asahi_and_metal() {
        // The requirement, and the reason Budget is not just "RAM". On Honeykrisp the BO-VMA ceiling is
        // ~24 GiB no matter how much memory the machine has, so Scout (~34 GiB at q4) cannot be the
        // Thinker there even on a 192 GiB box — while the same list on Metal picks it happily.
        let models = ms(&["llama3.2-3b", "qwen3-30b-a3b", "llama4-scout-109b"]);

        let metal = plan_for_platform(&models, Budget::unified(192.0));   // 96 GiB budget
        assert_eq!(metal.thinker.as_deref(), Some("llama4-scout-109b"));
        assert_eq!(metal.liaison.as_deref(), Some("llama3.2-3b"), "smallest is the liaison");

        let asahi = plan_for_platform(&models, Budget::asahi());          // 24 GiB ceiling
        assert_eq!(asahi.thinker.as_deref(), Some("qwen3-30b-a3b"),
            "Scout does not fit the allocator ceiling; the 30B does");
        assert_eq!(asahi.liaison.as_deref(), Some("llama3.2-3b"));
        assert!(asahi.why.contains("regardless of system RAM"),
            "the message must say it is an ALLOCATOR limit, or someone buys more memory to fix it: {}",
            asahi.why);
    }

    #[test]
    fn the_liaison_loads_FIRST_so_chat_works_while_the_thinker_arrives() {
        // Loading the Thinker first optimises for the moment everything is ready and ignores the ninety
        // seconds before it. The Liaison is small, loads fast, and is the only role usable alone.
        let p = plan_for_platform(&ms(&["llama3.2-3b", "llama4-scout-109b"]), Budget::unified(192.0));
        let order = p.load_order();
        assert_eq!(order[0].0, Role::Liaison);
        assert_eq!(order.last().unwrap().0, Role::Thinker);

        // and chat is live as soon as the liaison is up, with the thinker still loading
        assert!(p.chat_ready(&ms(&["llama3.2-3b"])));
        assert!(!p.chat_ready(&ms(&["llama4-scout-109b"])), "the thinker alone cannot hold the conversation");
        assert!(!p.chat_ready(&[]));
    }

    #[test]
    fn a_budget_nothing_fits_says_so_rather_than_planning_anyway() {
        let p = plan_for_platform(&ms(&["llama4-scout-109b"]), Budget::asahi());
        assert!(p.thinker.is_none());
        assert!(p.why.contains("no model fits"));
        assert!(p.load_order().is_empty(), "an unfittable plan loads nothing rather than trying");
    }

    #[test]
    fn EVERY_role_must_fit_the_budget_not_just_the_thinker() {
        // Three separate holes, all found by one test asserting an unfittable plan loads nothing:
        //   - the liaison was "smallest present" with no budget check, so a 109B model became the
        //     liaison when it was the only one — and chat_ready then went true for something that
        //     could never load;
        //   - the disciple had no budget check either;
        //   - with no thinker, the disciple's size bound was f32::MAX * 0.5, which every model passes.
        // "The thinker fits" is not the same claim as "the plan fits".
        let big = ms(&["llama4-scout-109b"]);
        let p = plan_for_platform(&big, Budget::asahi());
        assert!(p.thinker.is_none() && p.liaison.is_none() && p.disciple.is_none(), "{p:?}");
        assert!(!p.chat_ready(&big), "nothing loadable means no conversation, however it is spelled");

        // and a disciple is never assigned without a thinker to draft FOR
        let solo = plan_for_platform(&ms(&["tiny-1b"]), Budget::asahi());
        assert_eq!(solo.thinker.as_deref(), Some("tiny-1b"), "it fits, so it is the thinker");
        assert!(solo.disciple.is_none(), "nothing smaller to draft with");
    }

    #[test]
    fn the_escape_hatch_assigns_nothing() {
        // LOGSCOPE_NO_MODEL_DEFAULTS, for isolating whether an auto-choice caused a problem. Named
        // negatively so the helpful behaviour is the default.
        std::env::set_var("LOGSCOPE_NO_MODEL_DEFAULTS", "1");
        let a = auto_assign(&["x-7b".to_string()], 100.0);
        assert!(a.iter().all(|(_, m)| m.is_none()));
        std::env::remove_var("LOGSCOPE_NO_MODEL_DEFAULTS");
    }
}
