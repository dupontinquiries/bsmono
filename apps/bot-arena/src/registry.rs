//! registry.rs — one organized home for corpora, the artifacts derived from them,
//! and the compiled pipeline binaries. Both Corpus Studio and Bot Arena read/write
//! here, so you pick a corpus from a list instead of juggling ad-hoc paths.
//!
//! Layout under `data_root()` (= $XDG_DATA_HOME/codebot-arena, override with
//! CODEBOT_ARENA_HOME):
//!
//!   bin/                       compiled pipeline binaries (built by pipeline/Makefile)
//!   corpora/
//!     <epoch>_<name>/
//!       corpus/                the text the artifacts were built from
//!       artifacts/             tokenizer.bin, vectors.bin, transitions.bin, gpt.bin, ...
//!       meta.txt               free-form notes (params, source)
//!
//! Pure std (no GTK) so it is shared by both binaries and unit-testable.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn data_root() -> PathBuf {
    if let Ok(d) = std::env::var("CODEBOT_ARENA_HOME") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".local").join("share")
        });
    base.join("codebot-arena")
}

pub fn bin_dir() -> PathBuf {
    data_root().join("bin")
}

/// Remember the most recently selected workspace so the GUI can offer a one-click
/// "use last" on the next launch. Stored as a plain path under the data root.
pub fn last_workspace_path() -> std::path::PathBuf {
    data_root().join("last_workspace.txt")
}
pub fn save_last_workspace(p: &std::path::Path) {
    let f = last_workspace_path();
    if let Some(parent) = f.parent() { let _ = std::fs::create_dir_all(parent); }
    let _ = std::fs::write(f, p.display().to_string());
}
pub fn load_last_workspace() -> Option<std::path::PathBuf> {
    let s = std::fs::read_to_string(last_workspace_path()).ok()?;
    let p = std::path::PathBuf::from(s.trim());
    if p.as_os_str().is_empty() || !p.exists() { return None; }
    Some(p)
}
pub fn corpora_dir() -> PathBuf {
    data_root().join("corpora")
}

/// True if the pipeline binaries look built (bpe_train is the first stage).
pub fn binaries_present() -> bool {
    bin_dir().join("bpe_train").exists()
}

#[derive(Clone)]
pub struct CorpusEntry {
    pub name: String,
    pub path: PathBuf,
}

impl CorpusEntry {
    pub fn corpus_dir(&self) -> PathBuf {
        self.path.join("corpus")
    }
    pub fn artifacts_dir(&self) -> PathBuf {
        self.path.join("artifacts")
    }
    pub fn has(&self, artifact: &str) -> bool {
        self.artifacts_dir().join(artifact).exists()
    }
    /// Which key artifacts exist, for display in the picker subtitle.
    pub fn artifact_summary(&self) -> String {
        let names = ["tokenizer.bin", "model.bin", "vectors.bin", "paragraph_vectors.bin", "transitions.bin", "gpt.bin"];
        let have: Vec<&str> = names.iter().copied().filter(|n| self.has(n)).collect();
        if have.is_empty() { "no artifacts yet".into() } else { have.join(", ") }
    }
}

fn list_corpora_in(root: &Path) -> Vec<CorpusEntry> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root.join("corpora")) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                v.push(CorpusEntry {
                    name: e.file_name().to_string_lossy().to_string(),
                    path: e.path(),
                });
            }
        }
    }
    v.sort_by(|a, b| b.name.cmp(&a.name)); // newest (highest epoch prefix) first
    v
}

/// All corpora, newest first.
pub fn list_corpora() -> Vec<CorpusEntry> {
    list_corpora_in(&data_root())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn new_entry_in(root: &Path, name: &str) -> CorpusEntry {
    let safe = sanitize(name);
    let dirname = if safe.is_empty() { format!("{}", epoch()) } else { format!("{}_{}", epoch(), safe) };
    let path = root.join("corpora").join(dirname);
    let _ = std::fs::create_dir_all(path.join("corpus"));
    let _ = std::fs::create_dir_all(path.join("artifacts"));
    let nm = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    CorpusEntry { name: nm, path }
}

/// Create a fresh `corpora/<epoch>_<name>/` entry with corpus/ + artifacts/.
pub fn new_corpus_entry(name: &str) -> CorpusEntry {
    new_entry_in(&data_root(), name)
}

/// Find an entry by directory name.
pub fn find(name: &str) -> Option<CorpusEntry> {
    list_corpora().into_iter().find(|e| e.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_safe_chars() {
        assert_eq!(sanitize("voice term!!"), "voice_term__");
        assert_eq!(sanitize("ok-name_1.2"), "ok-name_1.2");
    }

    #[test]
    fn entry_create_list_and_artifact_detection() {
        let root = std::env::temp_dir().join(format!("reg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let e = new_entry_in(&root, "voice term");
        assert!(e.name.contains("voice_term"));
        assert!(e.corpus_dir().is_dir());
        assert!(e.artifacts_dir().is_dir());
        assert_eq!(e.artifact_summary(), "no artifacts yet");

        std::fs::write(e.artifacts_dir().join("tokenizer.bin"), b"x").unwrap();
        assert!(e.has("tokenizer.bin"));
        assert!(e.artifact_summary().contains("tokenizer.bin"));

        let listed = list_corpora_in(&root);
        assert!(listed.iter().any(|x| x.name == e.name));

        let _ = std::fs::remove_dir_all(&root);
    }
}
