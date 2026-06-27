// src/workspaces.rs
//
// v0.1.282: per-speaker workspace foundation.
//
// A workspace is a named bundle of UI state belonging to one
// speaker. Today's app has a single implicit workspace (one
// terminal session, one browser tab); this module adds the
// structure so future ships can have multiple per speaker
// AND switching speakers swaps workspace context.
//
// Storage layout:
//   ~/.config/web2qml/workspaces/<speaker_id>/<name>.toml
//
// One file per workspace. The speaker_id is the stable id
// from speakers::Speaker, so renaming a speaker doesn't break
// workspace links. The name is user-given (defaults to
// "default" for the auto-created workspace).
//
// Contents of one workspace file:
//   - browser_tabs: list of URLs + active index
//   - terminal_sessions: list of cwds + recent commands (capped)
//   - name: display label
//   - created_ms / updated_ms: timestamps
//
// What this ship does NOT do:
//   - UI for multiple tabs (separate ship)
//   - Automatic context switch on speaker change (separate ship)
//   - Workspace name editing UI (separate ship)
//
// It DOES:
//   - Define the on-disk format
//   - Provide load/save/list APIs
//   - Auto-create a "default" workspace per speaker on first ref
//   - Capture current single-tab/single-terminal state into the
//     speaker's default workspace when called from the backend
//
// The point: future "multi-tab" / "switch-on-speaker-change"
// work has somewhere to read FROM and write TO. This is the
// structural foundation, not the user-visible feature.

use std::path::PathBuf;

const MAX_TABS_PER_WORKSPACE: usize = 20;
const MAX_TERMINAL_HISTORY_PER_SESSION: usize = 50;
const MAX_TERMINAL_SESSIONS: usize = 5;
const MAX_NAME_LEN: usize = 80;

/// One workspace's persisted state.
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    /// Display name. User-editable. Default: "default".
    pub name: String,
    /// URLs of open browser tabs in this workspace, in left-to-
    /// right order. First is leftmost. Empty = no browser
    /// state yet (will populate on first nav).
    pub browser_tabs: Vec<String>,
    /// Index into browser_tabs of the currently-active tab.
    /// 0 when there are tabs; ignored when browser_tabs is
    /// empty. Out-of-bounds reads clamp.
    pub active_browser_tab: usize,
    /// Terminal sessions in this workspace. Each holds cwd
    /// and recent commands. Capped at MAX_TERMINAL_SESSIONS.
    pub terminal_sessions: Vec<TerminalSession>,
    /// Index into terminal_sessions of the active one.
    pub active_terminal: usize,
    /// Unix ms when this workspace was created.
    pub created_ms: u64,
    /// Unix ms when this workspace was last touched.
    pub updated_ms: u64,
}

/// One terminal session's persisted state.
#[derive(Debug, Clone, Default)]
pub struct TerminalSession {
    /// Working directory at last save. Empty = unset.
    pub cwd: String,
    /// Recent commands (just text, not output). Most-recent-last.
    /// Capped at MAX_TERMINAL_HISTORY_PER_SESSION.
    pub recent_commands: Vec<String>,
}

impl Workspace {
    pub fn new(name: &str) -> Self {
        let now = now_ms();
        Self {
            name: name.chars().take(MAX_NAME_LEN).collect(),
            browser_tabs: Vec::new(),
            active_browser_tab: 0,
            terminal_sessions: Vec::new(),
            active_terminal: 0,
            created_ms: now,
            updated_ms: now,
        }
    }

    /// Add a browser tab URL. Bumps the active index to the
    /// new tab. Enforces MAX_TABS_PER_WORKSPACE by dropping
    /// the OLDEST (front) tab when full.
    pub fn push_tab(&mut self, url: &str) {
        if url.is_empty() { return; }
        self.browser_tabs.push(url.to_string());
        while self.browser_tabs.len() > MAX_TABS_PER_WORKSPACE {
            self.browser_tabs.remove(0);
        }
        self.active_browser_tab = self.browser_tabs.len().saturating_sub(1);
        self.updated_ms = now_ms();
    }

    /// Record a terminal command into the active session,
    /// creating a session if none exists.
    pub fn record_terminal_command(&mut self, cwd: &str, cmd: &str) {
        if cmd.is_empty() { return; }
        if self.terminal_sessions.is_empty() {
            self.terminal_sessions.push(TerminalSession::default());
        }
        let idx = self.active_terminal.min(self.terminal_sessions.len() - 1);
        let sess = &mut self.terminal_sessions[idx];
        if !cwd.is_empty() { sess.cwd = cwd.to_string(); }
        sess.recent_commands.push(cmd.to_string());
        while sess.recent_commands.len() > MAX_TERMINAL_HISTORY_PER_SESSION {
            sess.recent_commands.remove(0);
        }
        self.updated_ms = now_ms();
    }

    /// Serialize to TOML-ish line format. Hand-rolled because
    /// pulling in serde just for this would be overkill given
    /// the file's small size and our existing patterns
    /// elsewhere in the codebase (see speakers.rs).
    pub fn serialize(&self) -> String {
        let mut out = String::with_capacity(512 + self.browser_tabs.len() * 80);
        out.push_str(&format!("name = \"{}\"\n", escape_toml(&self.name)));
        out.push_str(&format!("created_ms = {}\n", self.created_ms));
        out.push_str(&format!("updated_ms = {}\n", self.updated_ms));
        out.push_str(&format!("active_browser_tab = {}\n", self.active_browser_tab));
        out.push_str(&format!("active_terminal = {}\n", self.active_terminal));
        out.push_str("browser_tabs = [\n");
        for url in &self.browser_tabs {
            out.push_str(&format!("    \"{}\",\n", escape_toml(url)));
        }
        out.push_str("]\n");

        for (i, sess) in self.terminal_sessions.iter().enumerate() {
            out.push_str(&format!("[[terminal_sessions]]\n"));
            out.push_str(&format!("# session {}\n", i));
            out.push_str(&format!("cwd = \"{}\"\n", escape_toml(&sess.cwd)));
            out.push_str("recent_commands = [\n");
            for cmd in &sess.recent_commands {
                out.push_str(&format!("    \"{}\",\n", escape_toml(cmd)));
            }
            out.push_str("]\n");
        }
        out
    }

    /// Parse from the serialize() format. Tolerant of missing
    /// fields and unknown keys.
    pub fn deserialize(text: &str) -> Self {
        let mut ws = Workspace::new("default");
        let mut current_section: Option<TerminalSession> = None;
        let mut in_browser_tabs = false;
        let mut in_recent = false;

        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') { continue; }

            if line == "[[terminal_sessions]]" {
                // Push any pending session, start new.
                if let Some(s) = current_section.take() {
                    ws.terminal_sessions.push(s);
                }
                current_section = Some(TerminalSession::default());
                in_browser_tabs = false;
                in_recent = false;
                continue;
            }
            if line == "browser_tabs = [" {
                in_browser_tabs = true;
                continue;
            }
            if line == "recent_commands = [" {
                in_recent = true;
                continue;
            }
            if line == "]" {
                in_browser_tabs = false;
                in_recent = false;
                continue;
            }

            // Array element line: "    \"<value>\",
            if in_browser_tabs || in_recent {
                if let Some(val) = parse_array_element(line) {
                    if in_browser_tabs {
                        ws.browser_tabs.push(val);
                    } else if let Some(s) = current_section.as_mut() {
                        s.recent_commands.push(val);
                    }
                }
                continue;
            }

            // Key = value line.
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim();
                let v_str = v.trim_matches('"');
                match (k, current_section.is_some()) {
                    ("name", false) => ws.name = unescape_toml(v_str),
                    ("created_ms", false) => ws.created_ms = v.parse().unwrap_or(0),
                    ("updated_ms", false) => ws.updated_ms = v.parse().unwrap_or(0),
                    ("active_browser_tab", false) => ws.active_browser_tab = v.parse().unwrap_or(0),
                    ("active_terminal", false) => ws.active_terminal = v.parse().unwrap_or(0),
                    ("cwd", true) => {
                        if let Some(s) = current_section.as_mut() {
                            s.cwd = unescape_toml(v_str);
                        }
                    }
                    _ => {}  // unknown key tolerance
                }
            }
        }
        if let Some(s) = current_section.take() {
            ws.terminal_sessions.push(s);
        }
        // Clamp counts.
        while ws.browser_tabs.len() > MAX_TABS_PER_WORKSPACE {
            ws.browser_tabs.remove(0);
        }
        while ws.terminal_sessions.len() > MAX_TERMINAL_SESSIONS {
            ws.terminal_sessions.remove(0);
        }
        ws
    }
}

/// Directory holding all workspaces for a speaker.
fn speaker_dir(speaker_id: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home)
        .join(".config/web2qml/workspaces")
        .join(sanitize_id(speaker_id)))
}

/// Sanitize a speaker id for filesystem use. Allow alphanum,
/// '-', '_'. Reject everything else to bytes.
fn sanitize_id(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect()
}

/// Sanitize a workspace name for filesystem use.
fn sanitize_name(s: &str) -> String {
    let mut out: String = s.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
    if out.is_empty() { out = "default".to_string(); }
    if out.len() > MAX_NAME_LEN { out.truncate(MAX_NAME_LEN); }
    out
}

/// Save a workspace to disk.
pub fn save(speaker_id: &str, ws: &Workspace) -> std::io::Result<()> {
    let dir = match speaker_dir(speaker_id) {
        Some(d) => d,
        None => return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no HOME")),
    };
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.toml", sanitize_name(&ws.name)));
    std::fs::write(&path, ws.serialize())
}

/// Load a workspace by name. Returns None if file doesn't
/// exist (caller should create a fresh one with Workspace::new).
pub fn load(speaker_id: &str, name: &str) -> Option<Workspace> {
    let dir = speaker_dir(speaker_id)?;
    let path = dir.join(format!("{}.toml", sanitize_name(name)));
    let text = std::fs::read_to_string(&path).ok()?;
    Some(Workspace::deserialize(&text))
}

/// List workspace names for a speaker. Empty if no directory.
pub fn list(speaker_id: &str) -> Vec<String> {
    let dir = match speaker_dir(speaker_id) {
        Some(d) => d,
        None => return Vec::new(),
    };
    let read = match std::fs::read_dir(&dir) { Ok(r) => r, Err(_) => return Vec::new() };
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if path.extension()?.to_str()? != "toml" { return None; }
            Some(path.file_stem()?.to_str()?.to_string())
        })
        .collect();
    names.sort();
    names
}

/// v0.1.389: list every speaker id that has at least one
/// workspace on disk. Reads the workspaces/ directory and
/// returns its sub-directory names. Used by
/// `web2qml://workspaces` to enumerate across all known
/// speakers.
///
/// Empty Vec when the workspaces directory doesn't exist
/// (first-launch case) or when no speakers have saved
/// workspaces yet. Order: alphabetical by sanitized id —
/// stable across calls, not a timestamp sort (the workspace
/// rows themselves carry their own updated_ms for that).
pub fn list_speakers() -> Vec<String> {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };
    let root = std::path::PathBuf::from(home)
        .join(".config/web2qml/workspaces");
    let read = match std::fs::read_dir(&root) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut ids: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if !path.is_dir() { return None; }
            // The directory name IS the (sanitized) speaker
            // id. We return it as-is — the caller can map
            // back to a display name via speakers.rs.
            Some(path.file_name()?.to_str()?.to_string())
        })
        .collect();
    ids.sort();
    ids
}

/// v0.1.301: delete a named workspace file. Refuses to delete
/// "default" (use load_or_create_default to reset it instead).
/// Returns true on success, false on any failure (not found,
/// I/O error, refused name).
pub fn delete(speaker_id: &str, name: &str) -> bool {
    if name.is_empty() || name == "default" { return false; }
    // Sanitization: same rules as save (no path separators,
    // no ".."). The path constructor enforces this; we just
    // sanity-check up-front to give a clean error path.
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        crate::clog!("workspaces", "delete refused unsafe name: {:?}", name);
        return false;
    }
    let dir = match speaker_dir(speaker_id) {
        Some(d) => d,
        None => return false,
    };
    let path = dir.join(format!("{}.toml", name));
    match std::fs::remove_file(&path) {
        Ok(()) => {
            crate::clog!("workspaces", "deleted {:?} for speaker {:?}", name, speaker_id);
            true
        }
        Err(e) => {
            crate::clog!("workspaces", "delete failed: {}", e);
            false
        }
    }
}

/// v0.1.310: rename a named workspace. Refuses to rename
/// "default" (it's an implicit name; renaming would orphan
/// the autosave path). Refuses to overwrite an existing
/// workspace at the new name. Returns true on success.
///
/// Implementation: load old, save under new, delete old.
/// Not atomic — if the delete fails after the save, both
/// names exist. That's a tolerable failure mode (no data
/// loss, just a duplicate).
pub fn rename(speaker_id: &str, old_name: &str, new_name: &str) -> bool {
    if old_name.is_empty() || new_name.is_empty() {
        crate::clog!("workspaces", "rename refused empty name");
        return false;
    }
    if old_name == "default" {
        crate::clog!("workspaces", "rename refused: cannot rename 'default'");
        return false;
    }
    if new_name == "default" {
        crate::clog!("workspaces", "rename refused: cannot rename TO 'default'");
        return false;
    }
    if new_name.contains('/') || new_name.contains('\\') || new_name.contains("..") {
        crate::clog!("workspaces", "rename refused unsafe target name: {:?}", new_name);
        return false;
    }
    if old_name == new_name {
        // No-op rename — treat as success but log.
        crate::clog!("workspaces", "rename no-op: old == new ({:?})", old_name);
        return true;
    }
    // Refuse if target already exists.
    if load(speaker_id, new_name).is_some() {
        crate::clog!("workspaces", "rename refused: target {:?} already exists", new_name);
        return false;
    }
    // Load old workspace.
    let mut ws = match load(speaker_id, old_name) {
        Some(w) => w,
        None => {
            crate::clog!("workspaces", "rename: source {:?} not found", old_name);
            return false;
        }
    };
    // Update the in-memory name so the file content reflects
    // the new name too (cosmetic but consistent).
    ws.name = new_name.to_string();
    if let Err(e) = save(speaker_id, &ws) {
        crate::clog!("workspaces", "rename: save to {:?} failed: {}", new_name, e);
        return false;
    }
    // Save was based on ws.name (now the new name). Delete
    // the old file.
    if !delete(speaker_id, old_name) {
        // Old file still exists; user has both. Best-effort.
        crate::clog!("workspaces", "rename: save succeeded but old delete failed; both names exist");
    }
    crate::clog!("workspaces", "renamed {:?} → {:?}", old_name, new_name);
    true
}

/// v0.1.313: copy a workspace under a new name. Source must
/// exist; target must not. Source can be "default" (unlike
/// rename), since the source isn't modified — only read.
/// Target cannot be "default" (would stomp the autosave).
/// Returns true on success.
pub fn copy(speaker_id: &str, src_name: &str, dst_name: &str) -> bool {
    if src_name.is_empty() || dst_name.is_empty() {
        crate::clog!("workspaces", "copy refused empty name");
        return false;
    }
    if dst_name == "default" {
        crate::clog!("workspaces", "copy refused: cannot copy TO 'default' (would stomp autosave)");
        return false;
    }
    if dst_name.contains('/') || dst_name.contains('\\') || dst_name.contains("..") {
        crate::clog!("workspaces", "copy refused unsafe target name: {:?}", dst_name);
        return false;
    }
    if src_name == dst_name {
        // No-op — copying to self.
        crate::clog!("workspaces", "copy no-op: src == dst ({:?})", src_name);
        return true;
    }
    if load(speaker_id, dst_name).is_some() {
        crate::clog!("workspaces", "copy refused: target {:?} already exists", dst_name);
        return false;
    }
    let mut ws = match load(speaker_id, src_name) {
        Some(w) => w,
        None => {
            crate::clog!("workspaces", "copy: source {:?} not found", src_name);
            return false;
        }
    };
    ws.name = dst_name.to_string();
    // Reset created_ms — this is a "new" workspace from the
    // user's perspective. Keep tabs, sessions, etc.
    ws.created_ms = now_ms();
    ws.updated_ms = ws.created_ms;
    if let Err(e) = save(speaker_id, &ws) {
        crate::clog!("workspaces", "copy: save to {:?} failed: {}", dst_name, e);
        return false;
    }
    crate::clog!("workspaces", "copied {:?} → {:?}", src_name, dst_name);
    true
}

/// Load OR create a "default" workspace for a speaker. Used
/// at startup as the entry point.
pub fn load_or_create_default(speaker_id: &str) -> Workspace {
    if let Some(ws) = load(speaker_id, "default") {
        return ws;
    }
    Workspace::new("default")
}

fn now_ms() -> u64 { core_kit::time::now_millis() }

fn escape_toml(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

fn unescape_toml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('n') => out.push('\n'),
                Some(other) => { out.push('\\'); out.push(other); }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_array_element(line: &str) -> Option<String> {
    // Expected: `"<value>",` with optional surrounding whitespace.
    let trimmed = line.trim();
    let trimmed = trimmed.trim_end_matches(',').trim();
    if !trimmed.starts_with('"') || !trimmed.ends_with('"') { return None; }
    let inner = &trimmed[1..trimmed.len() - 1];
    Some(unescape_toml(inner))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_empty() {
        let ws = Workspace::new("default");
        let serialized = ws.serialize();
        let parsed = Workspace::deserialize(&serialized);
        assert_eq!(parsed.name, "default");
        assert_eq!(parsed.browser_tabs.len(), 0);
        assert_eq!(parsed.terminal_sessions.len(), 0);
    }

    #[test]
    fn roundtrip_with_tabs() {
        let mut ws = Workspace::new("work");
        ws.push_tab("https://example.com");
        ws.push_tab("https://anthropic.com");
        let serialized = ws.serialize();
        let parsed = Workspace::deserialize(&serialized);
        assert_eq!(parsed.name, "work");
        assert_eq!(parsed.browser_tabs.len(), 2);
        assert_eq!(parsed.browser_tabs[0], "https://example.com");
        assert_eq!(parsed.browser_tabs[1], "https://anthropic.com");
        assert_eq!(parsed.active_browser_tab, 1);
    }

    #[test]
    fn roundtrip_with_terminal() {
        let mut ws = Workspace::new("dev");
        ws.record_terminal_command("/home/me/project", "cargo build");
        ws.record_terminal_command("/home/me/project", "cargo test");
        let serialized = ws.serialize();
        let parsed = Workspace::deserialize(&serialized);
        assert_eq!(parsed.terminal_sessions.len(), 1);
        assert_eq!(parsed.terminal_sessions[0].cwd, "/home/me/project");
        assert_eq!(parsed.terminal_sessions[0].recent_commands.len(), 2);
    }

    #[test]
    fn tab_cap_drops_oldest() {
        let mut ws = Workspace::new("flooded");
        for i in 0..(MAX_TABS_PER_WORKSPACE + 5) {
            ws.push_tab(&format!("https://example.com/{}", i));
        }
        assert_eq!(ws.browser_tabs.len(), MAX_TABS_PER_WORKSPACE);
        // First entries dropped; last entry should be the latest.
        assert!(ws.browser_tabs.last().unwrap().ends_with(&format!(
            "/{}", MAX_TABS_PER_WORKSPACE + 4
        )));
    }

    #[test]
    fn escape_handles_quotes_and_backslashes() {
        let s = r#"hello "world" \path"#;
        let escaped = escape_toml(s);
        let roundtrip = unescape_toml(&escaped);
        assert_eq!(roundtrip, s);
    }

    #[test]
    fn sanitize_name_handles_bad_chars() {
        assert_eq!(sanitize_name("my work"), "mywork");
        assert_eq!(sanitize_name("home/page"), "homepage");
        assert_eq!(sanitize_name(""), "default");
    }
}
