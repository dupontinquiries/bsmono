//! prechecks.rs — deterministic faults a model does not need to be asked about.
//!
//! Motivated by a real critique of a 4B model's Rust output. Of the nine problems listed, most are
//! MECHANICAL: spaces inside paths, `.await` in a non-async `fn`, `.max(0)` on an unsigned counter,
//! a tuple variant matched with no fields, two crates that solve the same problem in one manifest.
//!
//! None of those need a model. A 1B repair model reacting to compiler output is a real and good idea —
//! the compiler narrows the search space enormously — but it costs a load, a prompt and a generation, and
//! it answers differently each time. A regex over the text costs microseconds and answers the same way
//! twice. So this runs FIRST, and what survives goes to the compiler, and what survives that goes to a
//! model.
//!
//! ## What this deliberately does not do
//!
//! It does not score style, guess intent, or rank a model. Every finding names a specific span and a
//! specific fix, because a "quality score" that cannot point at a line is not actionable — and because
//! the moment a checker starts estimating quality it starts being wrong in ways nobody can check.

/// How sure the check is. A finding a human must adjudicate is not the same as a syntax error.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confidence {
    /// This cannot compile. No judgement involved.
    Certain,
    /// Almost always a bug, but a contrived program could mean it.
    Likely,
    /// Worth a look; legitimate uses exist.
    Suspicion,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub line: usize,
    pub confidence: Confidence,
    pub what: String,
    pub fix: String,
}

/// The code part of a line: comment removed AND string contents blanked.
///
/// Blanking matters more than it looks. Counting quotes only tells you the line is balanced, not that a
/// match lies outside them — `println!("use a :: b")` has even quotes and would still trip a naive path
/// check. A false positive is expensive here: the moment a checker cries wolf, people stop reading its
/// output, and then it catches nothing regardless of how right it is elsewhere.
fn code_only(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_str = false;
    let mut prev = '\0';
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' && prev != '\\' { in_str = !in_str; out.push('"'); prev = c; continue; }
        if !in_str && c == '/' && chars.peek() == Some(&'/') { break; }   // line comment
        out.push(if in_str { ' ' } else { c });
        prev = c;
    }
    out
}

/// Scan Rust source for mechanically-detectable faults.
pub fn scan_rust(src: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let has_async_fn = lines.iter().any(|l| l.contains("async fn"));

    for (i, raw) in lines.iter().enumerate() {
        let line = code_only(raw);
        let line = line.as_str();
        let n = i + 1;

        // 1. A space inside a path or identifier: `std::error ::Error`, `player_ y`. Certain: not
        //    parseable. String contents are already blanked, so this only sees code.
        {
            if line.contains(" ::") || line.contains(":: ") {
                out.push(Finding { line: n, confidence: Confidence::Certain,
                    what: "space inside a path separator (`x ::y`)".into(),
                    fix: "remove the space: `x::y`".into() });
            }
            // an identifier split by a space, e.g. `player_ y` — the underscore-then-space-then-word shape
            if let Some(p) = line.find("_ ") {
                let after = line[p + 2..].chars().next().unwrap_or(' ');
                if after.is_alphanumeric() {
                    out.push(Finding { line: n, confidence: Confidence::Certain,
                        what: "identifier split by a space (`name_ x`)".into(),
                        fix: "join it: `name_x`".into() });
                }
            }
        }

        // 2. `.await` with no `async fn` anywhere in the file.
        if line.contains(".await") && !has_async_fn {
            out.push(Finding { line: n, confidence: Confidence::Certain,
                what: "`.await` in a file with no `async fn`".into(),
                fix: "make the caller `async fn` and add a runtime, or use the blocking API \
(for crossterm: `event::poll` + `event::read`)".into() });
        }

        // 3. `.max(0)` guarding a subtraction — the unsigned-underflow tell. The subtraction has already
        //    panicked by the time max runs, so the guard is not merely useless, it is misleading.
        if line.contains(".max(0)") && line.contains('-') {
            out.push(Finding { line: n, confidence: Confidence::Likely,
                what: "`.max(0)` after a subtraction — on an unsigned type the subtract underflows first".into(),
                fix: "use `saturating_sub(1)`, or guard with `if x > 0`".into() });
        }

        // 4. A tuple variant matched with no fields. Checked for the known-arity cases rather than
        //    guessed generally, because arity is not knowable from the text alone.
        for v in ["Event::Resize", "Event::Key", "Event::Mouse"] {
            if let Some(p) = line.find(v) {
                let rest = line[p + v.len()..].trim_start();
                if rest.starts_with("=>") || rest.starts_with(')') {
                    out.push(Finding { line: n, confidence: Confidence::Certain,
                        what: format!("`{v}` is a tuple variant matched with no fields"),
                        fix: format!("bind or ignore its payload: `{v}(..)`") });
                }
            }
        }

        // 5. A `use` block opened with a brace that also contains `execute` as an item AND assigns —
        //    the `let x = crate::{...}` shape, which is a use-tree in expression position.
        if line.contains('=') && line.contains("::{") && !line.trim_start().starts_with("use ") {
            out.push(Finding { line: n, confidence: Confidence::Certain,
                what: "a `use`-tree (`path::{a, b}`) in expression position".into(),
                fix: "move the imports to a `use` statement and assign a real value".into() });
        }
    }

    // 6. Raw mode: reading key events without enabling it. Terminals deliver arrow keys as escape
    //    sequences in cooked mode, so the program builds and then silently does not respond.
    let code: String = src.lines().map(code_only).collect::<Vec<_>>().join("\n");
    if code.contains("KeyCode::") && !code.contains("enable_raw_mode") {
        out.push(Finding { line: 0, confidence: Confidence::Likely,
            what: "key events are read but raw mode is never enabled".into(),
            fix: "call `crossterm::terminal::enable_raw_mode()?` at start and \
`disable_raw_mode()?` before exit, or arrow keys will not arrive".into() });
    }
    out
}

/// Two crates in one manifest that solve the same problem.
///
/// Not a compile error, and occasionally deliberate — hence Suspicion. Worth raising because pulling in
/// a second terminal library is a sign the author was unsure which to use, and that uncertainty usually
/// shows up in the code too.
pub fn scan_manifest(toml: &str) -> Vec<Finding> {
    const RIVALS: &[(&str, &str, &str)] = &[
        ("crossterm", "termion", "terminal control"),
        ("tokio", "async-std", "async runtime"),
        ("serde_json", "simd-json", "JSON"),
    ];
    let mut out = Vec::new();
    for (a, b, what) in RIVALS {
        let dep = |n: &str| toml.lines().any(|l| l.trim_start().starts_with(n)
            && l.contains('=') && !l.trim_start().starts_with('#'));
        if dep(a) && dep(b) {
            out.push(Finding { line: 0, confidence: Confidence::Suspicion,
                what: format!("both `{a}` and `{b}` are dependencies; they both do {what}"),
                fix: "pick one — keeping both doubles the build and splits the API surface".to_string() });
        }
    }
    out
}

/// A report ordered so the blocking problems come first.
pub fn report(findings: &[Finding]) -> Vec<String> {
    let mut f: Vec<&Finding> = findings.iter().collect();
    let rank = |c: Confidence| match c {
        Confidence::Certain => 0, Confidence::Likely => 1, Confidence::Suspicion => 2 };
    f.sort_by_key(|x| (rank(x.confidence), x.line));
    f.iter().map(|x| {
        let tag = match x.confidence {
            Confidence::Certain => "WILL NOT COMPILE", Confidence::Likely => "likely bug",
            Confidence::Suspicion => "worth a look" };
        let where_ = if x.line == 0 { String::new() } else { format!(" (line {})", x.line) };
        format!("[{tag}]{where_} {} → {}", x.what, x.fix)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The corpus is the real 4B output that prompted this, so the checks are measured against code a
    // model actually produced rather than faults invented to be catchable.
    const BAD: &str = r#"
use crossterm::{
    event::{Event, EventStream, KeyCode},
};
fn main() -> Result<(), Box<dyn std::error ::Error>> {
    let mut stdout = crossterm::{ io::{stdout as p, Write}, execute, };
    loop {
        match events.next().await {
            Some(Event::Key(key)) => {
                match key.code {
                    KeyCode ::Up => player_y = (player_y - 1).max(0),
                    KeyCode::Down => player_ y = (player_y + 1).min(9),
                }
            }
            Some(Event::Resize) => { }
        }
    }
}
"#;

    #[test]
    fn every_certain_finding_in_the_real_sample_is_caught() {
        let f = scan_rust(BAD);
        let certain: Vec<&Finding> = f.iter().filter(|x| x.confidence == Confidence::Certain).collect();
        let has = |needle: &str| certain.iter().any(|x| x.what.contains(needle));
        assert!(has("space inside a path"), "`std::error ::Error` and `KeyCode ::Up`");
        assert!(has("identifier split by a space"), "`player_ y`");
        assert!(has("`.await` in a file with no `async fn`"));
        assert!(has("tuple variant matched with no fields"), "`Event::Resize`");
        assert!(has("use`-tree"), "a use-tree in expression position");
    }

    #[test]
    fn the_unsigned_underflow_tell_is_found_and_explained_correctly() {
        let f = scan_rust("player_y = (player_y - 1).max(0);");
        let u = f.iter().find(|x| x.what.contains("max(0)")).expect("caught");
        // the explanation must say WHY the guard fails, not just that it is wrong: the subtraction has
        // already panicked by the time max runs
        assert!(u.what.contains("underflows first"));
        assert!(u.fix.contains("saturating_sub"));
        assert_eq!(u.confidence, Confidence::Likely, "a signed type would make this fine");
    }

    #[test]
    fn raw_mode_is_reported_once_for_the_file_not_per_line() {
        let f = scan_rust("KeyCode::Up => {}\nKeyCode::Down => {}\n");
        assert_eq!(f.iter().filter(|x| x.what.contains("raw mode")).count(), 1);
        // and not at all when it IS enabled
        let ok = scan_rust("enable_raw_mode()?;\nKeyCode::Up => {}");
        assert!(!ok.iter().any(|x| x.what.contains("raw mode")));
    }

    #[test]
    fn correct_code_produces_no_findings() {
        // The cost of a false positive is that people stop reading the output, so clean code must be
        // silent.
        let good = r#"
use crossterm::event::{self, Event, KeyCode};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    crossterm::terminal::enable_raw_mode()?;
    let mut y: usize = 0;
    loop {
        if event::poll(std::time::Duration::from_millis(16))? {
            if let Event::Key(k) = event::read()? {
                match k.code {
                    KeyCode::Up => y = y.saturating_sub(1),
                    KeyCode::Esc => break,
                    _ => {}
                }
            }
        }
    }
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}
"#;
        assert_eq!(scan_rust(good), Vec::new(), "clean code must be silent");
    }

    #[test]
    fn prose_inside_strings_and_comments_is_not_code() {
        // `"a :: b"` in a message, or a comment mentioning `x ::y`, must not be flagged.
        let s = r#"println!("use a :: b like this");
// note: `std::x ::y` is wrong
"#;
        assert!(scan_rust(s).is_empty(), "{:?}", scan_rust(s));
    }

    #[test]
    fn rival_dependencies_are_a_suspicion_not_an_error() {
        let m = "[dependencies]\ncrossterm = \"0.34\"\ntermion = \"^7\"\n";
        let f = scan_manifest(m);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].confidence, Confidence::Suspicion, "occasionally deliberate");
        assert!(f[0].what.contains("terminal control"));
        // a commented-out rival is not a dependency
        assert!(scan_manifest("crossterm = \"1\"\n# termion = \"7\"\n").is_empty());
    }

    #[test]
    fn the_report_puts_blockers_first() {
        let mut f = scan_manifest("crossterm = \"1\"\ntermion = \"7\"\n");
        f.extend(scan_rust(BAD));
        let r = report(&f);
        assert!(r[0].starts_with("[WILL NOT COMPILE]"), "{}", r[0]);
        assert!(r.last().unwrap().starts_with("[worth a look]"), "{}", r.last().unwrap());
    }
}
