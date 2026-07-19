//! filesummary — show a FILE in the chat as a small-model summary instead of raw code, with a
//! per-block toggle. The gitmind tie-in: blocks whose info line names a file inside the loaded
//! `.gitmind` session's `/tmp` workspace (`gitmind_session::Session::work_dir`) default to the
//! summary view — the session's own files are exactly the ones the user has usually already
//! seen raw and wants *described*, not re-scrolled; everything else defaults to raw.
//!
//! Two summary tiers:
//! - a **structural digest** — pure, instant, deterministic: path, size, an outline of the
//!   declaration lines (fn/struct/impl/class/def/…), import and TODO counts. Always available.
//! - a **model summary** — generated in the background by a SMALL model through the existing
//!   dependency-free [`crate::ollama::OllamaExecutor`] when a sidecar is configured
//!   (`LOGSCOPE_OLLAMA`, host `ip:port`; `LOGSCOPE_SUMMARY_MODEL`, default `qwen3:0.6b`),
//!   cached by content hash so a file is summarized once. When no sidecar is configured the
//!   digest simply stands — the seam for routing through the in-process wgpu pipeline is
//!   `take_pending`/`fulfill`, the same queue the worker drains.
//!
//! Everything except the worker thread is pure and unit-tested. Content keys are
//! `math_kit::hash::fnv1a_64` (the kit's standard non-crypto hash).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// The file path a fenced block's info line names, if any: `rust src/main.rs` → `src/main.rs`,
/// bare `src/main.rs` → itself, plain `rust` → None. A "path" = the last whitespace token when
/// it contains a `/` or a `.` beyond position 0 (matches the BASE_SYSTEM file convention).
pub fn code_block_path(info: &str) -> Option<String> {
    let tok = info.split_whitespace().last()?;
    let looks_pathy = tok.contains('/') || tok.rfind('.').is_some_and(|i| i > 0 && i + 1 < tok.len());
    // a lone language tag like `rust`, `py`, `diff` is not a path
    if looks_pathy && !tok.starts_with("```") {
        Some(tok.to_string())
    } else {
        None
    }
}

/// True when `path` (as written in a block info line — usually relative) points inside the
/// gitmind session's `/tmp` workspace: absolute paths by prefix, relative ones by existing
/// under the workdir.
pub fn is_session_file(path: &str, work_dir: &std::path::Path) -> bool {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        p.starts_with(work_dir)
    } else {
        work_dir.join(p).exists()
    }
}

/// Content key for the caches: the kit-standard FNV-1a 64 over path + body.
pub fn content_key(path: &str, body: &str) -> u64 {
    let mut h = math_kit::hash::Fnv1a64::new();
    h.write(path.as_bytes()).write(&[0x1f]).write(body.as_bytes());
    h.finish()
}

/// The instant tier: a deterministic structural digest — what the file IS, before any model
/// says anything. Language-agnostic declaration scan (Rust/Python/JS/TS/C-family keywords),
/// bounded to keep the chat compact.
pub fn structural_digest(path: &str, body: &str) -> String {
    const DECL_HEADS: &[&str] = &[
        "pub fn ", "fn ", "pub struct ", "struct ", "pub enum ", "enum ", "impl ", "pub trait ", "trait ",
        "pub const ", "pub static ", "class ", "def ", "function ", "export function ", "export const ",
        "interface ", "type ", "#[derive", "mod ", "pub mod ",
    ];
    let lines = body.lines().count();
    let mut imports = 0usize;
    let mut todos = 0usize;
    let mut decls: Vec<&str> = Vec::new();
    for l in body.lines() {
        let t = l.trim_start();
        if t.starts_with("use ") || t.starts_with("import ") || t.starts_with("#include") || t.starts_with("from ") {
            imports += 1;
        }
        if t.contains("TODO") || t.contains("FIXME") {
            todos += 1;
        }
        if DECL_HEADS.iter().any(|h| t.starts_with(h)) {
            // signature only: cut at the body-opening brace, drop indentation
            decls.push(t.split('{').next().unwrap_or(t).trim_end());
        }
    }
    let mut out = format!("**{path}** — {lines} lines, {} KB, {imports} imports", body.len() / 1024);
    if todos > 0 {
        out.push_str(&format!(", {todos} TODO/FIXME"));
    }
    out.push('\n');
    const MAX_DECLS: usize = 24;
    for d in decls.iter().take(MAX_DECLS) {
        out.push_str("- `");
        out.push_str(d);
        out.push_str("`\n");
    }
    if decls.len() > MAX_DECLS {
        out.push_str(&format!("- … {} more declarations\n", decls.len() - MAX_DECLS));
    }
    if decls.is_empty() {
        out.push_str("- (no top-level declarations found — data/config/prose file)\n");
    }
    out
}

/// The prompt the small model gets. Tight task spec (small models follow a shape, not a
/// persona), bounded body, explicit length budget.
pub fn summary_prompt(path: &str, body: &str) -> String {
    const CAP: usize = 24 * 1024;
    let clipped = if body.len() > CAP {
        let cut = body[..CAP].rfind('\n').unwrap_or(CAP);
        &body[..cut]
    } else {
        body
    };
    format!(
        "Summarize this source file for an engineer deciding whether to open it. \
         3-6 short bullet points: what the file does, its key types/functions, anything unusual. \
         No preamble, no code blocks, bullets only.\n\nFILE {path}\n\n{clipped}"
    )
}

struct Store {
    summaries: HashMap<u64, String>,
    pending: Vec<(u64, String)>,
    requested: HashMap<u64, ()>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static WORKER: AtomicBool = AtomicBool::new(false);

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(|| Store { summaries: HashMap::new(), pending: Vec::new(), requested: HashMap::new() }))
}

/// The cached model summary for a key, if one has landed.
pub fn model_summary(key: u64) -> Option<String> {
    with_store(|s| s.summaries.get(&key).cloned())
}

/// Ask for a model summary (idempotent per key). Queues the prompt and, when an ollama sidecar
/// is configured, makes sure the background worker is running.
pub fn request_model_summary(key: u64, path: &str, body: &str) {
    let fresh = with_store(|s| {
        if s.summaries.contains_key(&key) || s.requested.contains_key(&key) {
            false
        } else {
            s.requested.insert(key, ());
            s.pending.push((key, summary_prompt(path, body)));
            true
        }
    });
    if fresh {
        try_spawn_worker();
    }
}

/// Take one queued request — the servicing seam for whatever backend generates (the ollama
/// worker uses it; the in-process pipeline can too).
pub fn take_pending() -> Option<(u64, String)> {
    with_store(|s| s.pending.pop())
}

/// Deliver a finished summary.
pub fn fulfill(key: u64, text: String) {
    with_store(|s| {
        s.summaries.insert(key, text);
    })
}

/// True when a request is in flight (queued or being generated) for `key`.
pub fn in_flight(key: u64) -> bool {
    with_store(|s| s.requested.contains_key(&key) && !s.summaries.contains_key(&key))
}

/// Spawn the ollama drain worker once, if `LOGSCOPE_OLLAMA` names a sidecar. Model from
/// `LOGSCOPE_SUMMARY_MODEL` (default `qwen3:0.6b` — the summary tier wants the SMALLEST model
/// that writes coherent bullets).
fn try_spawn_worker() {
    let Ok(host) = std::env::var("LOGSCOPE_OLLAMA") else { return };
    if WORKER.swap(true, Ordering::SeqCst) {
        return;
    }
    let model = std::env::var("LOGSCOPE_SUMMARY_MODEL").unwrap_or_else(|_| "qwen3:0.6b".into());
    std::thread::spawn(move || {
        let mut exec = crate::ollama::OllamaExecutor::new(&host, &model, 320);
        loop {
            match take_pending() {
                Some((key, prompt)) => {
                    use crate::dispatch::Executor;
                    let text = exec
                        .run(key, &prompt)
                        .unwrap_or_else(|e| format!("(summary unavailable: {e})"));
                    fulfill(key, text);
                }
                None => std::thread::sleep(std::time::Duration::from_millis(400)),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_line_path_extraction() {
        assert_eq!(code_block_path("rust src/main.rs").as_deref(), Some("src/main.rs"));
        assert_eq!(code_block_path("src/main.rs").as_deref(), Some("src/main.rs"));
        assert_eq!(code_block_path("py tools/x.py").as_deref(), Some("tools/x.py"));
        assert_eq!(code_block_path("Makefile.am").as_deref(), Some("Makefile.am"));
        assert_eq!(code_block_path("rust"), None); // lone language tag
        assert_eq!(code_block_path("diff"), None);
        assert_eq!(code_block_path(""), None);
        assert_eq!(code_block_path(".gitignore"), None); // leading dot ≠ extension; and no '/'
        assert_eq!(code_block_path("cfg/.gitignore").as_deref(), Some("cfg/.gitignore")); // slash wins
    }

    #[test]
    fn session_membership_absolute_and_relative() {
        let dir = std::env::temp_dir().join(format!("fsum-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn x() {}").unwrap();
        assert!(is_session_file("src/lib.rs", &dir));
        assert!(is_session_file(dir.join("src/lib.rs").to_str().unwrap(), &dir));
        assert!(!is_session_file("src/other.rs", &dir));
        assert!(!is_session_file("/etc/passwd", &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn digest_outlines_declarations_and_counts() {
        let body = "use std::fmt;\nuse std::io;\n\n// TODO: split\npub struct Foo { x: u32 }\n\nimpl Foo {\n    pub fn new() -> Foo { Foo { x: 0 } }\n}\n\nfn helper() {}\n";
        let d = structural_digest("src/foo.rs", body);
        assert!(d.starts_with("**src/foo.rs**"));
        assert!(d.contains("2 imports"));
        assert!(d.contains("1 TODO/FIXME"));
        assert!(d.contains("`pub struct Foo"));
        assert!(d.contains("`pub fn new() -> Foo"));
        assert!(d.contains("`fn helper()`"));
        // config file: no declarations
        let d2 = structural_digest("cfg.toml", "[a]\nb = 1\n");
        assert!(d2.contains("no top-level declarations"));
    }

    #[test]
    fn prompt_is_bounded_and_names_the_file() {
        let big = "x\n".repeat(40_000);
        let p = summary_prompt("src/big.rs", &big);
        assert!(p.contains("FILE src/big.rs"));
        assert!(p.len() < 26 * 1024);
        assert!(p.contains("bullets only"));
    }

    #[test]
    fn cache_request_fulfill_roundtrip() {
        let key = content_key("a.rs", "fn a() {}");
        assert_ne!(key, content_key("a.rs", "fn b() {}")); // content-sensitive
        assert_ne!(key, content_key("b.rs", "fn a() {}")); // path-sensitive
        assert!(model_summary(key).is_none());
        request_model_summary(key, "a.rs", "fn a() {}");
        assert!(in_flight(key));
        request_model_summary(key, "a.rs", "fn a() {}"); // idempotent
        let (k, prompt) = take_pending().expect("queued");
        assert_eq!(k, key);
        assert!(prompt.contains("FILE a.rs"));
        assert!(take_pending().is_none(), "no duplicate queue entry");
        fulfill(key, "- does a".into());
        assert_eq!(model_summary(key).as_deref(), Some("- does a"));
        assert!(!in_flight(key));
    }
}
