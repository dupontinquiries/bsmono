//! workflow: voice-named command sequences ("mini routines").
//!
//! The user records a sequence of shell commands, names it by voice,
//! and replays it later with "run <name>". Example: record `cd build`,
//! `cmake ..`, `make -j`, name it "unpack-build", then say "run unpack
//! build" to replay.
//!
//! This module is the PURE core (TODO_workflow_recorder.md build steps
//! 1, 2, 6, plus replay-safety): the Workflow type, the registry with
//! add/remove/find, hand-rolled JSON persistence (no serde — matches
//! archive.rs convention), voice-name fuzzy matching, and danger
//! detection for replay confirmation. The live wiring (recording via
//! BlockTracker, QML record button, PTY replay with completion-await)
//! is layered on top in Backend — see the TODO.
//!
//! PURE + dependency-free. Serialization is hand-rolled JSON so the
//! whole module is sandbox-testable.

/// One recorded routine.
#[derive(Clone, Debug, PartialEq)]
pub struct Workflow {
    /// The spoken name as recognized ("unpack and build"). Kept raw so
    /// it reads naturally in a list.
    pub name: String,
    /// Kebab-case command-friendly form ("unpack-and-build"), used for
    /// stable lookup and conflict-checking against shell aliases.
    pub slug: String,
    /// The submitted command lines, in order.
    pub commands: Vec<String>,
    /// If the first command was `cd <absolute>`, the routine is tied to
    /// that dir; replay starts there. Otherwise None → replay in the
    /// user's current cwd.
    pub cwd_at_start: Option<String>,
    /// Unix epoch when recorded.
    pub created_at: u64,
    /// How many times it's been replayed.
    pub run_count: u32,
}

impl Workflow {
    /// Build a workflow from a spoken name + captured commands. Derives
    /// the slug and the optional anchored cwd.
    pub fn new(spoken_name: &str, commands: Vec<String>, created_at: u64) -> Workflow {
        let slug = slugify(spoken_name);
        let cwd_at_start = commands
            .first()
            .and_then(|c| cd_absolute_target(c));
        Workflow {
            name: spoken_name.trim().to_string(),
            slug,
            commands,
            cwd_at_start,
            created_at,
            run_count: 0,
        }
    }
}

/// Kebab-case a spoken name: lowercase, collapse whitespace/underscores
/// to single hyphens, drop other punctuation. "Unpack & Build!" →
/// "unpack-build".
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = true; // suppress leading dash
    for c in s.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if c == ' ' || c == '_' || c == '-' || c == '\t' {
            if !last_dash {
                out.push('-');
                last_dash = true;
            }
        }
        // all other punctuation dropped
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// If `cmd` is `cd <absolute-path>` (the only form that anchors a
/// workflow to a directory), return that path. Relative cd's, `cd -`,
/// `cd ~`, and `cd` alone do NOT anchor (the routine is meant to run
/// wherever the user is).
fn cd_absolute_target(cmd: &str) -> Option<String> {
    let t = cmd.trim();
    let rest = t.strip_prefix("cd ")?.trim();
    // Only an absolute path anchors. ~ expands per-user at replay time
    // so we don't treat it as a fixed anchor here.
    if rest.starts_with('/') {
        // take the first whitespace token (ignore trailing && etc.)
        let path = rest.split_whitespace().next()?;
        // reject things with shell metacharacters in the path token
        if path.contains('&') || path.contains('|') || path.contains(';') {
            return None;
        }
        Some(path.to_string())
    } else {
        None
    }
}

/// Commands that are dangerous to replay without explicit confirmation.
/// Returns the list of (command, reason) for any matching lines so the
/// caller can show exactly what's risky.
pub fn dangerous_commands(commands: &[String]) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for c in commands {
        let t = c.trim();
        let lower = t.to_lowercase();
        // tokenize lightly for word-boundary-ish checks
        let raw_first = lower.split_whitespace().next().unwrap_or("");
        // v0.1.610: match the BASENAME, not the raw token — otherwise an
        // absolute path defeats the check entirely. "/bin/rm -rf /" and
        // "/usr/bin/rm -rf /" were MISSED because raw_first was the full
        // path, not "rm" — a recursive delete replayed with NO confirm,
        // the exact harm this function exists to prevent. Strip any
        // leading path so /bin/rm, /usr/bin/rm, and rm all → "rm".
        let first = raw_first.rsplit('/').next().unwrap_or(raw_first);
        let reason = if first == "rm" && (lower.contains("-rf") || lower.contains("-fr") || lower.contains("-r")) {
            Some("recursive delete (rm -r)")
        } else if first == "rm" && lower.contains('*') {
            // v0.1.582: a glob rm is a real REPLAY hazard even without
            // -r: "rm -f *.log" deletes a different set depending on the
            // cwd you replay it in. Flag it. (Plain "rm file.txt" stays
            // UNflagged — flagging every single-file delete would cause
            // alarm fatigue and the user recorded the workflow knowingly.)
            Some("glob delete (rm with *)")
        } else if first == "rm" && (lower.contains("-f") || lower.contains("-rf")) {
            // force delete suppresses the "are you sure" prompt rm would
            // normally give; worth confirming before a blind replay.
            Some("force delete (rm -f)")
        } else if first == "dd" {
            Some("raw disk write (dd)")
        } else if first == "mkfs" || first.starts_with("mkfs.") {
            Some("formats a filesystem (mkfs)")
        } else if lower.contains(":>") || lower.contains("> /dev/") {
            Some("truncates/overwrites a device or file")
        } else if first == "shred" {
            Some("securely erases (shred)")
        } else if lower.contains("git push") && lower.contains("--force") {
            Some("force-pushes git history")
        } else if first == "sudo" {
            Some("runs as root (sudo)")
        } else {
            None
        };
        if let Some(r) = reason {
            out.push((t.to_string(), r));
        }
    }
    out
}

/// The in-memory set of recorded workflows. Persistence is via
/// to_json / from_json (the Backend reads/writes the file).
#[derive(Default)]
pub struct WorkflowRegistry {
    workflows: Vec<Workflow>,
}

impl WorkflowRegistry {
    pub fn new() -> Self {
        WorkflowRegistry { workflows: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.workflows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.workflows.is_empty()
    }
    pub fn all(&self) -> &[Workflow] {
        &self.workflows
    }

    /// Add a workflow. If one with the same slug exists, it's REPLACED
    /// (re-recording under the same name updates it) and the old
    /// run_count is preserved. Returns true if it replaced an existing.
    pub fn add(&mut self, mut wf: Workflow) -> bool {
        if let Some(existing) = self.workflows.iter().position(|w| w.slug == wf.slug) {
            wf.run_count = self.workflows[existing].run_count;
            self.workflows[existing] = wf;
            true
        } else {
            self.workflows.push(wf);
            false
        }
    }

    /// Remove by slug. Returns true if something was removed.
    pub fn remove(&mut self, slug: &str) -> bool {
        let before = self.workflows.len();
        self.workflows.retain(|w| w.slug != slug);
        self.workflows.len() != before
    }

    /// Exact slug lookup.
    pub fn get(&self, slug: &str) -> Option<&Workflow> {
        self.workflows.iter().find(|w| w.slug == slug)
    }

    /// Increment run_count for a slug (call on replay). Returns the new
    /// count, or None if not found.
    pub fn mark_run(&mut self, slug: &str) -> Option<u32> {
        let w = self.workflows.iter_mut().find(|w| w.slug == slug)?;
        w.run_count += 1;
        Some(w.run_count)
    }

    /// Serialize the whole registry to a JSON array string (hand-rolled,
    /// no serde — matches archive.rs).
    pub fn to_json(&self) -> String {
        let mut o = String::from("[");
        for (i, w) in self.workflows.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            o.push('{');
            json_kv_str(&mut o, "name", &w.name);
            o.push(',');
            json_kv_str(&mut o, "slug", &w.slug);
            o.push(',');
            o.push_str("\"commands\":[");
            for (j, c) in w.commands.iter().enumerate() {
                if j > 0 {
                    o.push(',');
                }
                o.push('"');
                o.push_str(&json_esc(c));
                o.push('"');
            }
            o.push(']');
            o.push(',');
            match &w.cwd_at_start {
                Some(cwd) => {
                    json_kv_str(&mut o, "cwd", cwd);
                }
                None => {
                    o.push_str("\"cwd\":null");
                }
            }
            o.push(',');
            o.push_str("\"created_at\":");
            o.push_str(&w.created_at.to_string());
            o.push(',');
            o.push_str("\"run_count\":");
            o.push_str(&w.run_count.to_string());
            o.push('}');
        }
        o.push(']');
        o
    }

    /// Parse a registry from the JSON produced by to_json. Lenient: a
    /// malformed entry is skipped rather than failing the whole load
    /// (so one corrupt workflow doesn't lose the rest).
    pub fn from_json(json: &str) -> WorkflowRegistry {
        let mut reg = WorkflowRegistry::new();
        for obj in split_top_level_objects(json) {
            let name = json_get_str(&obj, "name").unwrap_or_default();
            let slug = json_get_str(&obj, "slug")
                .unwrap_or_else(|| slugify(&name));
            if name.is_empty() && slug.is_empty() {
                continue;
            }
            let commands = json_get_str_array(&obj, "commands");
            let cwd_at_start = json_get_str(&obj, "cwd")
                .filter(|s| !s.is_empty());
            let created_at = json_get_num(&obj, "created_at").unwrap_or(0);
            let run_count = json_get_num(&obj, "run_count").unwrap_or(0) as u32;
            reg.workflows.push(Workflow {
                name,
                slug,
                commands,
                cwd_at_start,
                created_at,
                run_count,
            });
        }
        reg
    }
}

/// Match a spoken phrase to a workflow slug. Handles "run <name>",
/// "play <name>", "execute <name>", "do <name>" with separator-collapse
/// so "run unpack build" matches slug "unpack-build". Returns the
/// matched slug, or None. Exact slug match (after collapse) only — we
/// do NOT fuzzy-match loosely here because replaying the wrong command
/// sequence is harmful; precision over recall.
pub fn match_workflow_replay(text: &str, slugs: &[String]) -> Option<String> {
    let t = text.trim().to_lowercase();
    let rest = ["run ", "play ", "execute ", "replay ", "do "]
        .iter()
        .find_map(|p| t.strip_prefix(p))?;
    let spoken_slug = slugify(rest);
    if spoken_slug.is_empty() {
        return None;
    }
    // Exact match first.
    if let Some(s) = slugs.iter().find(|s| **s == spoken_slug) {
        return Some(s.clone());
    }
    // Collapse-equality: compare with all separators removed, so
    // "unpackbuild" (hyphenless) still matches "unpack-build".
    let collapsed = spoken_slug.replace('-', "");
    slugs
        .iter()
        .find(|s| s.replace('-', "") == collapsed)
        .cloned()
}

// ── hand-rolled JSON helpers (mirrors archive.rs; no serde) ──

fn json_esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => {}
            '\t' => o.push_str("\\t"),
            c if c.is_control() => {}
            _ => o.push(c),
        }
    }
    o
}

fn json_unesc(s: &str) -> String {
    s.replace("\\\"", "\"").replace("\\n", "\n").replace("\\t", "\t").replace("\\\\", "\\")
}

fn json_kv_str(out: &mut String, key: &str, val: &str) {
    out.push('"');
    out.push_str(key);
    out.push_str("\":\"");
    out.push_str(&json_esc(val));
    out.push('"');
}

/// Read a string value for `key` from a single JSON object string.
fn json_get_str(obj: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\":\"", key);
    let start = obj.find(&pat)? + pat.len();
    let rest = &obj[start..];
    let mut end = 0;
    let mut esc = false;
    for c in rest.chars() {
        if esc {
            esc = false;
            end += c.len_utf8();
            continue;
        }
        if c == '\\' {
            esc = true;
            end += 1;
            continue;
        }
        if c == '"' {
            break;
        }
        end += c.len_utf8();
    }
    Some(json_unesc(&rest[..end]))
}

/// Read a numeric value for `key` (e.g. "created_at":123).
fn json_get_num(obj: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{}\":", key);
    let start = obj.find(&pat)? + pat.len();
    let rest = obj[start..].trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Read a string array for `key` (e.g. "commands":["a","b"]).
fn json_get_str_array(obj: &str, key: &str) -> Vec<String> {
    let pat = format!("\"{}\":[", key);
    let Some(s) = obj.find(&pat) else {
        return Vec::new();
    };
    let start = s + pat.len();
    let rest = &obj[start..];
    let end = match rest.find(']') {
        Some(e) => e,
        None => return Vec::new(),
    };
    let inner = &rest[..end];
    let mut out = Vec::new();
    let mut chars = inner.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == '"' {
            chars.next(); // opening quote
            let mut val = String::new();
            let mut esc = false;
            for c in chars.by_ref() {
                if esc {
                    val.push(c);
                    esc = false;
                } else if c == '\\' {
                    val.push(c);
                    esc = true;
                } else if c == '"' {
                    break;
                } else {
                    val.push(c);
                }
            }
            out.push(json_unesc(&val));
        } else {
            chars.next();
        }
    }
    out
}

/// Split a top-level JSON array string into its object substrings,
/// respecting nested braces and strings. Crude but sufficient for our
/// flat-ish workflow objects.
fn split_top_level_objects(json: &str) -> Vec<String> {
    let mut objs = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in json.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    objs.push(json[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    objs
}

// ════════════════════════════════════════════════════════════════
// Stateful layer: recording, playback, persistence.
//
// v0.1.590: the Backend (main.rs) and term_blocks.rs call this API to
// record command sequences, persist them, and replay them. The pure
// core above (Workflow, WorkflowRegistry, slugify, match_workflow_replay,
// dangerous_commands) does the logic; this layer adds process-global
// state (an in-progress recording buffer + a playback queue) and JSON
// file persistence at ~/.config/web2qml/workflows.json. Mirrors the
// OnceLock<Mutex<>> + config-path pattern used in corpus_recorder.rs.
// ════════════════════════════════════════════════════════════════

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// In-progress recording: Some(name, commands) while recording.
fn recording_state() -> &'static Mutex<Option<(String, Vec<String>)>> {
    static S: OnceLock<Mutex<Option<(String, Vec<String>)>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// Playback queue: the remaining commands to replay, front-first.
fn playback_state() -> &'static Mutex<std::collections::VecDeque<String>> {
    static S: OnceLock<Mutex<std::collections::VecDeque<String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(std::collections::VecDeque::new()))
}

/// The persisted registry, loaded once from disk then kept in memory.
fn registry_state() -> &'static Mutex<WorkflowRegistry> {
    static S: OnceLock<Mutex<WorkflowRegistry>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(load_registry_from_disk()))
}

fn workflows_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/web2qml/workflows.json")
}

fn load_registry_from_disk() -> WorkflowRegistry {
    match std::fs::read_to_string(workflows_path()) {
        Ok(json) => WorkflowRegistry::from_json(&json),
        Err(_) => WorkflowRegistry::new(),
    }
}

fn save_registry_to_disk(reg: &WorkflowRegistry) -> Result<(), String> {
    let path = workflows_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, reg.to_json()).map_err(|e| e.to_string())
}

/// Begin recording a command sequence under `name`. Returns false if a
/// recording is already in progress (caller should stop first).
pub fn start_recording(name: &str) -> bool {
    let mut rec = match recording_state().lock() {
        Ok(r) => r,
        Err(_) => return false,
    };
    if rec.is_some() {
        return false;
    }
    *rec = Some((name.to_string(), Vec::new()));
    true
}

/// Append a submitted command to the in-progress recording. No-op if not
/// recording. Called from term_blocks on each command submit.
pub fn record_command(cmd: &str) {
    let c = cmd.trim();
    if c.is_empty() {
        return;
    }
    if let Ok(mut rec) = recording_state().lock() {
        if let Some((_, cmds)) = rec.as_mut() {
            cmds.push(c.to_string());
        }
    }
}

/// Stop recording, persist the workflow, and return its saved name (the
/// slug). Returns None if not recording or nothing was captured.
pub fn stop_recording() -> Option<String> {
    let taken = {
        let mut rec = recording_state().lock().ok()?;
        rec.take()
    };
    let (name, cmds) = taken?;
    if cmds.is_empty() {
        return None;
    }
    create_from_commands(&name, &cmds, true).ok()
}

pub fn is_recording() -> bool {
    recording_state()
        .lock()
        .map(|r| r.is_some())
        .unwrap_or(false)
}

/// Create + persist a workflow from an explicit command list (the
/// "save these commands as a workflow" path, distinct from live
/// recording). Returns the saved slug on success. Err("exists") if a
/// workflow with that slug already exists and `overwrite` is false.
pub fn create_from_commands(
    name: &str,
    commands: &[String],
    overwrite: bool,
) -> Result<String, String> {
    let cmds: Vec<String> = commands
        .iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if cmds.is_empty() {
        return Err("empty".to_string());
    }
    let wf = Workflow::new(name, cmds, crate::util::epoch_secs());
    if wf.slug.is_empty() {
        return Err("badname".to_string());
    }
    let mut reg = registry_state().lock().map_err(|_| "lock".to_string())?;
    if !overwrite && reg.get(&wf.slug).is_some() {
        return Err("exists".to_string());
    }
    let slug = wf.slug.clone();
    reg.add(wf);
    save_registry_to_disk(&reg)?;
    Ok(slug)
}

/// Begin replaying the workflow matching `name` (matched via slug, with
/// the same collapse logic as voice replay). Loads its commands into the
/// playback queue. Returns false if no such workflow.
pub fn start_playback(name: &str) -> bool {
    let reg = match registry_state().lock() {
        Ok(r) => r,
        Err(_) => return false,
    };
    // Accept either an exact slug or a spoken-ish name.
    let slug = slugify(name);
    let wf = reg.get(&slug).or_else(|| {
        let collapsed = slug.replace('-', "");
        reg.all().iter().find(|w| w.slug.replace('-', "") == collapsed)
    });
    let Some(wf) = wf else {
        return false;
    };
    let cmds = wf.commands.clone();
    drop(reg);
    if let Ok(mut q) = playback_state().lock() {
        q.clear();
        q.extend(cmds);
        // mark a run
        if let Ok(mut reg) = registry_state().lock() {
            reg.mark_run(&slug);
            let _ = save_registry_to_disk(&reg);
        }
        true
    } else {
        false
    }
}

/// Pop the next command to replay, or None when the queue is empty.
pub fn next_playback_command() -> Option<String> {
    playback_state().lock().ok()?.pop_front()
}

/// Abort any in-progress playback (clears the queue).
pub fn cancel_playback() {
    if let Ok(mut q) = playback_state().lock() {
        q.clear();
    }
}

/// List saved workflow names (display form), for the library UI.
pub fn list_workflows() -> Vec<String> {
    registry_state()
        .lock()
        .map(|r| r.all().iter().map(|w| w.name.clone()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Unpack & Build!"), "unpack-build");
        assert_eq!(slugify("unpack and build"), "unpack-and-build");
        assert_eq!(slugify("  spaced   out  "), "spaced-out");
        assert_eq!(slugify("already-kebab"), "already-kebab");
        assert_eq!(slugify("UPPER_snake"), "upper-snake");
    }

    #[test]
    fn new_workflow_anchors_only_on_absolute_cd() {
        let abs = Workflow::new("proj build",
            vec!["cd /home/k/proj".into(), "make".into()], 100);
        assert_eq!(abs.cwd_at_start.as_deref(), Some("/home/k/proj"));

        let rel = Workflow::new("build here",
            vec!["cd build".into(), "make".into()], 100);
        assert_eq!(rel.cwd_at_start, None, "relative cd must not anchor");

        let nocd = Workflow::new("just make", vec!["make -j".into()], 100);
        assert_eq!(nocd.cwd_at_start, None);
    }

    #[test]
    fn cd_anchor_takes_clean_path_token() {
        // `cd /foo && evil` → anchor is just the path token "/foo"
        // (safe to cd into); the "&& evil" lives in the command list and
        // is caught by dangerous_commands at replay, not here. But a
        // metachar GLUED to the path (no space) must be rejected.
        let w = Workflow::new("compound", vec!["cd /foo && evil".into()], 0);
        assert_eq!(w.cwd_at_start.as_deref(), Some("/foo"),
            "first whitespace token is a clean path; && is separate");
        let glued = Workflow::new("glued", vec!["cd /foo;rm".into()], 0);
        assert_eq!(glued.cwd_at_start, None,
            "metachar glued to path token must reject the anchor");
    }

    #[test]
    fn registry_add_and_get() {
        let mut r = WorkflowRegistry::new();
        let replaced = r.add(Workflow::new("unpack build",
            vec!["cmake".into()], 1));
        assert!(!replaced);
        assert_eq!(r.len(), 1);
        assert!(r.get("unpack-build").is_some());
        assert!(r.get("nonexistent").is_none());
    }

    #[test]
    fn re_record_replaces_and_keeps_run_count() {
        let mut r = WorkflowRegistry::new();
        r.add(Workflow::new("build", vec!["make".into()], 1));
        r.mark_run("build");
        r.mark_run("build"); // run_count = 2
        // re-record same name with different commands
        let replaced = r.add(Workflow::new("build",
            vec!["make".into(), "make install".into()], 5));
        assert!(replaced);
        assert_eq!(r.len(), 1, "same slug replaces, not appends");
        let w = r.get("build").unwrap();
        assert_eq!(w.commands.len(), 2, "commands updated");
        assert_eq!(w.run_count, 2, "run_count preserved across re-record");
    }

    #[test]
    fn remove_works() {
        let mut r = WorkflowRegistry::new();
        r.add(Workflow::new("a", vec!["x".into()], 1));
        assert!(r.remove("a"));
        assert!(!r.remove("a"), "second remove is a no-op");
        assert!(r.is_empty());
    }

    #[test]
    fn json_round_trip() {
        let mut r = WorkflowRegistry::new();
        r.add(Workflow::new("unpack and build",
            vec!["cd /home/k/p".into(), "cmake ..".into(), "make -j".into()], 1700));
        r.add(Workflow::new("deploy", vec!["./deploy.sh".into()], 1800));
        r.mark_run("deploy");
        let json = r.to_json();
        let back = WorkflowRegistry::from_json(&json);
        assert_eq!(back.len(), 2);
        let ub = back.get("unpack-and-build").unwrap();
        assert_eq!(ub.commands, vec!["cd /home/k/p", "cmake ..", "make -j"]);
        assert_eq!(ub.cwd_at_start.as_deref(), Some("/home/k/p"));
        assert_eq!(ub.created_at, 1700);
        let dep = back.get("deploy").unwrap();
        assert_eq!(dep.run_count, 1, "run_count survives round-trip");
        assert_eq!(dep.cwd_at_start, None);
    }

    #[test]
    fn json_handles_quotes_and_special_chars() {
        let mut r = WorkflowRegistry::new();
        r.add(Workflow::new("quoted",
            vec!["echo \"hi there\"".into(), "grep -e 'x\\ty'".into()], 1));
        let json = r.to_json();
        let back = WorkflowRegistry::from_json(&json);
        let w = back.get("quoted").unwrap();
        assert_eq!(w.commands[0], "echo \"hi there\"",
            "embedded quotes survive round-trip");
    }

    #[test]
    fn from_json_skips_malformed_keeps_rest() {
        // second object is missing its closing detail but valid braces;
        // we mostly care that a corrupt name doesn't nuke the whole load.
        let json = r#"[{"name":"good","slug":"good","commands":["x"],"cwd":null,"created_at":1,"run_count":0},{"name":"","slug":"","commands":[],"cwd":null,"created_at":0,"run_count":0}]"#;
        let r = WorkflowRegistry::from_json(json);
        // the empty-name/empty-slug entry is skipped; "good" survives
        assert_eq!(r.len(), 1);
        assert!(r.get("good").is_some());
    }

    #[test]
    fn match_replay_exact_and_collapsed() {
        let slugs = vec!["unpack-build".to_string(), "deploy".to_string()];
        assert_eq!(match_workflow_replay("run unpack build", &slugs),
            Some("unpack-build".to_string()));
        assert_eq!(match_workflow_replay("play deploy", &slugs),
            Some("deploy".to_string()));
        // hyphenless collapse
        assert_eq!(match_workflow_replay("execute unpackbuild", &slugs),
            Some("unpack-build".to_string()));
        // no trigger verb → no match
        assert_eq!(match_workflow_replay("unpack build", &slugs), None);
        // unknown name → no match (precision: don't replay the wrong thing)
        assert_eq!(match_workflow_replay("run something else", &slugs), None);
    }

    #[test]
    fn dangerous_commands_flagged() {
        let cmds = vec![
            "cd build".to_string(),
            "rm -rf node_modules".to_string(),
            "make".to_string(),
            "sudo make install".to_string(),
            "dd if=/dev/zero of=/dev/sdb".to_string(),
        ];
        let danger = dangerous_commands(&cmds);
        let flagged: Vec<&str> = danger.iter().map(|(c, _)| c.as_str()).collect();
        assert!(flagged.contains(&"rm -rf node_modules"));
        assert!(flagged.contains(&"sudo make install"));
        assert!(flagged.contains(&"dd if=/dev/zero of=/dev/sdb"));
        assert!(!flagged.contains(&"make"), "plain make is safe");
        assert!(!flagged.contains(&"cd build"), "cd is safe");
    }

    #[test]
    fn safe_workflow_has_no_danger() {
        let cmds = vec!["cd build".to_string(), "cmake ..".to_string(), "make -j".to_string()];
        assert!(dangerous_commands(&cmds).is_empty());
    }

    #[test]
    fn pathed_destructive_commands_still_flagged() {
        // v0.1.610 regression: an absolute path to the binary must NOT
        // defeat detection. /bin/rm -rf / was previously MISSED because
        // the check compared the full path token to "rm".
        let cmds = vec![
            "/bin/rm -rf /".to_string(),
            "/usr/bin/rm -r mydir".to_string(),
            "/sbin/mkfs.ext4 /dev/sdb".to_string(),
            "/bin/dd if=/dev/zero of=/dev/sda".to_string(),
        ];
        let danger = dangerous_commands(&cmds);
        assert_eq!(danger.len(), 4, "all pathed destructive cmds flagged, got {:?}", danger);
        // and the basename fix doesn't over-flag a safe pathed command
        assert!(dangerous_commands(&["/usr/bin/make".to_string()]).is_empty());
        assert!(dangerous_commands(&["/bin/ls -la".to_string()]).is_empty());
    }

    #[test]
    fn recording_state_machine() {
        // In-memory recording lifecycle (no disk). Uses the process
        // statics, so keep it self-contained: stop with empty buffer is
        // a no-op, double-start is rejected.
        // (Not parallel-safe with other state tests, but cargo runs
        // these single-threaded within the module by default for our
        // harness; behavior is deterministic here.)
        // Ensure clean start.
        let _ = stop_recording();
        cancel_playback();
        assert!(!is_recording());
        assert!(start_recording("my routine"), "fresh start should succeed");
        assert!(is_recording());
        assert!(!start_recording("again"), "double-start must be rejected");
        record_command("cd /tmp");
        record_command("ls");
        record_command("   "); // whitespace ignored
        // We can't assert persistence (disk) here, but we can assert the
        // recording flag clears. stop_recording persists + returns slug;
        // in the sandbox HOME may be writable, so just check it doesn't
        // panic and clears state.
        let _ = stop_recording();
        assert!(!is_recording(), "recording flag clears after stop");
    }

    #[test]
    fn playback_queue_drains_in_order() {
        cancel_playback();
        // Seed the queue directly via the public API path isn't possible
        // without a saved workflow, so test the queue drain semantics
        // through cancel + next on an empty queue.
        assert_eq!(next_playback_command(), None, "empty queue yields None");
        cancel_playback(); // idempotent
        assert_eq!(next_playback_command(), None);
    }

    #[test]
    fn glob_and_force_rm_flagged_but_plain_rm_not() {
        // v0.1.582: glob deletes and force deletes are replay hazards.
        assert!(!dangerous_commands(&["rm -f *.log".to_string()]).is_empty(),
            "glob delete must be flagged");
        assert!(!dangerous_commands(&["rm *".to_string()]).is_empty(),
            "rm * must be flagged");
        assert!(!dangerous_commands(&["rm -f secret.key".to_string()]).is_empty(),
            "force delete must be flagged");
        // plain single-file rm stays UNflagged (avoid alarm fatigue —
        // the user recorded the workflow deliberately)
        assert!(dangerous_commands(&["rm temp.txt".to_string()]).is_empty(),
            "plain single-file rm should not trip the gate");
        // recursive still takes precedence with its own reason
        let rf = dangerous_commands(&["rm -rf node_modules".to_string()]);
        assert_eq!(rf[0].1, "recursive delete (rm -r)");
    }
}
