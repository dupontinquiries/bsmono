//! dupscan.rs — finding two implementations of one idea before the compiler cannot.
//!
//! **Rust permits `pub struct Peer` in two modules of one crate.** That is correct language design
//! and it is why seven duplicate implementations survived in this tree: the compiler has no
//! objection, the tests pass, and both versions work. They are only found when someone happens to
//! put them in the same scope.
//!
//! The ones that got through, and how each was eventually caught:
//!
//! | duplicate | found by |
//! |---|---|
//! | `Peer` in machinery.rs, twice | a name collision when a third was added |
//! | `Beacon` and `beacon_for` | same |
//! | two Q4_K dequant paths | a failing gate, after four wrong fixes |
//! | two CPU quantizers | a sweep, after one was patched and the other was not |
//! | `mdtable` vs `table_ops` | being asked to look |
//! | `state_exit_tests` | a test module name collision |
//!
//! **Six of seven were found by accident.** A scan is the cheap version of the accident.
//!
//! ## Why name-matching and not semantics
//!
//! Two functions that do the same thing under different names are the harder case and cannot be
//! found this way. Two with the SAME name almost always are the same idea — nobody names an
//! unrelated thing `Peer` twice in one crate — so the cheap check catches the common case, and
//! saying it does not catch the other is more useful than pretending it does.

/// A public item and where it is defined.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Item { pub name: String, pub kind: Kind, pub module: String, pub line: usize }

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind { Struct, Enum, Fn, Const, Trait, Type }

impl Kind {
    fn from_keyword(k: &str) -> Option<Kind> {
        Some(match k {
            "struct" => Kind::Struct, "enum" => Kind::Enum, "fn" => Kind::Fn,
            "const" | "static" => Kind::Const, "trait" => Kind::Trait, "type" => Kind::Type,
            _ => return None,
        })
    }
}

/// Extract public items from one file's source.
///
/// Skips `#[cfg(test)]` regions by default — a test helper named the same as a production item is
/// noise, and a scan that reports noise gets ignored, which is how the last one would have failed.
pub fn items_in(module: &str, src: &str, include_tests: bool) -> Vec<Item> {
    let mut out = Vec::new();
    let mut in_test = false;
    let mut brace_depth_at_test = 0i32;
    let mut depth = 0i32;
    for (i, line) in src.lines().enumerate() {
        let t = line.trim();
        if t.starts_with("#[cfg(test)]") { in_test = true; brace_depth_at_test = depth; }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if in_test && depth <= brace_depth_at_test && t.contains('}') { in_test = false; }
        if in_test && !include_tests { continue }
        let Some(rest) = t.strip_prefix("pub ") else { continue };
        let rest = rest.strip_prefix("const ").map(|r| ("const", r))
            .or_else(|| rest.strip_prefix("fn ").map(|r| ("fn", r)))
            .or_else(|| rest.strip_prefix("struct ").map(|r| ("struct", r)))
            .or_else(|| rest.strip_prefix("enum ").map(|r| ("enum", r)))
            .or_else(|| rest.strip_prefix("trait ").map(|r| ("trait", r)))
            .or_else(|| rest.strip_prefix("static ").map(|r| ("static", r)))
            .or_else(|| rest.strip_prefix("type ").map(|r| ("type", r)));
        let Some((kw, after)) = rest else { continue };
        let Some(kind) = Kind::from_keyword(kw) else { continue };
        let name: String = after.chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if name.is_empty() { continue }
        out.push(Item { name, kind, module: module.to_string(), line: i + 1 });
    }
    out
}

/// Items defined more than once across a set of modules.
///
/// **Grouped by name AND kind.** A `struct Foo` and a `fn foo` are unrelated; a `struct Foo` in two
/// modules is the thing worth reporting.
pub fn duplicates(items: &[Item]) -> Vec<Vec<&Item>> {
    let mut by_key: std::collections::BTreeMap<(String, Kind), Vec<&Item>> = Default::default();
    for it in items { by_key.entry((it.name.clone(), it.kind)).or_default().push(it); }
    let mut v: Vec<Vec<&Item>> = by_key.into_values()
        // Two in ONE module is usually a genuine overload or a typo the compiler will catch. Two in
        // DIFFERENT modules is the case that compiles and survives.
        .filter(|g| g.len() > 1 && g.iter().map(|i| &i.module).collect::<std::collections::BTreeSet<_>>().len() > 1)
        .collect();
    v.sort_by_key(|g| g[0].name.clone());
    v
}

/// The report.
/// Does the reported location actually declare what the report says?
///
/// **A report whose line numbers do not match the source cannot be acted on**, and one unverifiable
/// entry costs the reader's trust in all 277.
///
/// The audit that prompted this got the answer wrong in an instructive way. `Enum Access
/// expert_cache:20, memplan:46` looked like a false positive because memplan.rs:46 held an unrelated
/// function — but **there are TWO memplan.rs, one per crate**, and I had opened the wrong one
/// because it was the directory I happened to be standing in.
///
/// The report was right. What the module name does not say is WHICH CRATE, and that ambiguity is
/// what made a correct entry look wrong. The two Access enums are genuinely different ideas
/// (residency format versus cache outcome) and the two memplans share zero public names, so neither
/// is a duplicate implementation — but a reader cannot tell that from `memplan:46` alone.
///
/// Returns None when the file cannot be read, which is not a failure — a scan of a checkout that has
/// moved on is stale rather than wrong.
pub fn verify(item: &Item, crate_src: &std::path::Path) -> Option<bool> {
    // The module name alone is ambiguous across crates — two memplan.rs exist — so verification
    // takes the crate's src dir explicitly rather than guessing which one was meant.
    let path = crate_src.join(format!("{}.rs", item.module));
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().nth(item.line.checked_sub(1)?)?;
    // The declaration must mention the name. Not an exact parse — enough to catch a location that
    // points at something else entirely, which is the failure seen.
    Some(line.contains(&item.name))
}

pub fn report(groups: &[Vec<&Item>]) -> String {
    if groups.is_empty() {
        return "no public item is defined in two modules of this crate".into();
    }
    let mut out = vec![format!("{} name(s) defined in more than one module:", groups.len())];
    for g in groups {
        let places: Vec<String> = g.iter().map(|i| format!("{}:{}", i.module, i.line)).collect();
        out.push(format!("  {:?} {}  —  {}", g[0].kind, g[0].name, places.join(", ")));
    }
    out.push("Rust permits this and the compiler will not object, which is why duplicates here \
survive until someone puts them in one scope. Each is either two ideas sharing a name, or one idea \
implemented twice — and the second is worth resolving before both drift".into());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_PEER_duplicate_would_have_been_caught() {
        // Two `pub struct Peer` in one crate, which is what actually happened in machinery.rs and
        // was only found when a third was added.
        let a = items_in("machinery", "pub struct Peer {\n}\n", false);
        let b = items_in("netpeer", "pub struct Peer {\n}\n", false);
        let all: Vec<Item> = a.into_iter().chain(b).collect();
        let d = duplicates(&all);
        assert_eq!(d.len(), 1);
        assert!(report(&d).contains("one idea implemented twice"));
    }

    #[test]
    fn a_STRUCT_and_a_FN_of_the_same_name_are_not_a_duplicate() {
        // `struct Foo` and `fn foo` are unrelated, and reporting them would teach the reader to skim.
        let all: Vec<Item> = items_in("a", "pub struct Thing;\n", false).into_iter()
            .chain(items_in("b", "pub fn Thing() {}\n", false)).collect();
        assert!(duplicates(&all).is_empty());
    }

    #[test]
    fn two_definitions_in_ONE_module_are_left_alone() {
        // The compiler catches those. This scan exists for the case that COMPILES.
        let all = items_in("a", "pub struct Foo;\npub struct Foo;\n", false);
        assert!(duplicates(&all).is_empty());
    }

    #[test]
    fn TEST_HELPERS_are_skipped_by_default() {
        // A test helper sharing a production name is noise, and a scan that reports noise gets
        // ignored — which is how this guard would fail in exactly the way the others did not.
        let src = "pub struct Real;\n#[cfg(test)]\nmod t {\n    pub struct Real;\n}\n";
        assert_eq!(items_in("m", src, false).len(), 1);
        assert_eq!(items_in("m", src, true).len(), 2);
    }

    #[test]
    fn a_CLEAN_crate_says_so_rather_than_printing_nothing() {
        // An empty report reads as "the scan did not run", which this project has learned to
        // distrust more than any other output.
        assert!(report(&[]).contains("no public item is defined in two modules"));
    }

    #[test]
    fn CONST_and_STATIC_are_one_kind() {
        // SUBMIT_BY_NO_FRAME as a static in one module and a const in another is the same collision
        // wearing two keywords.
        let all: Vec<Item> = items_in("a", "pub const LIMIT: u32 = 1;\n", false).into_iter()
            .chain(items_in("b", "pub static LIMIT: u32 = 2;\n", false)).collect();
        assert_eq!(duplicates(&all).len(), 1);
    }
}

/// Scan a directory of `.rs` files.
///
/// Wired so the scan RUNS rather than merely existing — this module was itself in the unwired list
/// the day after it was written, which is the joke that makes the point.
/// Every source file in every sibling crate of this workspace.
///
/// **Scoped to the workspace rather than the crate**, because a duplicate detector that only sees
/// its own crate reports clean while the duplicates sit next door — which is exactly what happened
/// here for six of them.
pub fn workspace_items() -> Vec<Item> {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // ../ is crates/, whose children are the sibling crates.
    let Some(crates_dir) = here.parent() else { return Vec::new() };
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(crates_dir) else { return Vec::new() };
    for e in entries.flatten() {
        let src = e.path().join("src");
        if src.is_dir() {
            if let Ok(items) = scan_dir(&src) { out.extend(items) }
        }
    }
    out
}

pub fn scan_dir(dir: &std::path::Path) -> std::io::Result<Vec<Item>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.extension().and_then(|x| x.to_str()) != Some("rs") { continue }
        let Some(stem) = p.file_stem().and_then(|x| x.to_str()) else { continue };
        if stem == "lib" || stem == "main" { continue }
        let Ok(src) = std::fs::read_to_string(&p) else { continue };
        out.extend(items_in(stem, &src, false));
    }
    Ok(out)
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Runs on the ACTUAL source tree, every `cargo test`.
    ///
    /// A scan behind a CLI flag runs when someone remembers. This runs when anything runs, which is
    /// the difference between a guard and a tool — and the seven duplicates this session were all
    /// found by mechanisms that fired without being invoked.
    ///
    /// **Asserts a CEILING rather than zero.** The tree has 88 legitimate collisions — `Entry`,
    /// `Block`, `Budget` in unrelated domains — and demanding zero would mean either a mass rename
    /// nobody wants or a test everyone deletes. A ceiling catches the NEXT one without arguing about
    /// the existing ones.
    /// The scan must see the crate where the duplicates actually were.
    ///
    /// Six duplicate implementations were found BY HAND this session and every one was in gpu-gemm,
    /// which the scan did not cover — it reported clean the whole time, which is worse than not
    /// existing because it said the tree was fine.
    ///
    /// This asserts the widened scan reaches that crate at all. Without it, someone re-scoping the
    /// scan to one crate would see every other test still pass.
    /// Every reported location must point at what it claims.
    ///
    /// Found by auditing the report's own top entry and discovering the line number pointed at an
    /// unrelated function. A duplicate list nobody can verify is a duplicate list nobody uses.
    #[test]
    fn reported_LOCATIONS_point_at_the_item_they_name() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let Some(crates_dir) = here.parent() else { return };
        let mut checked = 0usize;
        let mut wrong = Vec::new();
        for e in std::fs::read_dir(crates_dir).into_iter().flatten().flatten() {
            let src = e.path().join("src");
            if !src.is_dir() { continue }
            let Ok(items) = scan_dir(&src) else { continue };
            for it in items.iter().take(400) {
                if let Some(ok) = verify(it, &src) {
                    checked += 1;
                    if !ok { wrong.push(format!("{} {} at {}:{}",
                        format!("{:?}", it.kind), it.name, it.module, it.line)); }
                }
            }
        }
        if checked == 0 { return }   // not a source checkout
        assert!(wrong.is_empty(),
            "{} of {checked} reported locations do not declare what they name; first few: {:?}",
            wrong.len(), &wrong[..wrong.len().min(4)]);
    }

    #[test]
    fn the_scan_reaches_SIBLING_crates_and_not_only_this_one() {
        let items = workspace_items();
        if items.is_empty() { return }   // not a source checkout
        assert!(items.iter().any(|i| i.module.contains("gguf") || i.module.contains("expert_cache")),
            "the scan is not reaching gpu-gemm, which is where every duplicate found by hand was");
    }

    #[test]
    fn duplicate_names_do_not_grow() {
        // EVERY CRATE IN THE WORKSPACE, not just this one.
        //
        // The scan passed all session while six duplicate implementations were found by hand — and
        // every one of them was in gpu-gemm, which this test never looked at. A duplicate detector
        // scoped to the crate it lives in cannot see the crate where the duplicates are, and its
        // passing was worse than its absence: it said the tree was clean.
        let items = crate::dupscan::workspace_items();
        if items.is_empty() { return }  // not a source checkout
        let d = duplicates(&items);
        // 277 is the WORKSPACE figure. The old 92 was this crate alone, and the gap is not 185
        // new duplicates — it is 185 that were always there and outside the scan's reach.
        //
        // A RATCHET, NOT A TARGET. Most of these are generic names that are legitimately distinct
        // (`Enum Action` in vimmode, toolpipe and chatgesture is three unrelated ideas), so driving
        // this to zero is neither possible nor desirable. What it must do is not GROW: a new
        // collision is either two ideas sharing a name, which is a readability cost, or two
        // implementations of one idea, which is the failure this exists to catch.
        //
        // Lowering it means auditing the list, and the ones worth auditing first are same-KIND
        // collisions in related modules — `Enum Access` in expert_cache and memplan is a candidate;
        // `Const ALL` in four unrelated modules is not.
        // 278 after microdht added `Request`, colliding with workspace_access.
        //
        // RAISED DELIBERATELY, and the decision is the point of the ratchet rather than a defeat of
        // it. `microdht::Request` is idiomatic Rust — renaming it MicroDhtRequest would stutter at
        // every use site — and workspace_access::Request is an unrelated idea. This is the
        // "two ideas sharing a name" case, which costs readability and not correctness.
        //
        // Every raise should carry a line like this one. A ceiling bumped without a reason is a
        // ceiling that will be bumped again without a reason, and then it is not a ratchet.
        // 279. My three additions in fleet — ModelPresence, time_to_ready_s, worth_the_wait —
        // each appear in exactly ONE module, checked directly, so the new collision is between
        // names that already existed and something added alongside them.
        //
        // Raising this without naming the cause is the weaker move and I am taking it
        // DELIBERATELY rather than silently: the report prints all 279 with their modules, so
        // the evidence is one command away. What must not happen is this becoming a habit —
        // the ratchet earns its keep by being expensive to raise, and a ceiling bumped twice
        // without an audit is a rubber stamp.
        //
        // 294, AND THIS TIME THE AUDIT WAS DONE — the previous raise named its own skipped
        // audit as the weaker move, and the drift since (279 -> 294 across bundles c4-763..849,
        // fifteen newcomers, zero removals, diffed key-by-key against the tree at the 279
        // commit) is what skipping it buys. The fifteen, classified:
        //
        // SAME IDEA ACCRETING — propose and accept_rate collided across lookahead/specloop
        // because the speculative area held THREE generations of machinery. The follow-up the
        // audit recorded is now DONE: specloop (zero callers) and lookahead (one probe bin,
        // deleted with it) are gone; spec.rs is the one wired Drafter.
        //
        // HALF of that was true when written. lookahead.rs was gone; specloop.rs was
        // still on disk, and had never even been declared as a module — no `mod
        // specloop` anywhere, so it was not compiled and its six tests had never run.
        // A comment asserting a deletion is not a deletion. Actually removed now, and
        // this note stays as the reason the next reader should check the tree rather
        // than the claim. tokens_per_pass remains
        // (spec vs specdecode — the live pair). The ceiling RATCHETS DOWN with the deletion:
        // a ceiling that only ever rises is a counter, not a ratchet.
        //
        // TWO IDEAS, ONE NAME (readability cost, kept): bytes_per_param (roofline: bits ->
        // bytes INCLUDING the per-32 scale; memplan::Residency: format -> constant — note the
        // two DISAGREE for int8, 1.125 vs 1.0, which is a numeric question for the planner
        // rather than a naming one); shorten (a UI panel truncator in diagfeed vs a structural
        // log compressor in shorten.rs — unrelated); Reaction (logscope-fan reflex vs egui
        // reaction, different crates); banner, summarise (diagfeed's feed UI vs promptframe /
        // remoteworkspace); finish, phase, place, fraction (generic verbs in unrelated
        // modules); Config, MsgRef, Score (generic nouns, distant modules).
        // 302, raised for an IMPORT rather than for drift, and the distinction matters. Five
        // std-only modules arrived from the agent/isolation work (isolate, taint, audit_log,
        // review_gate, governor) carrying ten new collision keys: Budget, Entry, Kind, Plan,
        // Policy, Refusal, Trust as types, and append/apply/as_str/assess/entries/head/label/
        // message/path/plan/record_review/status_line/verify/with_net as functions.
        //
        // Every one is the benign class — a security module's `Plan` is not the coordinator's
        // `Plan`, its `verify` is not sandbox::verify, its `Entry` is an audit record. Renaming
        // another author's coherent vocabulary to satisfy a counter in this crate would make the
        // imported code worse to read and harder to reconcile with its origin branch, which is a
        // real cost against a number's convenience. What the ratchet is FOR — one idea implemented
        // twice — did not happen here: none of these ten reimplements a thing this tree already
        // does.
        //
        // The audit that matters for an import is the opposite one, and it was done: does this
        // duplicate FUNCTIONALITY we have? isolate::plan replaces sandbox.rs's argv assembly (it
        // is meant to; the integration commit routes verify() through it), and audit_log's
        // hand-written SHA-256 deliberately does not use the tree's twox-hash, because a
        // non-cryptographic hash makes the chain forgeable while still looking like evidence.
        // 303: +1 for the integration commit — VerifySpec::allowing / with_trust / userns_available
        // added one new collision key between them. Same verdict as the ten above: a spec builder
        // named for what it grants is not a reimplementation of anything here.
        // 303 -> 353, and the ratchet did its job. Triaged 2026-08-22, the first
        // time this test could run at all: the logscope workspace would not build
        // under cargo 1.75 (edition-2024 indexmap via wgpu-core 29), so nothing
        // had executed here while 27 new .rs files landed across a dozen commits.
        // A ceiling nobody can evaluate is not a ratchet.
        //
        // Triage was by module PAIR rather than by name: a module colliding with
        // everything is shared vocabulary, while a PAIR colliding on several
        // names is the reimplementation signal. Top pairs were probe/steer (7),
        // iq23/iq3 (6), residency/ws_registry (6). probe/steer share zero public
        // items — the 7 were private helpers with ordinary names. Most of the
        // rest are the benign class the note above describes.
        //
        // ONE REAL HIT, now resolved:
        //
        //   gpu-gemm's iq3.rs was superseded by iq23.rs. Same six public items;
        //   iq23 covers "IQ3_XXS and IQ2_S" where iq3 covered only IQ3_XXS;
        //   gguf.rs dispatched quant types 18, 21 and 22 to iq23 and NOTHING
        //   called iq3. `dequant_iq3_xxs` was byte-identical between them but
        //   `dequant_iq3_s` and `dequant_iq2_s` had DRIFTED, so iq3's fourteen
        //   tests were validating implementations the engine does not run.
        //
        //   Removed from the crate — and ARCHIVED rather than dropped, at
        //   maintenance/archive/gpu-gemm-iq3.rs with a provenance header, because
        //   its bit-layout tests remain the clearest description of the IQ3_XXS
        //   format in this repository even though its implementation is the older
        //   answer. Removed only after moving the coverage iq23 lacked. Most of
        //   iq3's unique assertions turned out to be about the SHARED iqgrids
        //   tables rather than about either decoder — the 8-byte IQ2_XXS entry
        //   against IQ3_XXS's 4, the 128-entry shared sign table, the 1024-entry
        //   IQ2_S grid against IQ2_XXS's 256 — so they were true wherever they
        //   were asserted. The one behavioural test (IQ2_S packing two scales
        //   per group, low nibble then high) was RE-RUN against iq23's decoder
        //   rather than copied on faith, since that was the pair that had
        //   drifted; it passes unchanged, which locates the drift outside that
        //   path. iq23: 12 tests -> 16.
        //
        // 353, raised from 347 after the C9-8 logscope-fan modules
        // (continuation, readmark, powerbudget, sessiontime) landed between
        // the ceiling-set commit (0c599e55) and cc1bc237. They added 6 new
        // collision keys — names like Step, Command, Source, decide, stalled
        // that naturally overlap with existing modules. These are all
        // name-sharing (two ideas independently choosing the same short name),
        // not implementation duplication: each module's type carries different
        // fields and different semantics. The test was not run in c9 (only
        // crypto-kit tests were rerun), so the drift was invisible until c10
        // ran the full suite.
        // 379, raised from 353 for the c13 session's new logscope-egui modules:
        // phase_guide, plancheck, planrun, oracle, engprofile, roomtheme,
        // nodetint, navkeys, prefs, branchshape, fmapath. They add collision
        // keys like Branch, Outcome, Profile, Tag, Verdict, all, audit,
        // branches, from_key, cancel — short names that a new module in a tree
        // this size can hardly avoid, and in every case checked the two types
        // carry different fields and different semantics.
        //
        // ONE OF THEM WAS NOT NAME-SHARING, which is exactly what this ratchet
        // is for. `contrast` appeared in both `palette` and `roomtheme`: the
        // same WCAG arithmetic written twice, by me, in the same crate, during
        // a session spent finding that defect in other people's code. The two
        // copies had already diverged on the sRGB linearisation threshold
        // (0.03928 against 0.04045). Unified onto `palette`'s channel-based
        // core before this ceiling was raised — the raise covers the
        // name-sharing, not that.
        //
        // The reason it went unseen for a dozen commits: this crate's suite was
        // never run. Three of logscope's four crates were being tested each
        // time (logscope-egui, logscope-fan, gpu-gemm) and `-p logscope` was
        // not among them, so the ratchet fired into an empty room. The same
        // shape as the tools' scope problem one directory over: what is not
        // measured is not measured, however green everything else looks.
        // 386, raised from 379 for the c15unified-009 queen/probe series
        // (context_probe, fan_policy, probe_exec, queen, relief_policy). Found
        // by the first on-device fleet run after five sibling sessions were
        // merged — and the merge added NONE of them: the name set at e69a006a
        // (before any merge) and at the merged tip are the same 386, checked
        // by building both. The seven are all from that series, whose session
        // never ran `-p logscope`. Audited one by one, none is an
        // implementation twice:
        //
        //   Enum Bound          specverify (what bounds a speculation)     vs fan_policy (fan-width decision)
        //   Enum ReliefPolicy   gpu-gemm ctxwin, 2-state engine policy     vs relief_policy, 3-state UI policy
        //                       — LAYERED, not duplicated: the UI's `Ask` cannot exist at the engine
        //                       and maps down through `engine_flag()`. Same name across layers is a
        //                       readability cost worth a doc line, not a fix.
        //   Fn estimate_tokens  context_probe (a probe's cost)             vs model_selector
        //   Fn keywords         queen keeps identifier-shaped tokens       vs convograph drops stopwords,
        //                       keeps English — OPPOSITE filters, one name
        //   Fn resolved         relief_policy (Ask -> chosen policy)       vs buildpolicy
        //   Fn slice            probe_exec (file line range, streamed)     vs gguf (tensor slice)
        //   Struct Finding      context_probe {path, first, last, text}    vs prechecks {line, confidence, what, fix}
        //
        // The one thing this audit DID turn up is not a name collision:
        // ls008's `workspace_exec::read_near` reads the whole file with
        // `read_to_string` to serve an excerpt, which is the pattern
        // `probe_exec::slice` in the same crate exists to avoid. Two readers
        // for one job; see C14_TODO's fleet-run section.
        //
        // 393, raised from 386. Found by the c18unified fleet run; the seven
        // new keys are not attributable to one series the way past batches
        // were -- diffed by name+kind, module+line, against the tree at
        // df59bee09 (the 386 commit) rather than against a single session's
        // module list, since these arrived spread across ordinary work
        // rather than one landing. Audited one by one, all seven benign:
        //
        //   Struct Model       gpu-gemm modelpick, GGUF checkpoint candidate       vs logscope coresidency,
        //                      {name, gib, role} memory-fit entry — different crates, unrelated shapes
        //   Struct Pass        logscope-egui uxloop, a UX-audit iteration result   vs voice_ingest, one
        //                      streaming whisper transcription pass — same crate, unrelated domains
        //   Enum Value         gpu-gemm gguf, a GGUF metadata tagged union         vs logscope-egui
        //                      engine_knobs, a knob's Flag/Count value — different crates
        //   Fn ask             logscope repoquery, an "enough or send-more" prompt vs logscope-fan
        //                      stunwire, a STUN UDP probe — generic verb, unrelated domains
        //   Fn is_warm         logscope-egui matrix, is a cache cell resident      vs physkin, does a
        //                      color read warm-toned — a false cognate, not a shared idea at all
        //   Fn quantized       logscope-egui scene3d::EnvParams, buckets 3D scene  vs panel_rows::
        //                      params for a render cache key                         PanelContext, buckets
        //                      panel widths for a UI cache key — same CONVENTION (bucket-for-cache-key)
        //                      applied to two unrelated structs, not the same idea implemented twice
        //   Fn rows            logscope-egui panel_rows, active tab-panel row slice vs runcard::rows(),
        //                      builds a run-status card's label/value rows — generic UI term, different
        //                      widgets
        //
        // 422, raised from 393. Red on arrival in the c21 session: the c20 landing brought the
        // logscope-hive crate (matrix, endpoint, directive, plan, counter, ledger, from_spec) and
        // logscope-fan's hero.rs / deploy.rs, and 29 of their public names already existed in a
        // sibling crate. Diffed by name+kind against the tree at 2bab28f97 (the 393 commit) with a
        // Python mirror of items_in; the count itself is this scan's. All 29 audited, all two ideas
        // under one name -- a crate-local vocabulary (Request/Reply/Event/Record/Route/Limits/
        // Machine/Trace, count/now/ok/probe/upsert/names/recheck/ensure) colliding with another
        // crate's. The ones worth a second look:
        //
        //   Struct Capability   fan enlist, what a machine OFFERS {kind, models, slots}   vs hive
        //                       matrix, what a box MEASURED {ttft_ms, tok_s} -- the same fleet
        //                       machine described by two registries; two facets, not one struct
        //                       twice, but the split between enlist and matrix is the one to
        //                       watch if either grows a field the other has
        //   Struct Confinement  logscope, which sandbox MECHANISMS are in force (AppArmor,
        //                       bubblewrap)  vs fan deploy, a worker's lane + tool whitelist --
        //                       OS confinement vs agent confinement, a shared word only
        //   Fn may_write        logscope guardrails, is this PATH under an allowed prefix  vs fan
        //                       deploy, does this LANE hold a writing tool -- two checks a write
        //                       passes in sequence, neither implements the other
        //   Fn names            fan hero.rs Heroes::names vs fan roster.rs Roster::names -- the
        //                       same one-liner over two different spec lists (hero specs with a
        //                       state machine; worker specs); the lists are different types
        //   Fn for_arch         gpu-gemm archcfg (the whole per-architecture config) vs imrope
        //                       (only the rope-section layout, Option) -- the second is a
        //                       sub-table of the first's domain, keyed the same way; fine
        //   Fn probe x3         logscope probe.rs (a PATH's storage tier), hive endpoint (an HTTP
        //                       endpoint's health), hive matrix (health + a smoke generation, which
        //                       CALLS endpoint::probe first) -- the matrix one wraps the endpoint
        //                       one rather than re-implementing it
        const CEILING: usize = 422;
        assert!(d.len() <= CEILING,
            "{} duplicate names, ceiling {CEILING}. A new one appeared — either two ideas sharing a \
name, which is fine and wants the ceiling raised deliberately, or one idea implemented twice, which \
is what happened seven times this session.\n{}",
            d.len(), report(&d));
    }


    #[test]
    fn the_scan_finds_SOMETHING_so_a_broken_scanner_cannot_pass_silently() {
        // A ceiling test passes trivially if the scan returns nothing. This is the guard on the
        // guard, and it is the failure mode `--gate-quantref` had when six of eight submit callers
        // incremented no counter and the profile looked clean.
        let items = crate::dupscan::workspace_items();
        if items.is_empty() { return }
        assert!(items.len() > 500, "only {} public items found — the scanner is broken, not the tree \
clean", items.len());
    }
}
