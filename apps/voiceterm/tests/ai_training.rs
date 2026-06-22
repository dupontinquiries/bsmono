//! AI-training test suite (28 tests across three tiers).
//!
//! Built for use as a scoring signal in training: an AI agent attempts
//! to (re)implement the targeted modules; passing tests at higher
//! tiers reflects deeper understanding.
//!
//!  - **Tier 1 — function contract.** A single pure function in / out.
//!    Passing means the AI got *this one function* right. Low signal
//!    per test, but a clean baseline.
//!  - **Tier 2 — module behaviour.** Two-to-five functions composed,
//!    state, sequencing, or invariants that span a module's surface.
//!    Passing requires reasoning about contracts, not just bodies.
//!  - **Tier 3 — composed behaviour.** A workflow-style invariant that
//!    spans multiple modules OR simulates how a module participates
//!    in a larger flow. Highest signal per test.
//!
//! Each test is prefixed `t1_` / `t2_` / `t3_` so a scoring pipeline
//! can sort by tier with one `grep`. Each carries a one-line
//! `// LEARNS:` doc stating the invariant in plain language so an AI
//! can introspect what it's being graded on.
//!
//! Targets are restricted to the pure-runnable set listed in
//! `scripts/cargo_check_pure.sh` — so this file builds without Qt /
//! FFI / audio infra, the same way the AI's grading environment will.

// The targeted modules. Keep this list small: every entry must be a
// pure module (no FFI / Qt / unrelated crate deps) so the suite stays
// runnable in any minimal sandbox.
use web2qml::{
    dead_air,
    dumb_summary,
    llm_coalesce::{self, Admission, Registry},
    llm_scheduler::{
        self, DeviceBudget, ModelCost, PendingRequest, Priority, SizeClass,
    },
    mic_pipeline_fsm::{Decision, FsmConfig, MicPipelineFsm},
    mic_scheduler::Budget,
    mic_topology::PairStat,
    util,
    whisper_models,
};

// ─────────────────────────────────────────────────────────────────────
// TIER 1 — function contracts (10)
// ─────────────────────────────────────────────────────────────────────

#[test]
fn t1_util_fnv1a_is_deterministic_and_nontrivial() {
    // LEARNS: a hash function must be deterministic across calls and
    // must distinguish at least some distinct inputs (no all-zero).
    let a = util::fnv1a("hello");
    let b = util::fnv1a("hello");
    let c = util::fnv1a("world");
    assert_eq!(a, b, "same input must hash the same");
    assert_ne!(a, c, "different inputs must not collide on these short strings");
    assert_ne!(a, 0, "non-empty input must not hash to zero");
}

#[test]
fn t1_util_short_hash_is_compact_and_stable() {
    // LEARNS: short_hash compresses fnv1a to a short string; identical
    // inputs share representation, length is bounded.
    let h = util::short_hash("the quick brown fox");
    let h2 = util::short_hash("the quick brown fox");
    assert_eq!(h, h2);
    assert!(!h.is_empty() && h.len() <= 16,
        "short_hash should be short and non-empty, got {h:?}");
}

#[test]
fn t1_dumb_summary_score_relevance_in_unit_interval() {
    // LEARNS: relevance is a score in [0, 1]; identical utterance vs
    // its own keyword scores higher than an unrelated word.
    let kws = vec!["compile".to_string(), "error".to_string()];
    let on = dumb_summary::score_relevance("compile error in module", &kws);
    let off = dumb_summary::score_relevance("kitten napping under chair", &kws);
    assert!((0.0..=1.0).contains(&on), "on-topic score in [0,1]: {on}");
    assert!((0.0..=1.0).contains(&off), "off-topic score in [0,1]: {off}");
    assert!(on > off, "matching keywords should score higher: on={on} off={off}");
}

#[test]
fn t1_dumb_summary_handles_empty_output() {
    // LEARNS: summarize on an empty buffer must not panic and must
    // produce *some* sensible summary (not garbage).
    let s = dumb_summary::summarize("ls", "");
    // Don't assert exact text — that's brittle. Just: it returned
    // and the text field is a string we can inspect.
    let _ = &s.text;
}

#[test]
fn t1_dumb_summary_detect_progress_bars_finds_none_in_plain_prose() {
    // LEARNS: progress-bar detection is precise — plain prose without
    // [###] / 45% / loading markers must NOT spuriously match.
    let bars = dumb_summary::detect_progress_bars(
        "Rust is a systems language. The compiler enforces ownership at compile time.");
    assert!(bars.is_empty(), "no progress bars in prose, got {bars:?}");
}

#[test]
fn t1_scheduler_fit_resident_grows_with_budget() {
    // LEARNS: more memory budget fits at least as many models, never
    // fewer. (Monotonicity is a robust invariant — much harder to
    // accidentally pass than an exact-equality assertion.)
    let small = DeviceBudget { mem_mb: 4_000, concurrency: 2 };
    let big = DeviceBudget { mem_mb: 32_000, concurrency: 2 };
    let m = ModelCost { mem_mb: 2_000, slots: 1 };
    let s = llm_scheduler::fit_resident(&small, m);
    let b = llm_scheduler::fit_resident(&big, m);
    assert!(b >= s, "more budget must fit ≥ models: small={s} big={b}");
    assert!(s >= 1, "a 4GB budget should fit at least one 2GB model");
}

#[test]
fn t1_scheduler_classify_size_zero_budget_is_too_large() {
    // LEARNS: a model that cannot fit is classified TooLarge, not
    // silently downgraded. Defensive classification.
    let zero = DeviceBudget { mem_mb: 0, concurrency: 1 };
    let m = ModelCost { mem_mb: 2_000, slots: 1 };
    assert!(matches!(llm_scheduler::classify_size(&zero, m), SizeClass::TooLarge));
}

#[test]
fn t1_scheduler_derive_extreme_inputs_no_panic() {
    // LEARNS: derive() saturates rather than overflowing on adversarial
    // u32 inputs. Crashes on edge cases would be a worse failure than
    // an unrealistic-but-finite result.
    let _ = DeviceBudget::derive(u32::MAX, 64);
    let _ = DeviceBudget::derive(0, 0);
    let _ = DeviceBudget::derive(1, u32::MAX);
}

#[test]
fn t1_whisper_models_default_set_is_a_real_subset() {
    // LEARNS: the curated default set must reference models that
    // actually exist in the catalog (no dead pointers).
    for m in whisper_models::default_set() {
        assert!(whisper_models::find(&m.filename).is_some(),
            "default-set entry {:?} missing from catalog", m.filename);
    }
    assert!(!whisper_models::default_set().is_empty(),
        "default set must not be empty (would silently disable voice setup)");
}

#[test]
fn t1_whisper_models_url_for_is_https_and_contains_filename() {
    // LEARNS: the download URL must be HTTPS (no plaintext download of
    // model binaries) and must reference the requested filename.
    let url = whisper_models::url_for("ggml-base.en.bin");
    assert!(url.starts_with("https://"), "url must be https, got {url}");
    assert!(url.contains("ggml-base.en.bin"), "url must contain filename, got {url}");
}

// ─────────────────────────────────────────────────────────────────────
// TIER 2 — module behaviour (10)
// ─────────────────────────────────────────────────────────────────────

#[test]
fn t2_coalesce_registry_first_computes_others_attach() {
    // LEARNS: the deduplication contract — the FIRST submitter for a
    // hash is assigned Compute (must do the work); every subsequent
    // submitter for the same hash is Attach (waits for the result).
    let mut r = Registry::new();
    assert_eq!(r.submit(42), Admission::Compute);
    assert_eq!(r.submit(42), Admission::Attach);
    assert_eq!(r.submit(42), Admission::Attach);
    // distinct hash gets its own Compute
    assert_eq!(r.submit(99), Admission::Compute);
}

#[test]
fn t2_coalesce_complete_returns_total_submitter_count() {
    // LEARNS: complete() returns the TOTAL number of submitters for
    // that hash — the original Compute + every subsequent Attach —
    // because every submitter is a party that will receive the result.
    // (A common AI mistake here is to assume `complete` reports just
    // the attachers; it doesn't, and the contract matters for the
    // fan-out caller to deliver to the original requester too.)
    let mut r = Registry::new();
    assert_eq!(r.submit(7), Admission::Compute);
    assert_eq!(r.submit(7), Admission::Attach);
    assert_eq!(r.submit(7), Admission::Attach);
    // 1 Compute + 2 Attach = 3 total parties expecting a result.
    let n = r.complete(7);
    assert_eq!(n, 3, "complete reports total submitter count, got {n}");
    // After completion, the hash is no longer in-flight.
    assert!(!r.is_in_flight(7));
}

#[test]
fn t2_coalesce_complete_unknown_hash_is_safe() {
    // LEARNS: complete() on a hash that was never submitted must be
    // idempotent (no panic, no spurious fan-out). Real systems hit this
    // when a worker errors after the dedup table was cleared.
    let mut r = Registry::new();
    let n = r.complete(0xDEAD_BEEF);
    assert_eq!(n, 0, "unknown hash → zero fan-out, not a panic");
}

#[test]
fn t2_dumb_summary_summarize_command_is_used() {
    // LEARNS: the summary considers BOTH the command and its output —
    // running `git status` on the same buffer should produce different
    // emphasis than `cargo build`. We don't assert exact text (brittle)
    // but we assert that varying the command changes *something*.
    let out = "all good\nno errors\nbuild complete";
    let s1 = dumb_summary::summarize("git status", out);
    let s2 = dumb_summary::summarize("cargo build", out);
    // We only require *some* signal that the command influenced
    // output. If both are byte-identical the command was ignored.
    // (Either text or keywords differing counts.)
    assert!(s1.text != s2.text || s1.keywords != s2.keywords,
        "varying the command should change SOMETHING in the summary; \
         s1={:?} s2={:?}", s1.text, s2.text);
}

#[test]
fn t2_dumb_summary_keywords_strip_trailing_period_keep_paths() {
    // LEARNS: tokenisation must remove sentence-final punctuation
    // (avoid "performance." in keyword lists) BUT preserve interior
    // dots used in version strings and paths.
    let s = dumb_summary::summarize(
        "info",
        "Focused on performance. Edit src/main.rs and bump v0.1.567 now.");
    let kws = &s.keywords;
    assert!(!kws.iter().any(|k| k.ends_with('.')),
        "no keyword should end with '.', got {kws:?}");
    // version-like and path-like tokens with interior dots must
    // survive somewhere in the keyword list.
    let has_path_or_version = kws.iter()
        .any(|k| k.contains("src/main") || k.contains("0.1.567"));
    assert!(has_path_or_version,
        "version/path tokens with interior dots must survive: {kws:?}");
}

#[test]
fn t2_scheduler_classify_size_is_monotone_in_model_size() {
    // LEARNS: classify_size partitions models by fit. As a model grows
    // (holding budget constant), classification can only get *tighter*
    // (Resident → Running → TooLarge), never looser.
    let b = DeviceBudget { mem_mb: 16_000, concurrency: 4 };
    // We sample increasing model sizes and check the ordering of
    // classes never regresses. Order: SmallForDevice < LargeForDevice < TooLarge.
    let rank = |c: SizeClass| match c {
        SizeClass::SmallForDevice => 0,
        SizeClass::LargeForDevice => 1,
        SizeClass::TooLarge => 2,
    };
    let mut prev = -1;
    for &mb in &[500u32, 2_000, 8_000, 20_000, 200_000] {
        let class = llm_scheduler::classify_size(
            &b, ModelCost { mem_mb: mb, slots: 1 });
        let r = rank(class) as i32;
        assert!(r >= prev,
            "classification should not relax as model grows; mb={mb} \
             class={class:?} prev_rank={prev}");
        prev = r;
    }
}

#[test]
fn t2_scheduler_derive_tiny_device_zero_budget() {
    // LEARNS: device-budget derivation reserves enough headroom that a
    // tiny machine (≤2GB) reports zero LLM budget rather than
    // underflowing or claiming negative memory.
    let tiny = DeviceBudget::derive(2_000, 2);
    assert_eq!(tiny.mem_mb, 0, "2GB device must report 0 LLM budget");
    assert!(tiny.concurrency >= 1, "concurrency floor is 1, not 0");
}

#[test]
fn t2_whisper_models_catalog_json_is_valid_and_lists_all() {
    // LEARNS: catalog_json must round-trip every model in the catalog.
    // No silent truncation of the catalog.
    let json = whisper_models::catalog_json();
    assert!(json.starts_with('[') && json.trim_end().ends_with(']'),
        "catalog_json must be a JSON array, got {json:.80}...");
    // Cheap structural check: every default-set filename appears.
    for m in whisper_models::default_set() {
        assert!(json.contains(&m.filename),
            "catalog_json must list default-set entry {:?}", m.filename);
    }
}

#[test]
fn t2_whisper_models_default_set_size_is_sum_of_members() {
    // LEARNS: a derived aggregate must equal the sum of its parts.
    // (Easy invariant to violate when adding a new model — the
    // pre-computed total drifts from the iterated total.)
    let computed: u32 = whisper_models::default_set()
        .iter().map(|m| m.size_mb).sum();
    let reported = whisper_models::default_set_size_mb();
    assert_eq!(reported, computed,
        "default_set_size_mb={reported} but iterated sum={computed}");
}

#[test]
fn t2_util_short_hash_collision_resistance_on_small_corpus() {
    // LEARNS: short_hash on a small corpus of realistic strings must
    // not collide. (The full hash space is large; this is a sanity
    // floor — collisions on 100 short distinct strings would indicate
    // a truncation bug.)
    let mut seen = std::collections::HashSet::new();
    for i in 0..100 {
        let s = format!("conversation-{i:04}");
        let h = util::short_hash(&s);
        assert!(seen.insert(h.clone()),
            "collision at i={i} on short_hash({s:?}) = {h}");
    }
}

// ─────────────────────────────────────────────────────────────────────
// TIER 3 — composed behaviour (8)
// ─────────────────────────────────────────────────────────────────────

#[test]
fn t3_coalesce_then_scheduler_plan_under_dedup() {
    // LEARNS: when two identical prompts arrive, coalesce assigns one
    // Compute and one Attach — so the scheduler sees ONE unit of work
    // to admit, not two. Composed contract: dedup happens BEFORE
    // admission accounting.
    let mut r = Registry::new();
    let h = llm_coalesce::hash_prompt("write a haiku about rust");
    let a = r.submit(h);
    let b = r.submit(h);
    // Count Computes — that's the work the scheduler must plan for.
    let computes = [a, b].iter().filter(|x| matches!(x, Admission::Compute)).count();
    assert_eq!(computes, 1, "exactly one Compute per unique prompt hash");
    // And admission of that one job must be plannable in a reasonable
    // budget — i.e. the scheduler doesn't reject a single-job batch.
    let bud = DeviceBudget { mem_mb: 8_000, concurrency: 2 };
    let pending = vec![PendingRequest {
        id: 1, priority: Priority::Interactive, slots: 1,
    }];
    let plan = llm_scheduler::plan_admission(&bud, 0, &pending);
    assert_eq!(plan.admit.len(), 1,
        "scheduler must admit the one deduped job, got {plan:?}");
    assert!(plan.queue.is_empty(), "no queueing needed for a single job");
}

#[test]
fn t3_coalesce_recovers_from_cancel() {
    // LEARNS: if the Compute path fails (worker error), cancel() must
    // (a) release attached waiters with None and (b) leave the
    // registry in a state where a fresh submit for the same hash
    // gets Compute again — the system retries from a clean slate.
    let h = llm_coalesce::hash_prompt("retry me");
    // Manually drive the global registry through a failure cycle.
    let (adm, _) = llm_coalesce::submit("retry me");
    assert!(matches!(adm, Admission::Compute));
    assert!(llm_coalesce::is_in_flight_global(h));
    llm_coalesce::cancel(h);
    assert!(!llm_coalesce::is_in_flight_global(h),
        "cancel must clear the in-flight marker");
    // Fresh submit gets Compute again, not Attach.
    let (adm2, _) = llm_coalesce::submit("retry me");
    assert!(matches!(adm2, Admission::Compute),
        "after cancel, the same prompt should compute again, got {adm2:?}");
    // Clean up so we don't leak to other tests sharing the global.
    llm_coalesce::cancel(h);
}

#[test]
fn t3_summarize_pipeline_handles_progress_bars() {
    // LEARNS: when output contains progress bars (npm install, cargo
    // build), the summarizer's bar detector and main summary must
    // agree the output is "in progress / install-like", not parrot
    // each bar line into the keyword list.
    let out = "\
        ████████░░░░░░░░░░░░░░░  45%\n\
        downloading [==>      ] 23%\n\
        npm WARN deprecated thing@1.0\n";
    let s = dumb_summary::summarize("npm install", out);
    let bars = dumb_summary::detect_progress_bars(out);
    assert!(!bars.is_empty(),
        "progress bars must be detected in npm install output");
    // The keyword list shouldn't be dominated by bar-glyph noise.
    // We assert no keyword is more than half non-alphanumeric.
    for kw in &s.keywords {
        let alnum = kw.chars().filter(|c| c.is_alphanumeric()).count();
        assert!(alnum * 2 >= kw.len(),
            "keyword {kw:?} looks like bar-glyph noise (mostly non-alnum)");
    }
}

#[test]
fn t3_summarize_long_log_caps_keywords() {
    // LEARNS: summary keywords must remain a bounded, useful set even
    // on very long inputs — the AI must NOT just return every token
    // it saw. This guards against "summarizer = identity" pathologies.
    let big_output: String = (0..5_000)
        .map(|i| format!("event {i} succeeded for module thing\n"))
        .collect();
    let s = dumb_summary::summarize("grep success", &big_output);
    assert!(s.keywords.len() <= 32,
        "keyword set must be bounded (got {}), not the full token bag",
        s.keywords.len());
}

#[test]
fn t3_scheduler_admission_respects_concurrency() {
    // LEARNS: planning N identical jobs against a budget with
    // concurrency C must not admit more than C at once; the rest queue.
    let bud = DeviceBudget { mem_mb: 100_000, concurrency: 3 };
    let pending: Vec<PendingRequest> = (0..10u64)
        .map(|i| PendingRequest { id: i, priority: Priority::Interactive, slots: 1 })
        .collect();
    let plan = llm_scheduler::plan_admission(&bud, 0, &pending);
    assert!(plan.admit.len() as u32 <= bud.concurrency,
        "admission ({}) must respect concurrency cap ({})",
        plan.admit.len(), bud.concurrency);
    assert_eq!(plan.admit.len() + plan.queue.len(), pending.len(),
        "every pending request must be either admitted or queued");
}

#[test]
fn t3_fsm_first_tick_must_establish_shape() {
    // LEARNS: the mic pipeline FSM contract — the very first tick on a
    // fresh FSM must always Reshape (it has no current topology to
    // hold). A Hold here would mean the system runs with no shape.
    let mut f = MicPipelineFsm::new(FsmConfig::default());
    let pairs = vec![PairStat { a: 0, b: 1, correlation: 0.6, delay_ms: 4.0 }];
    let budget = Budget { ram_mb: 4_000, cpus: 4, n_mics: 2 };
    let d = f.tick(2, &pairs, &budget);
    assert!(matches!(d, Decision::Reshape { .. }),
        "first tick MUST establish a shape, got {d:?}");
    // And after the reshape, the FSM reports a current topology.
    assert!(f.current_topology().is_some(),
        "after reshape, current_topology must be set");
}

#[test]
fn t3_fsm_stable_input_holds_after_first_reshape() {
    // LEARNS: once shaped, the FSM must NOT thrash — repeated identical
    // ticks against the same conditions should Hold, not flap to a new
    // shape every frame. This guards against missing hysteresis.
    let mut f = MicPipelineFsm::new(FsmConfig::default());
    let pairs = vec![PairStat { a: 0, b: 1, correlation: 0.6, delay_ms: 4.0 }];
    let b = Budget { ram_mb: 4_000, cpus: 4, n_mics: 2 };
    let _first = f.tick(2, &pairs, &b);
    // Five more identical ticks — must all Hold.
    for k in 0..5 {
        assert!(matches!(f.tick(2, &pairs, &b), Decision::Hold),
            "tick #{} on identical input must Hold (no thrash)", k + 2);
    }
}

#[test]
fn t3_dead_air_compresses_more_for_longer_gaps() {
    // LEARNS: the cleave planner's keep_factor must MONOTONICALLY
    // decrease as a dead-air run gets longer (longer silence →
    // compress harder). Frame carries {loudness, spread} only — time
    // is implied by frame order, so a longer gap means MORE consecutive
    // silent frames at the end of the slice.
    use dead_air::{CleaveConfig, Frame, SegmentKind};
    let cfg = CleaveConfig::default();
    // Build: 10 voice frames (loud) followed by N silent frames (very quiet).
    let mk = |silent_frames: usize| -> Vec<Frame> {
        let mut v: Vec<Frame> = (0..10)
            .map(|_| Frame { loudness: -10.0, spread: 80.0 })
            .collect();
        v.extend((0..silent_frames)
            .map(|_| Frame { loudness: -70.0, spread: 0.5 }));
        v
    };
    let mut last_keep: f32 = 2.0; // sentinel above 1.0
    for &n in &[50usize, 150, 500] {
        let frames = mk(n);
        let plan = dead_air::plan_cleave(&frames, &cfg);
        let Some(dead) = plan.segments.iter()
            .find(|s| matches!(s.kind, SegmentKind::DeadAir)) else {
            // Some configs may not cleave very short runs at all; that's
            // a separate test. Skip to the next size if no dead segment.
            continue;
        };
        assert!((0.0..=1.0).contains(&dead.keep_factor),
            "keep_factor must be in [0,1], got {}", dead.keep_factor);
        assert!(dead.keep_factor <= last_keep + 1e-6,
            "longer silence must compress at least as hard; n={n} \
             keep_factor={} prev={last_keep}", dead.keep_factor);
        last_keep = dead.keep_factor;
    }
    // If NO size produced a dead-air segment, the test is vacuous —
    // fail so the AI doesn't get a free pass for a broken cleaver.
    assert!(last_keep <= 1.0, "no DeadAir segment ever produced — \
        plan_cleave never classified silence as dead air");
}
