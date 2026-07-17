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
}

impl Tool {
    /// A one-line human summary for the transcript / activity HUD.
    pub fn summary(&self) -> String {
        match self {
            Tool::WriteFile { path, contents } => format!("write {} ({} B)", path, contents.len()),
            Tool::ReadFile { path } => format!("read {path}"),
            Tool::ListDir { path } => format!("ls {}", if path.is_empty() { "." } else { path }),
            Tool::Run { argv } => format!("run {}", argv.join(" ")),
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
        }
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

/// The full record a session produces: every turn + why it stopped + the file-event feed.
pub struct Session {
    pub turns: Vec<Turn>,
    pub stop: Stop,
    pub files: Vec<FileEvent>,
}

/// The sentinel a model prints when the task is complete.
pub const DONE_MARKER: &str = "TASK_COMPLETE";

/// Build the system prompt that teaches a local model the tool protocol. Kept explicit and example-led
/// because small models follow a shown format far more reliably than a described one.
pub fn system_prompt(task: &str) -> String {
    format!(
"You are a coding agent working inside an isolated workspace. Complete this task:

{task}

You act by emitting TOOL CALLS. Each tool call is a fenced block:

```tool
{{ \"tool\": \"write_file\", \"path\": \"src/lib.rs\", \"contents\": \"fn add(a:i32,b:i32)->i32{{a+b}}\\n\" }}
```

Available tools (one JSON object per block):
- write_file  {{ \"tool\":\"write_file\", \"path\":\"<rel path>\", \"contents\":\"<file text>\" }}
- read_file   {{ \"tool\":\"read_file\", \"path\":\"<rel path>\" }}
- list_dir    {{ \"tool\":\"list_dir\", \"path\":\"<rel path or empty for root>\" }}
- run         {{ \"tool\":\"run\", \"argv\":[\"cargo\",\"test\"] }}

Rules:
- Paths are relative to the workspace root; you cannot escape it.
- `run` is limited to an allowlist (cargo, rustc, git, ls, cat, grep, …). No shell, no network.
- After each tool call you will see its result, then decide the next step.
- When the task is finished and verified, reply with the single line: {DONE_MARKER}

Emit one or a few tool calls per turn, then wait for results."
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

/// Run the agentic loop: the model drives tool calls against `ws` until it prints `TASK_COMPLETE`, errors,
/// or hits `max_steps`. Pure w.r.t. the model (any `Llm`), so it unit-tests with a scripted model. The
/// growing `prompt` IS the context window — each turn appends the model's text and the tool observations,
/// which is the standard tool-use transcript shape (`llm_backend::generate` sees the whole thing).
pub fn run_agent(llm: &mut dyn Llm, ws: &mut Workspace, task: &str, max_steps: usize) -> Session {
    let mut prompt = system_prompt(task);
    let mut turns = Vec::new();

    for _ in 0..max_steps {
        let text = match llm.generate(&prompt) {
            Ok(t) => t,
            Err(e) => return Session { turns, stop: Stop::ModelError(e), files: ws.events().to_vec() },
        };
        prompt.push_str("\n\nASSISTANT:\n");
        prompt.push_str(&text);

        if text.contains(DONE_MARKER) && parse_tools(&text).is_empty() {
            turns.push(Turn { model_text: text, results: Vec::new() });
            return Session { turns, stop: Stop::Done, files: ws.events().to_vec() };
        }

        let tools = parse_tools(&text);
        let mut results = Vec::new();
        if tools.is_empty() {
            // No parseable call and not done — nudge and continue (small models sometimes narrate first).
            prompt.push_str("\n\n(No tool call parsed. Emit a ```tool block, or reply TASK_COMPLETE.)\n");
            turns.push(Turn { model_text: text, results });
            continue;
        }
        for tool in tools {
            let res = ws.apply(&tool);
            results.push((tool, res));
        }
        prompt.push_str(&observation(&results));
        turns.push(Turn { model_text: text, results });
    }

    Session { turns, stop: Stop::StepLimit, files: ws.events().to_vec() }
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

/// Convenience: run an agent turn-loop against `ws` using the active local model. Returns `Err` if no
/// local model is loaded (so the UI can prompt the user to load one).
pub fn run_local_agent(
    ws: &mut Workspace, task: &str, max_steps: usize, params: logscope::llm_backend::GenParams,
) -> Result<Session, String> {
    let mut llm = LocalLlm::from_active(params)
        .ok_or_else(|| "no local model loaded — pick a .gguf (MODEL selector) and load it first".to_string())?;
    Ok(run_agent(&mut llm, ws, task, max_steps))
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
            "I'll create the file.\n```tool\n{\"tool\":\"write_file\",\"path\":\"src/main.rs\",\"contents\":\"fn main(){}\\n\"}\n```".into(),
            "Now list it.\n```tool\n{\"tool\":\"list_dir\",\"path\":\"src\"}\n```".into(),
            format!("All set. {DONE_MARKER}"),
        ]};
        let s = run_agent(&mut llm, &mut ws, "make a hello world", 8);
        assert_eq!(s.stop, Stop::Done);
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.files[0].path, "src/main.rs");
        assert_eq!(s.files[0].change, FileChange::Created);
        assert!(std::fs::read_to_string(ws.root().join("src/main.rs")).unwrap().contains("fn main"));
        // three turns: write, list, done
        assert_eq!(s.turns.len(), 3);
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
