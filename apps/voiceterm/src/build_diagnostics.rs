//! v0.1.764: build-output extractor. Cargo and CMake dump a wall of text — most
//! of it noise (dependency compile lines, repeated warning bodies) wrapping a
//! few diagnostics that actually matter. This module parses that output into a
//! compact, CATEGORIZED summary: per-diagnostic severity + code + location +
//! one-line message + fix hint, plus rolled-up counts by category. The terminal
//! UI turns this into a fuzzy, voice-addressable menu (jump to error 1, show
//! warnings, etc.) with theme-tinted category colors; `minify()` produces the
//! token-frugal text form for feeding to an assistant or a status line.
//!
//! Pure: text in, structs + wire out. No I/O, no QML.

/// Severity of a diagnostic line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity { Error, Warning }

impl Severity {
    pub fn tag(self) -> &'static str {
        match self { Severity::Error => "error", Severity::Warning => "warning" }
    }
}

/// A coarse category used for grouping + theme tinting. Derived from the
/// message text and the rustc error code. Kept small + stable so the UI can map
/// each to a fixed accent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// name resolution: cannot find value/function/type/macro (E0425/E0433/E0412…)
    Resolve,
    /// type mismatch (E0308) / arg count (E0061)
    TypeMismatch,
    /// borrow / move / lifetime (E0382/E0499/E0502/E0515…)
    Borrow,
    /// unsatisfied trait bound (E0277)
    Trait,
    /// unused var / mut / import / dead code / must-use
    Unused,
    /// deprecated API use
    Deprecated,
    /// unexpected / unknown cfg condition
    Cfg,
    /// style lints (non_snake_case, non_camel_case…)
    Style,
    /// linker / link-arg failures
    Link,
    /// CMake configure/build errors
    CMake,
    /// anything else
    Other,
}

impl Category {
    /// Stable lowercase key used in the wire + as the UI color/lookup key.
    pub fn key(self) -> &'static str {
        match self {
            Category::Resolve => "resolve",
            Category::TypeMismatch => "type",
            Category::Borrow => "borrow",
            Category::Trait => "trait",
            Category::Unused => "unused",
            Category::Deprecated => "deprecated",
            Category::Cfg => "cfg",
            Category::Style => "style",
            Category::Link => "link",
            Category::CMake => "cmake",
            Category::Other => "other",
        }
    }
    /// Short human label for the menu.
    pub fn label(self) -> &'static str {
        match self {
            Category::Resolve => "unresolved name",
            Category::TypeMismatch => "type mismatch",
            Category::Borrow => "borrow/move",
            Category::Trait => "trait bound",
            Category::Unused => "unused / dead",
            Category::Deprecated => "deprecated",
            Category::Cfg => "cfg condition",
            Category::Style => "style lint",
            Category::Link => "link error",
            Category::CMake => "cmake",
            Category::Other => "other",
        }
    }
}

/// One parsed diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// rustc code without brackets, e.g. "E0425". None for plain warnings.
    pub code: Option<String>,
    pub category: Category,
    /// short file name (last path component) for the menu; full path in `path`.
    pub file: Option<String>,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub col: Option<u32>,
    /// the message text (after `error:`/`warning:`), trimmed.
    pub message: String,
    /// a fix hint if the compiler offered one (the `help:` line, condensed).
    pub help: Option<String>,
}

impl Diagnostic {
    /// Compact one-line form for the minified summary / status line.
    pub fn one_line(&self) -> String {
        let mut s = String::new();
        if let Some(c) = &self.code { s.push_str(c); s.push(' '); }
        if let (Some(f), Some(l)) = (&self.file, self.line) {
            s.push_str(f); s.push(':'); s.push_str(&l.to_string()); s.push(' ');
        }
        s.push_str(&self.message);
        if let Some(h) = &self.help { s.push_str(" → "); s.push_str(h); }
        s
    }
}

/// The parsed result: every diagnostic plus rolled-up counts.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildSummary {
    pub diagnostics: Vec<Diagnostic>,
    pub error_count: usize,
    pub warning_count: usize,
    /// optional crate/target name parsed from a "could not compile `x`" line.
    pub failed_target: Option<String>,
}

impl BuildSummary {
    pub fn ok(&self) -> bool { self.error_count == 0 }

    /// Counts per category, most-frequent first (ties: category order).
    pub fn category_counts(&self) -> Vec<(Category, usize)> {
        let order = [
            Category::Resolve, Category::TypeMismatch, Category::Borrow, Category::Trait,
            Category::Link, Category::CMake, Category::Cfg, Category::Deprecated,
            Category::Unused, Category::Style, Category::Other,
        ];
        let mut out: Vec<(Category, usize)> = order.iter().map(|&c| {
            (c, self.diagnostics.iter().filter(|d| d.category == c).count())
        }).filter(|(_, n)| *n > 0).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }

    /// Just the errors (the things you usually want to jump to first).
    pub fn errors(&self) -> Vec<&Diagnostic> {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Error).collect()
    }
}

fn file_of(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// Categorize from the rustc code (if any) and the message text.
fn categorize(code: Option<&str>, msg: &str) -> Category {
    if let Some(c) = code {
        match c {
            "E0425" | "E0433" | "E0412" | "E0405" | "E0531" => return Category::Resolve,
            "E0308" | "E0061" => return Category::TypeMismatch,
            "E0277" => return Category::Trait,
            "E0382" | "E0499" | "E0502" | "E0515" | "E0505" | "E0506" | "E0507"
                => return Category::Borrow,
            _ => {}
        }
    }
    let m = msg.to_lowercase();
    // order matters: more specific first
    if m.contains("deprecated") { return Category::Deprecated; }
    if m.contains("unexpected `cfg`") || m.contains("unexpected cfg")
        || m.contains("condition value") || m.contains("condition name") {
        return Category::Cfg;
    }
    if m.contains("snake case") || m.contains("camel case")
        || m.contains("upper case") || m.contains("nonstandard style") {
        return Category::Style;
    }
    if m.contains("never used") || m.contains("never read") || m.contains("is never")
        || m.contains("unused") || m.contains("does not need to be mutable")
        || m.contains("must be used") || m.contains("dead_code") {
        return Category::Unused;
    }
    if m.contains("cannot find") || m.contains("unresolved") || m.contains("not found in this scope") {
        return Category::Resolve;
    }
    if m.contains("mismatched types") || m.contains("expected") && m.contains("found") {
        return Category::TypeMismatch;
    }
    if m.contains("borrow") || m.contains("does not live long enough") || m.contains("moved") {
        return Category::Borrow;
    }
    if m.contains("trait bound") || m.contains("is not implemented") {
        return Category::Trait;
    }
    if m.contains("linker") || m.contains("undefined reference") || m.contains("ld returned") {
        return Category::Link;
    }
    Category::Other
}

/// Pull the `error[E0425]:` / `error:` / `warning:` head off a line. Returns
/// (severity, code, message) when the line opens a diagnostic, else None.
fn parse_head(line: &str) -> Option<(Severity, Option<String>, String)> {
    let t = line.trim_start();
    for (prefix, sev) in [("error", Severity::Error), ("warning", Severity::Warning)] {
        if let Some(rest) = t.strip_prefix(prefix) {
            // rest is either "[E0425]: msg" or ": msg"
            let (code, after) = if let Some(r) = rest.strip_prefix('[') {
                let end = r.find(']')?;
                (Some(r[..end].to_string()), &r[end + 1..])
            } else {
                (None, rest)
            };
            let msg = after.strip_prefix(':')?.trim().to_string();
            if msg.is_empty() { return None; }
            return Some((sev, code, msg));
        }
    }
    None
}

/// Recognize cargo's roll-up lines that are SUMMARIES, not new diagnostics:
///   `warning: \`crate\` (lib) generated 23 warnings`
///   `error: could not compile \`crate\` (...) due to 1 previous error`
///   `error: aborting due to N previous errors`
/// Returns Some(failed_target) for a "could not compile" line.
fn classify_summary_line(sev: Severity, msg: &str) -> Option<Option<String>> {
    if sev == Severity::Warning && msg.contains("generated") && msg.contains("warning") {
        return Some(None);
    }
    if sev == Severity::Error && msg.starts_with("could not compile") {
        // extract the `name` in backticks
        let target = msg.split('`').nth(1).map(|s| s.to_string());
        return Some(target);
    }
    if sev == Severity::Error && msg.starts_with("aborting due to") {
        return Some(None);
    }
    None
}

/// Parse a `--> path:line:col` location line. Returns (path, line, col).
fn parse_location(line: &str) -> Option<(String, u32, u32)> {
    let t = line.trim_start();
    let rest = t.strip_prefix("-->")?.trim();
    // split from the RIGHT so windows drive letters / colons in paths survive:
    // path : line : col
    let mut it = rest.rsplitn(3, ':');
    let col = it.next()?.trim().parse::<u32>().ok()?;
    let lineno = it.next()?.trim().parse::<u32>().ok()?;
    let path = it.next()?.trim().to_string();
    Some((path, lineno, col))
}

/// Condense a `help:` line into a short hint. e.g.
/// "help: a local variable with a similar name exists" + a following suggestion
/// is handled by the caller; here we just trim the `help:` prefix.
fn parse_help(line: &str) -> Option<String> {
    let t = line.trim_start();
    let rest = t.strip_prefix("help:")?.trim();
    if rest.is_empty() { return None; }
    Some(rest.to_string())
}

/// Parse full cargo/rustc build output into a categorized summary.
pub fn parse_cargo(output: &str) -> BuildSummary {
    let lines: Vec<&str> = output.lines().collect();
    let mut diags: Vec<Diagnostic> = Vec::new();
    let mut failed_target: Option<String> = None;

    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if let Some((sev, code, msg)) = parse_head(line) {
            // summary roll-up lines are counted differently / skipped
            if let Some(tgt) = classify_summary_line(sev, &msg) {
                if let Some(t) = tgt { failed_target = Some(t); }
                i += 1;
                continue;
            }
            let category = categorize(code.as_deref(), &msg);
            // scan the following indented block for a `-->` location and a
            // `help:` hint, stopping at the next blank line or next diagnostic.
            let mut path = None;
            let mut lineno = None;
            let mut col = None;
            let mut help = None;
            let mut j = i + 1;
            while j < lines.len() {
                let l = lines[j];
                if l.trim().is_empty() { break; }
                if parse_head(l).is_some() { break; }
                if path.is_none() {
                    if let Some((p, ln, c)) = parse_location(l) {
                        path = Some(p); lineno = Some(ln); col = Some(c);
                    }
                }
                if help.is_none() {
                    if let Some(h) = parse_help(l) { help = Some(h); }
                }
                j += 1;
            }
            // a "similar name exists" help puts the real suggestion in the
            // code line above the `++` insertion marker. Find that marker line,
            // take the identifier at the marker's column from the line above.
            if let Some(h) = &help {
                if h.contains("similar name") || h.contains("did you mean") {
                    for k in (i + 1)..j.min(lines.len()) {
                        let lk = lines[k];
                        let trimmed = lk.trim_start();
                        // marker line: just `|`, spaces and `+`
                        let is_marker = trimmed.starts_with('|')
                            && trimmed.contains('+')
                            && trimmed.chars().all(|c| c == '|' || c == ' ' || c == '+');
                        if is_marker && k > 0 {
                            let col = lk.find('+').unwrap_or(0);
                            let code_line = lines[k - 1];
                            if col < code_line.len() {
                                let tail: String = code_line[col..].chars()
                                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                                    .collect();
                                if !tail.is_empty() {
                                    help = Some(format!("try `{}`", tail));
                                }
                            }
                            break;
                        }
                    }
                }
            }
            let file = path.as_deref().map(file_of);
            diags.push(Diagnostic {
                severity: sev, code, category,
                file, path, line: lineno, col, message: msg, help,
            });
        }
        i += 1;
    }

    let error_count = diags.iter().filter(|d| d.severity == Severity::Error).count();
    let warning_count = diags.iter().filter(|d| d.severity == Severity::Warning).count();
    BuildSummary { diagnostics: diags, error_count, warning_count, failed_target }
}

/// Parse a gcc/clang diagnostic line: `path:line:col: error|warning|note: msg`.
/// (Cargo/rustc uses a different shape handled by parse_head/parse_location;
/// this is for the C/C++ compiler lines that surface inside make/ninja output.)
fn parse_gcc_line(line: &str) -> Option<Diagnostic> {
    // find ": error:" / ": warning:" (note: ignored)
    let (sev, marker) = if let Some(p) = line.find(": error:") {
        (Severity::Error, p)
    } else if let Some(p) = line.find(": warning:") {
        (Severity::Warning, p)
    } else {
        return None;
    };
    let loc = &line[..marker];           // path:line:col
    let msg = line[marker..].trim_start_matches(':')
        .trim_start_matches(|c: char| c.is_alphabetic())  // skip "error"/"warning"
        .trim_start_matches(':').trim().to_string();
    // loc = path:line:col  (col may be absent)
    let mut it = loc.rsplitn(3, ':');
    let a = it.next()?;
    let b = it.next();
    let (path, lineno, col) = match (a.parse::<u32>().ok(), b) {
        (Some(c), Some(b)) => {
            // a=col, b=line, remainder=path
            let path = it.next().unwrap_or(b).to_string();
            (path, b.parse::<u32>().ok(), Some(c))
        }
        _ => (loc.to_string(), None, None),
    };
    let category = categorize(None, &msg);
    Some(Diagnostic {
        severity: sev, code: None,
        category: if category == Category::Other { Category::Other } else { category },
        file: Some(file_of(&path)), path: Some(path),
        line: lineno, col, message: msg, help: None,
    })
}

/// Parse CMake / make output. CMake diagnostics look like:
///   `CMake Error at CMakeLists.txt:42 (message):`
///   `CMake Warning (dev) at src/CMakeLists.txt:10:`
/// and make/ninja interleave `error:`/`undefined reference` from the compiler.
/// We reuse the cargo head/location parsing for the embedded compiler lines and
/// add the CMake-specific heads.
pub fn parse_cmake(output: &str) -> BuildSummary {
    let mut diags: Vec<Diagnostic> = Vec::new();
    for line in output.lines() {
        let t = line.trim_start();
        let (sev, head) = if let Some(r) = t.strip_prefix("CMake Error") {
            (Severity::Error, Some(r))
        } else if let Some(r) = t.strip_prefix("CMake Warning") {
            (Severity::Warning, Some(r))
        } else {
            (Severity::Error, None)
        };
        if let Some(r) = head {
            // `... at <path>:<line> (cmd):` or `(dev) at <path>:<line>:`
            let after_at = r.split(" at ").nth(1).unwrap_or(r);
            let loc = after_at.split_whitespace().next().unwrap_or("");
            let mut parts = loc.rsplitn(2, ':');
            let line_s = parts.next().unwrap_or("");
            let path_s = parts.next().unwrap_or(loc);
            let lineno = line_s.trim_end_matches(':').parse::<u32>().ok();
            diags.push(Diagnostic {
                severity: sev, code: None, category: Category::CMake,
                file: if path_s.is_empty() { None } else { Some(file_of(path_s)) },
                path: if path_s.is_empty() { None } else { Some(path_s.to_string()) },
                line: lineno, col: None,
                message: r.trim_start_matches(|c| c != ':').trim_start_matches(':').trim().to_string(),
                help: None,
            });
            continue;
        }
        // embedded compiler errors inside make output
        if let Some(d) = parse_gcc_line(line) {
            diags.push(d);
            continue;
        }
        if let Some((s, code, msg)) = parse_head(line) {
            if classify_summary_line(s, &msg).is_some() { continue; }
            let category = categorize(code.as_deref(), &msg);
            diags.push(Diagnostic {
                severity: s, code, category,
                file: None, path: None, line: None, col: None, message: msg, help: None,
            });
        }
    }
    let error_count = diags.iter().filter(|d| d.severity == Severity::Error).count();
    let warning_count = diags.iter().filter(|d| d.severity == Severity::Warning).count();
    BuildSummary { diagnostics: diags, error_count, warning_count, failed_target: None }
}

/// Token-frugal text form: a one-line header + each error on its own line +
/// warnings collapsed into category counts. This is the "for the token eaters"
/// view — the full per-warning bodies are dropped.
pub fn minify(s: &BuildSummary) -> String {
    let mut out = String::new();
    let mark = if s.ok() { "✓" } else { "✗" };
    out.push_str(&format!("{} {} error{} · {} warning{}",
        mark, s.error_count, if s.error_count == 1 { "" } else { "s" },
        s.warning_count, if s.warning_count == 1 { "" } else { "s" }));
    if let Some(t) = &s.failed_target { out.push_str(&format!("  ({})", t)); }
    out.push('\n');
    let errs = s.errors();
    if !errs.is_empty() {
        out.push_str("errors:\n");
        for e in &errs {
            out.push_str("  ");
            out.push_str(&e.one_line());
            out.push('\n');
        }
    }
    let cats = s.category_counts();
    let warn_cats: Vec<String> = cats.iter()
        .filter(|(c, _)| *c != Category::Resolve || s.error_count == 0)
        .map(|(c, n)| format!("{} ×{}", c.key(), n))
        .collect();
    if !warn_cats.is_empty() {
        out.push_str("by category: ");
        out.push_str(&warn_cats.join(" · "));
        out.push('\n');
    }
    out
}

/// Wire form for the QML fuzzy menu, one record per line:
///   `SUMMARY\x1F<errors>\x1F<warnings>\x1F<failed_target>`
///   `CAT\x1F<key>\x1F<label>\x1F<count>`        (one per non-empty category)
///   `DIAG\x1F<sev>\x1F<code>\x1F<catkey>\x1F<file>\x1F<line>\x1F<col>\x1F<message>\x1F<help>`
/// Errors are emitted before warnings so the menu lists them first. The UI maps
/// <catkey> to a theme-tinted color and supports voice selection by index/code.
pub fn to_wire(s: &BuildSummary) -> String {
    let esc = |t: &str| t.replace('\\', " ").replace('\u{1F}', " ").replace('\n', " ");
    let mut out = format!("SUMMARY\u{1F}{}\u{1F}{}\u{1F}{}\n",
        s.error_count, s.warning_count, esc(s.failed_target.as_deref().unwrap_or("")));
    for (c, n) in s.category_counts() {
        out.push_str(&format!("CAT\u{1F}{}\u{1F}{}\u{1F}{}\n", c.key(), c.label(), n));
    }
    // errors first, then warnings; stable order within each
    let mut ordered: Vec<&Diagnostic> = s.diagnostics.iter()
        .filter(|d| d.severity == Severity::Error).collect();
    ordered.extend(s.diagnostics.iter().filter(|d| d.severity == Severity::Warning));
    for d in ordered {
        out.push_str(&format!("DIAG\u{1F}{}\u{1F}{}\u{1F}{}\u{1F}{}\u{1F}{}\u{1F}{}\u{1F}{}\u{1F}{}\n",
            d.severity.tag(),
            esc(d.code.as_deref().unwrap_or("")),
            d.category.key(),
            esc(d.file.as_deref().unwrap_or("")),
            d.line.map(|l| l.to_string()).unwrap_or_default(),
            d.col.map(|c| c.to_string()).unwrap_or_default(),
            esc(&d.message),
            esc(d.help.as_deref().unwrap_or("")),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // the user's real unendlich build log (trimmed to the diagnostic-bearing part)
    const LOG: &str = r#"   Compiling unendlich-ui v0.1.0 (/home/k/crates/unendlich-ui)
warning: unexpected `cfg` condition value: `winit`
   --> crates/unendlich-engine/src/input_codes.rs:254:7
    |
254 | #[cfg(feature = "winit")]
    |       ^^^^^^^^^^^^^^^^^
warning: use of deprecated method `egui::CentralPanel::show`: Use show_inside() instead
  --> crates/unendlich-engine/src/app/menu_view.rs:70:35
warning: variable does not need to be mutable
   --> crates/unendlich-engine/src/sim/dragon_forge.rs:184:9
warning: unused variable: `BerryBush`
   --> crates/unendlich-engine/src/render/mesher.rs:848:9
warning: function `form_id_of` is never used
  --> crates/unendlich-engine/src/sim/hybrid_form.rs:66:4
warning: variable `BerryBush` should have a snake case name
   --> crates/unendlich-engine/src/render/mesher.rs:848:9
warning: `unendlich-engine` (lib) generated 23 warnings
error[E0425]: cannot find value `_music` in this scope
    --> crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1575:30
     |
1575 |         if let Some(ref m) = _music {
     |                              ^^^^^^
help: a local variable with a similar name exists
     |
1575 |         if let Some(ref m) = fg_music {
     |                              ++
error: could not compile `unendlich-engine` (bin "unendlich-fullgame") due to 1 previous error
"#;

    #[test]
    fn parses_the_real_log() {
        let s = parse_cargo(LOG);
        assert_eq!(s.error_count, 1, "exactly one real error");
        // 6 warnings (the "generated 23 warnings" roll-up is NOT counted)
        assert_eq!(s.warning_count, 6, "warnings (roll-up excluded)");
        assert_eq!(s.failed_target.as_deref(), Some("unendlich-engine"));
    }

    #[test]
    fn extracts_error_details_and_fix_hint() {
        let s = parse_cargo(LOG);
        let e = s.errors()[0];
        assert_eq!(e.code.as_deref(), Some("E0425"));
        assert_eq!(e.category, Category::Resolve);
        assert_eq!(e.file.as_deref(), Some("unendlich_fullgame.rs"));
        assert_eq!(e.line, Some(1575));
        assert_eq!(e.col, Some(30));
        assert!(e.message.contains("cannot find value"));
        // the "similar name" suggestion is enriched to the real identifier
        assert_eq!(e.help.as_deref(), Some("try `fg_music`"), "help was {:?}", e.help);
    }

    #[test]
    fn categorizes_warnings() {
        let s = parse_cargo(LOG);
        let cat = |c: Category| s.diagnostics.iter().filter(|d| d.category == c).count();
        assert_eq!(cat(Category::Cfg), 1);
        assert_eq!(cat(Category::Deprecated), 1);
        assert_eq!(cat(Category::Style), 1);          // snake case
        // unused-variable + does-not-need-mut + never-used all roll into Unused
        assert!(cat(Category::Unused) >= 3, "unused bucket: {}", cat(Category::Unused));
        assert_eq!(cat(Category::Resolve), 1);        // the E0425 error
    }

    #[test]
    fn minify_is_compact_and_leads_with_error() {
        let s = parse_cargo(LOG);
        let m = minify(&s);
        assert!(m.starts_with("✗ 1 error · 6 warnings"), "header: {:?}", m.lines().next());
        assert!(m.contains("unendlich-engine"));
        assert!(m.contains("E0425"));
        assert!(m.contains("fg_music"));
        // far shorter than the input
        assert!(m.len() < LOG.len() / 2, "minified should be much smaller");
    }

    #[test]
    fn wire_round_trips_fields() {
        let s = parse_cargo(LOG);
        let w = to_wire(&s);
        assert!(w.starts_with("SUMMARY\u{1F}1\u{1F}6\u{1F}unendlich-engine\n"));
        // a CAT line per category and a DIAG line for the error, error first
        assert!(w.contains("CAT\u{1F}resolve\u{1F}"));
        let first_diag = w.lines().find(|l| l.starts_with("DIAG")).unwrap();
        assert!(first_diag.contains("error\u{1F}E0425\u{1F}resolve"));
        assert_eq!(first_diag.split('\u{1F}').count(), 9, "9 wire fields per diag");
    }

    #[test]
    fn location_parse_survives_path_colons() {
        // a path that itself contains no colon, plus line:col
        let loc = parse_location("  --> crates/x/y.rs:1575:30").unwrap();
        assert_eq!(loc, ("crates/x/y.rs".to_string(), 1575, 30));
    }

    #[test]
    fn clean_build_reports_ok() {
        let s = parse_cargo("   Compiling foo v0.1.0\n    Finished release [optimized] target(s) in 4s\n");
        assert!(s.ok());
        assert_eq!(s.error_count, 0);
        assert_eq!(s.warning_count, 0);
        assert!(minify(&s).starts_with("✓ 0 errors · 0 warnings"));
    }

    #[test]
    fn cmake_errors_parse() {
        let cm = "CMake Error at CMakeLists.txt:42 (add_executable):\n  Cannot find source file: missing.cpp\nCMake Warning (dev) at src/CMakeLists.txt:10 (project):\n  cmake_minimum_required not called\n";
        let s = parse_cmake(cm);
        assert_eq!(s.error_count, 1);
        assert_eq!(s.warning_count, 1);
        let e = &s.diagnostics[0];
        assert_eq!(e.category, Category::CMake);
        assert_eq!(e.file.as_deref(), Some("CMakeLists.txt"));
        assert_eq!(e.line, Some(42));
    }

    #[test]
    fn cmake_picks_up_embedded_compiler_errors() {
        let mk = "[ 50%] Building CXX object foo.o\n/src/foo.cpp:12:5: error: 'bar' was not declared in this scope\nmake: *** [foo.o] Error 1\n";
        let s = parse_cmake(mk);
        assert!(s.error_count >= 1, "embedded compiler error counted");
    }

    #[test]
    fn empty_input_is_clean() {
        let s = parse_cargo("");
        assert!(s.ok() && s.diagnostics.is_empty());
        assert_eq!(to_wire(&s), "SUMMARY\u{1F}0\u{1F}0\u{1F}\n");
    }
}
