// src/data.rs
//
// Runtime-loaded data tables with user overlay support.
//
// Tables are stored as plain string arrays in a TOML-subset format.
// Default tables ship at `data/<name>.toml` next to src/. User
// overrides at `~/.config/web2qml/data/<name>.toml` ADD entries
// (or remove defaults via the [remove] section).
//
// This is the foundation for moving project_markers, skip_dirs,
// source_exts (currently hardcoded in smart_cd.rs) plus future
// tables (command_synonyms, ext_actions, build_patterns,
// drone_signatures) into user-extensible config.
//
// Why a custom parser instead of the `toml` crate? These tables
// are small + simple (just string arrays + an optional [remove]
// table). The full TOML crate is ~600KB of binary growth for
// features we don't need. A 60-line parser handles our actual
// requirements with zero dependencies.
//
// Format (per file):
//   # Comments allowed
//   project_markers = [
//     "Cargo.toml",
//     "package.json",
//     ".git",
//   ]
//
//   # Optional remove-section: user overlay removes from defaults
//   [remove]
//   project_markers = [
//     "go.mod",
//   ]
//
// All values are strings. Single-line and multi-line array forms
// supported. Trailing commas OK. Hash comments OK at start of
// any line or inline after a value.

use std::path::PathBuf;
use std::sync::OnceLock;

/// One loaded table — additions, plus removals (entries the user
/// wants stripped from defaults). `additions` is what gets added
/// to the default list; `removals` is what gets filtered out.
#[derive(Default, Clone, Debug)]
struct ParsedTable {
    additions: Vec<String>,
    removals: Vec<String>,
}

/// Parsed contents of a single data file. Each top-level array key
/// gets its own ParsedTable.
#[derive(Default, Clone, Debug)]
struct ParsedFile {
    tables: std::collections::HashMap<String, ParsedTable>,
}

/// Storage for the merged final tables. Each table is an `Vec<String>`
/// (alphabetical-stable order isn't preserved — original default order
/// then user additions appended).
#[derive(Default)]
struct AllTables {
    project_markers: Vec<String>,
    skip_dirs: Vec<String>,
    source_exts: Vec<String>,
}

static ALL: OnceLock<AllTables> = OnceLock::new();

/// Initialize data tables from default + user overlay. Called once
/// at startup. Idempotent — subsequent calls do nothing because
/// OnceLock won't re-init.
pub fn init() {
    let _ = ALL.get_or_init(|| {
        let mut tables = AllTables::default();
        // Defaults — hardcoded, embedded in binary so we always have
        // SOMETHING even when no data/ dir is present.
        tables.project_markers = default_project_markers();
        tables.skip_dirs = default_skip_dirs();
        tables.source_exts = default_source_exts();

        // Try to find data/ directory. We look in:
        //   1. ./data/ (cwd-relative — running from project root)
        //   2. <exe>/../data/ (next to compiled binary)
        //   3. <exe>/../../data/ (target/release/web2qml → ../../data)
        // Most users will hit #1 in dev mode and #2 or #3 in release.
        let default_dir = locate_data_dir();
        if let Some(dir) = default_dir {
            apply_dir(&dir, &mut tables, /*is_user=*/false);
        }
        // User overlay at ~/.config/web2qml/data/
        if let Ok(home) = std::env::var("HOME") {
            let user_dir = std::path::Path::new(&home)
                .join(".config/web2qml/data");
            if user_dir.exists() {
                apply_dir(&user_dir, &mut tables, /*is_user=*/true);
            }
        }

        crate::clog!("data", "loaded: project_markers={}, skip_dirs={}, source_exts={}",
            tables.project_markers.len(),
            tables.skip_dirs.len(),
            tables.source_exts.len());

        tables
    });
}

fn locate_data_dir() -> Option<PathBuf> {
    // Cwd-relative
    let cwd_path = PathBuf::from("data");
    if cwd_path.is_dir() { return Some(cwd_path); }
    // exe-relative
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let p1 = parent.join("data");
            if p1.is_dir() { return Some(p1); }
            if let Some(grand) = parent.parent() {
                let p2 = grand.join("data");
                if p2.is_dir() { return Some(p2); }
                if let Some(ggrand) = grand.parent() {
                    let p3 = ggrand.join("data");
                    if p3.is_dir() { return Some(p3); }
                }
            }
        }
    }
    None
}

fn apply_dir(dir: &std::path::Path, tables: &mut AllTables, is_user: bool) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !name.ends_with(".toml") { continue; }
        // v0.1.112: only parse files we own. Other modules (like
        // phonetic_confusions) keep their own .toml in the same data
        // dir but with different schemas. Trying to parse them with
        // OUR schema produces noisy parse errors. Whitelist the files
        // we expect.
        let known = matches!(name.as_str(),
            "project_markers.toml" | "skip_dirs.toml" | "source_exts.toml");
        if !known { continue; }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                crate::clog!("data", "couldn't read {}: {}", path.display(), e);
                continue;
            }
        };
        let parsed = match parse(&content) {
            Ok(p) => p,
            Err(e) => {
                crate::clog!("data", "parse error in {}: {}", path.display(), e);
                continue;
            }
        };
        let label = if is_user { "user" } else { "default" };
        for (key, table) in &parsed.tables {
            let target = match key.as_str() {
                "project_markers" => &mut tables.project_markers,
                "skip_dirs"       => &mut tables.skip_dirs,
                "source_exts"     => &mut tables.source_exts,
                _ => {
                    // Unknown table — skip silently. Future tables
                    // will be added here.
                    continue;
                }
            };
            // Apply additions (dedup, preserve insertion order).
            for item in &table.additions {
                if !target.iter().any(|x| x == item) {
                    target.push(item.clone());
                }
            }
            // Apply removals.
            for item in &table.removals {
                target.retain(|x| x != item);
            }
            if !table.additions.is_empty() || !table.removals.is_empty() {
                crate::clog!("data", "{} {} (+{}, -{})",
                    label, key, table.additions.len(), table.removals.len());
            }
        }
    }
}

/// Public accessors. All return slices over the merged table.
/// Panics if init() hasn't been called — but init() is idempotent
/// and called from main, so this is a programmer error if it fires.
pub fn project_markers() -> &'static [String] {
    let t = ALL.get().expect("data::init() not called");
    &t.project_markers
}
pub fn skip_dirs() -> &'static [String] {
    let t = ALL.get().expect("data::init() not called");
    &t.skip_dirs
}
pub fn source_exts() -> &'static [String] {
    let t = ALL.get().expect("data::init() not called");
    &t.source_exts
}

// ─── Parser ────────────────────────────────────────────────────────
//
// Subset of TOML we actually use:
//   - Top-level array assignments: `key = ["a", "b", "c"]`
//   - Multi-line arrays with trailing commas and inline comments
//   - `[remove]` table containing the same shape
//   - `#` line comments and trailing comments

#[derive(Debug)]
enum Section { Top, Remove }

fn parse(content: &str) -> Result<ParsedFile, String> {
    let mut out = ParsedFile::default();
    let mut section = Section::Top;
    let mut buf: String = String::new();
    let mut in_array = false;
    let mut current_key: String = String::new();

    for (lineno, raw_line) in content.lines().enumerate() {
        let lineno = lineno + 1;
        // Strip trailing inline comment (but only when not inside
        // a string — for our purposes strings can't contain # so
        // a simple split works).
        let line = strip_comment(raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }

        if !in_array {
            // Section header?
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                let name = &trimmed[1..trimmed.len()-1].trim();
                section = match *name {
                    "remove" => Section::Remove,
                    other => {
                        return Err(format!("line {}: unknown section [{}]", lineno, other));
                    }
                };
                continue;
            }
            // key = ... line.
            let eq_idx = match trimmed.find('=') {
                Some(i) => i,
                None => {
                    return Err(format!("line {}: expected 'key = [...]', got: {}",
                        lineno, trimmed));
                }
            };
            let key = trimmed[..eq_idx].trim().to_string();
            let rest = trimmed[eq_idx+1..].trim();
            if !rest.starts_with('[') {
                return Err(format!("line {}: value must be array (start with '[')",
                    lineno));
            }
            current_key = key;
            buf.clear();
            buf.push_str(&rest[1..]);  // skip opening '['
            in_array = true;
            // Maybe one-line array?
            if buf.contains(']') {
                finalize_array(&mut buf, &mut out, &section, &current_key)?;
                in_array = false;
            }
        } else {
            buf.push(' ');
            buf.push_str(trimmed);
            if trimmed.contains(']') {
                finalize_array(&mut buf, &mut out, &section, &current_key)?;
                in_array = false;
            }
        }
    }
    if in_array {
        return Err("unterminated array at EOF".to_string());
    }
    Ok(out)
}

/// Strip an inline `#` comment from a line, respecting strings (we
/// don't have escapes inside strings for our purposes — strings
/// can't contain `#` legally in our table content).
fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    for (i, c) in line.char_indices() {
        if c == '"' { in_str = !in_str; continue; }
        if c == '#' && !in_str {
            return &line[..i];
        }
    }
    line
}

fn finalize_array(buf: &mut String, out: &mut ParsedFile, section: &Section, key: &str)
    -> Result<(), String>
{
    // Find the closing bracket.
    let close = buf.find(']').ok_or("missing closing bracket")?;
    let inner = &buf[..close];
    let mut items: Vec<String> = Vec::new();
    // Split on commas (top-level only — no nested arrays in our format).
    for raw_item in inner.split(',') {
        let s = raw_item.trim();
        if s.is_empty() { continue; }
        if !s.starts_with('"') || !s.ends_with('"') {
            return Err(format!("expected quoted string, got: {:?}", s));
        }
        let inner_str = &s[1..s.len()-1];
        // Basic escape handling — we only support \\ and \" since
        // our content doesn't need anything fancier.
        let unescaped = inner_str
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
        items.push(unescaped);
    }
    let table = out.tables.entry(key.to_string()).or_default();
    match section {
        Section::Top    => table.additions.extend(items),
        Section::Remove => table.removals.extend(items),
    }
    Ok(())
}

// ─── Defaults ──────────────────────────────────────────────────────
//
// These mirror the hardcoded values previously in smart_cd.rs.
// Kept in code so the binary always has SOMETHING even if no data/
// directory is present. The data/ files duplicate these for users
// who want to see/edit them.

fn default_project_markers() -> Vec<String> {
    [
        // Rust
        "Cargo.toml",
        // Node
        "package.json", "yarn.lock", "pnpm-lock.yaml",
        // Python
        "pyproject.toml", "setup.py", "setup.cfg", "requirements.txt",
        "Pipfile", "poetry.lock",
        // Go
        "go.mod",
        // Java/Kotlin
        "pom.xml", "build.gradle", "build.gradle.kts", "settings.gradle",
        // Ruby
        "Gemfile", "Rakefile",
        // .NET
        "*.csproj", "*.sln",
        // C/C++
        "Makefile", "CMakeLists.txt", "configure", "configure.ac", "meson.build",
        // Generic
        "README.md", "README", "README.rst", "LICENSE",
        ".git", ".hg", ".svn",
        // Web
        "index.html",
        // Containers
        "Dockerfile", "docker-compose.yml",
        // CI
        ".github", ".gitlab-ci.yml",
    ].iter().map(|s| s.to_string()).collect()
}

fn default_skip_dirs() -> Vec<String> {
    [
        "node_modules", "target", "dist", "build", "out",
        ".git", ".hg", ".svn",
        ".cache", ".idea", ".vscode",
        "vendor", "__pycache__", ".pytest_cache",
        ".tox", ".mypy_cache", ".ruff_cache",
        "venv", ".venv", "env", ".env",
        "bower_components", "coverage", ".coverage",
        "logs", "tmp", "temp",
    ].iter().map(|s| s.to_string()).collect()
}

fn default_source_exts() -> Vec<String> {
    [
        ".rs", ".py", ".ts", ".tsx", ".js", ".jsx",
        ".go", ".java", ".kt", ".kts", ".scala",
        ".c", ".h", ".cpp", ".cc", ".hpp",
        ".rb", ".php", ".swift", ".m", ".mm",
        ".cs", ".fs", ".vb",
        ".sh", ".bash", ".zsh", ".fish",
        ".lua", ".pl", ".pm",
        ".vim", ".el", ".lisp", ".clj",
        ".html", ".css", ".scss", ".sass",
        ".sql",
    ].iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_array() {
        let input = "project_markers = [\"a\", \"b\", \"c\"]\n";
        let p = parse(input).unwrap();
        let t = p.tables.get("project_markers").unwrap();
        assert_eq!(t.additions, vec!["a", "b", "c"]);
        assert!(t.removals.is_empty());
    }

    #[test]
    fn parse_multiline_array() {
        let input = r#"
project_markers = [
  "a",
  "b",
  "c",
]
"#;
        let p = parse(input).unwrap();
        assert_eq!(p.tables["project_markers"].additions,
            vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_with_remove_section() {
        let input = r#"
project_markers = ["foo", "bar"]
[remove]
project_markers = ["go.mod"]
"#;
        let p = parse(input).unwrap();
        let t = &p.tables["project_markers"];
        assert_eq!(t.additions, vec!["foo", "bar"]);
        assert_eq!(t.removals, vec!["go.mod"]);
    }

    #[test]
    fn parse_with_comments() {
        let input = r#"
# This is a comment
project_markers = [
  "a",   # inline comment
  "b",
]
"#;
        let p = parse(input).unwrap();
        assert_eq!(p.tables["project_markers"].additions,
            vec!["a", "b"]);
    }

    #[test]
    fn parse_rejects_unknown_section() {
        let input = "[bogus]\nx = [\"y\"]\n";
        assert!(parse(input).is_err());
    }

    #[test]
    fn parse_handles_empty_file() {
        let p = parse("").unwrap();
        assert!(p.tables.is_empty());
    }

    #[test]
    fn defaults_are_populated() {
        // Smoke check for the embedded defaults.
        assert!(default_project_markers().len() > 20);
        assert!(default_skip_dirs().len() > 10);
        assert!(default_source_exts().len() > 20);
    }

    #[test]
    fn defaults_contain_known_markers() {
        let m = default_project_markers();
        assert!(m.iter().any(|s| s == "Cargo.toml"));
        assert!(m.iter().any(|s| s == "package.json"));
        assert!(m.iter().any(|s| s == ".git"));
    }
}
