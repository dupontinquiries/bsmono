//! VOCABULARY — the licensed-mark census, as a test that can only go down.
//!
//! `docs/films/README.md` states the one hard constraint: this universe is
//! Coalition / Hijacks / Blackstar Clans / Corrupted / Rodimus, and NOT the
//! Paramount/Hasbro vocabulary. It also says the two current screenplays were
//! grepped clean. They are. **`src/` is not**, and until now nothing in the
//! tree said so — the rule was enforced on 2 files out of ~110.
//!
//! `saga.rs` already had the right idea for one file:
//! `the_inherited_contamination_is_counted_not_hidden` asserts an exact count
//! so that removing a term FAILS and prompts you to lower the number, while
//! adding one fails too. This is that discipline widened to the whole crate.
//!
//! ## Why a count and not a ban
//!
//! Because the cleanup is blocked on authoring, not on effort, and a test that
//! demands zero would simply be disabled. Renaming Megatron or Optimus Prime
//! means deciding what this universe calls those characters, and inventing it
//! inside a cleanup commit buries a creative decision where nobody will find
//! it. A census makes the debt visible and monotone instead.
//!
//! ## What the audit turned up, and it is not what `RENAME_MAP.md` said
//!
//! That document claimed `autobot`/`decepticon` "map onto vocabulary that
//! already exists and ships: Coalition and Blackstar Clans / Corrupted.
//! Mostly mechanical." **That is wrong**, and `content.rs` disproves it in one
//! line: *"elites — they are the thing the coalition and the Autobots both"*.
//! The code treats Coalition and Autobots as DIFFERENT factions, and
//! `patrol::Faction` bears that out — `Coalition` is annotated "the elemental
//! coalition", i.e. the clone races, not the robots.
//!
//! So the robot factions need names of their own, exactly like the two
//! characters do. The whole prose half of the filter is one authoring
//! decision, not four mechanical ones. Corrected here rather than only in the
//! doc, because a wrong plan in a document is a plan somebody follows.
//!
//! ## What is actually mechanical
//!
//! ~~`energon` (145) is a live identifier…~~ **DONE.** It was a module,
//! `energon()`, `rec_energon()`, `energon_value()`, `at_energon` and several
//! test names, so the rename was compiler-checked end to end — the safest of
//! the large ones despite being the biggest, and the only one that was blocked
//! on a word rather than on authoring. It is **`valence`** now: real electron
//! physics, and what it names is the capacity to combine into something else,
//! which is what the fuel does for a machine that transforms. It pairs against
//! `chi.rs` without echoing it — chi is what a body has become, valence is what
//! a machine has available.
//!
//! Note `\b` is useless for this rename: underscores are word characters, so
//! `\benergon\b` misses `energon_value` and `at_energon` entirely. A plain
//! substring replace is correct here because the term is not a substring of any
//! unrelated word.
//!
//! `cybertron` (269) is a home-planet name that has become a type
//! (`Body::Cybertronian`) and a module (`cybertronian.rs`). Also blocked on
//! authoring.
//!
//! `AllSpark` (2) should STAY. Both are doc comments citing the reference
//! material as the reason for a rendering technique — `compact.rs` explains
//! the displacement-field fold that way. That is developer attribution, not
//! in-fiction vocabulary, and deleting it removes the reasoning that makes the
//! module reviewable.

/// Filename fragments that mark a model as belonging to a rights holder, so
/// the catalogue can show those models as their own family (`main.rs`
/// `model_family`) instead of scattering them. They live HERE because this is
/// the one file the census below exempts: in `main.rs` each raised its count by
/// one and failed `the_licensed_vocabulary_census_only_goes_down`.
pub const IP_MARKS: &[&str] = &[
    "transformer", "decepticon", "autobot", "optimus", "bumblebee",
    "megatron", "starscream", "grimlock", "cybertron", "gundam", "sazabi",
    "halo", "master_chief", "star_wars", "marvel", "spider", "octopus",
    "ferrari", "lamborghini", "porsche", "chevrolet", "camaro", "alfa_romeo",
    "bugatti", "mclaren", "bmw", "audi", "toyota", "nissan", "benelli",
    "call_of_duty", "pacific_rim", "ben_10", "wuthering",
    ];

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    /// Terms and their current count across `src/*.rs`.
    ///
    /// Case-sensitive and counted separately, because the prose form and the
    /// identifier form come down in different commits and collapsing them
    /// would hide which half moved.
    const CENSUS: &[(&str, usize)] = &[
        // Cybertron was really THREE things and needed three answers.
        //
        // `Cybertronian` (126) was an adjective and a type — `Body::Machine`
        // now, and `machine.rs`. The type system should not take a side in a
        // naming dispute the fiction is about.
        //
        // `Cybertron` the PLACE (137) is `the Foundry` in prose and `foundry`
        // as a site id: what humans called a dead machine world when they
        // found it, obviously wrong and obviously evocative, which is the
        // Directorate's whole habit. The Enduring themselves still just call
        // it home, lowercase, untranslated.
        //
        // The residue is ASSET FILENAMES — `transformers_fall_of_cybertron_ark`
        // and friends, real files on disk naming a real game. Same category as
        // the decepticon tank below: shielded from the sweep, and the reason
        // this term can never reach zero.
        ("Cybertron", 2),
        ("cybertron", 3),
        ("Cybertronian", 0),
        ("cybertronian", 0),
        // DONE, and held at zero so a reintroduction fails rather than passing
        // silently. energon -> valence; Optimus Prime -> Keelwright;
        // Megatron -> Cleave; Jhiaxus -> Assay; Rodimus -> Halyard.
        ("Energon", 0),
        ("energon", 0),
        // NOT zero, and cannot be: `saga.rs`'s own two guards name these words
        // as DATA — `no_licensed_vocabulary`'s banned list and
        // `the_inherited_contamination_is_counted_not_hidden`'s needles. An
        // occurrence that exists only to detect reintroduction is a FLOOR, not
        // debt. Going below it means a guard was deleted.
        // 6 rather than 1: five are CITATIONS in grandeur.rs, a table of
        // measurements from real reference material where the label identifies
        // the source. A sweep renamed them once and turned a published 28 ft
        // production figure into a false attribution to a fictional character.
        // Floors, like AllSpark.
        ("Optimus", 6),
        ("optimus", 1),
        ("Megatron", 1),
        ("megatron", 1),
        ("Jhiaxus", 0),
        ("jhiaxus", 0),
        // Rodimus was seeded at 25 from a hand-grep while the census reported
        // 24: this file's own header quotes the allowed-vocabulary sentence
        // from docs/films/README.md, which contains the word. The census
        // excludes itself; the grep did not. The guard caught it on its first
        // real use.
        ("Rodimus", 0),
        ("rodimus", 0),
        // Renamed once the factions had names of their own: Autobots ->
        // the Enduring, Decepticons -> the Sundered. `content.rs` had proved
        // Coalition is NOT the same thing as the Autobots ("the thing the
        // coalition and the Autobots both"), so this was never the mechanical
        // rename RENAME_MAP.md first claimed.
        //
        // 1 rather than 0 for the same reason as Optimus and Megatron below:
        // saga.rs's two guards name these words as data, and a guard cannot
        // detect a word without containing it.
        ("Decepticon", 1),
        // 2, not 1: one is the guard needle, the other is an ASSET FILENAME —
        // `census.rs` carries `tank_incision_aka_decepticon_tank_m1978_koksan
        // _north_korea`, matched against a real catalogue file on disk.
        // Renaming it broke `census::the_census_matches_its_source`, which is
        // the right outcome: a licensed word inside a filename is a different
        // category from one in prose, and this term can never reach zero while
        // the file is named that.
        ("decepticon", 2),
        ("Autobot", 1),
        ("autobot", 1),
        // Missed by the first audit. Hasbro, and Jhiaxus was Marvel/IDW.
        // docs/films/README.md names Rodimus in the very sentence that states
        // the vocabulary rule, so the rule has a hole in its own wording.
        // Unicron -> the Unending. Identifiers took the bare word (arena
        // "unending", claim unending_in_earth) because an article inside a key
        // is wrong; prose took the article.
        ("Unicron", 0),
        ("unicron", 0),
        // SURFACED BY THE RENAME rather than by the audit. Substituting the
        // character names left "Halyard Prime" and "Hot Rod" standing in the
        // prose, with the Matrix of Leadership behind them.
        //
        // `Prime` is a TITLE as well as a name, so replacing it is a further
        // decision — what the Enduring call the office — and `Hot Rod` is
        // Halyard before he inherits it, so those two are one decision.
        // 25 rather than 24: one is inside saga.rs's "Optimus Prime" needle.
        // Prime was a TITLE as well as a name. The office is now "the Keel"
        // and the inheritance is a NAME CHANGE -- Jib becomes Halyard -- so
        // the title stopped being needed in the prose at all. The residue is
        // grandeur.rs's citations plus policy notes.
        ("Prime", 8),
        ("Hot Rod", 0),
        ("hot rod", 0),
        // Matrix of Leadership -> the Charge: what you are charged with, and
        // a word a machine culture would already own.
        //
        // 1, not 0: `gltf::scene::Transform::Matrix` in rig.rs is the gltf
        // crate's enum variant, a library's identifier and not vocabulary. The
        // rename sweep rewrote it to `Transform::the Charge { matrix }` and the
        // rigpose example stopped compiling (C20_TODO "rig.rs:96 is
        // corrupted"). The census cannot tell an API name from a mark any more
        // than a use from a mention, so this floor is where that one lives.
        ("Matrix", 1),
        // NOT counted: lowercase `prime` and `matrix`. Both have honest
        // non-licensed uses here — `prime` is a Troop id and a player class,
        // `matrix` is linear algebra in a 3D engine — so a raw count would be
        // mostly false positives and could never reach zero. They need
        // reading, not counting.
        //
        // KEEP: AllSpark. Developer attribution in `compact.rs` and
        // `xform.rs`, not in-fiction vocabulary — both explain why the
        // displacement-field fold is shaped as it is. Counted so a
        // well-meaning sweep that deletes them fails and reads this.
        // 3, not 2. The third is in grandeur.rs's new header, which explains
        // the keep-citations policy BY NAMING the exemplar. This is the same
        // phenomenon as the guard floors and it has now appeared four times in
        // this cleanup: WRITING ABOUT A MARK CREATES AN OCCURRENCE OF IT. The
        // census cannot distinguish a use from a mention, so every policy note
        // raises a floor. That is acceptable — the alternative is policy that
        // cannot name what it is about — but it means a count going up by one
        // after a documentation commit is expected, not alarming.
        // SCREAMING_CASE, missed by the first three passes of this cleanup.
        // The census is case-sensitive on purpose — prose and identifiers come
        // down in different commits — but nobody enumerated the third case,
        // and it is exactly where DISPLAY STRINGS and public statics live.
        // `patrol.rs` had `name: "CYBERTRON"` on a destination the player
        // reads, `survival.rs` returned "CYBERTRONIAN" as a label, and
        // `saga.rs` exported RODIMUS_PRIME_ARC. Nine in src/ and thirteen more
        // in the screenplays, all reported as zero because the census only
        // ever asked about two of the three cases.
        ("RODIMUS", 0),
        ("OPTIMUS", 0),
        ("MEGATRON", 0),
        ("UNICRON", 0),
        ("AUTOBOT", 0),
        ("DECEPTICON", 0),
        ("CYBERTRON", 0),
        ("ENERGON", 0),
        // 1, not 0: `corruption.rs` cites TF_JHIAXUS_AND_THE_CORRUPTED.md,
        // which is a real source filename. Same category as the asset names.
        ("JHIAXUS", 1),
        ("AllSpark", 3),
    ];

    /// Files a vocabulary sweep MUST NOT touch, and why.
    ///
    /// Every entry here has been damaged by a sweep at least once. They are
    /// listed rather than remembered because remembering has failed five times
    /// in one cleanup.
    ///
    /// * `saga.rs` — two guards name the licensed words as DATA (a banned-word
    ///   list and a contamination counter). A sweep rewrites the needles, and
    ///   the counter then reports the REPLACEMENT names as contamination,
    ///   which reads exactly like a reintroduction. Caught five times.
    /// * `grandeur.rs` — a citation table of measurements from real reference
    ///   material, where the label identifies the source. Renaming it
    ///   attributed a published 28 ft production figure to a fictional
    ///   character.
    /// * `vocab.rs` — this file.
    ///
    /// `no_sweep_has_eaten_a_guard` below fails if the needles ever stop being
    /// the licensed words, which is the mechanical version of this warning.
    pub const DO_NOT_SWEEP: &[&str] = &["saga.rs", "grandeur.rs", "vocab.rs"];

    /// This file names every term it counts, so counting itself would double
    /// every number and make the census meaningless.
    const SELF: &str = "vocab.rs";

    fn census() -> Vec<(&'static str, usize)> {
        let dir = Path::new("src");
        assert!(
            dir.is_dir(),
            "cannot find src/ — this test assumes cargo runs it from the package root"
        );
        let mut sources = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(p) = stack.pop() {
            for e in fs::read_dir(&p).expect("read_dir").flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs")
                    && path.file_name().is_some_and(|n| n != SELF)
                {
                    sources.push(fs::read_to_string(&path).expect("read"));
                }
            }
        }
        assert!(sources.len() > 50, "only found {} sources; the walk is wrong", sources.len());
        CENSUS
            .iter()
            .map(|(term, _)| (*term, sources.iter().map(|s| s.matches(term).count()).sum()))
            .collect()
    }

    #[test]
    fn the_licensed_vocabulary_census_only_goes_down() {
        let now = census();
        let mut moved = Vec::new();
        for ((term, expected), (_, actual)) in CENSUS.iter().zip(&now) {
            if expected != actual {
                moved.push(format!(
                    "  {term}: {expected} -> {actual} ({})",
                    if actual < expected { "DOWN, good — update the number" } else { "UP" }
                ));
            }
        }
        assert!(
            moved.is_empty(),
            "the licensed-vocabulary census changed:\n{}\n\nIf a count went DOWN, update \
             CENSUS in vocab.rs and know the debt shrank. If it went UP, something \
             reintroduced a licensed mark — see docs/films/README.md and \
             docs/RENAME_MAP.md.",
            moved.join("\n")
        );
    }

    #[test]
    fn nothing_new_has_crept_in_beside_the_terms_being_tracked() {
        // The census can only guard terms it knows about. This catches the
        // other obvious marks arriving, which is the failure mode a fixed list
        // has.
        let dir = Path::new("src");
        assert!(dir.is_dir());
        let mut stack = vec![dir.to_path_buf()];
        let mut hits = Vec::new();
        // Split so this file does not match its own needles.
        let untracked = [
            [b"Bumble".as_slice(), b"bee".as_slice()],
            [b"Starscre".as_slice(), b"am".as_slice()],
            [b"Soundwa".as_slice(), b"ve".as_slice()],
            [b"Prim".as_slice(), b"us".as_slice()],
        ];
        while let Some(p) = stack.pop() {
            for e in fs::read_dir(&p).expect("read_dir").flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|x| x != "rs")
                    || path.file_name().is_some_and(|n| n == SELF)
                {
                    continue;
                }
                let text = fs::read_to_string(&path).expect("read");
                for parts in &untracked {
                    let needle = String::from_utf8(
                        parts.iter().flat_map(|p| p.iter().copied()).collect(),
                    )
                    .unwrap();
                    if text.contains(&needle) {
                        hits.push(format!("{}: {needle}", path.display()));
                    }
                }
            }
        }
        assert!(hits.is_empty(), "untracked licensed vocabulary appeared:\n  {}", hits.join("\n  "));
    }


    #[test]
    fn no_sweep_has_eaten_a_guard() {
        // The recurring failure, mechanised. Five separate times in this
        // cleanup a blanket rename rewrote the needles inside saga.rs's own
        // guards, turning a licensed-mark detector into a detector of the
        // replacement names. Every time, the symptom was a count going UP and
        // looking like a reintroduction.
        //
        // A guard has to contain the word it guards against. So: assert it
        // still does.
        let saga = fs::read_to_string("src/saga.rs").expect("saga.rs");
        for needle in ["\"optimus prime\"", "\"megatron\"", "\"autobot\"", "\"decepticon\""] {
            assert!(
                saga.contains(needle),
                "saga.rs's banned-word list no longer contains {needle} — a sweep \
                 probably renamed the needle. See DO_NOT_SWEEP."
            );
        }
        for needle in ["count(\"Autobot\")", "count(\"Decepticon\")", "count(\"Megatron\")"] {
            assert!(saga.contains(needle), "saga.rs's contamination counter lost {needle}");
        }
    }

    #[test]
    fn the_census_is_measuring_a_real_tree() {
        // A walk that silently found nothing would make every count zero and
        // every assertion in this module pass.
        //
        // The first two versions of this anchored on the DEBT — "total > 400",
        // then "> 200" — and both were wrong in the same way: they conflate
        // "the walk works" with "the cleanup is unfinished". The debt fell from
        // ~680 to 51 and the anchor failed while nothing was broken, which is a
        // test reporting its own obsolescence as a defect.
        //
        // A positive control is the right shape. Count a word that is this
        // universe's OWN vocabulary, is everywhere, and will never be removed.
        // It goes to zero only if the walk is broken.
        let dir = Path::new("src");
        assert!(dir.is_dir());
        let mut stack = vec![dir.to_path_buf()];
        let (mut files, mut control) = (0usize, 0usize);
        while let Some(p) = stack.pop() {
            for e in fs::read_dir(&p).expect("read_dir").flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    files += 1;
                    control += fs::read_to_string(&path).expect("read").matches("Coalition").count();
                }
            }
        }
        assert!(files > 100, "walked only {files} sources");
        assert!(control > 20, "found only {control} uses of the game's own vocabulary");
    }
}
