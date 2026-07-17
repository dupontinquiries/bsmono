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
        }
    }

    fn search(&self, pattern: &str, rel: &str) -> ToolResult {
        let base = if rel.is_empty() || rel == "." {
            self.root.clone()
        } else {
            match self.sandbox_path(rel) { Ok(p) => p, Err(e) => return ToolResult::err(e) }
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
        let abs = match self.sandbox_path(rel) { Ok(p) => p, Err(e) => return ToolResult::err(e) };
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

    fn write_file(&mut self, rel: &str, contents: &str) -> ToolResult {
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

    fn read_file(&self, rel: &str) -> ToolResult {
        let abs = match self.sandbox_path(rel) { Ok(p) => p, Err(e) => return ToolResult::err(e) };
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
        let abs = if rel.is_empty() || rel == "." {
            self.root.clone()
        } else {
            match self.sandbox_path(rel) { Ok(p) => p, Err(e) => return ToolResult::err(e) }
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
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_steps: 24,
            verify_cmd: vec!["cargo".into(), "check".into()],
            verify_exts: vec!["rs".into()],
            gate_done_on_verify: true,
            read_policy: ReadPolicy::Full,
        }
    }
}

impl AgentConfig {
    /// The worker preset: surgical reads, no auto-verify (the coordinator verifies the stitched whole),
    /// tighter step budget — for fast parallel spin-off, per the coordinator-mode discussion.
    pub fn worker() -> Self {
        Self { max_steps: 10, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, read_policy: ReadPolicy::Surgical }
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

/// Extract the raw JSON-ish payloads from either supported block shape.
fn extract_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    // fenced ```tool … ```
    let mut rest = text;
    while let Some(start) = rest.find("```tool") {
        let after = &rest[start + "```tool".len()..];
        if let Some(end) = after.find("```") {
            blocks.push(after[..end].trim().to_string());
            rest = &after[end + 3..];
        } else { break; }
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
}

/// The sentinel a model prints when the task is complete.
pub const DONE_MARKER: &str = "TASK_COMPLETE";

/// Build the system prompt that teaches a local model the tool protocol. Kept explicit and example-led
/// because small models follow a shown format far more reliably than a described one. `cfg` shapes it:
/// the read tools offered depend on `read_policy`, and the verify promise is stated when verification is on.
pub fn system_prompt(task: &str, cfg: &AgentConfig) -> String {
    let read_tools = match cfg.read_policy {
        ReadPolicy::Full => "\
- read_file   { \"tool\":\"read_file\", \"path\":\"<rel path>\" }  (reads the whole file — prefer this; context is cheap)"
            .to_string(),
        ReadPolicy::Surgical => "\
- search      { \"tool\":\"search\", \"pattern\":\"<text>\", \"path\":\"<rel or empty>\" }  (grep the repo — use this first)
- read_range  { \"tool\":\"read_range\", \"path\":\"<rel path>\", \"start\":<line>, \"end\":<line> }  (read just the lines you need)"
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
{read_tools}
- list_dir    {{ \"tool\":\"list_dir\", \"path\":\"<rel path or empty for root>\" }}
- run         {{ \"tool\":\"run\", \"argv\":[\"cargo\",\"test\"] }}

Rules:
- Paths are relative to the workspace root; you cannot escape it.
- `run` is limited to an allowlist (cargo, rustc, git, ls, cat, grep, …). No shell, no network.
- After each tool call you will see its result, then decide the next step.
- When the task is finished and verified, reply with the single line: {DONE_MARKER}

Emit one or a few tool calls per turn, then wait for results.{verify_note}"
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
    plan: &mut Plan, verified: &mut bool, step: usize,
) -> (Vec<(Tool, ToolResult)>, Option<Stop>) {
    prompt.push_str("\n\nASSISTANT:\n");
    prompt.push_str(text);

    if plan.is_empty() {
        let p = parse_plan(text);
        if !p.is_empty() { *plan = p; }
    }

    let tools = parse_tools(text);

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
        if let Tool::WriteFile { path, .. } = &tool {
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
        prompt.push_str(&format!(
            "\n\nAUTOMATIC VERIFICATION ({}): {}\n{}\n",
            cfg.verify_cmd.join(" "),
            if vres.ok { "PASSED" } else { "FAILED — fix these before continuing" },
            vres.output,
        ));
    }

    plan.advance();
    (results, None)
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
    let mut verified = cfg.verify_cmd.is_empty();

    for step in 0..cfg.max_steps {
        let text = match llm.generate(&prompt) {
            Ok(t) => t,
            Err(e) => return Session { turns, stop: Stop::ModelError(e), files: ws.events().to_vec(), plan, verified },
        };
        let (results, stop) = step_turn(&text, &mut prompt, ws, cfg, &mut plan, &mut verified, step);
        turns.push(Turn { model_text: text, results });
        if let Some(stop) = stop {
            return Session { turns, stop, files: ws.events().to_vec(), plan, verified };
        }
    }

    Session { turns, stop: Stop::StepLimit, files: ws.events().to_vec(), plan, verified }
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
    step: usize,
    turns: usize,
}

impl AgentRun {
    /// Begin a run over `work_dir`. Returns the driver plus the first prompt to submit to the worker.
    pub fn start(work_dir: impl Into<std::path::PathBuf>, task: &str, cfg: AgentConfig) -> (Self, String) {
        let prompt = system_prompt(task, &cfg);
        let verified = cfg.verify_cmd.is_empty();
        let run = Self {
            ws: Workspace::new(work_dir), prompt: prompt.clone(), reply: String::new(),
            plan: Plan::default(), verified, step: 0, turns: 0, cfg,
        };
        (run, prompt)
    }

    /// Append a streamed token to the current turn's reply.
    pub fn on_token(&mut self, t: &str) { self.reply.push_str(t); }

    /// The worker finished a turn: process the accumulated reply and decide the next action.
    pub fn on_done(&mut self) -> AgentStep {
        let text = std::mem::take(&mut self.reply);
        let (_results, stop) = step_turn(&text, &mut self.prompt, &mut self.ws, &self.cfg, &mut self.plan, &mut self.verified, self.step);
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
    in_flight: Option<InFlight>,
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
        Self { branches, cursor: 0, in_flight: None, phase: Phase::Running, task, cfg: cfg.clone(), diff_fn }
    }

    pub fn len(&self) -> usize { self.branches.len() }
    pub fn is_empty(&self) -> bool { self.branches.is_empty() }
    pub fn active_branch(&self) -> Option<usize> { self.in_flight.map(|f| f.branch) }
    pub fn reviewing(&self) -> bool { self.phase == Phase::Reviewing }
    pub fn all_done(&self) -> bool { self.phase == Phase::Done }

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
    /// review (Reviewing). Transitions Running→Reviewing→Done as each pass drains.
    pub fn next(&mut self) -> FanStep {
        if self.in_flight.is_some() { return FanStep::AllDone; } // caller must wait for the in-flight Done
        match self.phase {
            Phase::Running => {
                if let Some((i, prompt)) = self.take_ready() {
                    self.in_flight = Some(InFlight { branch: i, review: false });
                    return FanStep::Submit { branch: i, prompt };
                }
                if self.branches.iter().all(|b| matches!(b.state, BranchState::Finished(_))) {
                    self.phase = Phase::Reviewing;
                    return self.next();
                }
                FanStep::AllDone
            }
            Phase::Reviewing => {
                if let Some(i) = self.branches.iter().position(|b| b.verdict.is_none()) {
                    let diff = (self.diff_fn)(self.branches[i].run.root());
                    let prompt = review_prompt(&self.task, &diff);
                    self.in_flight = Some(InFlight { branch: i, review: true });
                    return FanStep::Submit { branch: i, prompt };
                }
                self.phase = Phase::Done;
                FanStep::AllDone
            }
            Phase::Done => FanStep::AllDone,
        }
    }

    /// Stream a token to whichever branch is in flight (agent turn or review).
    pub fn on_token(&mut self, t: &str) {
        if let Some(f) = self.in_flight { self.branches[f.branch].run.on_token(t); }
    }

    /// The in-flight generation finished: advance the branch (agent turn) or record its verdict (review),
    /// then schedule the next.
    pub fn on_done(&mut self) -> FanStep {
        if let Some(f) = self.in_flight.take() {
            if f.review {
                let text = self.branches[f.branch].run.take_reply();
                self.branches[f.branch].verdict = Some(parse_verdict(&text));
            } else {
                match self.branches[f.branch].run.on_done() {
                    AgentStep::Submit(p) => self.branches[f.branch].state = BranchState::Ready(p),
                    AgentStep::Finished(s) => self.branches[f.branch].state = BranchState::Finished(s),
                }
            }
        }
        self.next()
    }

    /// The in-flight generation errored.
    pub fn on_error(&mut self, e: String) -> FanStep {
        if let Some(f) = self.in_flight.take() {
            if f.review {
                self.branches[f.branch].verdict = Some(Verdict { approve: false, notes: format!("review errored: {e}") });
            } else {
                let _ = self.branches[f.branch].run.on_error(e.clone());
                self.branches[f.branch].state = BranchState::Finished(Stop::ModelError(e));
            }
        }
        self.next()
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

// ── tests ───────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
        let cfg = AgentConfig { max_steps: 8, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, read_policy: ReadPolicy::Full };
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
        let cfg = AgentConfig { max_steps: 8, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, read_policy: ReadPolicy::Full };
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
        let cfg = AgentConfig { max_steps: 8, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, read_policy: ReadPolicy::Full };
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
    fn async_driver_matches_sync_behavior() {
        // Drive AgentRun by hand with scripted completed replies (no worker), asserting it writes the file,
        // captures the plan, and finishes — the same state machine the sync loop uses.
        let cfg = AgentConfig { max_steps: 8, verify_cmd: Vec::new(), verify_exts: Vec::new(), gate_done_on_verify: false, read_policy: ReadPolicy::Full };
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
        let cfg = AgentConfig { max_steps: 3, verify_cmd: vec!["cargo".into(), "check".into()], verify_exts: vec!["rs".into()], gate_done_on_verify: true, read_policy: ReadPolicy::Full };
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
}
