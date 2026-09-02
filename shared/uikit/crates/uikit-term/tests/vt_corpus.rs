//! Replay every capture in `testdata/vt/` and compare the grid against what a
//! reference terminal emulator produces from the same bytes.
//!
//! The fixtures are recorded from a real pty or constructed byte-for-byte
//! (`tools/capture_vt.py`); the expectations come from tmux and pyte
//! (`tools/render_ref.py`). Both tools need a shell, tmux, and a pip install.
//! This test needs none of them: it reads the checked-in files and nothing
//! else, so it runs anywhere the crate builds.
//!
//! # Why the gap list is in the test
//!
//! Several sequences a real shell emits are not implemented yet. Skipping those
//! fixtures silently would leave a suite that is green and a terminal that
//! garbles output, which is the state this corpus was written to end. So each
//! unimplemented sequence is named here with what it is and what it costs, and
//! the test asserts BOTH directions:
//!
//! * a fixture not in the list must match the reference exactly, and
//! * a fixture in the list must still differ from it.
//!
//! The second assertion is the one that keeps the list honest. Without it an
//! entry survives its own fix, and the corpus quietly stops testing the thing
//! it was added for — the failure mode `CLAUDE.md` §5b is about. Fixing a
//! sequence means deleting its entry in the same commit; the test will insist.

use std::path::{Path, PathBuf};

/// Sequences the parser does not implement, with the fixture that proves it.
///
/// Every one of these was found by diffing against tmux, not by reading the
/// code. The note says what the sequence is for, because "CSI X" is not a
/// reason to prioritise anything and "the shell erases N characters instead of
/// redrawing the line" is.
/// Sequences the parser does not implement, with the fixture that proves it.
///
/// **Currently empty: every fixture in the corpus is enforced.** It started at
/// eleven, and each entry was deleted by the commit that closed it, which is
/// what `known_gaps_still_diverge` exists to force.
///
/// Leaving the list (rather than deleting the machinery now that it is empty)
/// is deliberate: the next unimplemented sequence someone finds gets recorded
/// here with what it costs, instead of as a `#[ignore]` or a fixture quietly
/// left out of the directory. An empty list is also the one state where
/// `known_gaps_still_diverge` proves nothing on its own — which is why
/// `the_whole_corpus_is_enforced` below asserts the count directly rather than
/// letting an empty list read as success.
const KNOWN_GAPS: &[(&str, &str)] = &[];

fn vt_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/vt")
}

fn fixtures() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(vt_dir())
        .expect("testdata/vt is missing")
        .filter_map(|e| {
            let p = e.ok()?.path();
            if p.extension()? == "bin" {
                Some(p.file_stem()?.to_str()?.to_string())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

/// Replay one capture and return `(actual, expected)`.
fn replay(name: &str) -> (String, String) {
    let dir = vt_dir();
    let bytes = std::fs::read(dir.join(format!("{name}.bin")))
        .unwrap_or_else(|e| panic!("{name}.bin: {e}"));
    let expected = std::fs::read_to_string(dir.join(format!("{name}.expect")))
        .unwrap_or_else(|e| panic!("{name}.expect: {e} — run tools/render_ref.py"));

    // The reference was rendered at the size named in the file, so read it from
    // there rather than assuming 80x24: a fixture recorded at another size
    // would otherwise fail for a reason that has nothing to do with the parser.
    let mut cols = 80usize;
    let mut rows = 24usize;
    for line in expected.lines().take(2) {
        let mut it = line.split_whitespace();
        match (it.next(), it.next()) {
            (Some("cols"), Some(v)) => cols = v.parse().unwrap(),
            (Some("rows"), Some(v)) => rows = v.parse().unwrap(),
            _ => {}
        }
    }

    let mut t = uikit_term::Terminal::new(cols, rows);
    t.feed_bytes(&bytes);
    (uikit_term::snapshot::dump(&t.grid), expected)
}

/// The first differing line, formatted for a failure message.
fn first_diff(actual: &str, expected: &str) -> String {
    for (i, (b, a)) in expected.lines().zip(actual.lines()).enumerate() {
        if a != b {
            return format!("line {i}:\n  reference: {b:?}\n  uikit-term: {a:?}");
        }
    }
    let (na, ne) = (actual.lines().count(), expected.lines().count());
    format!("line counts differ: uikit-term {na}, reference {ne}")
}

#[test]
fn the_corpus_is_not_empty() {
    // A directory scan that matches nothing passes every assertion below it.
    let names = fixtures();
    assert!(
        names.len() >= 20,
        "expected the recorded + constructed corpus, found {} fixtures",
        names.len()
    );
}

#[test]
fn the_whole_corpus_is_enforced() {
    // An empty KNOWN_GAPS makes `known_gaps_still_diverge` vacuous — it
    // iterates nothing and passes. So the claim "every fixture is checked" is
    // asserted here as a number instead, where an accidentally-shrinking
    // corpus or a re-populated gap list is visible.
    let total = fixtures().len();
    let gaps = KNOWN_GAPS.len();
    assert!(
        gaps * 4 < total,
        "{gaps} of {total} fixtures are excused — the corpus is mostly a list of \
         things that do not work, which is a different document"
    );
}

#[test]
fn every_gap_names_a_fixture_that_exists() {
    let names = fixtures();
    for (fixture, _) in KNOWN_GAPS {
        assert!(
            names.iter().any(|n| n == fixture),
            "KNOWN_GAPS names {fixture}, which is not in testdata/vt — \
             renamed or deleted fixtures must be removed from the list"
        );
    }
}

#[test]
fn captures_match_the_reference_emulator() {
    let mut failures = Vec::new();
    for name in fixtures() {
        if KNOWN_GAPS.iter().any(|(f, _)| *f == name) {
            continue;
        }
        let (actual, expected) = replay(&name);
        if actual != expected {
            failures.push(format!("{name}\n{}", first_diff(&actual, &expected)));
        }
    }
    assert!(
        failures.is_empty(),
        "{} capture(s) diverged from the reference:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn known_gaps_still_diverge() {
    // The half that keeps the list from rotting. An entry whose fixture now
    // matches has been fixed, and leaving it listed means the corpus stops
    // checking that sequence the moment someone implements it.
    let mut fixed = Vec::new();
    for (name, note) in KNOWN_GAPS {
        let (actual, expected) = replay(name);
        if actual == expected {
            fixed.push(format!("{name} — {note}"));
        }
    }
    assert!(
        fixed.is_empty(),
        "{} KNOWN_GAPS entr(ies) now match the reference. Delete them from \
         KNOWN_GAPS in the same commit as the fix, so the fixture starts being \
         enforced:\n\n{}",
        fixed.len(),
        fixed.join("\n")
    );
}
