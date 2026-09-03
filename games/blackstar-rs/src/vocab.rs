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
//! `energon` (145) is a live identifier — a module, `energon()`,
//! `rec_energon()`, `energon_value()`, `at_energon`, several test names — so
//! the rename is compiler-checked end to end, which makes it the SAFEST of the
//! large ones despite being the biggest. It needs a replacement word and
//! nothing else.
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
        ("Cybertron", 152),
        ("cybertron", 117),
        ("Energon", 8),
        ("energon", 137),
        ("Optimus", 24),
        ("optimus", 21),
        ("Megatron", 15),
        ("megatron", 17),
        ("Decepticon", 7),
        ("decepticon", 8),
        ("Autobot", 12),
        ("autobot", 3),
        // KEEP. Developer attribution in `compact.rs` and `xform.rs`, not
        // in-fiction vocabulary. Counted so that a well-meaning sweep that
        // deletes them fails and reads this comment.
        ("AllSpark", 2),
    ];

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
    fn the_census_is_measuring_a_real_tree() {
        // A walk that silently found nothing would make every count zero and
        // every assertion pass. Anchor on something that must exist.
        let now = census();
        let total: usize = now.iter().map(|(_, n)| n).sum();
        assert!(total > 400, "census total {total} — the walk probably missed the tree");
        let cyber = now.iter().find(|(t, _)| *t == "Cybertron").unwrap().1;
        assert!(cyber > 0, "no occurrences of the most common term; walk is broken");
    }
}
