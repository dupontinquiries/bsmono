//! # assist — suggestions with a closed tool vocabulary
//!
//! Two backends produce the SAME structured suggestions from the same
//! desync/duplicate reports: a deterministic rule engine (always
//! available, no network), and an LLM backend (ollama, mirroring
//! logscope-fan's std-only client) for richer judgment over large messy
//! stores — "these three dumps look like generations of the same photo
//! library; consolidate onto the drive with free space" is pattern
//! recognition a rule can't do.
//!
//! The isolation contract, enforced structurally rather than by prompt:
//! * The LLM's ONLY output channel is JSON parsed into [`Suggestion`].
//!   Anything that doesn't parse, or names a tool outside [`ToolAction`],
//!   is DROPPED with a note — there is no free-text escape hatch into
//!   execution.
//! * [`ToolAction`] is a closed enum of non-destructive operations
//!   (copy, verify, note). There is deliberately no delete/format/move
//!   variant — bulk deletion advice arrives as `Note` text for a human
//!   to act on with human tools.
//! * Nothing executes here at all. Backends return plans; the CLI/GUI
//!   executes a single suggestion only on explicit operator selection
//!   (`dvm apply <id> --yes`). The LLM cannot reach the executor.

use crate::syncdiff::{DesyncReport, DupGroup};
use serde::{Deserialize, Serialize};

/// The complete tool vocabulary an assistant (rule or LLM) may propose.
/// Closed set, non-destructive by construction.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolAction {
    /// Copy one file dev→dev (paths relative to the shared dir root).
    CopyFile { from_dev: String, to_dev: String, rel: String },
    /// Copy a whole subtree dev→dev.
    CopyTree { from_dev: String, to_dev: String, rel: String },
    /// Re-scan both sides of a pair with hashing to upgrade Probable
    /// matches to Confirmed before trusting them.
    VerifyPair { dev_a: String, dev_b: String, dir: String },
    /// Advice only; never executed.
    Note { text: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Suggestion {
    pub id: String,
    /// "quick_win" | "bulk" | "hygiene"
    pub kind: String,
    /// One-line human rationale.
    pub human: String,
    pub actions: Vec<ToolAction>,
    /// Bytes this suggestion would move/affect (0 for notes).
    pub bytes: u64,
}

pub trait SuggestionBackend {
    fn suggest(
        &mut self,
        desyncs: &[DesyncReport],
        dups: &[DupGroup],
    ) -> Result<Vec<Suggestion>, String>;
}

// ── Rule backend ─────────────────────────────────────────────────────

/// Deterministic suggestions: the floor the LLM has to beat.
pub struct RuleBackend {
    /// Quick-win cap: individually suggest at most this many files per
    /// desync pair; the rest roll into one bulk CopyTree suggestion.
    pub quick_win_limit: usize,
}

impl Default for RuleBackend {
    fn default() -> Self {
        Self { quick_win_limit: 5 }
    }
}

impl SuggestionBackend for RuleBackend {
    fn suggest(
        &mut self,
        desyncs: &[DesyncReport],
        dups: &[DupGroup],
    ) -> Result<Vec<Suggestion>, String> {
        let mut out = Vec::new();
        let mut n = 0usize;
        for rep in desyncs {
            if rep.in_sync() {
                continue;
            }
            for (side, only, from, to) in [
                ('a', &rep.only_a, &rep.dev_a, &rep.dev_b),
                ('b', &rep.only_b, &rep.dev_b, &rep.dev_a),
            ] {
                if only.is_empty() {
                    continue;
                }
                let quick = &only[..only.len().min(self.quick_win_limit)];
                for (rel, sz) in quick {
                    n += 1;
                    out.push(Suggestion {
                        id: format!("qw{n}"),
                        kind: "quick_win".into(),
                        human: format!(
                            "'{}/{}' ({} B) exists only on {} — copy to {}",
                            rep.dir, rel, sz, from, to
                        ),
                        actions: vec![ToolAction::CopyFile {
                            from_dev: from.clone(),
                            to_dev: to.clone(),
                            rel: format!("{}/{}", rep.dir, rel),
                        }],
                        bytes: *sz,
                    });
                }
                if only.len() > self.quick_win_limit {
                    n += 1;
                    let total: u64 = only.iter().map(|(_, s)| s).sum();
                    out.push(Suggestion {
                        id: format!("blk{n}"),
                        kind: "bulk".into(),
                        human: format!(
                            "{} files ({} B) in '{}' exist only on {} — sync the whole dir to {}",
                            only.len(),
                            total,
                            rep.dir,
                            from,
                            to
                        ),
                        actions: vec![ToolAction::CopyTree {
                            from_dev: from.clone(),
                            to_dev: to.clone(),
                            rel: rep.dir.clone(),
                        }],
                        bytes: total,
                    });
                }
                let _ = side;
            }
            if !rep.differing.is_empty() {
                n += 1;
                out.push(Suggestion {
                    id: format!("hy{n}"),
                    kind: "hygiene".into(),
                    human: format!(
                        "{} files in '{}' differ between {} and {} — verify with hashes before trusting either side",
                        rep.differing.len(),
                        rep.dir,
                        rep.dev_a,
                        rep.dev_b
                    ),
                    actions: vec![ToolAction::VerifyPair {
                        dev_a: rep.dev_a.clone(),
                        dev_b: rep.dev_b.clone(),
                        dir: rep.dir.clone(),
                    }],
                    bytes: 0,
                });
            }
        }
        if !dups.is_empty() {
            let probable = dups
                .iter()
                .filter(|d| d.grade == crate::syncdiff::MatchGrade::Probable)
                .count();
            if probable > 0 {
                n += 1;
                out.push(Suggestion {
                    id: format!("hy{n}"),
                    kind: "hygiene".into(),
                    human: format!(
                        "{probable} cross-drive duplicate groups are name+size matches only — a hashing re-scan would confirm them"
                    ),
                    actions: vec![ToolAction::Note {
                        text: "run `dvm catalog scan <mount> --hash` on each side".into(),
                    }],
                    bytes: 0,
                });
            }
        }
        Ok(out)
    }
}

// ── LLM backend (ollama) ─────────────────────────────────────────────

/// std-only ollama client, same shape as logscope-fan's: pure request
/// builder + pure response parser, one socket function.
pub struct OllamaBackend {
    pub host: String,
    pub model: String,
    pub num_predict: u32,
    pub timeout: std::time::Duration,
    /// Suggestions the model emitted that were REJECTED (unparseable /
    /// unknown tool), kept for operator inspection — silent dropping
    /// would hide model misbehavior.
    pub rejected: Vec<String>,
}

impl OllamaBackend {
    pub fn new(host: &str, model: &str) -> Self {
        Self {
            host: host.into(),
            model: model.into(),
            num_predict: 1024,
            timeout: std::time::Duration::from_secs(120),
            rejected: Vec::new(),
        }
    }

    /// The prompt: compact JSON of the reports + the tool schema + the
    /// contract. Pure, unit-testable.
    pub fn build_prompt(desyncs: &[DesyncReport], dups: &[DupGroup]) -> String {
        let reports = serde_json::json!({
            "desync": desyncs,
            "duplicates_top": dups.iter().take(20).collect::<Vec<_>>(),
        });
        format!(
            "You are a storage-hygiene assistant. Given these reports about the \
operator's external drives, reply with ONLY a JSON array of suggestions. \
Each suggestion: {{\"id\":string,\"kind\":\"quick_win\"|\"bulk\"|\"hygiene\",\
\"human\":string,\"bytes\":number,\"actions\":[{{\"tool\":\"copy_file\",\
\"from_dev\":s,\"to_dev\":s,\"rel\":s}} | {{\"tool\":\"copy_tree\",...}} | \
{{\"tool\":\"verify_pair\",\"dev_a\":s,\"dev_b\":s,\"dir\":s}} | \
{{\"tool\":\"note\",\"text\":s}}]}}. \
No other tools exist. You cannot delete, move, or format anything. \
Suggestions are shown to a human who approves each one individually. \
Reports: {reports}"
        )
    }

    /// Parse the model's reply into validated suggestions. Anything that
    /// fails serde (including unknown tools, which the tagged enum
    /// rejects) lands in a rejection list instead of the plan.
    pub fn parse_reply(reply: &str, rejected: &mut Vec<String>) -> Vec<Suggestion> {
        // Tolerate prose around the array: take the outermost [...] span.
        let (s, e) = match (reply.find('['), reply.rfind(']')) {
            (Some(s), Some(e)) if e > s => (s, e),
            _ => {
                rejected.push(format!("no JSON array in reply: {}", &reply[..reply.len().min(120)]));
                return Vec::new();
            }
        };
        let arr: Vec<serde_json::Value> = match serde_json::from_str(&reply[s..=e]) {
            Ok(a) => a,
            Err(err) => {
                rejected.push(format!("array parse: {err}"));
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for v in arr {
            match serde_json::from_value::<Suggestion>(v.clone()) {
                Ok(sg) => out.push(sg),
                Err(err) => rejected.push(format!("rejected suggestion ({err}): {v}")),
            }
        }
        out
    }
}

impl SuggestionBackend for OllamaBackend {
    fn suggest(
        &mut self,
        desyncs: &[DesyncReport],
        dups: &[DupGroup],
    ) -> Result<Vec<Suggestion>, String> {
        let prompt = Self::build_prompt(desyncs, dups);
        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false,
            "options": { "num_predict": self.num_predict },
        })
        .to_string();
        let req = format!(
            "POST /api/generate HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.host,
            body.len(),
            body
        );
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(&self.host).map_err(|e| format!("connect {}: {e}", self.host))?;
        stream.set_read_timeout(Some(self.timeout)).ok();
        stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        let mut raw = String::new();
        stream.read_to_string(&mut raw).map_err(|e| e.to_string())?;
        let json_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
        let v: serde_json::Value =
            serde_json::from_str(raw[json_start..].trim()).map_err(|e| format!("ollama reply: {e}"))?;
        let text = v
            .get("response")
            .and_then(|r| r.as_str())
            .ok_or("no response field")?;
        Ok(Self::parse_reply(text, &mut self.rejected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syncdiff::DesyncReport;

    fn rep() -> DesyncReport {
        DesyncReport {
            dir: "docs".into(),
            dev_a: "DRV-A".into(),
            dev_b: "DRV-B".into(),
            only_a: (0..8).map(|i| (format!("f{i}.pdf"), 100 * (8 - i as u64))).collect(),
            only_b: vec![("solo.txt".into(), 42)],
            differing: vec![("cv.docx".into(), 50, 60, 'b')],
            only_a_bytes: 3600,
            only_b_bytes: 42,
        }
    }

    #[test]
    fn rules_emit_quick_wins_then_bulk_then_hygiene() {
        let mut rb = RuleBackend::default();
        let s = rb.suggest(&[rep()], &[]).unwrap();
        let quick = s.iter().filter(|x| x.kind == "quick_win").count();
        let bulk = s.iter().filter(|x| x.kind == "bulk").count();
        let hyg = s.iter().filter(|x| x.kind == "hygiene").count();
        assert_eq!(quick, 6, "5 capped from side a + 1 from side b");
        assert_eq!(bulk, 1, "side a overflow rolls into one CopyTree");
        assert_eq!(hyg, 1, "differing files trigger a VerifyPair");
        // largest gap first among side-a quick wins
        assert!(s[0].human.contains("f0.pdf"));
    }

    #[test]
    fn llm_reply_parsing_accepts_valid_rejects_unknown_tools() {
        let reply = r#"Here you go:
[
 {"id":"s1","kind":"quick_win","human":"copy it","bytes":800,
  "actions":[{"tool":"copy_file","from_dev":"DRV-A","to_dev":"DRV-B","rel":"docs/f0.pdf"}]},
 {"id":"s2","kind":"bulk","human":"nuke everything","bytes":0,
  "actions":[{"tool":"delete_tree","dev":"DRV-B","rel":"docs"}]},
 {"id":"s3","kind":"hygiene","human":"just a thought","bytes":0,
  "actions":[{"tool":"note","text":"consider one primary drive"}]}
]"#;
        let mut rejected = Vec::new();
        let parsed = OllamaBackend::parse_reply(reply, &mut rejected);
        assert_eq!(parsed.len(), 2, "valid suggestions pass");
        assert_eq!(rejected.len(), 1, "the delete_tree attempt is rejected, visibly");
        assert!(rejected[0].contains("delete_tree"));
        assert!(matches!(parsed[0].actions[0], ToolAction::CopyFile { .. }));
    }
}
