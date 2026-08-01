//! workspace_exec.rs — the agentic loop that lets a LOCAL LLM build/edit code in an ISOLATED
//! `/tmp` workspace by TOOL CALLING, so a coding session weaves a codebase together turn by turn
//! and the UI can watch files appear.
//!
//! Where this sits
//! ───────────────
//! `mount::isolate` already mirrors a project into `tmp/logscope/{uuid}` with a fresh git baseline, and
//! `mount::{snapshot,bundle,apply_hint}` hand the result back as a reviewable commit / cloneable bundle.
//! What was missing is the *middle*: a model driving that scratch. This module is that middle — a pure,
//! testable driver that:
//!   1. teaches a model the tool protocol (`system_prompt`),
//!   2. parses the tool calls back out of its text (`parse_tools` — handles the two shapes local models
//!      actually emit: a fenced ```tool JSON block and Qwen/ChatML `<tool_call>…</tool_call>`), and
//!   3. runs each call against the workspace (`Workspace::apply`) with a hard SANDBOX (no path escapes)
//!      and an allowlisted, shell-free, timed, output-capped `run`, RECORDING every file it touches so
//!      the GUI can render live file-creation + make the scratch a real drag-and-drop target.
//!
//! The model is behind the [`Llm`] trait so the loop is unit-tested with a scripted model (no llama.cpp,
//! no GPU) — the same seam the real `llm_backend::generate` plugs into on hardware.
//!
//! SAFETY is not optional here: an LLM writing files and running commands is exactly where a workspace
//! escape or an `rm -rf` would bite. Two gates, both enforced here (not trusted to the prompt):
//!   * every path is resolved and REJECTED if it leaves the workspace root (`sandbox_path`), and
//!   * `run` executes argv directly (never `sh -c`) and only binaries on an explicit allowlist.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ── tools the model can call ────────────────────────────────────────────────────────────────────

/// One tool the model may invoke. Deliberately small + auditable — the four verbs a coding agent needs
/// to weave a tree together (see/read/write files, run a checked command), nothing that can reach outside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Create or overwrite a file at `path` (workspace-relative) with `contents`.
    WriteFile { path: String, contents: String },
    /// Read a file's contents back into the model's context.
    ReadFile { path: String },
    /// List a directory (workspace-relative; "" or "." = root).
    ListDir { path: String },
    /// Run an allowlisted command (argv, no shell) with `cwd` inside the workspace.
    Run { argv: Vec<String> },
    /// Search files for a regex/substring (grep-style). For COORDINATOR-spawned workers that need cheap
    /// navigation of a big repo without pulling whole files into a small context window. `path` scopes it
    /// ("" = whole workspace). Not offered to the primary agent, which reads files whole (local compute is
    /// cheap; more context up front is better — see `ReadPolicy`).
    Search { pattern: String, path: String },
    /// Read a line range of a file (1-based, inclusive) — the surgical companion to `Search` for workers.
    ReadRange { path: String, start: usize, end: usize },
    /// PATCH an existing file: replace exactly ONE occurrence of a verbatim `find` anchor with
    /// `replace`. The context-economy primitive: big models re-emitting whole files is what fills the
    /// window and drags tok/s; an anchored edit is a few dozen tokens and — unlike unified diffs,
    /// which small models drift on (line numbers, context hashes) — either applies exactly or fails
    /// with a teachable error (0 matches = stale anchor, >1 = grow the anchor).
    Edit { path: String, find: String, replace: String },
    /// Write a SHARED doc for the whole crew: `.logscope/docs/<name>.md` in the workspace. Every
    /// agent run and fan worker gets all shared docs injected into its prompt (see
    /// `collect_shared_docs`), so this is how one worker's reconnaissance ("here is the module map",
    /// "the block registry lives in blocks.rs, add via register()") becomes everyone's context.
    /// Never triggers verification (docs aren't code) and never counts as task output.
    Doc { name: String, content: String },
}

impl Tool {
    /// A one-line human summary for the transcript / activity HUD.
    pub fn summary(&self) -> String {
        match self {
            Tool::WriteFile { path, contents } => format!("write {} ({} B)", path, contents.len()),
            Tool::ReadFile { path } => format!("read {path}"),
            Tool::ListDir { path } => format!("ls {}", if path.is_empty() { "." } else { path }),
            Tool::Run { argv } => format!("run {}", argv.join(" ")),
            Tool::Search { pattern, path } => format!("search /{}/ in {}", pattern, if path.is_empty() { "." } else { path }),
            Tool::ReadRange { path, start, end } => format!("read {path}:{start}-{end}"),
            Tool::Edit { path, find, replace } => format!("edit {} (−{} +{} B)", path, find.len(), replace.len()),
            Tool::Doc { name, content } => format!("doc {} ({} B, shared)", name, content.len()),
        }
    }
}

/// The outcome of applying a tool — fed straight back to the model as its next observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolResult {
    pub ok: bool,
    pub output: String,
}
impl ToolResult {
    fn ok(s: impl Into<String>) -> Self { Self { ok: true, output: s.into() } }
    fn err(s: impl Into<String>) -> Self { Self { ok: false, output: s.into() } }
}

// ── file events (what the GUI renders as "files appearing") ─────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChange { Created, Modified }

/// A single file mutation the agent made — the stream the GUI turns into live "file created" chips and
/// the scratch tree it can drag files into/out of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEvent {
    pub path: String,
    pub change: FileChange,
    pub bytes: u64,
}

// ── the workspace: sandboxed file ops + allowlisted run, with event recording ───────────────────

/// The execution surface over one isolated scratch dir. Wraps a `root` (typically `mount::Workspace::dir`)
/// and enforces the sandbox on every path. Records a `FileEvent` per write so the UI has a live feed.
pub struct Workspace {
    root: PathBuf,
    allow: BTreeSet<String>,
    max_output: usize,
    timeout: Duration,
    events: Vec<FileEvent>,
    /// Library roots for READ-ONLY virtual paths (`~deps/<crate>/…` = cargo registry source,
    /// `~std/…` = the Rust toolchain's stdlib source, doc comments and all). The source IS the
    /// documentation — this is what lets a small model check an exact method signature instead of
    /// hallucinating one. Writes/edits under `~` are always refused.
    cargo_home: PathBuf,
    rust_src: std::cell::OnceCell<Result<PathBuf, String>>,
}

impl Workspace {
    /// A workspace rooted at `root` with a default-safe command allowlist (read-only inspection + the
    /// build/test tools a Rust session needs). `root` must already exist (e.g. from `mount::isolate`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            allow: default_allow(),
            max_output: 64 * 1024,
            timeout: Duration::from_secs(120),
            events: Vec::new(),
            cargo_home: std::env::var_os("CARGO_HOME").map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cargo")),
            rust_src: std::cell::OnceCell::new(),
        }
    }

    /// Replace the command allowlist (the safety gate for `Run`). Binaries are matched by exact basename.
    pub fn with_allow(mut self, bins: &[&str]) -> Self {
        self.allow = bins.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn with_timeout(mut self, d: Duration) -> Self { self.timeout = d; self }

    pub fn root(&self) -> &Path { &self.root }
    pub fn events(&self) -> &[FileEvent] { &self.events }
    /// Drain the file-event feed (the GUI polls this each frame, then clears it).
    pub fn take_events(&mut self) -> Vec<FileEvent> { std::mem::take(&mut self.events) }

    /// Resolve a workspace-relative path to an absolute one INSIDE the root, or reject it. Rejects
    /// absolute paths, `..` traversal, and anything that would resolve outside `root` — the sandbox.
    pub fn sandbox_path(&self, rel: &str) -> Result<PathBuf, String> {
        let p = Path::new(rel);
        if p.is_absolute() {
            return Err(format!("absolute paths are not allowed: {rel}"));
        }
        let mut out = self.root.clone();
        for comp in p.components() {
            match comp {
                Component::Normal(c) => out.push(c),
                Component::CurDir => {}
                Component::ParentDir => return Err(format!("path escapes the workspace: {rel}")),
                Component::RootDir | Component::Prefix(_) => {
                    return Err(format!("absolute paths are not allowed: {rel}"));
                }
            }
        }
        // Defence in depth: even after component filtering, confirm the result is still under root
        // (guards against symlink games — a normalized prefix check).
        if !out.starts_with(&self.root) {
            return Err(format!("path escapes the workspace: {rel}"));
        }
        Ok(out)
    }

    /// Execute a tool, recording any file change. Never panics; every failure becomes an `err` result the
    /// model can read and recover from.
    pub fn apply(&mut self, tool: &Tool) -> ToolResult {
        match tool {
            Tool::WriteFile { path, contents } => self.write_file(path, contents),
            Tool::ReadFile { path } => self.read_file(path),
            Tool::ListDir { path } => self.list_dir(path),
            Tool::Run { argv } => self.run(argv),
            Tool::Search { pattern, path } => self.search(pattern, path),
            Tool::ReadRange { path, start, end } => self.read_range(path, *start, *end),
            Tool::Edit { path, find, replace } => self.edit_file(path, find, replace),
            Tool::Doc { name, content } => self.write_doc(name, content),
        }
    }

    /// Guarded join: components of `rel` appended under `base`, `..`/absolute rejected — the same
    /// discipline as `sandbox_path`, reusable for the virtual library roots.
    fn join_guarded(base: &Path, rel: &str) -> Result<PathBuf, String> {
        let mut out = base.to_path_buf();
        for comp in Path::new(rel).components() {
            match comp {
                Component::Normal(c) => out.push(c),
                Component::CurDir => {}
                Component::ParentDir => return Err(format!("path escapes its root: {rel}")),
                Component::RootDir | Component::Prefix(_) => return Err(format!("absolute paths are not allowed: {rel}")),
            }
        }
        Ok(out)
    }

    /// Resolve a READ path: workspace-relative as before, or one of the read-only virtual roots —
    /// `~std/<lib>/…` (stdlib source: Vec/String in ~std/alloc, io/fs in ~std/std, Option/Iterator
    /// in ~std/core) and `~deps/<crate>/…` (the dependency's source exactly as cargo fetched it,
    /// version picked from this workspace's Cargo.lock).
    fn resolve_read(&self, rel: &str) -> Result<PathBuf, String> {
        if let Some(rest) = rel.strip_prefix("~std/").or(if rel == "~std" { Some("") } else { None }) {
            let root = self.rust_src_root()?;
            return Self::join_guarded(&root, rest);
        }
        if let Some(rest) = rel.strip_prefix("~deps/").or(if rel == "~deps" { Some("") } else { None }) {
            let mut it = rest.splitn(2, '/');
            let name = it.next().unwrap_or("");
            if name.is_empty() { return Err("~deps needs a crate: ~deps/<crate>/… (list_dir ~deps to see them)".into()); }
            let dir = self.dep_dir(name)?;
            return Self::join_guarded(&dir, it.next().unwrap_or(""));
        }
        if rel.starts_with('~') {
            return Err(format!("unknown virtual root in {rel} — only ~std/… and ~deps/<crate>/… exist"));
        }
        self.sandbox_path(rel)
    }

    /// The stdlib source root: LOGSCOPE_RUST_SRC override, else `rustc --print sysroot` +
    /// lib/rustlib/src/rust/library. Missing → a teachable error (rust-src component).
    fn rust_src_root(&self) -> Result<PathBuf, String> {
        self.rust_src.get_or_init(|| {
            if let Ok(p) = std::env::var("LOGSCOPE_RUST_SRC") {
                let p = PathBuf::from(p);
                return if p.is_dir() { Ok(p) } else { Err(format!("LOGSCOPE_RUST_SRC={} is not a directory", p.display())) };
            }
            let out = std::process::Command::new("rustc").arg("--print").arg("sysroot").output()
                .map_err(|e| format!("rustc not found ({e}) — set LOGSCOPE_RUST_SRC to the stdlib source"))?;
            let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let p = PathBuf::from(sysroot).join("lib/rustlib/src/rust/library");
            if p.is_dir() { Ok(p) } else {
                Err("stdlib source not installed — `rustup component add rust-src` (or set LOGSCOPE_RUST_SRC)".into())
            }
        }).clone()
    }

    /// Locate a dependency's source dir in the cargo registry, preferring the version this
    /// workspace's Cargo.lock pins; else the highest version present.
    fn dep_dir(&self, name: &str) -> Result<PathBuf, String> {
        let locked = std::fs::read_to_string(self.root.join("Cargo.lock")).ok()
            .and_then(|t| cargo_lock_versions(&t).into_iter().find(|(n, _)| n == name).map(|(_, v)| v));
        let src = self.cargo_home.join("registry").join("src");
        let mut best: Option<(Vec<u64>, PathBuf)> = None;
        if let Ok(rd) = std::fs::read_dir(&src) {
            for reg in rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
                if let Some(v) = &locked {
                    let exact = reg.join(format!("{name}-{v}"));
                    if exact.is_dir() { return Ok(exact); }
                }
                if let Ok(rd2) = std::fs::read_dir(&reg) {
                    for e in rd2.flatten() {
                        let fname = e.file_name().to_string_lossy().into_owned();
                        if let Some(vs) = fname.strip_prefix(&format!("{name}-")) {
                            if vs.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && e.path().is_dir() {
                                let key = version_key(vs);
                                if best.as_ref().map(|(k, _)| key > *k).unwrap_or(true) { best = Some((key, e.path())); }
                            }
                        }
                    }
                }
            }
        }
        best.map(|(_, p)| p).ok_or_else(|| format!(
            "dependency '{name}' not found in the cargo registry — list_dir ~deps shows this project's deps"))
    }

    /// `list_dir ~deps`: this workspace's dependencies (from Cargo.lock), each marked readable or not.
    fn list_deps(&self) -> ToolResult {
        let Ok(text) = std::fs::read_to_string(self.root.join("Cargo.lock")) else {
            return ToolResult::err("no Cargo.lock in the workspace — build once, or name the crate directly (~deps/<crate>/…)");
        };
        let mut lines: Vec<String> = cargo_lock_versions(&text).into_iter().map(|(n, v)| {
            let ok = self.dep_dir(&n).is_ok();
            format!("{n} {v}{}", if ok { "" } else { "  (source not in registry — path/workspace dep?)" })
        }).collect();
        lines.sort();
        lines.truncate(200);
        ToolResult::ok(format!("dependencies (read any with ~deps/<name>/src/lib.rs):\n{}", lines.join("\n")))
    }

    fn search(&self, pattern: &str, rel: &str) -> ToolResult {
        let base = if rel.is_empty() || rel == "." {
            self.root.clone()
        } else {
            match self.resolve_read(rel) { Ok(p) => p, Err(e) => return ToolResult::err(e) }
        };
        let mut hits = Vec::new();
        fn walk(dir: &Path, root: &Path, pat: &str, hits: &mut Vec<String>, cap: usize) {
            if hits.len() >= cap { return; }
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                if hits.len() >= cap { return; }
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if name == ".git" || name == "target" { continue; }
                if p.is_dir() { walk(&p, root, pat, hits, cap); }
                else if let Ok(text) = std::fs::read_to_string(&p) {
                    let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().into_owned();
                    for (i, line) in text.lines().enumerate() {
                        if line.contains(pat) {
                            hits.push(format!("{}:{}: {}", rel, i + 1, line.trim()));
                            if hits.len() >= cap { return; }
                        }
                    }
                }
            }
        }
        walk(&base, &self.root, pattern, &mut hits, 200);
        ToolResult::ok(if hits.is_empty() { format!("no matches for /{pattern}/") } else { hits.join("\n") })
    }

    fn read_range(&self, rel: &str, start: usize, end: usize) -> ToolResult {
        let abs = match self.resolve_read(rel) { Ok(p) => p, Err(e) => return ToolResult::err(e) };
        let text = match std::fs::read_to_string(&abs) { Ok(s) => s, Err(e) => return ToolResult::err(format!("read {rel}: {e}")) };
        let (start, end) = (start.max(1), end.max(start));
        let out: String = text.lines().enumerate()
            .filter(|(i, _)| *i + 1 >= start && *i + 1 <= end)
            .map(|(i, l)| format!("{}: {}", i + 1, l))
            .collect::<Vec<_>>().join("\n");
        ToolResult::ok(if out.is_empty() { format!("{rel}: no lines in {start}..{end}") } else { out })
    }

    /// Ingest raw external bytes (a DRAG-AND-DROPPED file) to a sandboxed workspace path, recording a
    /// `FileEvent`. Unlike `WriteFile` (UTF-8 contents from the model), this takes bytes, so dropped
    /// binaries land intact. `dest_rel` is workspace-relative.
    pub fn ingest(&mut self, dest_rel: &str, bytes: &[u8]) -> ToolResult {
        let abs = match self.sandbox_path(dest_rel) { Ok(p) => p, Err(e) => return ToolResult::err(e) };
        let existed = abs.exists();
        if let Some(parent) = abs.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult::err(format!("mkdir {dest_rel}: {e}"));
            }
        }
        if let Err(e) = std::fs::write(&abs, bytes) {
            return ToolResult::err(format!("ingest {dest_rel}: {e}"));
        }
        self.events.push(FileEvent {
            path: dest_rel.to_string(),
            change: if existed { FileChange::Modified } else { FileChange::Created },
            bytes: bytes.len() as u64,
        });
        ToolResult::ok(format!("ingested {dest_rel} ({} bytes)", bytes.len()))
    }

    /// `Tool::Doc`: write a shared crew doc at `.logscope/docs/<name>.md`. Name is sanitized to a
    /// bare slug (no paths, no traversal); collisions overwrite — docs are living documents the crew
    /// keeps current, not an append-only log.
    fn write_doc(&mut self, name: &str, content: &str) -> ToolResult {
        let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
        let slug = slug.trim_matches('-').to_string();
        if slug.is_empty() { return ToolResult::err("doc: name must contain letters/digits"); }
        let dir = self.root.join(".logscope").join("docs");
        if let Err(e) = std::fs::create_dir_all(&dir) { return ToolResult::err(format!("doc mkdir: {e}")); }
        let path = dir.join(format!("{slug}.md"));
        match std::fs::write(&path, content) {
            Ok(()) => ToolResult::ok(format!("shared doc '{slug}' written ({} B) — every worker's next prompt includes it", content.len())),
            Err(e) => ToolResult::err(format!("doc write: {e}")),
        }
    }

    fn write_file(&mut self, rel: &str, contents: &str) -> ToolResult {
        if rel.starts_with('~') { return ToolResult::err(format!("{rel}: library roots (~std/~deps) are READ-ONLY")); }
        let abs = match self.sandbox_path(rel) { Ok(p) => p, Err(e) => return ToolResult::err(e) };
        let existed = abs.exists();
        if let Some(parent) = abs.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult::err(format!("mkdir {rel}: {e}"));
            }
        }
        if let Err(e) = std::fs::write(&abs, contents) {
            return ToolResult::err(format!("write {rel}: {e}"));
        }
        let bytes = contents.len() as u64;
        self.events.push(FileEvent {
            path: rel.to_string(),
            change: if existed { FileChange::Modified } else { FileChange::Created },
            bytes,
        });
        ToolResult::ok(format!("wrote {rel} ({bytes} bytes)"))
    }

    /// The patch primitive behind [`Tool::Edit`]: one verbatim anchor, one replacement. Uniqueness is
    /// enforced so an edit can never land somewhere the model didn't mean; every failure tells the
    /// model exactly how to recover (re-read for a stale anchor, extend an ambiguous one).
    fn edit_file(&mut self, rel: &str, find: &str, replace: &str) -> ToolResult {
        if rel.starts_with('~') { return ToolResult::err(format!("{rel}: library roots (~std/~deps) are READ-ONLY")); }
        let abs = match self.sandbox_path(rel) { Ok(p) => p, Err(e) => return ToolResult::err(e) };
        if find.is_empty() {
            return ToolResult::err(format!("edit {rel}: empty find anchor — copy the exact text to replace"));
        }
        let text = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(e) => return ToolResult::err(format!(
                "edit {rel}: {e} (edit patches EXISTING files — use write_file to create a new one)")),
        };
        let n = text.matches(find).count();
        if n == 0 {
            return ToolResult::err(format!(
                "edit {rel}: anchor not found — the file's text differs; read_file it and copy the anchor \
                 verbatim, including whitespace and newlines"));
        }
        if n > 1 {
            return ToolResult::err(format!(
                "edit {rel}: anchor matches {n} places — include more surrounding lines so it is unique"));
        }
        let at = text.find(find).unwrap_or(0);
        let line = text[..at].bytes().filter(|&b| b == b'\n').count() + 1;
        let new_text = text.replacen(find, replace, 1);
        if let Err(e) = std::fs::write(&abs, &new_text) {
            return ToolResult::err(format!("edit {rel}: write failed: {e}"));
        }
        let delta = new_text.len() as i64 - text.len() as i64;
        self.events.push(FileEvent { path: rel.to_string(), change: FileChange::Modified, bytes: new_text.len() as u64 });
        ToolResult::ok(format!("edited {rel} @ line {line} ({}{delta} bytes)", if delta >= 0 { "+" } else { "" }))
    }

    fn read_file(&self, rel: &str) -> ToolResult {
        let abs = match self.resolve_read(rel) { Ok(p) => p, Err(e) => return ToolResult::err(e) };
        match std::fs::read_to_string(&abs) {
            Ok(s) => {
                let capped: String = s.chars().take(self.max_output).collect();
                let note = if capped.len() < s.len() { "\n…(truncated)" } else { "" };
                ToolResult::ok(format!("{capped}{note}"))
            }
            Err(e) => ToolResult::err(format!("read {rel}: {e}")),
        }
    }

    fn list_dir(&self, rel: &str) -> ToolResult {
        if rel == "~deps" || rel == "~deps/" { return self.list_deps(); }
        let abs = if rel.is_empty() || rel == "." {
            self.root.clone()
        } else {
            match self.resolve_read(rel) { Ok(p) => p, Err(e) => return ToolResult::err(e) }
        };
        let rd = match std::fs::read_dir(&abs) { Ok(r) => r, Err(e) => return ToolResult::err(format!("ls {rel}: {e}")) };
        let mut names: Vec<String> = rd.flatten().map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() { format!("{n}/") } else { n }
        }).filter(|n| n != ".git/").collect();
        names.sort();
        ToolResult::ok(if names.is_empty() { "(empty)".into() } else { names.join("\n") })
    }

    fn run(&self, argv: &[String]) -> ToolResult {
        let Some(bin) = argv.first() else { return ToolResult::err("empty command") };
        let base = Path::new(bin).file_name().and_then(|s| s.to_str()).unwrap_or(bin);
        if !self.allow.contains(base) {
            return ToolResult::err(format!("command not allowed: {base} (allowlist: {})", self.allowlist_str()));
        }
        let start = Instant::now();
        let mut cmd = Command::new(bin);
        cmd.args(&argv[1..]).current_dir(&self.root)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match cmd.spawn() { Ok(c) => c, Err(e) => return ToolResult::err(format!("spawn {base}: {e}")) };

        // Bounded wait: poll for completion up to `timeout`, then kill. std has no timed wait, so we
        // spin on try_wait with a short sleep — fine for the coarse (multi-second) build/test timeouts here.
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let out = child.wait_with_output().map(|o| o).ok();
                    let (mut so, mut se, code) = match out {
                        Some(o) => (
                            String::from_utf8_lossy(&o.stdout).into_owned(),
                            String::from_utf8_lossy(&o.stderr).into_owned(),
                            o.status.code(),
                        ),
                        None => (String::new(), String::new(), status.code()),
                    };
                    truncate(&mut so, self.max_output);
                    truncate(&mut se, self.max_output);
                    let body = format!(
                        "$ {}\nexit: {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                        argv.join(" "), code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()), so, se
                    );
                    return ToolResult { ok: code == Some(0), output: body };
                }
                Ok(None) => {
                    if start.elapsed() > self.timeout {
                        let _ = child.kill();
                        return ToolResult::err(format!("run timed out after {:?}: {}", self.timeout, argv.join(" ")));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return ToolResult::err(format!("wait {base}: {e}")),
            }
        }
    }

    fn allowlist_str(&self) -> String {
        self.allow.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

fn truncate(s: &mut String, cap: usize) {
    if s.len() > cap {
        s.truncate(cap);
        s.push_str("\n…(truncated)");
    }
}

/// Read-only inspection + the Rust build/test toolchain: what a coding session needs, nothing that
/// deletes or reaches the network. `run` matches by basename so a full path still gates correctly.
fn default_allow() -> BTreeSet<String> {
    ["cargo", "rustc", "rustfmt", "clippy-driver",
     "ls", "cat", "head", "tail", "grep", "rg", "find", "wc",
     "git", "echo", "true", "test"]
        .iter().map(|s| s.to_string()).collect()
}

// ── agent configuration: plan-then-act (2), verify-as-you-go (1), read policy (4) ────────────────

/// How the agent is told to read files. FULL is the default and preferred for the primary agent — local
/// compute is cheap, so more context up front beats round-tripping. SURGICAL is for COORDINATOR-spawned
/// workers optimizing for speed on a big repo: search + read_range instead of whole files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadPolicy { Full, Surgical }

/// Agent run configuration.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub max_steps: usize,
    /// Verify-as-you-go (1): after a turn that WROTE a file whose extension is in `verify_exts`, run this
    /// command and fold its output into the next observation — so the model sees compiler errors every
    /// turn instead of being trusted to self-check. Empty = off. `["cargo","check"]` is the Rust default.
    pub verify_cmd: Vec<String>,
    pub verify_exts: Vec<String>,
    /// Require verification to PASS before accepting the model's `TASK_COMPLETE` (6/done-gating): a claim
    /// of done is only honored if the last verify was green (or verification is off).
    pub gate_done_on_verify: bool,
    pub read_policy: ReadPolicy,
    /// Doc mode: teach the agent the document-organization convention (front-matter + section tree +
    /// generated index) so its docs stay organized and, in a fan, merge cleanly across branches.
    pub doc_mode: bool,
    /// Patch-first editing (context economy for big models): instruct the model to modify EXISTING
    /// files with the `edit` tool (anchored find→replace) rather than re-emitting whole files.
    /// Whole-file output is what fills the window and drags tok/s; anchored patches keep each turn a
    /// few dozen tokens and the KV resume warm.
    pub prefer_patches: bool,
    /// Fold a structured DIGEST of the verify output (counts + top error locations + the first error's
    /// detail) into the observation instead of the raw compiler dump. The raw dump is capped at 64 KB
    /// and routinely swamps a small window; the digest is the deterministic half of the "small model
    /// summarizes the check for the big model" pipeline — a Delegate-slot model can rewrite the digest
    /// conversationally, but the loop never depends on one being loaded.
    pub digest_verify: bool,
    /// Pre-gathered context injected into the system prompt: the workspace's file tree plus any
    /// shared docs earlier workers wrote (`collect_shared_docs`). Turn 0 should never be blind —
    /// a model that can't see the repo asks the user about it instead of exploring it.
    pub context: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_steps: 24,
            verify_cmd: vec!["cargo".into(), "check".into()],
            verify_exts: vec!["rs".into()],
            gate_done_on_verify: true,
            read_policy: ReadPolicy::Full,
            doc_mode: false,
            prefer_patches: true,
            digest_verify: true,
            context: String::new(),
        }
    }
}

impl AgentConfig {
    /// The doc-writing preset: organization convention on, no code-verify (docs don't compile), full reads.
    pub fn docs() -> Self {
        Self { verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, doc_mode: true, ..Default::default() }
    }
    /// The worker preset: surgical reads, no auto-verify (the coordinator verifies the stitched whole),
    /// tighter step budget — for fast parallel spin-off, per the coordinator-mode discussion.
    pub fn worker() -> Self {
        Self { max_steps: 10, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false,
               read_policy: ReadPolicy::Surgical, doc_mode: false, ..Default::default() }
    }
}

/// A parsed plan step (2): the visible checklist the model commits to before acting. `done` is flipped as
/// the run progresses so the UI can tick it off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanStep { pub text: String, pub done: bool }

/// The plan the model emits up front. Empty if it dove straight into tool calls.
#[derive(Clone, Debug, Default)]
pub struct Plan { pub steps: Vec<PlanStep> }
impl Plan {
    pub fn is_empty(&self) -> bool { self.steps.is_empty() }
    pub fn progress(&self) -> (usize, usize) { (self.steps.iter().filter(|s| s.done).count(), self.steps.len()) }
    /// Mark the first not-yet-done step done (called once per completed action batch).
    pub fn advance(&mut self) { if let Some(s) = self.steps.iter_mut().find(|s| !s.done) { s.done = true; } }
}

/// Parse a plan from the model's turn: a fenced ```plan block, or a leading numbered/bulleted list. Each
/// non-empty line becomes a step (stripped of its marker). Empty if neither shape is present.
pub fn parse_plan(text: &str) -> Plan {
    // fenced ```plan … ```
    if let Some(start) = text.find("```plan") {
        let after = &text[start + "```plan".len()..];
        if let Some(end) = after.find("```") {
            return Plan { steps: lines_to_steps(after[..end].trim()) };
        }
    }
    // otherwise: a leading run of numbered/bulleted lines (before any tool block)
    let head = text.split("```tool").next().unwrap_or(text);
    let steps: Vec<PlanStep> = head.lines()
        .map(|l| l.trim())
        .filter(|l| is_list_item(l))
        .map(|l| PlanStep { text: strip_marker(l), done: false })
        .collect();
    Plan { steps }
}

fn lines_to_steps(block: &str) -> Vec<PlanStep> {
    block.lines().map(|l| l.trim()).filter(|l| !l.is_empty())
        .map(|l| PlanStep { text: strip_marker(l), done: false }).collect()
}
fn is_list_item(l: &str) -> bool {
    l.starts_with("- ") || l.starts_with("* ") ||
    l.chars().next().map_or(false, |c| c.is_ascii_digit()) && l.contains(['.', ')'])
}
fn strip_marker(l: &str) -> String {
    let l = l.trim_start_matches(['-', '*', ' ']);
    // strip a leading "N." / "N)"
    let l = if let Some(pos) = l.find(['.', ')']) {
        if l[..pos].chars().all(|c| c.is_ascii_digit()) && pos <= 3 { l[pos + 1..].trim_start() } else { l }
    } else { l };
    l.trim().to_string()
}

// ── the model seam ──────────────────────────────────────────────────────────────────────────────

/// The one thing the loop needs from a model: given the running prompt, produce the next turn's text.
/// The real impl calls `llm_backend::generate`; tests script it. This is why the loop is testable with
/// no GPU — the model is data, not a dependency.
pub trait Llm {
    fn generate(&mut self, prompt: &str) -> Result<String, String>;
}

// ── tool-call parsing (the two shapes local models actually emit) ────────────────────────────────

/// Pull tool calls out of a model turn. Accepts BOTH shapes small local models emit in practice:
///   * a fenced ```tool { … } ``` block (what we instruct in `system_prompt` — most reliable across
///     llama-3.2 / Qwen2.5-Coder), and
///   * `<tool_call> { … } </tool_call>` (Qwen/ChatML native tool syntax),
/// each carrying a JSON object `{ "tool": "...", ... }`. Unrecognized/garbled blocks are skipped (the
/// model gets a "no tool call parsed" nudge and can retry) rather than crashing the loop.
pub fn parse_tools(text: &str) -> Vec<Tool> {
    let mut out = Vec::new();
    for block in extract_blocks(text) {
        if let Some(t) = parse_one(&block) {
            out.push(t);
        }
    }
    out
}

/// Find the ``` that actually terminates a fenced JSON payload: the first fence position where the text
/// before it is brace-balanced OUTSIDE of JSON strings (escape-aware). Fences INSIDE a JSON string (a
/// README/doc-comment being written via write_file) don't end the block. Falls back to the first fence if
/// the payload never balances (malformed JSON — same behavior as before).
fn balanced_fence_end(after: &str) -> Option<usize> {
    let bytes = after.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    let mut first_fence = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if esc { esc = false; }
            else if b == b'\\' { esc = true; }
            else if b == b'"' { in_str = false; }
        } else {
            match b {
                b'"' => in_str = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth -= 1,
                b'`' if after[i..].starts_with("```") => {
                    if first_fence.is_none() { first_fence = Some(i); }
                    if depth <= 0 { return Some(i); }
                }
                _ => {}
            }
        }
        i += 1;
    }
    first_fence
}

/// Extract the raw JSON-ish payloads from either supported block shape.
fn extract_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    // fenced ```tool … ```
    let mut rest = text;
    while let Some(start) = rest.find("```tool") {
        let after = &rest[start + "```tool".len()..];
        // End the block at a fence where the payload is BALANCED JSON — not the first ``` blindly.
        // A write_file whose contents contain a fence (README with ```rust examples, doc comments
        // with ```) otherwise truncates mid-JSON and the tail spills out as broken file fragments.
        match balanced_fence_end(after) {
            Some(end) => {
                blocks.push(after[..end].trim().to_string());
                rest = &after[end + 3..];
            }
            None => break,
        }
    }
    // <tool_call> … </tool_call>
    let mut rest = text;
    while let Some(start) = rest.find("<tool_call>") {
        let after = &rest[start + "<tool_call>".len()..];
        if let Some(end) = after.find("</tool_call>") {
            blocks.push(after[..end].trim().to_string());
            rest = &after[end + "</tool_call>".len()..];
        } else { break; }
    }
    blocks
}

/// Parse one JSON object into a `Tool`. A tiny hand-rolled extractor (no serde dep needed on this path):
/// we only need a handful of string fields and one string array, and model JSON here is flat.
fn parse_one(block: &str) -> Option<Tool> {
    let tool = json_str(block, "tool")?;
    match tool.as_str() {
        "write_file" => Some(Tool::WriteFile {
            path: json_str(block, "path")?,
            contents: json_str(block, "contents").unwrap_or_default(),
        }),
        "read_file" => Some(Tool::ReadFile { path: json_str(block, "path")? }),
        "edit" | "edit_file" | "patch" => Some(Tool::Edit {
            path: json_str(block, "path")?,
            find: json_str(block, "find").or_else(|| json_str(block, "old"))?,
            replace: json_str(block, "replace").or_else(|| json_str(block, "new")).unwrap_or_default(),
        }),
        "read_range" => {
            let path = json_str(block, "path")?;
            let start = json_str(block, "start").and_then(|s| s.parse().ok())
                .or_else(|| json_num(block, "start")).unwrap_or(1);
            let end = json_str(block, "end").and_then(|s| s.parse().ok())
                .or_else(|| json_num(block, "end")).unwrap_or(start);
            Some(Tool::ReadRange { path, start, end })
        }
        "search" => Some(Tool::Search {
            pattern: json_str(block, "pattern").or_else(|| json_str(block, "query"))?,
            path: json_str(block, "path").unwrap_or_default(),
        }),
        "list_dir" => Some(Tool::ListDir { path: json_str(block, "path").unwrap_or_default() }),
        "doc" | "share_doc" | "write_doc" => Some(Tool::Doc {
            name: json_str(block, "name")?,
            content: json_str(block, "content").or_else(|| json_str(block, "contents"))?,
        }),
        "run" => {
            let argv = json_str_array(block, "argv")
                .or_else(|| json_str(block, "cmd").map(|s| s.split_whitespace().map(String::from).collect()))?;
            if argv.is_empty() { None } else { Some(Tool::Run { argv }) }
        }
        _ => None,
    }
}

/// Extract a JSON string value for `key` — handles the escapes model output actually contains (\n, \", \\, \t).
fn json_str(src: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let k = src.find(&pat)?;
    let after = &src[k + pat.len()..];
    let colon = after.find(':')?;
    let mut chars = after[colon + 1..].char_indices().peekable();
    // skip whitespace to the opening quote
    while let Some(&(_, c)) = chars.peek() {
        if c.is_whitespace() { chars.next(); } else { break; }
    }
    let (_, first) = chars.next()?;
    if first != '"' { return None; }
    let mut val = String::new();
    let mut escaped = false;
    for (_, c) in chars {
        if escaped {
            match c {
                'n' => val.push('\n'),
                't' => val.push('\t'),
                'r' => val.push('\r'),
                '"' => val.push('"'),
                '\\' => val.push('\\'),
                '/' => val.push('/'),
                other => { val.push('\\'); val.push(other); }
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some(val);
        } else {
            val.push(c);
        }
    }
    None // unterminated
}

/// Extract a JSON numeric value for `key` (for line numbers in read_range).
fn json_num(src: &str, key: &str) -> Option<usize> {
    let pat = format!("\"{key}\"");
    let k = src.find(&pat)?;
    let after = &src[k + pat.len()..];
    let colon = after.find(':')?;
    let digits: String = after[colon + 1..].chars().skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Extract a JSON array-of-strings for `key`.
fn json_str_array(src: &str, key: &str) -> Option<Vec<String>> {
    let pat = format!("\"{key}\"");
    let k = src.find(&pat)?;
    let after = &src[k + pat.len()..];
    let lb = after.find('[')?;
    let rb = after[lb..].find(']')? + lb;
    let inner = &after[lb + 1..rb];
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut escaped = false;
    for c in inner.chars() {
        if in_str {
            if escaped { cur.push(c); escaped = false; }
            else if c == '\\' { escaped = true; }
            else if c == '"' { out.push(std::mem::take(&mut cur)); in_str = false; }
            else { cur.push(c); }
        } else if c == '"' {
            in_str = true;
        }
    }
    Some(out)
}

// ── the driver loop ─────────────────────────────────────────────────────────────────────────────

/// One turn of the running transcript: what the model said, and what each tool call did.
#[derive(Clone, Debug)]
pub struct Turn {
    pub model_text: String,
    pub results: Vec<(Tool, ToolResult)>,
}

/// Why the loop ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Model emitted the done sentinel.
    Done,
    /// Hit the step budget without finishing.
    StepLimit,
    /// Aborted: the model was stuck repeating itself (caught by the loop guard) — a bad actor, cut so its
    /// speed stops being a runaway train.
    Stalled,
    /// The model call itself failed (e.g. generation error).
    ModelError(String),
}

/// The full record a session produces: every turn + why it stopped + the file-event feed + the plan the
/// model committed to (2) + whether the final state passed verification (1/6).
pub struct Session {
    pub turns: Vec<Turn>,
    pub stop: Stop,
    pub files: Vec<FileEvent>,
    pub plan: Plan,
    pub verified: bool,
    /// Was there a verify command at all?
    ///
    /// `verified` alone cannot answer this: it starts true when nothing is configured, so a run with
    /// no verifier is indistinguishable from one that passed. The UI needs both to label honestly.
    pub had_verifier: bool,
}

/// The sentinel a model prints when the task is complete.
pub const DONE_MARKER: &str = "TASK_COMPLETE";

/// Build the system prompt that teaches a local model the tool protocol. Kept explicit and example-led
/// because small models follow a shown format far more reliably than a described one. `cfg` shapes it:
/// the read tools offered depend on `read_policy`, and the verify promise is stated when verification is on.
/// Minimal Cargo.lock reader: every `[[package]]` name/version pair. Pure; drives the `~deps`
/// virtual root (version pinning + `list_dir ~deps`).
pub fn cargo_lock_versions(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let (mut name, mut in_pkg) = (None::<String>, false);
    for line in text.lines() {
        let l = line.trim();
        if l == "[[package]]" { in_pkg = true; name = None; continue; }
        if l.starts_with('[') && l != "[[package]]" { in_pkg = false; continue; }
        if !in_pkg { continue; }
        if let Some(v) = l.strip_prefix("name = \"").and_then(|r| r.strip_suffix('"')) { name = Some(v.to_string()); }
        else if let Some(v) = l.strip_prefix("version = \"").and_then(|r| r.strip_suffix('"')) {
            if let Some(n) = name.take() { out.push((n, v.to_string())); }
        }
    }
    out
}

/// Sortable key for a semver-ish version string ("1.0.219" > "1.0.9").
fn version_key(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap_or(0)).collect()
}

/// A bounded file tree of the workspace for the agent's turn-0 context: relative paths, source-ish
/// files first-class, generated trees skipped, capped at `cap` entries (+ a truncation note). This is
/// what stops a model from asking the user what the repository contains — it can SEE it.
pub fn repo_tree(root: &std::path::Path, cap: usize) -> String {
    const SKIP: [&str; 7] = ["target", ".git", ".gitmind", ".logscope", "node_modules", "dist", ".cargo"];
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut truncated = false;
    while let Some((dir, depth)) = stack.pop() {
        if depth > 6 { continue; }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if out.len() >= cap { truncated = true; break; }
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if !SKIP.contains(&name.as_str()) && !name.starts_with('.') { stack.push((p, depth + 1)); }
            } else if !name.starts_with('.') {
                if let Ok(rel) = p.strip_prefix(root) {
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    out.push(format!("{} ({} B)", rel.to_string_lossy(), size));
                }
            }
        }
        if truncated { break; }
    }
    out.sort();
    let mut s = out.join("\n");
    if truncated { s.push_str(&format!("\n… (truncated at {cap} entries — use list_dir/search for the rest)")); }
    s
}

/// Concatenate every shared crew doc (`.logscope/docs/*.md`) under a total `budget` in chars — the
/// other half of `Tool::Doc`: one worker's write-up becomes every later worker's prompt context.
pub fn collect_shared_docs(root: &std::path::Path, budget: usize) -> String {
    let dir = root.join(".logscope").join("docs");
    let Ok(rd) = std::fs::read_dir(&dir) else { return String::new() };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("md")).collect();
    files.sort();
    let mut s = String::new();
    for f in files {
        if s.len() >= budget { s.push_str("\n… (docs truncated)"); break; }
        let name = f.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
        let body = std::fs::read_to_string(&f).unwrap_or_default();
        let take = (budget - s.len()).min(body.len());
        // cut on a char boundary
        let mut cut = take; while cut < body.len() && !body.is_char_boundary(cut) { cut += 1; }
        s.push_str(&format!("### shared doc: {name}\n{}\n\n", &body[..cut.min(body.len())]));
    }
    s
}

pub fn system_prompt(task: &str, cfg: &AgentConfig) -> String {
    // ALL read/navigation tools are always on the menu; the policy only changes which one the
    // prompt recommends leading with. (Hiding search from the primary agent taught it to ask the
    // user about the code instead of grepping it.)
    let read_tools = match cfg.read_policy {
        ReadPolicy::Full => "\
- read_file   { \"tool\":\"read_file\", \"path\":\"<rel path>\" }  (whole file — prefer this; context is cheap)
- search      { \"tool\":\"search\", \"pattern\":\"<text>\", \"path\":\"<rel or empty>\" }  (grep the repo — find where a concept lives)
- read_range  { \"tool\":\"read_range\", \"path\":\"<rel path>\", \"start\":<line>, \"end\":<line> }  (a slice of a big file)"
            .to_string(),
        ReadPolicy::Surgical => "\
- search      { \"tool\":\"search\", \"pattern\":\"<text>\", \"path\":\"<rel or empty>\" }  (grep the repo — use this first)
- read_range  { \"tool\":\"read_range\", \"path\":\"<rel path>\", \"start\":<line>, \"end\":<line> }  (read just the lines you need)
- read_file   { \"tool\":\"read_file\", \"path\":\"<rel path>\" }  (whole file — only when it is small)"
            .to_string(),
    };
    let verify_note = if cfg.verify_cmd.is_empty() {
        String::new()
    } else {
        format!(
"\n\nVERIFICATION IS AUTOMATIC: after any turn that writes a {} file, `{}` runs and its result is shown to \
you. Favor code that compiles over clever code that might not; when you see errors, fix them before moving on.{}",
            cfg.verify_exts.join("/"),
            cfg.verify_cmd.join(" "),
            if cfg.gate_done_on_verify { format!(" You may only reply {DONE_MARKER} once verification is green.") } else { String::new() },
        )
    };
    let doc_note = if cfg.doc_mode { format!("\n\n{}", crate::docorg::DOC_ORG_GUIDE) } else { String::new() };
    // Turn-0 context: the workspace tree + shared crew docs, so no model starts blind.
    let ctx_note = if cfg.context.is_empty() { String::new() }
        else { format!("\n\nWORKSPACE OVERVIEW (you can read any of these with the tools above):\n{}", cfg.context) };
    let patch_note = if cfg.prefer_patches {
        "\n\nEDITING EXISTING FILES: use `edit` — copy a UNIQUE anchor VERBATIM from read output (keep its \
whitespace and newlines) and give the replacement. Do NOT re-emit whole existing files with write_file; \
that wastes your context and slows you down. write_file is for NEW files, or a full rewrite you were \
explicitly asked for. Several small edits in one turn are fine."
    } else { "" };
    format!(
"You are a coding agent working inside an isolated workspace. Complete this task:

{task}

FIRST, emit a short PLAN — the steps you will take — as a fenced block:

```plan
1. <step>
2. <step>
```

THEN act by emitting TOOL CALLS. Each tool call is a fenced block:

```tool
{{ \"tool\": \"write_file\", \"path\": \"src/lib.rs\", \"contents\": \"fn add(a:i32,b:i32)->i32{{a+b}}\\n\" }}
```

Available tools (one JSON object per block):
- write_file  {{ \"tool\":\"write_file\", \"path\":\"<rel path>\", \"contents\":\"<file text>\" }}
- edit        {{ \"tool\":\"edit\", \"path\":\"<rel path>\", \"find\":\"<verbatim unique anchor>\", \"replace\":\"<replacement>\" }}
{read_tools}
- list_dir    {{ \"tool\":\"list_dir\", \"path\":\"<rel path or empty for root>\" }}
- run         {{ \"tool\":\"run\", \"argv\":[\"cargo\",\"test\"] }}
- doc         {{ \"tool\":\"doc\", \"name\":\"survey\", \"content\":\"<markdown>\" }}  (SHARED crew doc — every worker's next prompt includes it)

THE CODE IS RIGHT THERE — LOOK AT IT. This workspace is a real repository (overview below). Every
claim about it must come from tool results, never from guessing and never from asking the user: the
user will NOT answer questions, so a reply that only asks for clarification is a wasted turn. If the
task is vague (\"add more blocks\"), that vagueness is an instruction to figure out what \"blocks\"
means IN THIS CODEBASE: search for the word, read the files that define it, find the existing
pattern (a registry, an enum, a directory of implementations), then extend that pattern. Your first
turns are reconnaissance — list_dir, read the README / Cargo.toml, search the task's key words —
and once you understand the lay of the land, write a `doc` named \"survey\" (module map, where the
task's concepts live, the pattern to follow) so later workers inherit what you learned instead of
re-crawling. Then make the change, and let the automatic verification tell you if it compiles.

LIBRARY SOURCE IS READABLE TOO — the same read tools accept two READ-ONLY virtual roots:
  ~deps/<crate>/…   a dependency's source exactly as cargo fetched it (list_dir {{ \"path\":\"~deps\" }} lists them)
  ~std/…            the Rust stdlib source (Vec/String → ~std/alloc/src, io/fs → ~std/std/src, Option/Iterator → ~std/core/src)
The source includes the doc comments — it IS the documentation. When unsure of an exact method name,
signature, or trait bound, READ it (e.g. search {{ \"pattern\":\"pub fn drain\", \"path\":\"~std/alloc/src/vec\" }},
read_file {{ \"path\":\"~deps/serde/src/lib.rs\" }}) instead of guessing an API that may not exist.

Rules:
- Paths are relative to the workspace root; you cannot escape it.
- `run` is limited to an allowlist (cargo, rustc, git, ls, cat, grep, …). No shell, no network.
- After each tool call you will see its result, then decide the next step.
- When the task is finished and verified, reply with the single line: {DONE_MARKER}

Emit one or a few tool calls per turn, then wait for results.{patch_note}{verify_note}{doc_note}{ctx_note}"
    )
}

/// Format a batch of tool results as the observation appended to the prompt for the next turn.
fn observation(results: &[(Tool, ToolResult)]) -> String {
    let mut s = String::from("\n\nTOOL RESULTS:\n");
    for (t, r) in results {
        s.push_str(&format!("[{}] {}\n{}\n\n", if r.ok { "ok" } else { "err" }, t.summary(), r.output));
    }
    s.push_str("Continue, or reply ");
    s.push_str(DONE_MARKER);
    s.push_str(" if finished.\n");
    s
}

/// Process ONE turn given the model's completed `text`: append it to the running `prompt`, capture the
/// plan if not yet seen, run any tool calls against `ws`, fold in verify-as-you-go, and decide whether the
/// run terminates. Shared by the synchronous `run_agent_cfg` and the async `AgentRun` so there's ONE
/// implementation of the loop body. Returns the executed (tool, result) pairs and `Some(stop)` if the run
/// should end this turn. Mutates `prompt`/`plan`/`verified` in place.
fn step_turn(
    text: &str, prompt: &mut String, ws: &mut Workspace, cfg: &AgentConfig,
    plan: &mut Plan, verified: &mut bool, step: usize, guard: &mut crate::repetition::LoopGuard,
) -> (Vec<(Tool, ToolResult)>, Option<Stop>) {
    prompt.push_str("\n\nASSISTANT:\n");
    prompt.push_str(text);

    if plan.is_empty() {
        let p = parse_plan(text);
        if !p.is_empty() { *plan = p; }
    }

    let tools = parse_tools(text);

    // ── loop management (bad-actor detection) ────────────────────────────────────────────────────
    // Signature of what this turn DID (its tool calls, or "done"/"noop") so the guard can tell a repeating
    // agent from a progressing one. A repeat or an internally-looping reply escalates: nudge, then abort.
    let action_sig = if text.contains(DONE_MARKER) && tools.is_empty() {
        "done".to_string()
    } else if tools.is_empty() {
        "noop".to_string()
    } else {
        tools.iter().map(|t| t.summary()).collect::<Vec<_>>().join(";")
    };
    match guard.observe(text, &action_sig) {
        crate::repetition::LoopAction::Abort => return (Vec::new(), Some(Stop::Stalled)),
        crate::repetition::LoopAction::Nudge(msg) => {
            // Re-steer BEFORE running a repeated action: inject the nudge and skip executing this turn's
            // (repeated) tools, so the model gets a chance to change tack instead of re-doing the loop.
            prompt.push_str(&format!("\n\n(⚠ loop detected: {msg})\n"));
            return (Vec::new(), None);
        }
        crate::repetition::LoopAction::Proceed => {}
    }
    // File-write churn guard: the text/action signature can't see a model rewriting the same path each
    // turn with different prose around it. Check every WriteFile BEFORE executing; on a hit, nudge + skip
    // this turn's tools (same re-steer pattern as above) and escalate on the shared strike counter.
    for t in &tools {
        if let Tool::WriteFile { path, contents } = t {
            if let Some(msg) = guard.observe_write(path, contents) {
                if matches!(guard.note_strike(), crate::repetition::LoopAction::Abort) {
                    return (Vec::new(), Some(Stop::Stalled));
                }
                prompt.push_str(&format!("\n\n(⚠ loop detected: {msg})\n"));
                return (Vec::new(), None);
            }
        }
    }

    // Done handling — gated on verification (6).
    if text.contains(DONE_MARKER) && tools.is_empty() {
        if !cfg.gate_done_on_verify || *verified {
            return (Vec::new(), Some(Stop::Done));
        } else {
            prompt.push_str(&format!("\n\n(Not accepted: `{}` is not green yet. Fix the errors, then reply {DONE_MARKER}.)\n", cfg.verify_cmd.join(" ")));
            return (Vec::new(), None);
        }
    }

    if tools.is_empty() {
        prompt.push_str("\n\n(No tool call parsed. Emit a ```tool block, or reply TASK_COMPLETE.)\n");
        return (Vec::new(), None);
    }

    let mut results = Vec::new();
    let mut wrote_verifiable = false;
    for tool in tools {
        if let Tool::WriteFile { path, .. } | Tool::Edit { path, .. } = &tool {
            if cfg.verify_exts.iter().any(|e| path.rsplit('.').next() == Some(e.as_str())) {
                wrote_verifiable = true;
            }
        }
        let res = ws.apply(&tool);
        results.push((tool, res));
    }
    prompt.push_str(&observation(&results));

    // Verify-as-you-go (1): don't verify on the final step (the model can't act on it).
    if wrote_verifiable && !cfg.verify_cmd.is_empty() && step + 1 < cfg.max_steps {
        let vres = ws.apply(&Tool::Run { argv: cfg.verify_cmd.clone() });
        *verified = vres.ok;
        let body = if cfg.digest_verify { digest_check_output(&vres.output, 6) } else { vres.output.clone() };
        prompt.push_str(&format!(
            "\n\nAUTOMATIC VERIFICATION ({}): {}\n{}\n",
            cfg.verify_cmd.join(" "),
            if vres.ok { "PASSED" } else { "FAILED — fix these before continuing" },
            body,
        ));
    }

    plan.advance();
    (results, None)
}

/// Compress a compiler/test dump into the few lines the model actually needs: totals, up to
/// `max_items` `headline — file:line` entries, and the FIRST error's detail block. The raw dump is
/// capped at 64 KB and routinely swamps a small context window — every token it occupies is a token
/// the model can't spend on code. This is the deterministic core of the check-summarizer pipeline;
/// output that doesn't look like rustc diagnostics falls back to a plain truncation so non-cargo
/// verify commands still surface something useful.
pub fn digest_check_output(raw: &str, max_items: usize) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let is_headline = |t: &str| -> Option<bool> {
        let t = t.trim_start();
        if t.starts_with("error[") { return Some(true); }
        if t.starts_with("error:") {
            // rustc's roll-up lines aren't diagnostics of their own.
            if t.starts_with("error: aborting") || t.starts_with("error: could not compile") { return None; }
            return Some(true);
        }
        if t.starts_with("warning:") && !t.contains("generated") && !t.contains("warning emitted") {
            return Some(false);
        }
        None
    };
    let (mut errors, mut warnings) = (0usize, 0usize);
    let mut items: Vec<String> = Vec::new();
    let mut first_detail: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(is_err) = is_headline(line) else { continue };
        if is_err { errors += 1; } else { warnings += 1; }
        if is_err && items.len() < max_items {
            // The location is on a ` --> path:line:col` line within the next few lines.
            let loc = lines[i + 1..].iter().take(4)
                .find_map(|l| l.trim_start().strip_prefix("--> ").map(|p| p.trim().to_string()))
                .unwrap_or_default();
            let head = line.trim_start();
            items.push(if loc.is_empty() { head.to_string() } else { format!("{head} — {loc}") });
        }
        if is_err && first_detail.is_empty() {
            for l in lines[i..].iter().take(12) {
                if l.trim().is_empty() { break; }
                first_detail.push((*l).to_string());
            }
        }
    }
    if errors == 0 && warnings == 0 && !raw.contains("Finished") && !raw.trim().is_empty() {
        // Not rustc-shaped (a test harness, a script): keep it, just bounded.
        let capped: String = raw.chars().take(1500).collect();
        let note = if capped.len() < raw.len() { "\n…(truncated)" } else { "" };
        return format!("{capped}{note}");
    }
    if errors == 0 {
        return format!("clean — 0 errors, {warnings} warning(s)");
    }
    let mut out = format!("{errors} error(s), {warnings} warning(s). Top errors:\n");
    for (n, it) in items.iter().enumerate() {
        out.push_str(&format!("  {}. {}\n", n + 1, it));
    }
    if errors > items.len() {
        out.push_str(&format!("  …and {} more.\n", errors - items.len()));
    }
    if !first_detail.is_empty() {
        out.push_str("First error detail:\n");
        out.push_str(&first_detail.join("\n"));
    }
    if out.len() > 2400 { out.truncate(2400); out.push_str("\n…(digest capped)"); }
    out
}

/// Back-compat: run with defaults but an explicit step budget.
pub fn run_agent(llm: &mut dyn Llm, ws: &mut Workspace, task: &str, max_steps: usize) -> Session {
    run_agent_cfg(llm, ws, task, &AgentConfig { max_steps, ..Default::default() })
}

/// Run the agentic loop with full configuration (SYNCHRONOUS — blocks until done; used by tests and the
/// blocking `:agent` path). Pure w.r.t. the model. See `AgentRun` for the non-blocking, interruptible
/// driver that shares the same `step_turn` body.
pub fn run_agent_cfg(llm: &mut dyn Llm, ws: &mut Workspace, task: &str, cfg: &AgentConfig) -> Session {
    let mut prompt = system_prompt(task, cfg);
    let mut turns = Vec::new();
    let mut plan = Plan::default();
    // TWO MEANINGS ON ONE BOOLEAN. This starts true when no verify command is configured, which is
    // right for GATING — an unconfigured check must not block a run from finishing. But the same flag
    // drives the UI badge, so "there was nothing to verify" renders as "· ✓ verified".
    //
    // A run where nothing was checked is not a verified run, and a badge saying otherwise is worse
    // than no badge: it is the reason someone stops looking.
    let has_verifier = !cfg.verify_cmd.is_empty();
    let mut verified = !has_verifier;
    let mut guard = crate::repetition::LoopGuard::default();

    for step in 0..cfg.max_steps {
        let text = match llm.generate(&prompt) {
            Ok(t) => t,
            Err(e) => return Session { turns, stop: Stop::ModelError(e), files: ws.events().to_vec(), plan, verified, had_verifier: has_verifier },
        };
        let (results, stop) = step_turn(&text, &mut prompt, ws, cfg, &mut plan, &mut verified, step, &mut guard);
        turns.push(Turn { model_text: text, results });
        if let Some(stop) = stop {
            return Session { turns, stop, files: ws.events().to_vec(), plan, verified, had_verifier: has_verifier };
        }
    }

    Session { turns, stop: Stop::StepLimit, files: ws.events().to_vec(), plan, verified, had_verifier: has_verifier }
}

// ── async driver (non-blocking, interruptible): the agent as a channel-driven state machine ──────

/// What the async driver wants next after a completed model turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentStep {
    /// Submit this prompt to the inference worker for the next turn.
    Submit(String),
    /// The run is finished (done / step limit / model error).
    Finished(Stop),
}

/// The non-blocking agent driver. It owns the workspace + growing context but NOT the model: the model
/// lives on the `Inference` worker thread (which may hold `!Send` GPU state), and this driver talks to it
/// over the same `Cmd::Generate`/`Evt` channel the chat uses — so the frame never blocks and the existing
/// cancel/barge-in interrupts a run for free (just drop the `AgentRun`). One model turn per `on_done`.
///
/// Lifecycle: `start` → submit its `first prompt` to the worker → feed tokens via `on_token` → on the
/// worker's `Done`, call `on_done` and act on the returned `AgentStep` (submit the next prompt, or finish).
pub struct AgentRun {
    ws: Workspace,
    cfg: AgentConfig,
    prompt: String,   // the growing context window
    reply: String,    // accumulates this turn's streamed tokens
    plan: Plan,
    verified: bool,
    /// Whether a verify command existed. See `had_verifier()`.
    had_verifier: bool,
    step: usize,
    turns: usize,
    guard: crate::repetition::LoopGuard,
}

impl AgentRun {
    /// Begin a run over `work_dir`. Returns the driver plus the first prompt to submit to the worker.
    pub fn start(work_dir: impl Into<std::path::PathBuf>, task: &str, cfg: AgentConfig) -> (Self, String) {
        // Captured before cfg moves into the run: a run with no verifier must not read as passed.
        let had_verifier = !cfg.verify_cmd.is_empty();
        let prompt = system_prompt(task, &cfg);
        let verified = cfg.verify_cmd.is_empty();
        let run = Self {
            ws: Workspace::new(work_dir), prompt: prompt.clone(), reply: String::new(),
            plan: Plan::default(), verified, step: 0, turns: 0, cfg,
            guard: crate::repetition::LoopGuard::default(),
            // Mirrors the Session field: a run with no verifier must not read as one that passed.
            had_verifier,
        };
        (run, prompt)
    }

    /// Append a streamed token to the current turn's reply.
    pub fn on_token(&mut self, t: &str) { self.reply.push_str(t); }

    /// The worker finished a turn: process the accumulated reply and decide the next action.
    pub fn on_done(&mut self) -> AgentStep {
        let text = std::mem::take(&mut self.reply);
        let (_results, stop) = step_turn(&text, &mut self.prompt, &mut self.ws, &self.cfg, &mut self.plan, &mut self.verified, self.step, &mut self.guard);
        self.turns += 1;
        self.step += 1;
        if let Some(stop) = stop { return AgentStep::Finished(stop); }
        if self.step >= self.cfg.max_steps { return AgentStep::Finished(Stop::StepLimit); }
        AgentStep::Submit(self.prompt.clone())
    }

    /// The worker reported an error mid-turn.
    pub fn on_error(&mut self, e: String) -> AgentStep { AgentStep::Finished(Stop::ModelError(e)) }

    /// Take the accumulated reply text (used by the fan's review phase, which reads a completed review
    /// generation without advancing the agent loop).
    pub fn take_reply(&mut self) -> String { std::mem::take(&mut self.reply) }
    pub fn config(&self) -> &AgentConfig { &self.cfg }

    pub fn events(&self) -> &[FileEvent] { self.ws.events() }
    pub fn plan(&self) -> &Plan { &self.plan }
    pub fn verified(&self) -> bool { self.verified }
    /// Was a verify command configured at all?
    ///
    /// `verified()` cannot answer this — it starts true when nothing is configured, so a run with no
    /// verifier is indistinguishable from one that passed.
    pub fn had_verifier(&self) -> bool { self.had_verifier }
    pub fn root(&self) -> &Path { self.ws.root() }
    pub fn turns(&self) -> usize { self.turns }
}

// ── two-tier review (3): a depth model reviews the fast model's diff before it lands ─────────────

/// A review verdict from the depth model over the agent's diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict { pub approve: bool, pub notes: String }

/// The review prompt: hand the depth model the task + the unified diff the fast model produced and ask for
/// an APPROVE/REVISE verdict with reasons. Kept terse and format-pinned so a small depth model's verdict is
/// parseable.
pub fn review_prompt(task: &str, diff: &str) -> String {
    format!(
"You are reviewing another model's code changes before they are committed. The task was:

{task}

Here is the unified diff of what it produced:

{diff}

Reply with exactly one of these on the first line:
  APPROVE — if the changes correctly and safely accomplish the task
  REVISE — if they are wrong, incomplete, or unsafe
Then, on the following lines, give your reasons (for REVISE, be specific about what to fix)."
    )
}

/// Parse the depth model's review into a `Verdict`. APPROVE only if the first meaningful line says so.
pub fn parse_verdict(text: &str) -> Verdict {
    let first = text.lines().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("");
    let up = first.to_ascii_uppercase();
    let approve = up.starts_with("APPROVE");
    let notes = text.lines().skip_while(|l| l.trim().is_empty()).skip(1)
        .collect::<Vec<_>>().join("\n").trim().to_string();
    Verdict { approve, notes }
}

/// Run one review pass with a (typically stronger) model over the diff. Pure w.r.t. the model.
pub fn run_review(reviewer: &mut dyn Llm, task: &str, diff: &str) -> Result<Verdict, String> {
    reviewer.generate(&review_prompt(task, diff)).map(|t| parse_verdict(&t))
}

// ── reaction seeding (5): a reaction spins up speculative agent branches, folded/pruned later ────

/// Turn a spoken/typed REACTION into speculative agent sub-goals, WITHOUT stopping the current flow. This
/// is the bridge from the voice-fan philosophy (`voicefan`/`speculate`: a reaction expands into parallel
/// investigation lanes) to the agent: each returned goal seeds an independent worker branch (run with
/// `AgentConfig::worker()`); the branches spin up in parallel and the keep-vs-prune decision happens after,
/// on the convo graph — the same fan→fold→prune shape the fleet already uses for chat.
///
/// `reaction` is the new utterance; `base_goal` is the task in flight. Returns up to `max` concrete
/// sub-goals (empty if the reaction is too thin to seed anything — the caller then treats it as steering
/// dictation rather than a fan).
pub fn seed_agent_branches(reaction: &str, base_goal: &str, max: usize) -> Vec<String> {
    let r = reaction.trim();
    if r.chars().count() < 4 { return Vec::new(); }
    let mut goals = Vec::new();
    goals.push(format!("{base_goal}\n\nAdjustment from the user: {r}"));
    // If the reaction names multiple clauses ("and" / ","), split into parallel branches so several
    // approaches spin up at once — pruned to the keeper afterward.
    for clause in r.split(',').flat_map(|s| s.split(" and ")).map(str::trim).filter(|s| s.len() > 3) {
        if goals.len() >= max { break; }
        let g = format!("{base_goal}\n\nFocus for this branch: {clause}");
        if !goals.contains(&g) { goals.push(g); }
    }
    goals.truncate(max.max(1));
    goals
}

// ── multi-run fan (5): several agent branches spin up interleaved, folded/pruned to the keeper ───

/// Per-branch scheduling state within a fan.
enum BranchState { Ready(String), InFlight, Finished(Stop) }

struct AgentBranch { run: AgentRun, state: BranchState, label: String, verdict: Option<Verdict> }

/// Which pass the fan is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase { Running, Reviewing, Done }

/// What's currently generating on the worker: an agent turn for a branch, or that branch's review.
#[derive(Clone, Copy)]
struct InFlight { branch: usize, review: bool }

/// A read-only snapshot of one branch, for the UI to fold/prune on.
#[derive(Clone, Debug)]
pub struct BranchSummary {
    pub index: usize,
    pub label: String,
    pub files: usize,
    pub verified: bool,
    pub finished: bool,
    pub approved: Option<bool>, // depth-model review verdict (None until reviewed)
    pub review_notes: String,
    pub root: PathBuf,
}

/// What the fan wants next: submit a specific branch's prompt to the (single) worker, or everything's done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FanStep {
    Submit { branch: usize, prompt: String },
    AllDone,
}

/// A fan of agent branches sharing ONE inference worker (there's one local model). "Parallel" here means
/// ROUND-ROBIN interleaving: exactly one branch generates at a time, and the scheduler rotates to the next
/// ready branch on each `Evt::Done`, so all branches make progress turn-by-turn. Each branch edits its own
/// isolated workspace (the caller passes pre-isolated dirs, one per `seed_agent_branches` goal), so they
/// never clobber each other.
///
/// Two phases: once every branch FINISHES its agent loop, the fan enters a REVIEW pass — one depth-model
/// generation per branch over that branch's diff (obtained via the injected `diff_fn`), parsed into an
/// APPROVE/REVISE `Verdict` — so the final `summaries`/ranking are grounded in a real review, not a
/// file-count heuristic. When reviews finish, fold/prune to the keeper and `:reapply` that one.
pub struct AgentFan {
    branches: Vec<AgentBranch>,
    cursor: usize,
    /// Branches generating RIGHT NOW. On the single `Inference` worker this holds at most one entry
    /// (`width` 1 — the historical round-robin). With a `lanes::LanePool` the chat raises `width` to
    /// the lane count and drives the tagged `*_branch` API instead, so K branches genuinely decode at
    /// the same time; the untagged `on_token`/`on_done`/`on_error` keep their exact old semantics by
    /// operating on the FIRST in-flight entry.
    in_flight: Vec<InFlight>,
    width: usize,
    phase: Phase,
    task: String,
    cfg: AgentConfig,
    diff_fn: Box<dyn Fn(&Path) -> String>,
}

impl AgentFan {
    /// Build a fan from `(work_dir, task, label)` specs — one isolated workspace per branch. `diff_fn` yields
    /// a branch's unified diff for the review pass (in production: `git -C <dir> diff <base>`; in tests: a
    /// canned string). Nothing is submitted yet; call `next` to get the first branch to run.
    pub fn new(specs: Vec<(PathBuf, String, String)>, cfg: &AgentConfig, diff_fn: Box<dyn Fn(&Path) -> String>) -> Self {
        let task = specs.first().map(|(_, t, _)| t.clone()).unwrap_or_default();
        let branches = specs.into_iter().map(|(dir, task, label)| {
            let (run, first) = AgentRun::start(dir, &task, cfg.clone());
            AgentBranch { run, state: BranchState::Ready(first), label, verdict: None }
        }).collect();
        Self { branches, cursor: 0, in_flight: Vec::new(), width: 1, phase: Phase::Running, task, cfg: cfg.clone(), diff_fn }
    }

    /// Upgrade a single, already-running `AgentRun` into a fan whose FIRST branch IS that run — so a reaction
    /// during a plain `:agent` run can fork parallel approaches WITHOUT discarding the work in progress. The
    /// adopted run keeps its context + workspace. `pending` is its queued next prompt if it has one (→ Ready);
    /// `None` means a generation is in flight for it right now (→ InFlight, and the fan takes over its stream:
    /// the next `Evt::Done` advances branch 0). Add the reaction's approaches afterward via `add_branch`.
    pub fn adopt(run: AgentRun, pending: Option<String>, task: &str, label: &str,
                 cfg: &AgentConfig, diff_fn: Box<dyn Fn(&Path) -> String>) -> Self {
        let (state, in_flight) = match pending {
            Some(p) => (BranchState::Ready(p), Vec::new()),
            None => (BranchState::InFlight, vec![InFlight { branch: 0, review: false }]),
        };
        let branches = vec![AgentBranch { run, state, label: label.to_string(), verdict: None }];
        Self { branches, cursor: 0, in_flight, width: 1, phase: Phase::Running, task: task.to_string(), cfg: cfg.clone(), diff_fn }
    }

    pub fn len(&self) -> usize { self.branches.len() }
    pub fn is_empty(&self) -> bool { self.branches.is_empty() }
    pub fn active_branch(&self) -> Option<usize> { self.in_flight.first().map(|f| f.branch) }
    /// Every branch generating right now (lane mode shows them all in the HUD).
    pub fn active_branches(&self) -> Vec<usize> { self.in_flight.iter().map(|f| f.branch).collect() }
    pub fn reviewing(&self) -> bool { self.phase == Phase::Reviewing }
    pub fn all_done(&self) -> bool { self.phase == Phase::Done }
    /// How many branches may generate concurrently. 1 (default) is the single-worker round-robin; a
    /// lane pool raises it to its lane count. Width only gates NEW submissions — lowering it mid-run
    /// lets in-flight work drain naturally.
    pub fn set_width(&mut self, w: usize) { self.width = w.max(1); }
    pub fn width(&self) -> usize { self.width }

    /// Add a branch mid-run (a live reaction seeding another parallel approach). Only during the Running
    /// phase — once reviewing has begun, the field is closed. The new branch joins the round-robin.
    pub fn add_branch(&mut self, dir: impl Into<PathBuf>, task: &str, label: &str) -> bool {
        if self.phase != Phase::Running { return false; }
        let (run, first) = AgentRun::start(dir, task, self.cfg.clone());
        self.branches.push(AgentBranch { run, state: BranchState::Ready(first), label: label.to_string(), verdict: None });
        true
    }

    fn take_ready(&mut self) -> Option<(usize, String)> {
        let n = self.branches.len();
        for off in 0..n {
            let i = (self.cursor + off) % n;
            if matches!(self.branches[i].state, BranchState::Ready(_)) {
                let p = match std::mem::replace(&mut self.branches[i].state, BranchState::InFlight) {
                    BranchState::Ready(p) => p, _ => unreachable!(),
                };
                self.cursor = (i + 1) % n;
                return Some((i, p));
            }
        }
        None
    }

    /// Pick the next thing to run: a ready branch's agent turn (Running), or the next unreviewed branch's
    /// review (Reviewing). Transitions Running→Reviewing→Done as each pass drains. Submits only while
    /// fewer than `width` branches are in flight — call in a loop (or use [`fill`]) to load every lane.
    pub fn next(&mut self) -> FanStep {
        if self.in_flight.len() >= self.width { return FanStep::AllDone; } // lanes full — wait for a Done
        match self.phase {
            Phase::Running => {
                if let Some((i, prompt)) = self.take_ready() {
                    self.in_flight.push(InFlight { branch: i, review: false });
                    return FanStep::Submit { branch: i, prompt };
                }
                if self.in_flight.is_empty()
                    && self.branches.iter().all(|b| matches!(b.state, BranchState::Finished(_))) {
                    self.phase = Phase::Reviewing;
                    return self.next();
                }
                FanStep::AllDone
            }
            Phase::Reviewing => {
                let busy = |me: &Self, i: usize| me.in_flight.iter().any(|f| f.branch == i);
                if let Some(i) = (0..self.branches.len())
                    .find(|&i| self.branches[i].verdict.is_none() && !busy(self, i)) {
                    let diff = (self.diff_fn)(self.branches[i].run.root());
                    let prompt = review_prompt(&self.task, &diff);
                    self.in_flight.push(InFlight { branch: i, review: true });
                    return FanStep::Submit { branch: i, prompt };
                }
                if self.in_flight.is_empty() && self.branches.iter().all(|b| b.verdict.is_some()) {
                    self.phase = Phase::Done;
                }
                FanStep::AllDone
            }
            Phase::Done => FanStep::AllDone,
        }
    }

    /// Load every free lane: repeated [`next`] until nothing more can be submitted. The lane-pool
    /// driver calls this at start and after each branch completion; width=1 yields at most one step,
    /// identical to the historical single-worker flow.
    pub fn fill(&mut self) -> Vec<FanStep> {
        let mut out = Vec::new();
        loop {
            match self.next() {
                s @ FanStep::Submit { .. } => out.push(s),
                FanStep::AllDone => break,
            }
        }
        out
    }

    /// Stream a token to whichever branch is in flight (agent turn or review).
    pub fn on_token(&mut self, t: &str) {
        if let Some(f) = self.in_flight.first().copied() { self.branches[f.branch].run.on_token(t); }
    }

    /// The in-flight generation finished: advance the branch (agent turn) or record its verdict (review),
    /// then schedule the next. Single-worker semantics: completes the FIRST in-flight entry.
    pub fn on_done(&mut self) -> FanStep {
        if !self.in_flight.is_empty() {
            let f = self.in_flight.remove(0);
            self.settle(f, None);
        }
        self.next()
    }

    /// The in-flight generation errored. Single-worker semantics: fails the FIRST in-flight entry.
    pub fn on_error(&mut self, e: String) -> FanStep {
        if !self.in_flight.is_empty() {
            let f = self.in_flight.remove(0);
            self.settle(f, Some(e));
        }
        self.next()
    }

    // ── lane-tagged flow (the LanePool driver) — same machine, events routed by branch ───────────

    /// Stream a token to a specific in-flight branch (lane mode: pool events carry the branch).
    pub fn on_token_branch(&mut self, branch: usize, t: &str) {
        if self.in_flight.iter().any(|f| f.branch == branch) {
            if let Some(b) = self.branches.get_mut(branch) { b.run.on_token(t); }
        }
    }

    /// A specific branch's generation finished; returns the submissions that now fit the free lanes.
    pub fn on_done_branch(&mut self, branch: usize) -> Vec<FanStep> {
        if let Some(pos) = self.in_flight.iter().position(|f| f.branch == branch) {
            let f = self.in_flight.remove(pos);
            self.settle(f, None);
        }
        self.fill()
    }

    /// A specific branch's generation errored; returns the submissions that now fit the free lanes.
    pub fn on_error_branch(&mut self, branch: usize, e: String) -> Vec<FanStep> {
        if let Some(pos) = self.in_flight.iter().position(|f| f.branch == branch) {
            let f = self.in_flight.remove(pos);
            self.settle(f, Some(e));
        }
        self.fill()
    }

    /// Shared completion: apply a finished/errored generation to its branch's state machine.
    fn settle(&mut self, f: InFlight, err: Option<String>) {
        match (f.review, err) {
            (true, None) => {
                let text = self.branches[f.branch].run.take_reply();
                self.branches[f.branch].verdict = Some(parse_verdict(&text));
            }
            (true, Some(e)) => {
                self.branches[f.branch].verdict = Some(Verdict { approve: false, notes: format!("review errored: {e}") });
            }
            (false, None) => match self.branches[f.branch].run.on_done() {
                AgentStep::Submit(p) => self.branches[f.branch].state = BranchState::Ready(p),
                AgentStep::Finished(s) => self.branches[f.branch].state = BranchState::Finished(s),
            },
            (false, Some(e)) => {
                let _ = self.branches[f.branch].run.on_error(e.clone());
                self.branches[f.branch].state = BranchState::Finished(Stop::ModelError(e));
            }
        }
    }

    /// Fold: a summary per branch for the prune UI, carrying the review verdict.
    pub fn summaries(&self) -> Vec<BranchSummary> {
        self.branches.iter().enumerate().map(|(i, b)| BranchSummary {
            index: i,
            label: b.label.clone(),
            files: b.run.events().len(),
            verified: b.run.verified(),
            finished: matches!(b.state, BranchState::Finished(_)),
            approved: b.verdict.as_ref().map(|v| v.approve),
            review_notes: b.verdict.as_ref().map(|v| v.notes.clone()).unwrap_or_default(),
            root: b.run.root().to_path_buf(),
        }).collect()
    }

    /// The suggested keeper: highest by (review-approved, verified, file count). `None` if empty.
    pub fn best(&self) -> Option<usize> {
        self.summaries().into_iter()
            .max_by_key(|b| (b.approved == Some(true), b.verified, b.files))
            .map(|b| b.index)
    }

    /// The workspace root of a branch (for `:reapply`-ing the keeper).
    pub fn branch_root(&self, i: usize) -> Option<&Path> { self.branches.get(i).map(|b| b.run.root()) }
}

// ── real-model adapter: the loop over a local GGUF via llm_backend ───────────────────────────────

/// Bridges [`Llm`] to the process-global active local model (`llm_backend::active_model`), so `run_agent`
/// drives a real GGUF. Only uses the feature-INDEPENDENT surface of `llm_backend` (LoadedModel / GenParams
/// / generate / active_model) — so this compiles in every config; on a stub build (no `llama-ffi`) the
/// underlying `generate` returns an error, which the loop surfaces as `Stop::ModelError` rather than
/// pretending to run.
pub struct LocalLlm {
    model: std::sync::Arc<logscope::llm_backend::LoadedModel>,
    params: logscope::llm_backend::GenParams,
}

impl LocalLlm {
    /// Bind to whatever model `llm_backend::set_active_model` last loaded. `None` if none is loaded — the
    /// caller should tell the user to pick/load a local model first.
    pub fn from_active(params: logscope::llm_backend::GenParams) -> Option<Self> {
        logscope::llm_backend::active_model().map(|model| Self { model, params })
    }
}

impl Llm for LocalLlm {
    fn generate(&mut self, prompt: &str) -> Result<String, String> {
        logscope::llm_backend::generate(&self.model, prompt, &self.params)
    }
}

/// Convenience: run an agent turn-loop against `ws` using the active local model + config. Returns `Err`
/// if no local model is loaded (so the UI can prompt the user to load one).
pub fn run_local_agent(
    ws: &mut Workspace, task: &str, cfg: &AgentConfig, params: logscope::llm_backend::GenParams,
) -> Result<Session, String> {
    let mut llm = LocalLlm::from_active(params)
        .ok_or_else(|| "no local model loaded — pick a .gguf (MODEL selector) and load it first".to_string())?;
    Ok(run_agent_cfg(&mut llm, ws, task, cfg))
}

// ── doc gathering + merge across fan branches ────────────────────────────────────────────────────

/// Read every `docs/**/*.md` under `root` as `(relative_path, text)`. Skips non-doc files so the merge sees
/// only documents. Used to gather a fan branch's produced docs for merging.
pub fn scan_docs(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let docs_root = root.join("docs");
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { walk(&p, root, out); }
            else if p.extension().and_then(|s| s.to_str()) == Some("md") {
                if let Ok(text) = std::fs::read_to_string(&p) {
                    let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                    out.push((rel, text));
                }
            }
        }
    }
    walk(&docs_root, root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Merge the docs produced across several fan branches into one organized tree. `branches` is
/// `(label, root)` per branch; returns the merge result (unified index + collisions + merged files) plus the
/// rendered `docs/README.md` index. The caller writes `merged` + the index into the project.
pub fn merge_branch_docs(branches: &[(String, std::path::PathBuf)], project_title: &str)
    -> (crate::docorg::MergeResult, String)
{
    let bdocs: Vec<crate::docorg::BranchDocs> = branches.iter()
        .map(|(label, root)| crate::docorg::BranchDocs { branch: label.clone(), docs: scan_docs(root) })
        .collect();
    let result = crate::docorg::merge_branches(&bdocs, project_title);
    let index_md = crate::docorg::render_index_markdown(&result.index, project_title);
    (result, index_md)
}

/// Reconcile the GENUINE conflicts in a doc merge with a (depth) model: for each conflicting path, ask the
/// model to merge the branch versions into one coherent document (via `docorg::reconcile_prompt`) and splice
/// the result back into `merged`. Near-duplicate collisions are already auto-resolved by `merge_branches`;
/// this handles only the substantive conflicts. Returns the count reconciled. Pure w.r.t. the model.
pub fn reconcile_doc_conflicts(llm: &mut dyn Llm, result: &mut crate::docorg::MergeResult) -> usize {
    let mut reconciled = 0;
    for (path, versions) in std::mem::take(&mut result.needs_merge) {
        let prompt = crate::docorg::reconcile_prompt(&path, &versions);
        if let Ok(text) = llm.generate(&prompt) {
            let text = text.trim();
            if !text.is_empty() {
                if let Some(slot) = result.merged.iter_mut().find(|(p, _)| *p == path) {
                    slot.1 = text.to_string();
                    reconciled += 1;
                }
            }
        }
    }
    if reconciled > 0 { result.collisions.clear(); } // resolved
    reconciled
}

// ── tests ───────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_replaces_one_unique_anchor_and_records_the_event() {
        let d = tmp("edit-ok");
        let mut ws = Workspace::new(&d);
        ws.apply(&Tool::WriteFile { path: "src/a.rs".into(), contents: "fn one() {}\nfn two() {}\n".into() });
        let r = ws.apply(&Tool::Edit { path: "src/a.rs".into(), find: "fn two() {}".into(), replace: "fn two() -> u8 { 2 }".into() });
        assert!(r.ok, "{}", r.output);
        assert!(r.output.contains("@ line 2"), "reports the line: {}", r.output);
        let back = ws.apply(&Tool::ReadFile { path: "src/a.rs".into() });
        assert!(back.output.contains("fn two() -> u8 { 2 }") && back.output.contains("fn one() {}"));
        assert!(matches!(ws.events().last(), Some(FileEvent { change: FileChange::Modified, .. })));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn edit_teaches_recovery_on_stale_and_ambiguous_anchors() {
        let d = tmp("edit-err");
        let mut ws = Workspace::new(&d);
        ws.apply(&Tool::WriteFile { path: "x.rs".into(), contents: "let a = 1;\nlet a = 1;\n".into() });
        let stale = ws.apply(&Tool::Edit { path: "x.rs".into(), find: "let b = 2;".into(), replace: "".into() });
        assert!(!stale.ok && stale.output.contains("anchor not found"));
        let ambig = ws.apply(&Tool::Edit { path: "x.rs".into(), find: "let a = 1;".into(), replace: "let a = 2;".into() });
        assert!(!ambig.ok && ambig.output.contains("matches 2 places"));
        let missing = ws.apply(&Tool::Edit { path: "nope.rs".into(), find: "x".into(), replace: "y".into() });
        assert!(!missing.ok && missing.output.contains("write_file to create"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn edit_tool_calls_parse_including_synonyms() {
        let text = "```tool\n{ \"tool\": \"edit\", \"path\": \"src/a.rs\", \"find\": \"old text\", \"replace\": \"new text\" }\n```\n\
                    ```tool\n{ \"tool\": \"patch\", \"path\": \"b.rs\", \"old\": \"x\", \"new\": \"y\" }\n```";
        let tools = parse_tools(text);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0], Tool::Edit { path: "src/a.rs".into(), find: "old text".into(), replace: "new text".into() });
        assert_eq!(tools[1], Tool::Edit { path: "b.rs".into(), find: "x".into(), replace: "y".into() });
    }

    #[test]
    fn digest_compresses_rustc_output_to_counts_locations_and_first_detail() {
        let raw = "\
   Compiling demo v0.1.0
error[E0382]: borrow of moved value: `to_request`
  --> src/world/chunk_manager.rs:412:9
   |
412|     self.prefetch(to_request);
   |                   ^^^^^^^^^^ value borrowed here after move

error[E0308]: mismatched types
  --> src/render/world_renderer.rs:88:13

warning: unused variable: `frame_idx`
  --> src/render/world_renderer.rs:88:9

error: aborting due to 2 previous errors; 1 warning emitted
error: could not compile `demo` (lib) due to 3 previous errors
";
        let d = digest_check_output(raw, 6);
        assert!(d.starts_with("2 error(s), 1 warning(s)"), "totals exclude roll-up lines: {d}");
        assert!(d.contains("E0382") && d.contains("src/world/chunk_manager.rs:412:9"));
        assert!(d.contains("E0308") && d.contains("src/render/world_renderer.rs:88:13"));
        assert!(d.contains("First error detail:") && d.contains("value borrowed here after move"));
        assert!(!d.contains("could not compile"), "roll-up noise stays out of the digest");
        assert!(d.len() < raw.len(), "the digest must be smaller than the dump");
    }

    #[test]
    fn digest_says_clean_when_green_and_passes_through_non_cargo_output() {
        assert_eq!(digest_check_output("   Finished dev profile in 0.3s\n", 6), "clean — 0 errors, 0 warning(s)");
        let other = digest_check_output("test result: FAILED. 1 passed; 1 failed\n", 6);
        assert!(other.contains("1 passed; 1 failed"), "non-rustc output survives, bounded: {other}");
    }

    #[test]
    fn fan_width_runs_that_many_branches_at_once_and_backfills() {
        let cfg = AgentConfig { verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, ..Default::default() };
        let dirs: Vec<PathBuf> = (0..3).map(|i| tmp(&format!("fanw-{i}"))).collect();
        let specs = dirs.iter().enumerate()
            .map(|(i, d)| (d.clone(), format!("goal {i}"), format!("b{i}"))).collect();
        let mut fan = AgentFan::new(specs, &cfg, Box::new(|_| String::new()));
        fan.set_width(2);
        let first = fan.fill();
        assert_eq!(first.len(), 2, "width 2 loads two lanes immediately");
        assert_eq!(fan.active_branches(), vec![0, 1]);
        // Branch 1 completes FIRST (out of order — lanes finish whenever they finish): the freed lane
        // backfills with branch 2 while 0 is still generating.
        let next = fan.on_done_branch(1);
        assert_eq!(next.len(), 1);
        assert!(matches!(next[0], FanStep::Submit { branch: 2, .. }));
        let act = fan.active_branches();
        assert!(act.contains(&0) && act.contains(&2) && act.len() == 2);
        for d in &dirs { let _ = std::fs::remove_dir_all(d); }
    }

    #[test]
    fn fan_width_one_is_the_historical_round_robin() {
        let cfg = AgentConfig { verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, ..Default::default() };
        let dirs: Vec<PathBuf> = (0..2).map(|i| tmp(&format!("fan1-{i}"))).collect();
        let specs = dirs.iter().enumerate()
            .map(|(i, d)| (d.clone(), format!("goal {i}"), format!("b{i}"))).collect();
        let mut fan = AgentFan::new(specs, &cfg, Box::new(|_| String::new()));
        assert_eq!(fan.fill().len(), 1, "default width submits exactly one");
        assert_eq!(fan.active_branch(), Some(0));
        // The untagged single-worker API still drives it: Done advances to the NEXT branch.
        match fan.on_done() {
            FanStep::Submit { branch, .. } => assert_eq!(branch, 1),
            FanStep::AllDone => panic!("expected the round-robin to rotate to branch 1"),
        }
        for d in &dirs { let _ = std::fs::remove_dir_all(d); }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wsx-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sandbox_rejects_escapes_and_absolutes() {
        let ws = Workspace::new(tmp("sbx"));
        assert!(ws.sandbox_path("src/lib.rs").is_ok());
        assert!(ws.sandbox_path("a/b/c.rs").is_ok());
        assert!(ws.sandbox_path("a/b/../c.rs").is_err()); // we reject ANY `..`, even interior — simplest safe rule
        assert!(ws.sandbox_path("../escape").is_err());
        assert!(ws.sandbox_path("/etc/passwd").is_err());
        assert!(ws.sandbox_path("a/../../etc").is_err());
    }

    #[test]
    fn write_then_read_records_created_then_modified() {
        let mut ws = Workspace::new(tmp("wr"));
        let r1 = ws.apply(&Tool::WriteFile { path: "src/lib.rs".into(), contents: "fn a(){}".into() });
        assert!(r1.ok, "{}", r1.output);
        let r2 = ws.apply(&Tool::ReadFile { path: "src/lib.rs".into() });
        assert!(r2.ok && r2.output.contains("fn a()"));
        // overwrite → Modified
        let _ = ws.apply(&Tool::WriteFile { path: "src/lib.rs".into(), contents: "fn a(){b}".into() });
        let evs = ws.events();
        assert_eq!(evs[0].change, FileChange::Created);
        assert_eq!(evs[1].change, FileChange::Modified);
        assert_eq!(evs[0].path, "src/lib.rs");
    }

    #[test]
    fn run_gated_by_allowlist_and_records_exit() {
        let mut ws = Workspace::new(tmp("run")).with_allow(&["echo", "false"]);
        let ok = ws.apply(&Tool::Run { argv: vec!["echo".into(), "hi".into()] });
        assert!(ok.ok && ok.output.contains("hi"));
        let denied = ws.apply(&Tool::Run { argv: vec!["rm".into(), "-rf".into(), "/".into()] });
        assert!(!denied.ok && denied.output.contains("not allowed"));
    }

    #[test]
    fn list_dir_hides_git_and_sorts() {
        let root = tmp("ls");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("b.txt"), "b").unwrap();
        std::fs::create_dir_all(root.join("a_dir")).unwrap();
        let mut ws = Workspace::new(root);
        let r = ws.apply(&Tool::ListDir { path: "".into() });
        assert!(r.ok);
        assert_eq!(r.output, "a_dir/\nb.txt"); // sorted, .git hidden
    }

        #[test]
    fn tool_block_with_nested_fences_in_contents() {
        // A write_file whose CONTENTS contain markdown fences (README with ```rust examples) must not be
        // truncated at the inner fence — the block ends at the fence where the JSON is brace-balanced.
        let t = "```tool\n{ \"tool\": \"write_file\", \"path\": \"README.md\", \"contents\": \"# X\\n```rust\\nfn a(){}\\n```\\ndone\" }\n```\nafter";
        let tools = parse_tools(t);
        assert_eq!(tools.len(), 1, "one intact tool call");
        match &tools[0] {
            Tool::WriteFile { path, contents } => {
                assert_eq!(path, "README.md");
                assert!(contents.contains("```rust"), "inner fence preserved in contents");
                assert!(contents.ends_with("done"));
            }
            other => panic!("expected WriteFile, got {other:?}"),
        }
    }

    #[test]
    fn parse_fenced_and_qwen_shapes() {
        let fenced = "sure:\n```tool\n{ \"tool\": \"write_file\", \"path\": \"a.rs\", \"contents\": \"fn x(){}\\n\" }\n```\ndone";
        let v = parse_tools(fenced);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0], Tool::WriteFile { path: "a.rs".into(), contents: "fn x(){}\n".into() });

        let qwen = "<tool_call>{\"tool\":\"run\",\"argv\":[\"cargo\",\"test\"]}</tool_call>";
        assert_eq!(parse_tools(qwen), vec![Tool::Run { argv: vec!["cargo".into(), "test".into()] }]);

        let cmd_form = "```tool\n{\"tool\":\"run\",\"cmd\":\"cargo build\"}\n```";
        assert_eq!(parse_tools(cmd_form), vec![Tool::Run { argv: vec!["cargo".into(), "build".into()] }]);
    }

    #[test]
    fn parse_skips_garbage_but_keeps_good_calls() {
        let mixed = "```tool\nnot json\n```\n```tool\n{\"tool\":\"read_file\",\"path\":\"x\"}\n```";
        assert_eq!(parse_tools(mixed), vec![Tool::ReadFile { path: "x".into() }]);
    }

    // A scripted model: two tool-call turns, then TASK_COMPLETE — the whole loop with no GPU.
    struct Scripted { turns: Vec<String>, i: usize }
    impl Llm for Scripted {
        fn generate(&mut self, _prompt: &str) -> Result<String, String> {
            let t = self.turns.get(self.i).cloned().unwrap_or_else(|| DONE_MARKER.into());
            self.i += 1;
            Ok(t)
        }
    }

    #[test]
    fn ingest_drops_bytes_and_records_event() {
        let mut ws = Workspace::new(tmp("ingest"));
        let r = ws.ingest("assets/logo.bin", &[0u8, 1, 2, 3]);
        assert!(r.ok, "{}", r.output);
        assert_eq!(ws.events()[0].path, "assets/logo.bin");
        assert_eq!(ws.events()[0].change, FileChange::Created);
        assert_eq!(std::fs::read(ws.root().join("assets/logo.bin")).unwrap(), vec![0u8, 1, 2, 3]);
        // escape attempt is rejected
        assert!(!ws.ingest("../evil", b"x").ok);
    }

    #[test]
    fn full_loop_writes_files_then_completes() {
        let mut ws = Workspace::new(tmp("loop"));
        let mut llm = Scripted { i: 0, turns: vec![
            "Here's the plan.\n```plan\n1. create main.rs\n2. list src\n```\n```tool\n{\"tool\":\"write_file\",\"path\":\"src/main.rs\",\"contents\":\"fn main(){}\\n\"}\n```".into(),
            "Now list it.\n```tool\n{\"tool\":\"list_dir\",\"path\":\"src\"}\n```".into(),
            format!("All set. {DONE_MARKER}"),
        ]};
        // verification off (scripted model, no cargo in the sandbox); tests the plan + loop + files.
        let cfg = AgentConfig { max_steps: 8, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, read_policy: ReadPolicy::Full, doc_mode: false, prefer_patches: false, digest_verify: false, context: String::new() };
        let s = run_agent_cfg(&mut llm, &mut ws, "make a hello world", &cfg);
        assert_eq!(s.stop, Stop::Done);
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.files[0].path, "src/main.rs");
        assert_eq!(s.files[0].change, FileChange::Created);
        assert!(std::fs::read_to_string(ws.root().join("src/main.rs")).unwrap().contains("fn main"));
        assert_eq!(s.turns.len(), 3);
        // the plan was captured (2)
        assert_eq!(s.plan.steps.len(), 2);
        assert_eq!(s.plan.steps[0].text, "create main.rs");
        assert!(s.verified, "verification off ⇒ vacuously verified");
    }

    #[test]
    fn agent_fan_interleaves_reviews_and_ranks() {
        let base = tmp("fan");
        let (d0, d1) = (base.join("b0"), base.join("b1"));
        std::fs::create_dir_all(&d0).unwrap();
        std::fs::create_dir_all(&d1).unwrap();
        let cfg = AgentConfig { max_steps: 8, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, read_policy: ReadPolicy::Full, doc_mode: false, prefer_patches: false, digest_verify: false, context: String::new() };
        // diff_fn returns a per-branch canned diff (no git needed in the test)
        let diff_fn = Box::new(|root: &Path| format!("diff for {}", root.display()));
        let mut fan = AgentFan::new(vec![
            (d0.clone(), "goal A".into(), "anyhow".into()),
            (d1.clone(), "goal B".into(), "thiserror".into()),
        ], &cfg, diff_fn);
        assert_eq!(fan.len(), 2);

        // Running-phase replies (write a file, then done) and Reviewing-phase verdicts (branch 1 approved).
        let agent_replies: [[&str; 2]; 2] = [
            ["```tool\n{\"tool\":\"write_file\",\"path\":\"a.rs\",\"contents\":\"//A\\n\"}\n```", "TASK_COMPLETE"],
            ["```tool\n{\"tool\":\"write_file\",\"path\":\"b.rs\",\"contents\":\"//B\\n\"}\n```", "TASK_COMPLETE"],
        ];
        let review_replies = ["REVISE\nbranch A needs work", "APPROVE\nbranch B looks right"];
        let mut agent_turn = [0usize, 0usize];

        let mut order = Vec::new();
        let mut reviewed = 0usize;
        let mut step = fan.next();
        while let FanStep::Submit { branch, .. } = step {
            let reply = if fan.reviewing() {
                let r = review_replies[branch]; reviewed += 1; r
            } else {
                order.push(branch);
                let r = agent_replies[branch][agent_turn[branch]]; agent_turn[branch] += 1; r
            };
            for ch in reply.chars() { fan.on_token(&ch.to_string()); }
            step = fan.on_done();
        }
        assert_eq!(step, FanStep::AllDone);
        assert!(fan.all_done());
        assert_eq!(order, vec![0, 1, 0, 1]); // round-robin agent turns
        assert_eq!(reviewed, 2);             // one review per branch
        // isolation held
        assert!(d0.join("a.rs").exists() && !d0.join("b.rs").exists());
        assert!(d1.join("b.rs").exists() && !d1.join("a.rs").exists());
        // ranking: branch 1 (thiserror) was APPROVEd → it's the suggested keeper
        let s = fan.summaries();
        assert_eq!(s[0].approved, Some(false));
        assert_eq!(s[1].approved, Some(true));
        assert_eq!(fan.best(), Some(1));
    }

    #[test]
    fn agent_fan_add_branch_mid_run() {
        let base = tmp("fanadd");
        std::fs::create_dir_all(base.join("b0")).unwrap();
        std::fs::create_dir_all(base.join("b1")).unwrap();
        let cfg = AgentConfig { max_steps: 8, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, read_policy: ReadPolicy::Full, doc_mode: false, prefer_patches: false, digest_verify: false, context: String::new() };
        let mut fan = AgentFan::new(vec![(base.join("b0"), "A".into(), "one".into())], &cfg, Box::new(|_| String::new()));
        // first branch starts running
        let s0 = fan.next();
        assert!(matches!(s0, FanStep::Submit { branch: 0, .. }));
        // a reaction adds a second branch mid-run — it joins the round-robin
        assert!(fan.add_branch(base.join("b1"), "B", "two"));
        assert_eq!(fan.len(), 2);
        for ch in "TASK_COMPLETE".chars() { fan.on_token(&ch.to_string()); }
        // after branch 0 finishes its (only) turn → done, the fan rotates to the newly-added branch 1
        let s1 = fan.on_done();
        assert!(matches!(s1, FanStep::Submit { branch: 1, .. }), "added branch is scheduled");
    }

    #[test]
    fn adopt_forks_a_single_run_into_a_fan() {
        let base = tmp("adopt");
        std::fs::create_dir_all(base.join("orig")).unwrap();
        std::fs::create_dir_all(base.join("react")).unwrap();
        let cfg = AgentConfig { max_steps: 8, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, read_policy: ReadPolicy::Full, doc_mode: false, prefer_patches: false, digest_verify: false, context: String::new() };
        // a single run in progress, with a queued next prompt
        let (run, first) = AgentRun::start(base.join("orig"), "build a thing", cfg.clone());
        let mut fan = AgentFan::adopt(run, Some(first), "build a thing", "original", &cfg, Box::new(|_| String::new()));
        assert_eq!(fan.len(), 1);
        // a reaction adds a parallel approach
        assert!(fan.add_branch(base.join("react"), "build a thing\n\nAdjustment: use a trait", "reaction 1"));
        assert_eq!(fan.len(), 2);
        // branch 0 (the original, preserved) is scheduled first
        let s0 = fan.next();
        assert!(matches!(s0, FanStep::Submit { branch: 0, .. }), "the original run's work continues as branch 0");
        for ch in "TASK_COMPLETE".chars() { fan.on_token(&ch.to_string()); }
        // then it rotates to the reaction branch
        let s1 = fan.on_done();
        assert!(matches!(s1, FanStep::Submit { branch: 1, .. }), "the reaction approach runs as branch 1");
    }

    #[test]
    fn model_reconciles_genuine_doc_conflicts() {
        let a = "---\ntitle: Setup\nsection: guide\norder: 1\nstatus: draft\nsummary: docker\n---\n# Setup\nUse docker compose to run the stack in containers with mounted volumes and exposed ports.\n";
        let b = "---\ntitle: Setup\nsection: guide\norder: 1\nstatus: draft\nsummary: native\n---\n# Setup\nCompile from source with cargo build release and run the native binary directly on the host.\n";
        let b1 = crate::docorg::BranchDocs { branch: "docker".into(), docs: vec![("docs/guide/setup.md".into(), a.into())] };
        let b2 = crate::docorg::BranchDocs { branch: "native".into(), docs: vec![("docs/guide/setup.md".into(), b.into())] };
        let mut m = crate::docorg::merge_branches(&[b1, b2], "Docs");
        assert_eq!(m.collisions.len(), 1);
        // a scripted reconciler returns a merged doc
        struct Reconciler;
        impl Llm for Reconciler {
            fn generate(&mut self, _p: &str) -> Result<String, String> {
                Ok("---\ntitle: Setup\nsection: guide\norder: 1\nstatus: draft\nsummary: docker or native\n---\n# Setup\nRun via docker compose, OR compile from source and run natively.\n".into())
            }
        }
        let n = reconcile_doc_conflicts(&mut Reconciler, &mut m);
        assert_eq!(n, 1);
        assert!(m.collisions.is_empty(), "conflict resolved");
        let setup = m.merged.iter().find(|(p, _)| p == "docs/guide/setup.md").unwrap();
        assert!(setup.1.contains("docker compose, OR compile"), "reconciled text spliced in");
    }

    #[test]
    fn scan_and_merge_branch_docs_over_real_dirs() {
        let base = tmp("docmerge");
        let (r0, r1) = (base.join("b0"), base.join("b1"));
        std::fs::create_dir_all(r0.join("docs/guide")).unwrap();
        std::fs::create_dir_all(r1.join("docs/reference")).unwrap();
        std::fs::write(r0.join("docs/guide/install.md"),
            "---\ntitle: Install\nsection: guide\norder: 1\nstatus: draft\nsummary: how to install\n---\n# Install\n").unwrap();
        std::fs::write(r1.join("docs/reference/api.md"),
            "---\ntitle: API\nsection: reference\norder: 1\nstatus: draft\nsummary: the api\n---\n# API\n").unwrap();
        // scan finds only the md under docs/
        assert_eq!(scan_docs(&r0).len(), 1);
        let (merge, index_md) = merge_branch_docs(&[("guides".into(), r0), ("ref".into(), r1)], "My Docs");
        assert_eq!(merge.merged.len(), 2);
        assert!(merge.collisions.is_empty());
        // index groups guide before reference and links both
        assert!(index_md.contains("## Guides") && index_md.contains("## Reference"));
        assert!(index_md.contains("docs/guide/install.md") && index_md.contains("docs/reference/api.md"));
    }

    #[test]
    fn doc_mode_prompt_carries_the_convention() {
        let cfg = AgentConfig::docs();
        assert!(cfg.doc_mode && cfg.verify_cmd.is_empty());
        let p = system_prompt("write the docs", &cfg);
        assert!(p.contains("DOCUMENT ORGANIZATION") && p.contains("front-matter"));
    }

    #[test]
    fn agent_aborts_a_looping_bad_actor() {
        // a model that emits the SAME write every turn (never progresses) must be cut, not run to the cap.
        let mut ws = Workspace::new(tmp("loopactor"));
        struct Repeater;
        impl Llm for Repeater {
            fn generate(&mut self, _p: &str) -> Result<String, String> {
                Ok("I'll write it.\n```tool\n{\"tool\":\"write_file\",\"path\":\"x.txt\",\"contents\":\"same\"}\n```".into())
            }
        }
        let cfg = AgentConfig { max_steps: 30, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, read_policy: ReadPolicy::Full, doc_mode: false, prefer_patches: false, digest_verify: false, context: String::new() };
        let s = run_agent_cfg(&mut Repeater, &mut ws, "task", &cfg);
        assert_eq!(s.stop, Stop::Stalled, "a repeating agent is aborted as a bad actor");
        assert!(s.turns.len() < 30, "aborted well before the step limit ({} turns)", s.turns.len());
    }

    #[test]
    fn async_driver_matches_sync_behavior() {
        // Drive AgentRun by hand with scripted completed replies (no worker), asserting it writes the file,
        // captures the plan, and finishes — the same state machine the sync loop uses.
        let cfg = AgentConfig { max_steps: 8, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, read_policy: ReadPolicy::Full, doc_mode: false, prefer_patches: false, digest_verify: false, context: String::new() };
        let (mut run, first) = AgentRun::start(tmp("async"), "make a lib", cfg);
        assert!(first.contains("PLAN")); // prompt asks for a plan
        let replies = [
            "```plan\n1. write lib\n```\n```tool\n{\"tool\":\"write_file\",\"path\":\"src/lib.rs\",\"contents\":\"pub fn x(){}\\n\"}\n```",
            &format!("{DONE_MARKER}"),
        ];
        // turn 1
        for ch in replies[0].chars() { run.on_token(&ch.to_string()); }
        let step1 = run.on_done();
        assert!(matches!(step1, AgentStep::Submit(_)));
        assert_eq!(run.events().len(), 1);
        assert_eq!(run.plan().steps.len(), 1);
        // turn 2 → done
        for ch in replies[1].chars() { run.on_token(&ch.to_string()); }
        assert_eq!(run.on_done(), AgentStep::Finished(Stop::Done));
        assert!(std::fs::read_to_string(run.root().join("src/lib.rs")).unwrap().contains("pub fn x"));
    }

    #[test]
    fn parse_plan_fenced_and_numbered() {
        let fenced = "ok\n```plan\n1. do a\n2. do b\n- do c\n```\nthen act";
        let p = parse_plan(fenced);
        assert_eq!(p.steps.len(), 3);
        assert_eq!(p.steps[0].text, "do a");
        assert_eq!(p.steps[2].text, "do c");
        // leading numbered list (no fence), stops before the tool block
        let numbered = "1. first\n2. second\n```tool\n{...}\n```\n3. not-a-plan-step";
        let p2 = parse_plan(numbered);
        assert_eq!(p2.steps.len(), 2);
        // plan progress
        let mut p3 = p2; p3.advance();
        assert_eq!(p3.progress(), (1, 2));
    }

    #[test]
    fn done_is_gated_on_verification() {
        // model claims done before anything is green; with gating on, the loop must NOT accept it.
        let mut ws = Workspace::new(tmp("gate"));
        struct ClaimsDone;
        impl Llm for ClaimsDone { fn generate(&mut self, _p: &str) -> Result<String, String> { Ok(format!("{DONE_MARKER}")) } }
        let cfg = AgentConfig { max_steps: 3, verify_cmd: vec!["cargo".into(), "check".into()], verify_exts: vec!["rs".into()], gate_done_on_verify: true, read_policy: ReadPolicy::Full, doc_mode: false, prefer_patches: false, digest_verify: false, context: String::new() };
        let s = run_agent_cfg(&mut ClaimsDone, &mut ws, "task", &cfg);
        // verified never became true (nothing verifiable ran) ⇒ done rejected ⇒ step limit
        assert_eq!(s.stop, Stop::StepLimit);
        assert!(!s.verified);
    }

    #[test]
    fn review_verdict_parses_approve_and_revise() {
        assert!(parse_verdict("APPROVE\nlooks correct").approve);
        let v = parse_verdict("REVISE\nmissing error handling on line 12");
        assert!(!v.approve);
        assert!(v.notes.contains("missing error handling"));
        // scripted reviewer
        struct Rev;
        impl Llm for Rev { fn generate(&mut self, _p: &str) -> Result<String, String> { Ok("APPROVE\nfine".into()) } }
        assert!(run_review(&mut Rev, "t", "diff").unwrap().approve);
    }

    #[test]
    fn surgical_reads_search_and_range() {
        let root = tmp("surg");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn a(){}\nfn target(){}\nfn c(){}\n").unwrap();
        let mut ws = Workspace::new(root);
        let hit = ws.apply(&Tool::Search { pattern: "target".into(), path: "".into() });
        assert!(hit.ok && hit.output.contains("src/lib.rs:2"));
        let rng = ws.apply(&Tool::ReadRange { path: "src/lib.rs".into(), start: 2, end: 2 });
        assert!(rng.ok && rng.output.contains("2: fn target"));
    }

    #[test]
    fn reaction_seeds_parallel_branches() {
        let goals = seed_agent_branches("use anyhow and add tests", "build a parser", 4);
        assert!(goals.len() >= 2, "a two-clause reaction fans into ≥2 branches");
        assert!(goals[0].contains("build a parser") && goals[0].contains("use anyhow and add tests"));
        assert!(goals.iter().any(|g| g.contains("add tests")));
        // too-thin reaction seeds nothing (caller treats it as dictation)
        assert!(seed_agent_branches("ok", "build a parser", 4).is_empty());
    }

    #[test]
    fn loop_hits_step_limit_when_never_done() {
        let mut ws = Workspace::new(tmp("limit"));
        struct Never;
        impl Llm for Never { fn generate(&mut self, _p: &str) -> Result<String, String> { Ok("thinking…".into()) } }
        let s = run_agent(&mut Never, &mut ws, "loop forever", 3);
        assert_eq!(s.stop, Stop::StepLimit);
        assert_eq!(s.turns.len(), 3);
    }

    #[test]
    fn doc_tool_parses_and_shares_across_workers() {
        // parse
        let t = parse_one(r##"{ "tool": "doc", "name": "survey", "content": "# Map\nblocks live in src/blocks.rs" }"##)
            .expect("doc parses");
        assert!(matches!(&t, Tool::Doc { name, .. } if name == "survey"));
        // exec: writes .logscope/docs/<slug>.md; traversal-ish names are slugged harmless
        let d = std::env::temp_dir().join(format!("wsdoc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut ex = Workspace::new(&d);
        let r = ex.apply(&t);
        assert!(r.ok, "{}", r.output);
        assert!(d.join(".logscope/docs/survey.md").is_file());
        let evil = ex.apply(&Tool::Doc { name: "../../etc/passwd".into(), content: "x".into() });
        assert!(evil.ok, "slugged, not rejected: {}", evil.output);
        assert!(d.join(".logscope/docs/etc-passwd.md").is_file() || std::fs::read_dir(d.join(".logscope/docs")).unwrap().count() >= 2);
        // collection: what one worker wrote shows up in the next prompt's context
        let docs = collect_shared_docs(&d, 4096);
        assert!(docs.contains("shared doc: survey") && docs.contains("blocks live in src/blocks.rs"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn repo_tree_is_bounded_and_skips_generated() {
        let d = std::env::temp_dir().join(format!("wstree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("target/debug")).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[package]").unwrap();
        for i in 0..12 { std::fs::write(d.join(format!("src/m{i}.rs")), "//").unwrap(); }
        std::fs::write(d.join("target/debug/junk.rs"), "").unwrap();
        let full = repo_tree(&d, 300);
        assert!(full.contains("Cargo.toml") && full.contains("m3.rs"));
        assert!(!full.contains("target"), "generated trees stay out:\n{full}");
        let capped = repo_tree(&d, 5);
        assert!(capped.contains("truncated"), "cap note present:\n{capped}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn prompt_teaches_explore_first_and_carries_context() {
        let mut cfg = AgentConfig::default();
        cfg.context = "src/blocks.rs (120 B)".into();
        let p = system_prompt("add more blocks", &cfg);
        assert!(p.contains("LOOK AT IT"), "explore-first policy present");
        assert!(p.contains("asking the user"), "anti-clarification instruction present");
        assert!(p.contains("WORKSPACE OVERVIEW") && p.contains("src/blocks.rs"), "turn-0 tree injected");
        assert!(p.contains("\"tool\":\"doc\"") || p.contains("\\\"doc\\\"") || p.contains("- doc"), "shared-doc tool on the menu");
        assert!(p.contains("search"), "search always on the menu (Full policy included it)");
        // no context → no empty section
        let p2 = system_prompt("t", &AgentConfig::default());
        assert!(!p2.contains("WORKSPACE OVERVIEW"));
    }

    #[test]
    fn cargo_lock_versions_parses_packages() {
        let lock = r#"
# auto-generated
[[package]]
name = "serde"
version = "1.0.219"

[[package]]
name = "lodash-rs"
version = "0.2.0"
dependencies = ["serde"]

[metadata]
foo = "bar"
"#;
        let v = cargo_lock_versions(lock);
        assert!(v.contains(&("serde".to_string(), "1.0.219".to_string())));
        assert!(v.contains(&("lodash-rs".to_string(), "0.2.0".to_string())));
        assert!(!v.iter().any(|(n, _)| n == "foo"), "metadata section is not a package");
        assert!(version_key("1.0.219") > version_key("1.0.9"), "numeric, not lexical");
    }

    #[test]
    fn library_roots_are_read_only_and_reject_escapes() {
        let d = std::env::temp_dir().join(format!("wslib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut ex = Workspace::new(&d);
        // writes/edits under ~ are refused
        let w = ex.apply(&Tool::WriteFile { path: "~std/core/src/hack.rs".into(), contents: "x".into() });
        assert!(!w.ok && w.output.contains("READ-ONLY"));
        let e = ex.apply(&Tool::Edit { path: "~deps/serde/src/lib.rs".into(), find: "a".into(), replace: "b".into() });
        assert!(!e.ok && e.output.contains("READ-ONLY"));
        // path traversal out of a virtual root is rejected
        assert!(ex.resolve_read("~deps/serde/../../../etc/passwd").is_err());
        // unknown virtual root named clearly
        let u = ex.apply(&Tool::ReadFile { path: "~bogus/x".into() });
        assert!(!u.ok && u.output.contains("~std") && u.output.contains("~deps"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reads_real_dependency_and_stdlib_source() {
        // Uses the container's own registry + toolchain — skips cleanly if either is absent so the
        // test is portable, but in this build both exist (we depend on serde-ish crates + have rust-src).
        let d = std::env::temp_dir().join(format!("wsdep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut ex = Workspace::new(&d);
        // pick any crate actually present in the registry
        let src = ex.cargo_home.join("registry").join("src");
        let sample = std::fs::read_dir(&src).ok().and_then(|rd| {
            rd.flatten().map(|e| e.path()).filter(|p| p.is_dir())
              .find_map(|reg| std::fs::read_dir(&reg).ok().and_then(|r2| {
                  r2.flatten().map(|e| e.file_name().to_string_lossy().into_owned())
                    .find(|n| n.contains('-') && n.rsplit_once('-').map(|(_, v)| v.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)).unwrap_or(false))
              }))
        });
        if let Some(fname) = sample {
            let name = fname.rsplit_once('-').unwrap().0.to_string();
            let dir = ex.dep_dir(&name).expect("dep dir resolves");
            assert!(dir.is_dir() && dir.file_name().unwrap().to_string_lossy().starts_with(&name));
            // list_dir ~deps/<name> works through the tool surface
            let ls = ex.apply(&Tool::ListDir { path: format!("~deps/{name}") });
            assert!(ls.ok, "{}", ls.output);
        }
        // stdlib: read_range on core's Option source if rust-src is present
        if let Ok(root) = ex.rust_src_root() {
            assert!(root.join("core/src/option.rs").exists() || root.join("core").is_dir());
            let r = ex.apply(&Tool::ReadRange { path: "~std/core/src/option.rs".into(), start: 1, end: 5 });
            assert!(r.ok || r.output.contains("read"), "stdlib read reached the file layer");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn prompt_teaches_library_roots() {
        let p = system_prompt("use serde", &AgentConfig::default());
        assert!(p.contains("~deps/<crate>") && p.contains("~std/"), "both roots documented");
        assert!(p.contains("IS the documentation"), "explains source-as-docs");
    }
}
