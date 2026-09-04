//! Running a [`Probe`] against the filesystem.
//!
//! The half of `context_probe` that touches disk. The vocabulary says what may
//! be asked and what may come back; this does the asking, and it is where the
//! bounds stop being a promise.
//!
//! # Never read a whole file to return part of one
//!
//! A `Slice` of lines 400–440 that reads the file into a `String` first has
//! already paid the memory, and on the eight-hundred-line files this exists to
//! avoid it will do that on every probe. Both readers here stream and stop:
//! the slice stops at its last line, the grep stops at its hit ceiling.
//!
//! That matters more than it sounds. The failure this subsystem addresses is a
//! run that pulls whole files into a prompt; a probe layer that pulls whole
//! files into memory to produce excerpts has kept the shape of the bug and
//! only hidden it from the token count.
//!
//! # Errors are findings too
//!
//! A missing file returns `Err`, not an empty result. "No matches in
//! src/pane.rs" and "src/pane.rs does not exist" lead the queen to opposite
//! next moves, and collapsing them into an empty vector is how a survey
//! concludes the answer is elsewhere when the path was simply mistyped.

use crate::context_probe::{Finding, Probe, MAX_FINDING_LINES};
use std::io::{BufRead, BufReader};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    NotFound(String),
    Unreadable { path: String, why: String },
    /// A `Slice` asking for more than [`MAX_FINDING_LINES`].
    ///
    /// Refused HERE rather than truncated, because a caller that asked for 400
    /// lines and silently got 120 will reason about the 280 it never saw.
    TooWide { asked: u32, max: u32 },
    BadRange,
}

/// Execute one probe. `root` is the repository root; paths are relative to it.
pub fn run(root: &std::path::Path, p: &Probe) -> Result<Vec<Finding>, ProbeError> {
    match p {
        Probe::Slice { path, first, last } => slice(root, path, *first, *last).map(|f| vec![f]),
        Probe::Grep { pattern, glob, max_hits } => grep(root, pattern, glob.as_deref(), *max_hits),
        Probe::Outline { path } => outline(root, path).map(|f| vec![f]),
        Probe::Symbol { name, path_hint } => symbol(root, name, path_hint.as_deref()),
        // Not yet executable. Returning an empty vec would read as "nothing
        // found", which is a lie the queen would act on.
        Probe::Deps { .. } => {
            Err(ProbeError::Unreadable {
                path: String::new(),
                why: "probe kind has no executor yet".into(),
            })
        }
    }
}

/// Classify a failure to open.
///
/// Pulled out as a function so the distinction is testable. A permission
/// error cannot be manufactured in a sandbox running as root, and a wildcard
/// `_ => NotFound` therefore passes every filesystem test that can be written
/// — found by breaking exactly that and watching nothing fail. Missing and
/// unreadable send the queen in opposite directions, so the mapping is worth
/// pinning even when the filesystem cannot exercise it.
fn open_err(kind: std::io::ErrorKind, path: &str, why: &str) -> ProbeError {
    match kind {
        std::io::ErrorKind::NotFound => ProbeError::NotFound(path.into()),
        _ => ProbeError::Unreadable { path: path.into(), why: why.into() },
    }
}

/// Read an inclusive 1-based line range, without reading past it.
pub fn slice(root: &std::path::Path, path: &str, first: u32, last: u32)
    -> Result<Finding, ProbeError>
{
    if first == 0 || last < first {
        return Err(ProbeError::BadRange);
    }
    let want = last - first + 1;
    if want > MAX_FINDING_LINES {
        return Err(ProbeError::TooWide { asked: want, max: MAX_FINDING_LINES });
    }
    let full = root.join(path);
    let file = std::fs::File::open(&full).map_err(|e| open_err(e.kind(), path, &e.to_string()))?;
    let mut out = String::new();
    let mut seen = 0u32;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| ProbeError::Unreadable { path: path.into(), why: e.to_string() })?;
        seen += 1;
        if seen < first {
            continue;
        }
        out.push_str(&line);
        out.push('\n');
        // STOP. The point of a slice is not paying for the rest of the file.
        if seen >= last {
            break;
        }
    }
    if seen < first {
        // The range is past the end. Distinct from an empty file and from a
        // missing one: the path was right and the line numbers were stale,
        // which is what happens when a survey's findings age.
        return Err(ProbeError::BadRange);
    }
    Ok(Finding { path: path.into(), first, last: seen.min(last), text: out })
}

/// Item names and their line spans — the survey probe.
///
/// # This one DOES read the whole file, and that is the point
///
/// `ast::extract` parses the source, so the bytes are read. What matters is
/// what leaves: the finding is a list of item names and line numbers, not the
/// code. A 900-line file becomes about forty lines of index, and those forty
/// lines are what decide whether any `Slice` of it is worth the tokens.
///
/// Reading once on disk to avoid reading repeatedly into the prompt is the
/// whole trade. It is also why `Outline` is priced at 300 tokens against a
/// file's ten thousand.
///
/// Uses `crate::ast`, which already extracts exactly this — kind, name,
/// enclosing impl or mod, and a line span — and had no caller.
pub fn outline(root: &std::path::Path, path: &str) -> Result<Finding, ProbeError> {
    let full = root.join(path);
    let src = std::fs::read_to_string(&full)
        .map_err(|e| open_err(e.kind(), path, &e.to_string()))?;
    let items = crate::ast::extract(&src);
    let total = src.lines().count().max(1) as u32;

    let mut text = String::new();
    for it in &items {
        // 1-based, to match `slice` and grep. `ast` reports 0-based, and a
        // survey whose line numbers are one out sends every slice it justifies
        // one line off — which reads as the model quoting the wrong statement.
        let line = it.start_line as u32 + 1;
        match &it.parent {
            Some(p) => text.push_str(&format!("{line}: {:?} {p}::{}\n", it.kind, it.name)),
            None => text.push_str(&format!("{line}: {:?} {}\n", it.kind, it.name)),
        }
    }
    if items.is_empty() {
        // Distinct from a parse failure only in that it is not an error: a
        // file with no items is a real answer, and saying so beats an empty
        // finding the queen reads as a probe that did not run.
        text.push_str("(no items)\n");
    }
    Ok(Finding { path: path.into(), first: 1, last: total, text })
}

/// One named item's definition, bounded by its own span.
///
/// # Why this is not a grep plus a slice
///
/// A grep for `fn split_pane` finds the line; a slice around it guesses how
/// far the body runs and is wrong in both directions — too short on a long
/// function, and reaching into the next one on a short one. `ast` already
/// knows where the item ENDS, so the slice is exact.
///
/// # Long items are truncated, not refused
///
/// A slice wider than the cap is an error because the caller chose the range
/// and deserves to know it was not honoured. Here the ITEM chose it: a
/// 400-line function is a fact about the code, and refusing to show any of it
/// helps nobody. The head is returned with its real span recorded, so the
/// queen can slice further down if it needs to.
pub fn symbol(root: &std::path::Path, name: &str, path_hint: Option<&str>)
    -> Result<Vec<Finding>, ProbeError>
{
    let hint = path_hint.ok_or_else(|| ProbeError::Unreadable {
        path: String::new(),
        // A repository-wide symbol search is a grep over every file, and the
        // caller has a cheaper way to get there: grep first, then ask for the
        // symbol in the file the grep named. Refusing keeps the survey-first
        // order rather than quietly walking the tree.
        why: "Symbol needs a path_hint — grep for the name first".into(),
    })?;
    let full = root.join(hint);
    let src = std::fs::read_to_string(&full)
        .map_err(|e| open_err(e.kind(), hint, &e.to_string()))?;
    let lines: Vec<&str> = src.lines().collect();

    let mut out = Vec::new();
    for it in crate::ast::extract(&src) {
        if it.name != name {
            continue;
        }
        let first = it.start_line as u32 + 1;
        let true_last = it.end_line as u32 + 1;
        let last = true_last.min(first + MAX_FINDING_LINES - 1);
        let body: String = lines
            .iter()
            .skip(first as usize - 1)
            .take((last - first + 1) as usize)
            .map(|l| format!("{l}\n"))
            .collect();
        let text = if last < true_last {
            // Say so IN the finding. A truncated body that looks complete is
            // how a reader concludes a function does less than it does.
            format!("{body}// ... truncated at {MAX_FINDING_LINES} lines; item ends at {true_last}\n")
        } else {
            body
        };
        out.push(Finding { path: hint.into(), first, last, text });
    }
    Ok(out)
}

/// Literal substring search. One finding per matching line.
///
/// Literal rather than regex on purpose: a regex from a model is a syntax
/// error waiting to happen, and the failure mode — no matches — is
/// indistinguishable from an honest miss.
pub fn grep(root: &std::path::Path, pattern: &str, glob: Option<&str>, max_hits: u16)
    -> Result<Vec<Finding>, ProbeError>
{
    if pattern.is_empty() {
        return Ok(Vec::new());
    }
    let mut hits = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                // `target` and `.git` are most of the bytes and none of the
                // answers; walking them turns a survey into a minute.
                if name == "target" || name == ".git" || name == "node_modules" {
                    continue;
                }
                stack.push(p);
                continue;
            }
            if let Some(g) = glob {
                if !name.ends_with(g) {
                    continue;
                }
            }
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().into_owned();
            let Ok(f) = std::fs::File::open(&p) else { continue };
            for (i, line) in BufReader::new(f).lines().enumerate() {
                let Ok(line) = line else { break };
                if line.contains(pattern) {
                    let n = i as u32 + 1;
                    hits.push(Finding { path: rel.clone(), first: n, last: n, text: line });
                    if hits.len() >= max_hits as usize {
                        return Ok(hits);
                    }
                }
            }
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(name: &str, lines: usize) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("probe-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("f.rs");
        let mut f = std::fs::File::create(&p).unwrap();
        for i in 1..=lines {
            writeln!(f, "line {i} needle{}", if i == 7 { "-HERE" } else { "" }).unwrap();
        }
        dir
    }

    #[test]
    fn a_slice_returns_exactly_the_lines_asked_for() {
        let root = fixture("exact", 200);
        let f = slice(&root, "f.rs", 10, 14).unwrap();
        assert_eq!(f.lines(), 5);
        assert_eq!(f.first, 10);
        assert!(f.text.starts_with("line 10 "), "{}", f.text);
        // The fixture writes `line N needle`, so the last line ends with the
        // suffix rather than the number. My first version asserted on the
        // number alone and failed against my own fixture.
        assert!(f.text.trim_end().ends_with("line 14 needle"), "{}", f.text);
        // And nothing from beyond the range — the whole point.
        assert!(!f.text.contains("line 15"), "the slice ran past its last line");
        assert!(!f.text.contains("line 9"));
    }

    #[test]
    fn a_slice_wider_than_the_cap_is_refused_not_truncated() {
        // A caller that asked for 400 lines and silently got 120 will reason
        // about the 280 it never saw.
        let root = fixture("wide", 500);
        let e = slice(&root, "f.rs", 1, 400).unwrap_err();
        assert_eq!(e, ProbeError::TooWide { asked: 400, max: MAX_FINDING_LINES });
    }

    #[test]
    fn a_missing_file_is_distinct_from_no_matches() {
        // "No matches in src/pane.rs" and "src/pane.rs does not exist" lead to
        // opposite next moves. Collapsing them is how a survey concludes the
        // answer is elsewhere when the path was mistyped.
        let root = fixture("missing", 10);
        assert_eq!(slice(&root, "nope.rs", 1, 5).unwrap_err(),
                   ProbeError::NotFound("nope.rs".into()));
        // Whereas a real file with no hits is an empty Ok.
        assert_eq!(grep(&root, "zzz-not-present", None, 20).unwrap().len(), 0);
    }

    #[test]
    fn a_range_past_the_end_of_the_file_says_so() {
        // Stale line numbers from an ageing survey. Returning an empty finding
        // would look like a blank region of the file.
        let root = fixture("past", 10);
        assert_eq!(slice(&root, "f.rs", 50, 60).unwrap_err(), ProbeError::BadRange);
        // A range that STARTS inside and runs past clamps rather than failing:
        // those lines really are the end of the file.
        let f = slice(&root, "f.rs", 8, 20).unwrap();
        assert_eq!(f.last, 10);
    }

    #[test]
    fn grep_stops_at_its_hit_ceiling() {
        // The bound that keeps a common word from returning the repository.
        let root = fixture("ceiling", 300);
        let hits = grep(&root, "needle", None, 5).unwrap();
        assert_eq!(hits.len(), 5, "the ceiling was not enforced");
        // Every hit is one line and carries where it came from.
        for h in &hits {
            assert_eq!(h.lines(), 1);
            assert!(h.first >= 1);
            assert!(!h.path.is_empty());
        }
    }

    #[test]
    fn grep_reports_the_line_number_the_editor_would_show() {
        // 1-based. Off by one here sends every subsequent slice one line out,
        // which reads as the model quoting the wrong statement.
        let root = fixture("lineno", 20);
        let hits = grep(&root, "needle-HERE", None, 4).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].first, 7, "line numbers are not 1-based");
        assert!(hits[0].text.contains("line 7"));
    }

    #[test]
    fn an_unexecutable_probe_errors_rather_than_returning_nothing() {
        // Symbol and Deps have no executor. An empty vec would read as
        // "nothing found" — a lie the queen would act on by looking somewhere
        // else.
        //
        // This test named Outline until Outline gained an executor, and then
        // failed — correctly. A test that asserts a feature is MISSING has to
        // be revisited when it arrives, which is the one kind of failing test
        // worth being pleased about.
        let root = fixture("noexec", 5);
        for p in [Probe::Symbol { name: "x".into(), path_hint: None },
                  Probe::Deps { krate: "k".into(), depth: 1 }] {
            let r = run(&root, &p);
            assert!(matches!(r, Err(ProbeError::Unreadable { .. })), "{p:?} -> {r:?}");
        }
    }

    #[test]
    fn an_empty_pattern_matches_nothing_rather_than_everything() {
        // `"".contains()` is true for every line, so the obvious
        // implementation returns the entire repository for an empty query —
        // which a model will send the moment its extraction step misfires.
        let root = fixture("empty", 50);
        assert_eq!(grep(&root, "", None, 500).unwrap().len(), 0);
    }
    #[test]
    fn a_permission_error_is_not_reported_as_a_missing_file() {
        // Tested on the classifier rather than through the filesystem: this
        // sandbox runs as root, so `chmod 000` is still readable and no test
        // that opens a real file can tell the two arms apart. A wildcard
        // `_ => NotFound` passed every other test here.
        //
        // The distinction matters because it decides the queen's next move:
        // a missing path means look elsewhere, an unreadable one means the
        // path was right.
        assert_eq!(open_err(std::io::ErrorKind::NotFound, "a.rs", "no such file"),
                   ProbeError::NotFound("a.rs".into()));
        assert_eq!(open_err(std::io::ErrorKind::PermissionDenied, "a.rs", "denied"),
                   ProbeError::Unreadable { path: "a.rs".into(), why: "denied".into() });
        assert!(matches!(open_err(std::io::ErrorKind::IsADirectory, "d", "is a dir"),
                         ProbeError::Unreadable { .. }));
    }

    fn rust_fixture(name: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("outline-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("f.rs"), body).unwrap();
        dir
    }

    #[test]
    fn an_outline_names_the_items_without_returning_the_code() {
        // The economic claim the whole survey-first rule rests on. If the
        // outline carried bodies it would cost what the file costs and there
        // would be no reason to run it first.
        let body = "fn alpha() {\n    let x = 1;\n    println!(\"a long body line that must not appear\");\n}\nstruct Beta { f: u32 }\nimpl Beta {\n    fn gamma(&self) {}\n}\n";
        let root = rust_fixture("names", body);
        let f = outline(&root, "f.rs").unwrap();
        for want in ["alpha", "Beta", "gamma"] {
            assert!(f.text.contains(want), "{want} missing from outline:\n{}", f.text);
        }
        assert!(!f.text.contains("must not appear"),
                "the outline carried a body:\n{}", f.text);
        assert!(f.text.len() < body.len(), "the outline is not smaller than the file");
    }

    #[test]
    fn outline_line_numbers_are_one_based_like_every_other_probe() {
        // `ast` reports 0-based. A survey one line out sends every slice it
        // justifies one line off, which reads as the model quoting the wrong
        // statement — and is invisible unless something checks.
        let root = rust_fixture("lines", "// c\nfn first() {}\n");
        let f = outline(&root, "f.rs").unwrap();
        assert!(f.text.starts_with("2: "), "expected line 2, got:\n{}", f.text);
    }

    #[test]
    fn a_file_with_no_items_says_so_rather_than_returning_nothing() {
        // An empty finding reads as a probe that did not run. "No items" is a
        // real answer and moves the queen on.
        let root = rust_fixture("empty", "// just a comment\n");
        let f = outline(&root, "f.rs").unwrap();
        assert!(f.text.contains("no items"), "{}", f.text);
        assert!(!f.text.is_empty());
    }

    #[test]
    fn outline_of_a_missing_file_is_an_error_not_an_empty_index() {
        let root = rust_fixture("gone", "fn a() {}\n");
        assert_eq!(outline(&root, "nope.rs").unwrap_err(),
                   ProbeError::NotFound("nope.rs".into()));
    }

    #[test]
    fn run_dispatches_outline_now_that_it_has_an_executor() {
        // The wiring, which `outline`'s own tests cannot see: they pass
        // perfectly with `run` still returning the not-implemented error.
        let root = rust_fixture("dispatch", "fn a() {}\n");
        let r = run(&root, &Probe::Outline { path: "f.rs".into() }).expect("outline runs");
        assert_eq!(r.len(), 1);
        assert!(r[0].text.contains("a"), "{}", r[0].text);
    }

    #[test]
    fn a_symbol_is_bounded_by_its_own_span_not_a_guess() {
        // The reason this is not grep-plus-slice: a fixed context window is
        // too short on a long function and reaches into the next one on a
        // short one. `ast` knows where the item ends.
        let body = "fn short() {}\nfn target() {\n    let a = 1;\n    let b = 2;\n}\nfn after() { let poison = 9; }\n";
        let root = rust_fixture("span", body);
        let fs = symbol(&root, "target", Some("f.rs")).unwrap();
        assert_eq!(fs.len(), 1, "{fs:?}");
        assert!(fs[0].text.contains("let a = 1"), "{}", fs[0].text);
        assert!(!fs[0].text.contains("poison"), "the symbol ran into the next item:\n{}", fs[0].text);
        assert!(!fs[0].text.contains("fn short"), "the symbol started too early:\n{}", fs[0].text);
    }

    #[test]
    fn a_symbol_with_no_path_hint_is_refused_rather_than_searched_for() {
        // A repository-wide symbol search is a grep over every file. The
        // caller has a cheaper route — grep, then ask in the file it named —
        // and refusing keeps the survey-first order instead of quietly
        // walking the tree.
        let root = rust_fixture("nohint", "fn a() {}\n");
        let e = symbol(&root, "a", None).unwrap_err();
        assert!(matches!(e, ProbeError::Unreadable { .. }), "{e:?}");
    }

    #[test]
    fn an_over_long_item_is_truncated_and_says_so() {
        // The item chose the length, not the caller, so refusing helps nobody
        // — but a truncated body that looks complete is how a reader concludes
        // a function does less than it does.
        let mut body = String::from("fn huge() {\n");
        for i in 0..(MAX_FINDING_LINES + 40) {
            body.push_str(&format!("    let v{i} = {i};\n"));
        }
        body.push_str("}\n");
        let root = rust_fixture("long", &body);
        let fs = symbol(&root, "huge", Some("f.rs")).unwrap();
        assert_eq!(fs.len(), 1);
        assert!(fs[0].lines() <= MAX_FINDING_LINES, "{} lines", fs[0].lines());
        assert!(fs[0].text.contains("truncated"), "silently cut:\n{}", fs[0].text);
    }

    #[test]
    fn a_symbol_that_is_not_there_is_an_empty_result_not_an_error() {
        // The file was readable and the answer is "not in here" — which is
        // information. An error would read as a broken probe.
        let root = rust_fixture("absent", "fn a() {}\n");
        assert!(symbol(&root, "nonexistent", Some("f.rs")).unwrap().is_empty());
        // A missing FILE is still an error.
        assert_eq!(symbol(&root, "a", Some("gone.rs")).unwrap_err(),
                   ProbeError::NotFound("gone.rs".into()));
    }

}
