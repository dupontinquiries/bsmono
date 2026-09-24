//! counter — the measurement that decides whether a scheduler is worth building. Reads Claude Code
//! transcripts (project JSONL or `stream-json` output) and counts, per assistant turn, whether tool use
//! was fan-out (several calls in one message), pipeline/serial (one call that consumes the previous
//! result), and what kind of move each call was: read, verify, build, run. Keyword buckets over what
//! the transcript exposes; direction is solid, the numbers are soft.
use std::collections::HashMap;

use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub struct Counts {
    pub assistant_turns: usize,
    pub tool_calls: usize,
    pub fanout_turns: usize,      // ≥2 tool_use blocks in one assistant message
    pub fanout_calls: usize,
    pub serial_turns: usize,      // exactly one tool_use whose input reuses a token from the previous tool_result
    pub single_turns: usize,      // one tool_use, no visible dependence
    pub by_kind: HashMap<String, usize>,
    pub verify_before_build: usize,   // a verify call immediately preceding a build call
    pub diagnose_after_error: usize,  // a read/verify call whose previous tool result was an error (reactive)
    pub parsed_lines: usize,
    pub skipped_lines: usize,
}

fn kind_of(name: &str, input: &serde_json::Value) -> &'static str {
    let cmd = input.get("command").and_then(|c| c.as_str()).unwrap_or("").to_ascii_lowercase();
    match name {
        "Read" | "Glob" | "Grep" | "LS" | "WebFetch" | "WebSearch" => "read",
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => "build",
        "Bash" => {
            if ["cargo check", "cargo test", "cargo clippy", "cargo build", "pytest", "npm test", "go test", "make test", "rspec"].iter().any(|k| cmd.contains(k)) { "verify" }
            else if ["git diff", "git status", "git log", "ls ", "cat ", "rg ", "grep ", "find ", "head ", "tail ", "wc ", "ffprobe", "stat "].iter().any(|k| cmd.starts_with(k.trim()) || cmd.contains(&format!("&& {k}")) || cmd.contains(&format!("| {k}"))) { "read" }
            else if ["sed -i", "tee ", ">>", "cp ", "mv ", "mkdir", "cargo new", "git commit", "git checkout -b", "patch "].iter().any(|k| cmd.contains(k)) { "build" }
            else { "run" }
        }
        _ => "other",
    }
}

fn tokens(s: &str) -> Vec<String> { s.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '/' && c != '.').filter(|t| t.len() >= 6).map(|t| t.to_string()).collect() }

pub fn count(text: &str) -> Counts {
    let mut c = Counts::default();
    let mut prev_result_text = String::new();
    let mut prev_kind: Option<&'static str> = None;
    let mut prev_result_was_error = false;
    for line in text.lines() {
        let Ok(j) = serde_json::from_str::<serde_json::Value>(line) else { c.skipped_lines += 1; continue };
        c.parsed_lines += 1;
        let ty = j.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let content = j.pointer("/message/content").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        if ty == "assistant" {
            c.assistant_turns += 1;
            let uses: Vec<&serde_json::Value> = content.iter().filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use")).collect();
            c.tool_calls += uses.len();
            if uses.len() >= 2 { c.fanout_turns += 1; c.fanout_calls += uses.len(); }
            else if uses.len() == 1 {
                let input = uses[0].get("input").cloned().unwrap_or_default();
                let itext = input.to_string();
                let dependent = tokens(&prev_result_text).iter().any(|t| itext.contains(t));
                if dependent { c.serial_turns += 1; } else { c.single_turns += 1; }
            }
            for u in &uses {
                let name = u.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                let k = kind_of(name, u.get("input").unwrap_or(&serde_json::Value::Null));
                *c.by_kind.entry(k.to_string()).or_default() += 1;
                if k == "build" && prev_kind == Some("verify") { c.verify_before_build += 1; }
                if (k == "verify" || k == "read") && prev_result_was_error { c.diagnose_after_error += 1; }
                prev_kind = Some(k);
            }
        } else if ty == "user" {
            // tool results come back as user messages with tool_result blocks
            let mut txt = String::new(); let mut err = false;
            for b in &content {
                if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                    err |= b.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                    match b.get("content") { Some(serde_json::Value::String(s)) => txt.push_str(s), Some(v) => txt.push_str(&v.to_string()), None => {} }
                }
            }
            if !txt.is_empty() { prev_result_text = txt; prev_result_was_error = err; }
        }
    }
    c
}
