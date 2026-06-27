//! chatdump.rs — save/reload a conversation as a `logscope/chats/<id>/` dump inside a crate workspace.
//!
//! A dump is a chat (its turns + any attached/derived files) written into a project so it can be committed
//! alongside code, reloaded later, and have its files staged into the bubblewrap /tmp area for the sandboxed
//! tools (`summarize files`, verify, etc.). One directory in a directory of many workspaces is fine — the
//! importer walks for `logscope/chats/<id>/chat.json` at any depth and respects each project's folder layout.
//!
//! Layout under a workspace root:
//!   <root>/logscope/chats/<id>/chat.json          — the conversation + a manifest of its files
//!   <root>/logscope/chats/<id>/files/<rel path>   — the attached/derived file contents (paths sanitized)
//!
//! Path safety: file paths run through `workspace::safe_rel`, so a dump can never read or write outside its
//! own folder (absolute paths and `..` traversal are rejected).

use serde::{Deserialize, Serialize};

/// Sanitize a model/user-supplied relative path: reject absolute paths and `..` traversal so a dump can
/// never read or write outside its own folder. Mirrors `workspace::safe_rel` but keeps this module
/// self-contained (no gui-feature dependency, so the CLI can dump/import too).
fn safe_rel(path: &str) -> Option<String> {
    let p = Path::new(path);
    if p.is_absolute() { return None; }
    let mut clean = PathBuf::new();
    for comp in p.components() {
        use std::path::Component::*;
        match comp {
            Normal(c) => clean.push(c),
            CurDir => {}
            ParentDir | RootDir | Prefix(_) => return None,
        }
    }
    let s = clean.to_string_lossy().to_string();
    if s.is_empty() { None } else { Some(s) }
}
use std::path::{Path, PathBuf};
use std::{fs, io};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct DumpTurn { pub role: String, pub text: String }

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct DumpFile { pub path: String, pub contents: String }

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct ChatDump {
    pub id: String,
    pub title: String,
    pub created: u64, // unix seconds
    pub turns: Vec<DumpTurn>,
    pub files: Vec<DumpFile>,
}

impl ChatDump {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        ChatDump { id: id.into(), title: title.into(), created: now_secs(), turns: Vec::new(), files: Vec::new() }
    }

    /// Build a dump from a linear conversation (role, text) — the active lineage.
    pub fn from_turns(id: impl Into<String>, title: impl Into<String>, turns: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut d = ChatDump::new(id, title);
        d.turns = turns.into_iter().map(|(role, text)| DumpTurn { role, text }).collect();
        d
    }

    pub fn add_file(&mut self, path: impl Into<String>, contents: impl Into<String>) {
        self.files.push(DumpFile { path: path.into(), contents: contents.into() });
    }

    /// `<root>/logscope/chats` — the dumps directory inside a workspace.
    pub fn chats_dir(root: &Path) -> PathBuf { root.join("logscope").join("chats") }
    /// The directory this dump occupies under `root`.
    pub fn dir(&self, root: &Path) -> PathBuf { Self::chats_dir(root).join(&self.id) }

    /// Write `chat.json` + `files/<rel>` under `<root>/logscope/chats/<id>/`. Returns the dump directory.
    pub fn export_to_workspace(&self, root: &Path) -> io::Result<PathBuf> {
        let dir = self.dir(root);
        fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        atomic_write(&dir.join("chat.json"), json.as_bytes())?;
        let files_dir = dir.join("files");
        for f in &self.files {
            if let Some(rel) = safe_rel(&f.path) {
                let p = files_dir.join(&rel);
                if let Some(parent) = p.parent() { fs::create_dir_all(parent)?; }
                atomic_write(&p, f.contents.as_bytes())?;
            }
        }
        Ok(dir)
    }

    /// Load a dump from its directory (`chat.json`).
    pub fn load(dir: &Path) -> io::Result<ChatDump> {
        let json = fs::read_to_string(dir.join("chat.json"))?;
        serde_json::from_str(&json).map_err(io::Error::other)
    }

    /// List the dump ids found under `<root>/logscope/chats` (sorted).
    pub fn list(root: &Path) -> Vec<String> {
        let mut ids = Vec::new();
        if let Ok(rd) = fs::read_dir(Self::chats_dir(root)) {
            for e in rd.flatten() {
                if e.path().join("chat.json").exists() {
                    if let Some(name) = e.file_name().to_str() { ids.push(name.to_string()); }
                }
            }
        }
        ids.sort();
        ids
    }

    /// Materialize this dump's files into `scratch` (e.g. the bubblewrap /tmp mirror) so the sandboxed tools
    /// can read them. Project folder structure is preserved (sanitized). Returns how many files were written.
    pub fn stage_files(&self, scratch: &Path) -> io::Result<usize> {
        let mut n = 0;
        for f in &self.files {
            if let Some(rel) = safe_rel(&f.path) {
                let p = scratch.join(&rel);
                if let Some(parent) = p.parent() { fs::create_dir_all(parent)?; }
                fs::write(&p, f.contents.as_bytes())?;
                n += 1;
            }
        }
        Ok(n)
    }
}

/// Import every chat dump found under `dir` — a directory that may hold one or many workspaces. The walk
/// looks for `logscope/chats/<id>/chat.json` at any depth, so project folder structure is respected and you
/// can point it at a parent folder of several checked-out projects.
pub fn import_dir(dir: &Path) -> Vec<ChatDump> {
    let mut out = Vec::new();
    visit(dir, 0, &mut out);
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn visit(dir: &Path, depth: usize, out: &mut Vec<ChatDump>) {
    if depth > 8 { return; } // bound the walk so a huge tree can't run away
    let Ok(rd) = fs::read_dir(dir) else { return; };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() { continue; }
        let is_chats = p.file_name().and_then(|n| n.to_str()) == Some("chats")
            && p.parent().and_then(|q| q.file_name()).and_then(|n| n.to_str()) == Some("logscope");
        if is_chats {
            if let Ok(sub) = fs::read_dir(&p) {
                for d in sub.flatten() {
                    if d.path().join("chat.json").exists() {
                        if let Ok(c) = ChatDump::load(&d.path()) { out.push(c); }
                    }
                }
            }
        } else if p.file_name().and_then(|n| n.to_str()) != Some("target") {
            visit(&p, depth + 1, out); // skip build dirs
        }
    }
}

fn now_secs() -> u64 {
        core_kit::time::now_secs()
    }

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("writing.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("logscope_chatdump_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn round_trips_through_a_workspace() {
        let root = scratch("rt");
        let mut d = ChatDump::from_turns("abc123", "fix the parser", [
            ("user".to_string(), "why does the parser drop the last token?".to_string()),
            ("assistant".to_string(), "because the loop exits early; here's the fix".to_string()),
        ]);
        d.add_file("src/lib.rs", "pub fn x() {}\n");
        d.add_file("notes/todo.md", "- [ ] write tests\n");

        let dir = d.export_to_workspace(&root).unwrap();
        assert!(dir.join("chat.json").exists());
        assert!(dir.join("files/src/lib.rs").exists(), "nested file written under files/");
        assert!(dir.join("files/notes/todo.md").exists());

        let back = ChatDump::load(&dir).unwrap();
        assert_eq!(back, d, "dump round-trips exactly");
        assert_eq!(ChatDump::list(&root), vec!["abc123".to_string()]);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_path_traversal_in_files() {
        let root = scratch("safe");
        let mut d = ChatDump::new("evil", "escape attempt");
        d.add_file("../../etc/passwd", "root:x:0:0");
        d.add_file("/abs/secret", "nope");
        d.add_file("ok/here.txt", "fine");
        let dir = d.export_to_workspace(&root).unwrap();
        // the unsafe paths are skipped; only the safe one lands, and strictly under files/
        assert!(dir.join("files/ok/here.txt").exists());
        assert!(!root.join("etc/passwd").exists() && !root.parent().map(|p| p.join("etc/passwd").exists()).unwrap_or(false));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stages_files_into_a_scratch_mirror() {
        let mut d = ChatDump::new("stg", "stage");
        d.add_file("src/main.rs", "fn main() {}\n");
        let mirror = scratch("mirror");
        let n = d.stage_files(&mirror).unwrap();
        assert_eq!(n, 1);
        assert_eq!(fs::read_to_string(mirror.join("src/main.rs")).unwrap(), "fn main() {}\n");
        let _ = fs::remove_dir_all(&mirror);
    }

    #[test]
    fn import_dir_finds_dumps_at_depth() {
        let root = scratch("imp");
        // two "workspaces" nested under a parent dir
        ChatDump::from_turns("p1", "one", []).export_to_workspace(&root.join("projA")).unwrap();
        ChatDump::from_turns("p2", "two", []).export_to_workspace(&root.join("nested/projB")).unwrap();
        let found = import_dir(&root);
        let ids: Vec<_> = found.iter().map(|d| d.id.clone()).collect();
        assert_eq!(ids, vec!["p1".to_string(), "p2".to_string()], "both workspaces discovered, respecting structure");
        let _ = fs::remove_dir_all(&root);
    }
}
