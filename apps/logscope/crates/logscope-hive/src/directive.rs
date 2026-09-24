//! directive — what one box tells another. The same JSON on both ends: logscope-egui on the Ultra
//! composes a `Directive`, `logscope-shim` on the mini executes it and answers with a `Reply`.
//! A directive is an instruction about models, plans and status; the model's own behavioural
//! directives (`:fuzz`) are a different thing and live in logscope-egui's `directives.rs`.
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::plan::Plan;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Directive {
    /// What is loaded, what is cached, how much room is left, what the server reports.
    Status,
    /// The library (NVMe) and the cache (internal) listings.
    Models,
    /// Ensure `model` is in the cache (copy from the library, evict LRU past the budget), then
    /// start llama-server on it with the named profile's arguments. Idempotent.
    Load { model: String, #[serde(default)] profile: Option<String> },
    /// Stop the server; the cache is untouched.
    Unload,
    /// Run a plan on the receiving box's matrix; the reply carries the outcomes as text and the
    /// ledger keeps the rest.
    Run { plan: Plan },
    /// The last `n` ledger lines plus the artifact list, as one readable text.
    Summary { #[serde(default = "default_n")] n: usize },
    /// Publish a status line through the box's sinks (the DHT to the phone, if configured).
    Say { text: String, #[serde(default)] level: Option<String> },
    /// Land a workspace bundle where the code actually lives: verify the bundle, fetch its branch
    /// into `refs/logscope/<name>` of `repo`, check it out in a throwaway worktree, run `verify`
    /// there (default `cargo check`), and reply with the result and the exact command to merge.
    /// Never merges: the reply is advice, the person's `git merge --ff-only` is the action.
    Integrate { name: String, bundle_b64: String, repo: String, branch: String, #[serde(default)] verify: Option<Vec<String>> },
}

/// Minimal base64 (RFC 4648, no line breaks) so a bundle can ride inside the JSON directive
/// without a dependency on either end.
pub mod b64 {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    pub fn encode(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len() * 4 / 3 + 4);
        for chunk in data.chunks(3) {
            let n = chunk.len();
            let b = [chunk[0], if n > 1 { chunk[1] } else { 0 }, if n > 2 { chunk[2] } else { 0 }];
            let v = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(T[(v >> 18) as usize & 63] as char); out.push(T[(v >> 12) as usize & 63] as char);
            out.push(if n > 1 { T[(v >> 6) as usize & 63] as char } else { '=' });
            out.push(if n > 2 { T[v as usize & 63] as char } else { '=' });
        }
        out
    }
    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        let val = |c: u8| -> Result<u32, String> { T.iter().position(|&t| t == c).map(|p| p as u32).ok_or_else(|| format!("bad base64 byte {c}")) };
        let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        if bytes.len() % 4 != 0 { return Err("base64 length".into()); }
        let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
        for q in bytes.chunks(4) {
            let pad = q.iter().filter(|&&c| c == b'=').count();
            let v = (val(q[0])? << 18) | (val(q[1])? << 12) | (if pad < 2 { val(q[2])? } else { 0 } << 6) | if pad < 1 { val(q[3])? } else { 0 };
            out.push((v >> 16) as u8); if pad < 2 { out.push((v >> 8) as u8); } if pad < 1 { out.push(v as u8); }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_roundtrips_every_padding() {
        for s in ["", "a", "ab", "abc", "abcd", "hello world!!"] {
            assert_eq!(super::b64::decode(&super::b64::encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
        assert_eq!(super::b64::encode(b"Man"), "TWFu"); assert_eq!(super::b64::encode(b"Ma"), "TWE=");
    }
}
fn default_n() -> usize { 20 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply { pub ok: bool, pub text: String, #[serde(default)] pub data: serde_json::Value }

impl Reply {
    pub fn ok(text: impl Into<String>) -> Reply { Reply { ok: true, text: text.into(), data: serde_json::Value::Null } }
    pub fn with(text: impl Into<String>, data: serde_json::Value) -> Reply { Reply { ok: true, text: text.into(), data } }
    pub fn err(text: impl Into<String>) -> Reply { Reply { ok: false, text: text.into(), data: serde_json::Value::Null } }
}

/// POST `{base}/directive` with a bearer token. Blocking; a `Run` can take minutes, so the timeout
/// is generous and callers put this on a thread.
pub fn send(base: &str, token: Option<&str>, d: &Directive) -> Result<Reply, String> {
    let mut r = ureq::post(&format!("{}/directive", base.trim_end_matches('/'))).timeout(Duration::from_secs(3600));
    if let Some(t) = token { r = r.set("Authorization", &format!("Bearer {t}")); }
    let resp = r.send_json(serde_json::to_value(d).map_err(|e| e.to_string())?).map_err(|e| match e {
        ureq::Error::Status(code, resp) => format!("{code}: {}", resp.into_string().unwrap_or_default().chars().take(200).collect::<String>()),
        other => other.to_string(),
    })?;
    resp.into_json::<Reply>().map_err(|e| format!("reply: {e}"))
}

/// A quick liveness check that costs one small request.
pub fn ping(base: &str, token: Option<&str>) -> Result<Reply, String> { send(base, token, &Directive::Status) }
