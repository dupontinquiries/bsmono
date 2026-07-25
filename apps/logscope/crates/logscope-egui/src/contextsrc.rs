//! contextsrc.rs — what the session is working ON, when it isn't a log.
//!
//! LogScope's input has been a log file, and with none loaded the UI dead-ends ("no log loaded — use the
//! Sources tab"). That's the wrong default in two directions: a user pointed at a real project has a rich
//! context sitting right there (a `.gitmind` project, or any cargo crate/workspace), and a user with
//! nothing loaded should still be able to just talk to the model rather than hit a wall.
//!
//! So this resolves a directory to one of four sources — GitMind, Cargo, Logs, or OpenEnded — and turns
//! the first three into FACTS for the system prompt: crate name, edition, dependency list, module layout.
//! Never a narration of what the project "seems to be about", which would be exactly the invention the
//! Liaison forbids: everything here is read off files that exist, and when nothing is found it says so and
//! opens the session up rather than manufacturing a subject.
//!
//! The parsing entry points are pure (they take file contents), so detection and summarisation are
//! testable without touching a filesystem; `detect_dir` is the thin IO shell over them.

#![allow(dead_code)]

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectKind {
    /// A `.gitmind` project — has its own session/commit discipline (see `gitmind_session`).
    GitMind,
    /// A cargo crate or workspace.
    Cargo,
    /// A directory of logs — the historical input.
    Logs,
    /// Nothing identifiable: talk to the model with no project context rather than refusing.
    OpenEnded,
}

impl ProjectKind {
    pub fn label(&self) -> &'static str {
        match self { ProjectKind::GitMind => "gitmind", ProjectKind::Cargo => "cargo",
                     ProjectKind::Logs => "logs", ProjectKind::OpenEnded => "open-ended" }
    }
}

/// Facts read out of a Cargo.toml. Deliberately a small, literal subset — enough to ground a conversation,
/// nothing inferred.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CargoFacts {
    pub name: String,
    pub version: String,
    pub edition: String,
    pub deps: Vec<String>,
    pub is_workspace: bool,
    pub members: Vec<String>,
}

/// Parse the handful of Cargo.toml fields we actually use. A deliberately small hand-rolled reader: it
/// tracks the current `[section]` and pulls `key = value` pairs from the ones that matter, which covers the
/// ordinary shape of a manifest without taking a TOML dependency for six fields.
pub fn parse_cargo(toml: &str) -> CargoFacts {
    let mut f = CargoFacts::default();
    let mut section = String::new();
    for raw in toml.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() { continue; }
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(|c| c == '[' || c == ']').to_string();
            if section == "workspace" { f.is_workspace = true; }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let unq = |s: &str| s.trim().trim_matches('"').trim_matches('\'').to_string();
        match section.as_str() {
            "package" => match k {
                "name" => f.name = unq(v),
                "version" => f.version = unq(v),
                "edition" => f.edition = unq(v),
                _ => {}
            },
            "dependencies" | "dev-dependencies" => {
                if !k.is_empty() && !f.deps.contains(&k.to_string()) { f.deps.push(k.to_string()); }
            }
            "workspace" => {
                if k == "members" {
                    for m in v.trim_matches(|c| c == '[' || c == ']').split(',') {
                        let m = unq(m);
                        if !m.is_empty() { f.members.push(m); }
                    }
                }
            }
            _ => {}
        }
    }
    f
}

/// What a session is working on, as facts ready for the prompt.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectContext {
    pub kind: ProjectKind,
    pub name: String,
    /// One fact per line, each read off a file that exists.
    pub facts: Vec<String>,
    /// Files worth offering the user first.
    pub entry_files: Vec<String>,
}

impl ProjectContext {
    /// The honest empty state: no project, no pretence, conversation still available.
    pub fn open_ended() -> Self {
        ProjectContext {
            kind: ProjectKind::OpenEnded,
            name: String::new(),
            facts: Vec::new(),
            entry_files: Vec::new(),
        }
    }
    pub fn is_open_ended(&self) -> bool { self.kind == ProjectKind::OpenEnded }
}

/// Rust source files, shallowest first then alphabetical, so `main.rs`/`lib.rs` lead.
fn rust_entry_files(paths: &[String]) -> Vec<String> {
    let mut v: Vec<String> = paths.iter()
        .filter(|p| p.ends_with(".rs"))
        .cloned().collect();
    v.sort_by_key(|p| {
        let depth = p.matches('/').count();
        let rank = if p.ends_with("/main.rs") || p == "main.rs" { 0 }
                   else if p.ends_with("/lib.rs") || p == "lib.rs" { 1 } else { 2 };
        (rank, depth, p.clone())
    });
    v.truncate(12);
    v
}

/// Build context for a cargo crate or workspace from its manifest and file list.
pub fn cargo_context(manifest: &str, paths: &[String]) -> ProjectContext {
    let f = parse_cargo(manifest);
    let mut facts = Vec::new();
    let name = if f.name.is_empty() && f.is_workspace { "workspace".to_string() } else { f.name.clone() };
    if f.is_workspace {
        facts.push(format!("cargo workspace with {} member(s): {}", f.members.len(), f.members.join(", ")));
    }
    if !f.name.is_empty() {
        let ver = if f.version.is_empty() { String::new() } else { format!(" v{}", f.version) };
        let ed = if f.edition.is_empty() { String::new() } else { format!(", edition {}", f.edition) };
        facts.push(format!("crate {}{}{}", f.name, ver, ed));
    }
    if !f.deps.is_empty() {
        facts.push(format!("{} direct dependencies: {}", f.deps.len(), f.deps.join(", ")));
    }
    let rs = paths.iter().filter(|p| p.ends_with(".rs")).count();
    if rs > 0 { facts.push(format!("{rs} Rust source file(s)")); }
    ProjectContext { kind: ProjectKind::Cargo, name, facts, entry_files: rust_entry_files(paths) }
}

/// Build context for a `.gitmind` project. The config is read for a name; everything else is the file
/// layout, because inventing a project's purpose from its directory names is exactly the failure mode.
pub fn gitmind_context(config: &str, paths: &[String]) -> ProjectContext {
    let mut name = String::new();
    let mut section = String::new();
    for raw in config.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(|c| c == '[' || c == ']').to_string();
            continue;
        }
        if section == "gitmind" {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "name" { name = v.trim().trim_matches('"').to_string(); }
            }
        }
    }
    let mut facts = vec![format!("gitmind project{}",
        if name.is_empty() { String::new() } else { format!(" \"{name}\"") })];
    facts.push("changes are mirrored to an isolated /tmp session and reapplied as one commit".into());
    // a gitmind project is usually also a cargo project; surface that rather than choosing one identity
    if paths.iter().any(|p| p == "Cargo.toml" || p.ends_with("/Cargo.toml")) {
        facts.push("also a cargo project".into());
    }
    ProjectContext { kind: ProjectKind::GitMind, name, facts, entry_files: rust_entry_files(paths) }
}

/// Resolve which source a directory represents, from its file list alone (pure; `detect_dir` does the IO).
/// GitMind wins over Cargo when both are present, because its session discipline changes how edits must be
/// applied — that is a behavioural difference, not a labelling preference.
pub fn detect(paths: &[String]) -> ProjectKind {
    let has = |suffix: &str| paths.iter().any(|p| p == suffix || p.ends_with(&format!("/{suffix}")));
    if has(".gitmind/config.toml") || paths.iter().any(|p| p.contains(".gitmind/config.toml")) {
        return ProjectKind::GitMind;
    }
    if has("Cargo.toml") { return ProjectKind::Cargo; }
    let logs = paths.iter().filter(|p| p.ends_with(".log") || p.ends_with(".jsonl")).count();
    if logs > 0 { return ProjectKind::Logs; }
    ProjectKind::OpenEnded
}

/// The prompt block describing the working context. Facts only, and when there is no project it says so
/// plainly — a model told "you are working on a project" with no project will invent one.
pub fn system_prompt_block(ctx: &ProjectContext) -> String {
    if ctx.is_open_ended() {
        return "\n\nNo project or log is loaded. There is no codebase in context: answer from general \
knowledge, and if a question depends on the user's files, say what you would need to see rather than \
assuming what their code contains.".to_string();
    }
    let mut s = format!("\n\nWORKING CONTEXT ({})", ctx.kind.label());
    if !ctx.name.is_empty() { s.push_str(&format!(": {}", ctx.name)); }
    s.push('\n');
    for f in &ctx.facts { s.push_str(&format!("- {f}\n")); }
    if !ctx.entry_files.is_empty() {
        s.push_str(&format!("Files available: {}\n", ctx.entry_files.join(", ")));
    }
    s.push_str("These facts are read from the project's own files. Anything not listed here you have NOT \
been shown — ask for it rather than assuming its contents.");
    s
}

/// IO shell: walk `dir` (bounded) and resolve it to a context.
pub fn detect_dir(dir: &Path) -> ProjectContext {
    let mut paths: Vec<String> = Vec::new();
    collect(dir, dir, 0, &mut paths);
    match detect(&paths) {
        ProjectKind::GitMind => {
            let cfg = std::fs::read_to_string(dir.join(".gitmind/config.toml")).unwrap_or_default();
            gitmind_context(&cfg, &paths)
        }
        ProjectKind::Cargo => {
            let man = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
            cargo_context(&man, &paths)
        }
        ProjectKind::Logs => {
            let logs: Vec<String> = paths.iter()
                .filter(|p| p.ends_with(".log") || p.ends_with(".jsonl")).take(12).cloned().collect();
            ProjectContext {
                kind: ProjectKind::Logs,
                name: dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                facts: vec![format!("{} log file(s)", logs.len())],
                entry_files: logs,
            }
        }
        ProjectKind::OpenEnded => ProjectContext::open_ended(),
    }
}

/// Bounded recursive walk: skips the directories that are never context (`.git`, `target`, `node_modules`)
/// and stops at depth 3, so pointing at a large tree cannot stall the UI.
fn collect(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > 3 || out.len() > 2000 { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), ".git" | "target" | "node_modules") { continue; }
        if p.is_dir() {
            if name == ".gitmind" {
                if p.join("config.toml").exists() { out.push(".gitmind/config.toml".into()); }
                continue;
            }
            if name.starts_with('.') { continue; }
            collect(root, &p, depth + 1, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cargo_gitmind_logs_and_nothing() {
        assert_eq!(detect(&["Cargo.toml".into(), "src/main.rs".into()]), ProjectKind::Cargo);
        assert_eq!(detect(&["run.log".into()]), ProjectKind::Logs);
        assert_eq!(detect(&["notes.md".into()]), ProjectKind::OpenEnded);
        // gitmind wins over cargo: its session discipline changes how edits are applied
        assert_eq!(detect(&[".gitmind/config.toml".into(), "Cargo.toml".into()]), ProjectKind::GitMind);
    }

    #[test]
    fn parses_the_manifest_fields_we_actually_use() {
        let m = r#"
[package]
name = "logscope"   # trailing comment
version = "0.4.1"
edition = "2021"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
wgpu = "0.19"
"#;
        let f = parse_cargo(m);
        assert_eq!(f.name, "logscope");
        assert_eq!(f.version, "0.4.1");
        assert_eq!(f.edition, "2021");
        assert_eq!(f.deps, vec!["serde", "wgpu"]);
        assert!(!f.is_workspace);
    }

    #[test]
    fn parses_a_workspace_manifest() {
        let m = "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n";
        let f = parse_cargo(m);
        assert!(f.is_workspace);
        assert_eq!(f.members, vec!["crates/a", "crates/b"]);
        let ctx = cargo_context(m, &["crates/a/src/lib.rs".into()]);
        assert_eq!(ctx.name, "workspace");
        assert!(ctx.facts.iter().any(|f| f.contains("2 member(s)")));
    }

    #[test]
    fn cargo_context_states_only_what_the_manifest_says() {
        let ctx = cargo_context("[package]\nname = \"demo\"\nversion = \"1.2.3\"\n",
                                &["src/main.rs".into(), "src/util.rs".into()]);
        let joined = ctx.facts.join("\n");
        assert!(joined.contains("crate demo v1.2.3"));
        assert!(joined.contains("2 Rust source file(s)"));
        // no dependencies section → no dependency claim at all, rather than "0 dependencies"
        assert!(!joined.contains("dependencies"));
        assert_eq!(ctx.entry_files[0], "src/main.rs", "entry points lead");
    }

    #[test]
    fn open_ended_says_so_instead_of_inventing_a_subject() {
        let ctx = ProjectContext::open_ended();
        assert!(ctx.is_open_ended());
        let p = system_prompt_block(&ctx);
        assert!(p.contains("No project or log is loaded"));
        assert!(p.contains("say what you would need to see"), "must not invite assumptions: {p}");
        // and it must NOT claim a working context
        assert!(!p.contains("WORKING CONTEXT"));
    }

    #[test]
    fn prompt_block_marks_unlisted_files_as_unseen() {
        let ctx = cargo_context("[package]\nname = \"demo\"\n", &["src/main.rs".into()]);
        let p = system_prompt_block(&ctx);
        assert!(p.contains("WORKING CONTEXT (cargo): demo"));
        assert!(p.contains("src/main.rs"));
        // the anti-invention clause is the point of the block
        assert!(p.contains("ask for it rather than assuming"));
    }

    #[test]
    fn gitmind_context_surfaces_both_identities() {
        let ctx = gitmind_context("[gitmind]\nname = \"myproj\"\n",
                                  &[".gitmind/config.toml".into(), "Cargo.toml".into(), "src/lib.rs".into()]);
        assert_eq!(ctx.kind, ProjectKind::GitMind);
        assert_eq!(ctx.name, "myproj");
        let j = ctx.facts.join("\n");
        assert!(j.contains("gitmind project \"myproj\""));
        assert!(j.contains("also a cargo project"), "does not hide the cargo identity");
        assert!(j.contains("reapplied as one commit"), "states the edit discipline that actually applies");
    }

    #[test]
    fn walks_a_real_directory_and_ignores_build_output() {
        let dir = std::env::temp_dir().join(format!("logscope_ctx_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"walked\"\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("target/debug/junk.rs"), "// build output").unwrap();
        let ctx = detect_dir(&dir);
        assert_eq!(ctx.kind, ProjectKind::Cargo);
        assert_eq!(ctx.name, "walked");
        assert!(ctx.entry_files.iter().any(|p| p == "src/main.rs"));
        assert!(!ctx.entry_files.iter().any(|p| p.contains("target/")), "build output is not context");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_directory_opens_the_session_rather_than_failing() {
        let dir = std::env::temp_dir().join(format!("logscope_ctx_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(detect_dir(&dir).is_open_ended());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
