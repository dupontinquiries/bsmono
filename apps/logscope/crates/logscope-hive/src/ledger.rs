//! ledger — append-only JSONL. The one caste that is never staled. It has a slot for absence
//! (`looked_for / did_not_find / command / scope`), because negative results are the cheapest thing
//! to verify and the least likely to survive a worker reset. `recheck` re-runs the command an absence
//! record carries, so a record doesn't go stale, it goes re-runnable: the check is the negotiation.
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    Result { task: String, r#box: String, out_tokens: u32, total_ms: f64, forwarded_chars: usize, full_chars: usize },
    Verdict { task: String, keep: bool, reason: String },
    Premise { task: String, command: String, expect: String, ok: bool, saw: String },
    Absence { looked_for: String, did_not_find: String, command: String, scope: String, #[serde(default)] task: Option<String> },
    /// A whole document (plan, design, guide, review) kept in the artifact store, not the JSONL.
    Artifact { task: String, artifact: String, r#box: String, chars: usize, path: String },
    /// A local attempt failed and the task was re-run one tier up.
    Escalation { task: String, from_box: String, to_box: String, reason: String },
    Note { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line { pub at: String, #[serde(flatten)] pub entry: Entry }

pub fn path() -> PathBuf {
    std::env::var("LOGSCOPE_LEDGER").map(PathBuf::from).unwrap_or_else(|_| crate::matrix::path().with_file_name("ledger.jsonl"))
}

pub fn append(entry: Entry) -> Result<(), String> {
    let p = path();
    if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&p).map_err(|e| e.to_string())?;
    let line = Line { at: crate::matrix::now(), entry };
    writeln!(f, "{}", serde_json::to_string(&line).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn artifact_dir() -> PathBuf { path().with_file_name("artifacts") }

/// Store a whole artifact; returns its path. Later versions overwrite: the ledger line is the history.
pub fn put_artifact(task: &str, kind: &str, text: &str) -> Result<PathBuf, String> {
    let d = artifact_dir(); std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let p = d.join(format!("{task}.{kind}.md"));
    std::fs::write(&p, text).map_err(|e| e.to_string())?;
    Ok(p)
}

pub fn get_artifact(task: &str, kind: &str) -> Option<String> {
    std::fs::read_to_string(artifact_dir().join(format!("{task}.{kind}.md"))).ok()
}

pub fn read_all() -> Vec<Line> {
    std::fs::read_to_string(path()).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Re-run every absence record's command. Returns (record, still_absent, output_head).
pub fn recheck() -> Vec<(Line, bool, String)> {
    read_all().into_iter().filter_map(|l| {
        let Entry::Absence { command, did_not_find, .. } = &l.entry else { return None };
        let out = Command::new("sh").arg("-c").arg(command).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr)).unwrap_or_default();
        let still_absent = !out.contains(did_not_find.as_str());
        Some((l.clone(), still_absent, out.lines().take(3).collect::<Vec<_>>().join(" | ")))
    }).collect()
}
