//! anchor.rs — placing a block where you meant to.
//!
//! The failure, from a real session: *"the block landed on the wrong `push_car` again — there are
//! two"*. The edit was correct, the anchor matched, and it matched the wrong one.
//!
//! **Uniqueness is the property, not correctness.** An anchor that matches twice is broken whichever
//! occurrence was intended, and — the useful part — that is checkable BEFORE the edit rather than
//! discovered after it.
//!
//! ## Why models get this wrong specifically
//!
//! A model asked to anchor on `fn push_car` produces the shortest string that names the thing, which
//! is exactly the string most likely to recur. The instinct that makes a good description makes a bad
//! anchor.
//!
//! ## What to ask for instead
//!
//! Not "be more specific" — that is advice, not a procedure. **Count the matches, and widen until
//! there is one.** Widening is mechanical: take the line before, then the line after, alternating,
//! until unique or until the window is large enough that the anchor is really a location.

/// Why an anchor cannot be used.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AnchorFault {
    /// Matches nowhere. Usually whitespace or a stale read.
    Absent,
    /// Matches more than once. **The dangerous one**, because the edit succeeds.
    Ambiguous { matches: usize, lines: Vec<usize> },
}

impl AnchorFault {
    pub fn explain(&self) -> String {
        match self {
            AnchorFault::Absent =>
                "the anchor matches nothing — usually trailing whitespace, a tab against spaces, or \
a file that changed since it was read. Re-read the region rather than guessing at the difference".into(),
            AnchorFault::Ambiguous { matches, lines } => format!(
                "the anchor matches {matches} times, at lines {lines:?}. THIS IS THE DANGEROUS CASE: \
the edit will succeed and land on the first one. Widen the anchor until it matches once"),
        }
    }

    /// Would an edit using this anchor appear to work?
    ///
    /// The distinction that decides how loudly to complain: an absent anchor fails visibly and an
    /// ambiguous one produces a plausible wrong result.
    pub fn fails_silently(&self) -> bool { matches!(self, AnchorFault::Ambiguous { .. }) }
}

/// Check an anchor against the file it will be applied to.
pub fn check(haystack: &str, anchor: &str) -> Result<usize, AnchorFault> {
    if anchor.is_empty() { return Err(AnchorFault::Absent) }
    let lines: Vec<usize> = haystack.match_indices(anchor)
        .map(|(i, _)| haystack[..i].matches('\n').count() + 1).collect();
    match lines.len() {
        0 => Err(AnchorFault::Absent),
        1 => Ok(lines[0]),
        n => Err(AnchorFault::Ambiguous { matches: n, lines }),
    }
}

/// Grow an anchor until it is unique.
///
/// Takes the line before, then after, alternating — nearer context disambiguates more often than
/// distant context, and alternating avoids running off one end of a function.
///
/// Returns None if it cannot be made unique within `max_lines`, which means the region is genuinely
/// repetitive and the caller should anchor on something structural instead.
pub fn widen(haystack: &str, anchor: &str, max_lines: usize) -> Option<String> {
    if check(haystack, anchor).is_ok() { return Some(anchor.to_string()) }
    let hl: Vec<&str> = haystack.lines().collect();
    // Every starting line where the anchor's first line appears — each is a candidate to grow from.
    let first = anchor.lines().next()?;
    let starts: Vec<usize> = hl.iter().enumerate()
        .filter(|(_, l)| l.contains(first)).map(|(i, _)| i).collect();
    if starts.is_empty() { return None }
    let span = anchor.lines().count();
    for grow in 1..=max_lines {
        for &s in &starts {
            let lo = s.saturating_sub(grow.div_ceil(2));
            let hi = (s + span + grow / 2).min(hl.len());
            if lo >= hi { continue }
            let cand = hl[lo..hi].join("\n");
            if check(haystack, &cand).is_ok() { return Some(cand) }
        }
    }
    None
}

/// The instruction to give a model, once an anchor is known to be ambiguous.
///
/// **Procedure, not advice.** "Be more specific" is what a model has already tried; "include the
/// enclosing function's signature line" is something it can do.
pub fn guidance(fault: &AnchorFault) -> &'static str {
    match fault {
        AnchorFault::Absent =>
            "re-read the exact lines you intend to replace and copy them verbatim, including \
indentation — do not retype them from memory",
        AnchorFault::Ambiguous { .. } =>
            "include the enclosing function's signature line above your anchor, or the closing brace \
below it. If both occurrences are inside the same function, anchor on the line that differs between \
them rather than on the line they share",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_PUSH_CARS: &str = "\
fn spawn_traffic() {
    push_car(lane);
}

fn respawn() {
    push_car(lane);
}
";

    #[test]
    fn the_REAL_FAILURE_is_caught_before_the_edit() {
        // "the block landed on the wrong push_car again — there are two". The edit was correct, the
        // anchor matched, and it matched the wrong one.
        let e = check(TWO_PUSH_CARS, "    push_car(lane);").unwrap_err();
        assert!(matches!(e, AnchorFault::Ambiguous { matches: 2, .. }));
        assert!(e.explain().contains("the edit will succeed and land on the first one"));
    }

    #[test]
    fn AMBIGUOUS_is_the_dangerous_case_and_ABSENT_is_not() {
        // An absent anchor fails visibly; an ambiguous one produces a plausible wrong result. That
        // decides how loudly to complain.
        assert!(AnchorFault::Ambiguous { matches: 2, lines: vec![2, 6] }.fails_silently());
        assert!(!AnchorFault::Absent.fails_silently());
    }

    #[test]
    fn WIDENING_finds_a_unique_anchor_for_the_first_occurrence() {
        // Mechanical rather than advisory: take the line before, then after, until unique.
        let w = widen(TWO_PUSH_CARS, "    push_car(lane);", 6).unwrap();
        assert!(check(TWO_PUSH_CARS, &w).is_ok());
        assert!(w.contains("spawn_traffic") || w.contains("respawn"));
    }

    #[test]
    fn an_ALREADY_UNIQUE_anchor_is_returned_unchanged() {
        // Widening a good anchor makes it more fragile against unrelated edits nearby.
        assert_eq!(widen(TWO_PUSH_CARS, "fn respawn() {", 6).unwrap(), "fn respawn() {");
    }

    #[test]
    fn a_GENUINELY_REPETITIVE_region_returns_None_rather_than_a_wrong_answer() {
        // Twenty identical lines cannot be disambiguated by context, and returning the first would be
        // the exact failure this module exists to prevent.
        let src = "x();\n".repeat(20);
        assert!(widen(&src, "x();", 4).is_none());
    }

    #[test]
    fn GUIDANCE_is_a_procedure_not_advice() {
        // "Be more specific" is what the model already tried. "Include the enclosing function's
        // signature line" is something it can do.
        let g = guidance(&AnchorFault::Ambiguous { matches: 2, lines: vec![2, 6] });
        assert!(g.contains("enclosing function's signature line"));
        assert!(g.contains("anchor on the line that differs"));
    }

    #[test]
    fn an_ABSENT_anchor_is_told_to_RE_READ_rather_than_to_guess() {
        // Trailing whitespace and tabs-against-spaces are the usual causes, and neither is fixable by
        // thinking harder about the intended text.
        assert!(guidance(&AnchorFault::Absent).contains("copy them verbatim"));
        assert!(AnchorFault::Absent.explain().contains("Re-read the region"));
    }

    #[test]
    fn an_EMPTY_anchor_is_absent_not_matching_everything() {
        // The empty string is a substring of everything, which would make it match at position zero
        // and silently edit the top of the file.
        assert_eq!(check("anything", ""), Err(AnchorFault::Absent));
    }
}
