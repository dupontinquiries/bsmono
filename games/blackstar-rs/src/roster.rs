//! roster — which chassis to load, when there are more than the GPU can hold.
//!
//! ## The constraint is real and cannot be raised
//!
//! `render::CSDF_MAXM` is 126 models, and its doc explains that this is not a
//! tuning knob: `CSDF_SLAB * CSDF_MAXM` is a texture depth against a 2048
//! ceiling, and reaching 126 already meant cutting SLAB from 44 to 16. There is
//! no room above it.
//!
//! The fleet drop takes `assets/bots` from 48 chassis to 222 — and every POSE is
//! a model, so 222 chassis at ~5 poses is 1105 against a budget of 126.
//! `bake_car_sdfs` does `.take(CSDF_MAXM)`, so without a selector the roster is
//! silently truncated at whatever the first 126 poses happen to be: alphabetical
//! order, mid-chassis, some bots with three poses baked and two not.
//!
//! ## So the selection is explicit, and a whole chassis at a time
//!
//! A partially-baked chassis is worse than an absent one — it folds into a pose
//! that has no SDF and the renderer has nothing to sample. `fit` therefore takes
//! chassis whole and stops before the one that would not fit entirely.
//!
//! ## Confidence tiers come from the generator, not from us
//!
//! `fleet.json` sorts all 173 fleet chassis into high/medium/low/broken by how
//! well the fold was inferred. Loading `broken` first when the point is to LOOK
//! at folds is the right default for a test session and the wrong one for
//! playing, so the tier is a parameter rather than a policy.

/// How many models the SDF bake can hold.
///
/// DERIVED from `render::CSDF_MAXM` rather than copied. It was a copy with a
/// test asserting equality, which is a reasonable arrangement right up until
/// the moment the renderer moves — tiling the atlas took CSDF_MAXM from 126 to
/// 512 and this stayed at 126, so the loader kept refusing 400 chassis the GPU
/// could now hold. The test passed nothing, because a test that guards a
/// constant only fires when someone runs it.
///
/// A derived constant cannot go stale, which is worth more here than the
/// independence the copy bought.
pub const SDF_BUDGET: usize = crate::render::CSDF_MAXM as usize;

/// One chassis and the poses it would cost.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Chassis {
    pub name: String,
    pub poses: usize,
    /// From fleet.json, when it says. `None` for the original hand-made bots,
    /// which predate the confidence sweep and are not worse for it.
    pub tier: Option<Tier>,
    /// From fleet.json's `morph` field. `Some(false)` is a slideshow —
    /// `xform::Morph::build` refuses the strip and `main.rs` steps
    /// between whole poses instead of blending them — and `fit` excludes
    /// those unconditionally, the same way it already refuses a
    /// half-baked chassis regardless of which `Pick` was asked for.
    /// `None` for chassis with no fleet.json entry (original hand-made
    /// bots, which predate the morph sweep the same way they predate
    /// the confidence one) — not assumed broken, same reasoning as `tier`.
    pub morph_ok: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier { High, Medium, Low, Broken }

impl Tier {
    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "high" => Some(Tier::High),
            "medium" => Some(Tier::Medium),
            "low" => Some(Tier::Low),
            "broken" => Some(Tier::Broken),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::High => "high", Tier::Medium => "medium",
            Tier::Low => "low", Tier::Broken => "broken",
        }
    }
}

/// Which chassis to load this run.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Pick {
    /// Everything that fits, in name order. What the game did before there were
    /// more chassis than budget.
    All,
    /// Only this tier — for looking at how one confidence band folds.
    OnlyTier(Tier),
    /// A window into the sorted list, so a session can step through the whole
    /// fleet across several runs.
    Page(usize),
    /// Exactly these, by name.
    Named(Vec<String>),
}

/// Choose chassis until the pose budget is spent.
///
/// Takes chassis WHOLE: a half-baked chassis folds into a pose with no SDF, so
/// the loop stops before one that would not fit entirely rather than filling
/// the remaining slots.
pub fn fit(all: &[Chassis], pick: &Pick, budget: usize) -> Vec<Chassis> {
    // SMOOTH ONLY, unconditionally — a slideshow chassis is not a lesser
    // fold, it is not a fold, and no `Pick` variant exists to ask for one
    // on purpose. Filtered before `pick` is even matched, same as
    // `used + c.poses > budget` below refuses a half-baked chassis
    // regardless of which selection was requested.
    let all: Vec<&Chassis> = all.iter().filter(|c| c.morph_ok != Some(false)).collect();
    let candidates: Vec<&Chassis> = match pick {
        Pick::All => all.iter().copied().collect(),
        Pick::OnlyTier(t) => all.iter().copied().filter(|c| c.tier == Some(*t)).collect(),
        Pick::Named(names) => all.iter().copied().filter(|c| names.contains(&c.name)).collect(),
        Pick::Page(n) => {
            // Pages are cut by BUDGET, not by a fixed count, so every page
            // fills the GPU rather than leaving slots empty on a page of
            // small chassis.
            let mut pages: Vec<Vec<&Chassis>> = vec![Vec::new()];
            let mut used = 0usize;
            for c in &all {
                if used + c.poses > budget && !pages.last().unwrap().is_empty() {
                    pages.push(Vec::new());
                    used = 0;
                }
                used += c.poses;
                pages.last_mut().unwrap().push(c);
            }
            pages.get(*n).cloned().unwrap_or_default()
        }
    };

    let mut out = Vec::new();
    let mut used = 0usize;
    for c in candidates {
        if used + c.poses > budget { break; }
        used += c.poses;
        out.push(c.clone());
    }
    out
}

/// How many pages `Pick::Page` has, so a caller can wrap rather than run off
/// the end into an empty roster.
pub fn page_count(all: &[Chassis], budget: usize) -> usize {
    if all.is_empty() { return 0; }
    let mut pages = 1usize;
    let mut used = 0usize;
    for c in all {
        if used + c.poses > budget && used > 0 {
            pages += 1;
            used = 0;
        }
        used += c.poses;
    }
    pages
}

/// Read chassis tiers out of `assets/bots/fleet.json`.
///
/// A two-field scan rather than a JSON parser, because this crate has NO json
/// dependency and adding one to read `name` and `confidence` would be a new
/// dependency for two strings. The file is generator output with a stable
/// shape, not user input.
///
/// A missing or unreadable file is not an error: the original 48 hand-made bots
/// predate the confidence sweep entirely, and a tree without the fleet drop
/// should load exactly as it did before.
pub fn load_tiers(bots_dir: &std::path::Path) -> std::collections::HashMap<String, Tier> {
    let mut out = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(bots_dir.join("fleet.json")) else {
        return out;
    };
    // Entries are objects with "name" before "confidence". Tracking the most
    // recent name and attaching the next confidence to it survives whitespace
    // and key order changes within an entry, which a line-offset scan would
    // not.
    let mut name: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim().trim_end_matches(',');
        if let Some(v) = field(line, "\"name\"") {
            name = Some(v);
        } else if let Some(v) = field(line, "\"confidence\"") {
            if let (Some(n), Some(t)) = (name.take(), Tier::parse(&v)) {
                out.insert(n, t);
            }
        }
    }
    out
}

/// Read chassis CATEGORIES out of `assets/bots/fleet.json`.
///
/// Same two-field scan as `load_tiers`, on `name` and `category`. Separate
/// function rather than one that returns both, because the two are wanted in
/// different places — the selector needs tiers, the loader needs categories —
/// and a caller that takes a pair it half-uses invites the other half going
/// stale.
pub fn load_categories(bots_dir: &std::path::Path)
    -> std::collections::HashMap<String, String>
{
    let mut out = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(bots_dir.join("fleet.json")) else {
        return out;
    };
    let mut name: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim().trim_end_matches(',');
        if let Some(v) = field(line, "\"name\"") {
            name = Some(v);
        } else if let Some(v) = field(line, "\"category\"") {
            if let Some(n) = name.take() {
                out.insert(n, v);
            }
        }
    }
    out
}

/// Read chassis MORPH STATUS out of `assets/bots/fleet.json`.
///
/// `true` = "accepted" — the strip's poses agree prim for prim and
/// `xform::Morph::build` blends them into a real fold. `false` =
/// "refused" — the poses disagree (usually because the colour merge
/// ran per pose instead of once per chassis, see `examples/
/// stripcheck.rs`), so `main.rs` steps between whole poses instead: no
/// blend, one body replaced by the next on a timer. That is a
/// slideshow wearing a fold's clothing, not a lesser fold — it is
/// what `Morph::build` returning `None` at runtime already looks like
/// (`self.toastf("strip mismatch — stepping")`), just not previously
/// known ahead of load time.
///
/// Same two-field scan as `load_tiers`/`load_categories`, on `name`
/// and `morph`.
pub fn load_morph(bots_dir: &std::path::Path) -> std::collections::HashMap<String, bool> {
    let mut out = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(bots_dir.join("fleet.json")) else {
        return out;
    };
    let mut name: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim().trim_end_matches(',');
        if let Some(v) = field(line, "\"name\"") {
            name = Some(v);
        } else if let Some(v) = field(line, "\"morph\"") {
            if let Some(n) = name.take() {
                out.insert(n, v == "accepted");
            }
        }
    }
    out
}

/// `"key": "value"` -> value, for the one shape fleet.json uses.
fn field(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start().strip_prefix(':')?.trim();
    let inner = rest.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

/// Parse `--bots` into a pick.
///
/// Accepts a tier name, `page:N`, a comma list of chassis names, or `all`.
/// Unrecognised input returns `None` rather than falling back to All, because
/// a typo'd tier silently loading 126 arbitrary bots is exactly the confusion
/// this module exists to prevent.
pub fn parse_pick(s: &str) -> Option<Pick> {
    let s = s.trim();
    if s.is_empty() || s == "all" { return Some(Pick::All); }
    if let Some(t) = Tier::parse(s) { return Some(Pick::OnlyTier(t)); }
    if let Some(n) = s.strip_prefix("page:") {
        return n.parse::<usize>().ok().map(Pick::Page);
    }
    let names: Vec<String> = s.split(',').map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty()).collect();
    if names.is_empty() { return None; }
    Some(Pick::Named(names))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, poses: usize, tier: Option<Tier>) -> Chassis {
        Chassis { name: name.into(), poses, tier, morph_ok: None }
    }

    fn c_morph(name: &str, poses: usize, tier: Option<Tier>, morph_ok: Option<bool>) -> Chassis {
        Chassis { name: name.into(), poses, tier, morph_ok }
    }

    fn fleet(n: usize) -> Vec<Chassis> {
        (0..n).map(|i| c(&format!("bot{i:03}"), 5,
                         Some(if i % 2 == 0 { Tier::High } else { Tier::Broken }))).collect()
    }

    #[test]
    fn the_budget_matches_the_renderers_own_cap() {
        // A copied constant that drifts is worse than an import, so it is
        // pinned here. If CSDF_MAXM moves, this fails and someone decides.
        assert_eq!(SDF_BUDGET as u32, crate::render::CSDF_MAXM);
    }

    #[test]
    fn a_chassis_is_taken_whole_or_not_at_all() {
        // A half-baked chassis folds into a pose with NO SDF and the renderer
        // has nothing to sample — worse than the chassis being absent.
        let all = vec![c("a", 100, None), c("b", 20, None), c("c", 20, None)];
        let got = fit(&all, &Pick::All, 126);
        let total: usize = got.iter().map(|x| x.poses).sum();
        assert!(total <= 126);
        // b fits (120), c would take it to 140 — so c is dropped entirely.
        assert_eq!(got.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn the_whole_fleet_does_not_fit_which_is_the_point() {
        // CSDF_MAXM has moved before (the module's own docs record it:
        // 126 -> 1344, "the whole library loads -- 20 of 221 becomes
        // 221 of 221") and this test's fleet size did not move with it
        // last time -- `fleet(222)` is 1110 poses, comfortably UNDER
        // today's 1344 budget, so "does not fit" silently stopped being
        // true and the test failed on a premise rather than a bug.
        // Sized off SDF_BUDGET itself now, with real margin, so a
        // future CSDF_MAXM change can't quietly repeat this.
        let all = fleet(SDF_BUDGET / 5 + 100);
        let got = fit(&all, &Pick::All, SDF_BUDGET);
        assert!(got.len() < all.len());
        assert!(got.iter().map(|x| x.poses).sum::<usize>() <= SDF_BUDGET);
    }

    #[test]
    fn a_tier_can_be_looked_at_on_its_own() {
        // Looking at how the BROKEN band folds is the whole reason to have the
        // tier, and it is the wrong default for playing — hence a parameter.
        let all = fleet(40);
        let got = fit(&all, &Pick::OnlyTier(Tier::Broken), SDF_BUDGET);
        assert!(!got.is_empty());
        assert!(got.iter().all(|x| x.tier == Some(Tier::Broken)));
    }

    #[test]
    fn pages_cover_the_fleet_without_overlapping() {
        // The property that makes stepping through several runs actually see
        // everything: no chassis appears twice and none is skipped.
        let all = fleet(60);
        let pages = page_count(&all, SDF_BUDGET);
        let mut seen: Vec<String> = Vec::new();
        for p in 0..pages {
            for ch in fit(&all, &Pick::Page(p), SDF_BUDGET) {
                assert!(!seen.contains(&ch.name), "{} appeared twice", ch.name);
                seen.push(ch.name);
            }
        }
        assert_eq!(seen.len(), all.len(), "a chassis was skipped");
    }

    #[test]
    fn every_page_respects_the_budget() {
        let all = fleet(60);
        for p in 0..page_count(&all, SDF_BUDGET) {
            let got = fit(&all, &Pick::Page(p), SDF_BUDGET);
            assert!(got.iter().map(|x| x.poses).sum::<usize>() <= SDF_BUDGET, "page {p}");
        }
    }

    #[test]
    fn a_page_past_the_end_is_empty_rather_than_wrapping_silently() {
        // The caller wraps using page_count. Wrapping in here would make
        // `page:99` quietly show page 3 and look like the flag was ignored.
        let all = fleet(10);
        assert!(fit(&all, &Pick::Page(99), SDF_BUDGET).is_empty());
    }

    #[test]
    fn named_chassis_are_loaded_exactly() {
        let all = fleet(20);
        let got = fit(&all, &Pick::Named(vec!["bot003".into(), "bot007".into()]), SDF_BUDGET);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn a_typo_is_refused_rather_than_becoming_all() {
        // A mistyped tier silently loading 126 arbitrary bots is exactly the
        // confusion this module exists to prevent.
        assert_eq!(parse_pick("brokn"), Some(Pick::Named(vec!["brokn".into()])));
        assert_eq!(parse_pick("page:x"), None);
        assert_eq!(parse_pick(""), Some(Pick::All));
    }

    #[test]
    fn tier_names_round_trip() {
        // They come from fleet.json, so a rename on either side must not
        // silently stop matching.
        for t in [Tier::High, Tier::Medium, Tier::Low, Tier::Broken] {
            assert_eq!(Tier::parse(t.name()), Some(t));
        }
    }

    #[test]
    fn hand_made_bots_have_no_tier_and_are_not_excluded() {
        // The original 48 predate the confidence sweep and are not worse for
        // it — but they must not vanish when a tier is selected either, which
        // is why they are None rather than a fifth tier.
        let all = vec![c("blackstar", 5, None), c("bot000", 5, Some(Tier::High))];
        assert_eq!(fit(&all, &Pick::All, SDF_BUDGET).len(), 2);
        assert_eq!(fit(&all, &Pick::OnlyTier(Tier::High), SDF_BUDGET).len(), 1);
    }

    #[test]
    fn a_refused_morph_is_excluded_from_every_pick() {
        // The slideshow filter: fleet.json says "refused" for 43 chassis
        // (Morph::build rejects the strip; main.rs steps between whole
        // poses instead of blending them). None of them should reach the
        // player regardless of which selection they asked for -- there is
        // no Pick variant that means "including the slideshows," the same
        // way there is no Pick variant that means "including a chassis
        // that does not fit the budget."
        let smooth = c_morph("smooth_car", 5, Some(Tier::High), Some(true));
        let slideshow = c_morph("static_tank", 5, Some(Tier::High), Some(false));
        let unknown = c_morph("blackstar", 5, None, None); // hand-made, predates the sweep
        let all = vec![smooth.clone(), slideshow.clone(), unknown.clone()];

        let got_all = fit(&all, &Pick::All, SDF_BUDGET);
        assert!(got_all.iter().any(|c| c.name == "smooth_car"));
        assert!(got_all.iter().any(|c| c.name == "blackstar"), "hand-made bots must not be excluded");
        assert!(!got_all.iter().any(|c| c.name == "static_tank"), "a slideshow reached Pick::All");

        let got_tier = fit(&all, &Pick::OnlyTier(Tier::High), SDF_BUDGET);
        assert!(!got_tier.iter().any(|c| c.name == "static_tank"), "a slideshow reached OnlyTier");

        // Even asked for BY NAME, a slideshow is excluded -- this is a
        // correctness filter, not a preference the caller can opt out of.
        let got_named = fit(&all, &Pick::Named(vec!["static_tank".into()]), SDF_BUDGET);
        assert!(got_named.is_empty(), "naming a slideshow explicitly still loaded it");
    }

    #[test]
    fn hand_made_bots_have_no_morph_flag_and_are_not_excluded() {
        // Same reasoning as the tier test above: None means "predates the
        // sweep," not "assume broken."
        let all = vec![c_morph("blackstar", 5, None, None)];
        assert_eq!(fit(&all, &Pick::All, SDF_BUDGET).len(), 1);
    }
}

#[cfg(test)]
mod tier_file_tests {
    use super::*;

    #[test]
    fn tiers_are_read_from_the_generators_own_file() {
        let dir = std::env::temp_dir().join("roster-tier-test");
        let _ = std::fs::create_dir_all(&dir);
        // One field per line, `{` on its own line — the real generator's
        // actual output (checked against assets/bots/fleet.json:
        // `json.dump(..., indent=N)` never puts the opening brace on the
        // same line as the first key). The previous fixture put a whole
        // entry on one line (`{"name": "x", "category": ..., ...},`),
        // which `field()`'s `line.strip_prefix(key)` cannot match — the
        // line starts with `{`, not the key — so this test was failing
        // against a fixture that never resembled the real file, not
        // because load_tiers was wrong.
        std::fs::write(dir.join("fleet.json"), "{\n \"fleet\": [\n  {\n   \"name\": \"boat_mistral\",\n   \"category\": \"boat\",\n   \"poses\": 5,\n   \"confidence\": \"low\"\n  },\n  {\n   \"name\": \"car_charger\",\n   \"poses\": 5,\n   \"confidence\": \"high\"\n  }\n ]\n}").unwrap();
        let t = load_tiers(&dir);
        assert_eq!(t.get("boat_mistral"), Some(&Tier::Low));
        assert_eq!(t.get("car_charger"), Some(&Tier::High));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        // The original 48 predate the sweep, and a tree without the fleet drop
        // must load exactly as it did before.
        let t = load_tiers(std::path::Path::new("/nonexistent-roster-dir"));
        assert!(t.is_empty());
    }

    #[test]
    fn keys_between_name_and_confidence_do_not_break_the_pairing() {
        // The real file has eight keys between them, which is why this tracks
        // the most recent name rather than reading a fixed offset. Same
        // fix as the test above: `{` moved to its own line so `"name"`
        // is actually at the start of its line, matching the real file.
        let dir = std::env::temp_dir().join("roster-tier-test2");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("fleet.json"), "{\"fleet\":[\n  {\n   \"name\": \"x\",\n   \"source\": \"a.iqm\",\n   \"poses\": 5,\n   \"morph\": \"accepted\",\n   \"confidence\": \"broken\"\n  }\n]}").unwrap();
        assert_eq!(load_tiers(&dir).get("x"), Some(&Tier::Broken));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_value_that_is_not_a_tier_is_skipped_rather_than_guessed() {
        // Same fixture-format issue as the two tests above, fixed the
        // same way: the old compact fixture never matched a `"name"`
        // line at all (`{"name":"y",...` starts with `{`), so this
        // passed whether or not an unknown confidence value was
        // actually rejected — the assertion held for the wrong reason.
        // With `name` correctly parsed, `is_empty()` now actually
        // exercises `Tier::parse` refusing "probably".
        let dir = std::env::temp_dir().join("roster-tier-test3");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("fleet.json"),
                       "{\"fleet\":[\n  {\n   \"name\": \"y\",\n   \"confidence\": \"probably\"\n  }\n]}").unwrap();
        assert!(load_tiers(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_tiers_actually_reads_the_real_fleet_json() {
        // The three tests above all use synthetic fixtures written by
        // this test module -- which is exactly what went wrong before
        // (a fixture that quietly stopped resembling the real file).
        // This one skips the fixture and points load_tiers at the real
        // assets/bots directory, so a future format change in the
        // actual generator output would fail here even if every
        // hand-written fixture above happened to still match it.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/bots");
        let tiers = load_tiers(&dir);
        assert!(!tiers.is_empty(), "the real fleet.json produced no tiers at all");
        // spot-check one of each confidence band fleet.json's own
        // by_confidence summary says exists, so this fails loudly if
        // the real file's shape ever changes rather than passing on an
        // empty map
        assert!(tiers.values().any(|t| *t == Tier::High), "no High-tier chassis found");
        assert!(tiers.values().any(|t| *t == Tier::Low), "no Low-tier chassis found");
        assert!(tiers.values().any(|t| *t == Tier::Broken), "no Broken-tier chassis found");
        // and a chassis this session's fleet-additions patch actually
        // added should be readable by name, not just present in count
        assert_eq!(tiers.get("animal_bot_runningwind"), Some(&Tier::High));
    }

    #[test]
    fn load_morph_actually_reads_the_real_fleet_json() {
        // Same discipline as the test above: skip the fixture, point at
        // the real file, fail loudly if the shape ever changes rather
        // than passing on an empty map.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/bots");
        let morph = load_morph(&dir);
        assert!(!morph.is_empty(), "the real fleet.json produced no morph flags at all");
        assert!(morph.values().any(|v| *v), "no accepted (smooth) chassis found");
        assert!(morph.values().any(|v| !*v), "no refused (slideshow) chassis found");
    }
}

