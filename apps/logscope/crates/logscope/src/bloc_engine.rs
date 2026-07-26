//! bloc_engine.rs — multi-signal scoring pipeline for bloc suggestion
//!
//! Runs entirely off the GTK main thread (rayon pool).
//! Produces `Vec<Suggestion>` ranked by composite score.
//!
//! Signal layers (each contributes 0.0–1.0 confidence):
//!   1. CharScore  — bigram/trigram frequency table → language hint
//!   2. TokenScore — regex token parser → code structure (no grammar needed)
//!   3. TreeScore  — tree-sitter AST → semantic node extraction
//!   4. LspScore   — LSP subprocess completions (rust-analyzer, pylsp, …)
//!   5. AiScore    — Ollama inference for next-bloc prediction
//!
//! The UI only calls `score_text(ctx)` and receives a flat ranked list.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;

// ── Public surface ────────────────────────────────────────────────────────────

/// Everything the engine needs to know about the editing context.
#[derive(Clone, Debug)]
pub struct ScoringCtx {
    /// Full text of the current buffer (or visible window)
    pub text:        String,
    /// Cursor byte offset into text
    pub cursor:      usize,
    /// File being edited (used for LSP and grammar selection)
    pub file:        Option<PathBuf>,
    /// Project root (used for rust-analyzer workspace)
    pub project_root: Option<PathBuf>,
    /// Ollama model to use for AI layer (empty = skip AI layer)
    pub ai_model:    String,
    /// Ollama host
    pub ai_host:     String,
}

/// A single suggested bloc, scored and ready to display.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub text:       String,
    /// Display label (shorter than text, e.g. function signature)
    pub label:      String,
    pub score:      f32,
    pub source:     SuggestionSource,
    /// Detail shown on long-press / hover
    pub detail:     String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SuggestionSource {
    CharFreq,
    TokenParse,
    TreeSitter,
    Lsp,
    Ai,
    Pinned,   // user-saved bloc
}

impl SuggestionSource {
    pub fn badge(&self) -> &'static str {
        match self {
            SuggestionSource::CharFreq   => "freq",
            SuggestionSource::TokenParse => "tok",
            SuggestionSource::TreeSitter => "ast",
            SuggestionSource::Lsp        => "lsp",
            SuggestionSource::Ai         => "ai",
            SuggestionSource::Pinned     => "pin",
        }
    }
    pub fn badge_css(&self) -> &'static str {
        match self {
            SuggestionSource::CharFreq   => "badge-freq",
            SuggestionSource::TokenParse => "badge-tok",
            SuggestionSource::TreeSitter => "badge-ast",
            SuggestionSource::Lsp        => "badge-lsp",
            SuggestionSource::Ai         => "badge-ai",
            SuggestionSource::Pinned     => "badge-pin",
        }
    }
}

// ── Layer 1: Character bigram scoring ─────────────────────────────────────────

/// Build a bigram frequency table from a corpus of text.
/// Returns (char, char) → relative_frequency.
fn bigram_table(corpus: &str) -> HashMap<(char,char), f32> {
    let mut counts: HashMap<(char,char), u32> = HashMap::new();
    let chars: Vec<char> = corpus.chars().collect();
    for w in chars.windows(2) {
        *counts.entry((w[0], w[1])).or_insert(0) += 1;
    }
    let total = counts.values().sum::<u32>().max(1) as f32;
    counts.into_iter().map(|(k,v)| (k, v as f32 / total)).collect()
}

/// Score how well a candidate string matches the language implied by the corpus.
fn bigram_score(candidate: &str, table: &HashMap<(char,char),f32>) -> f32 {
    let chars: Vec<char> = candidate.chars().collect();
    if chars.len() < 2 { return 0.5; }
    let sum: f32 = chars.windows(2)
        .map(|w| *table.get(&(w[0], w[1])).unwrap_or(&0.001))
        .sum();
    (sum / chars.len() as f32).min(1.0)
}

/// Detect language from the buffer and return candidates that fit its bigram
/// profile better than average.
pub fn char_freq_suggestions(ctx: &ScoringCtx, pinned: &[Suggestion]) -> Vec<Suggestion> {
    if ctx.text.len() < 20 { return vec![]; }
    // Use the text before the cursor as the corpus
    let corpus = &ctx.text[..ctx.cursor.min(ctx.text.len())];
    let table = bigram_table(corpus);

    // Score pinned blocs against the corpus bigrams
    let mut scored: Vec<Suggestion> = pinned.iter()
        .filter(|s| s.source == SuggestionSource::Pinned)
        .map(|s| {
            let score = bigram_score(&s.text, &table);
            Suggestion {
                text:   s.text.clone(),
                label:  s.label.clone(),
                score,
                source: SuggestionSource::CharFreq,
                detail: format!("bigram fit: {:.0}%", score * 100.0),
            }
        })
        .filter(|s| s.score > 0.3)
        .collect();

    scored.sort_by(|a,b| b.score.partial_cmp(&a.score).unwrap());
    scored.truncate(5);
    scored
}

// ── Layer 2: Token-level code parsing ─────────────────────────────────────────

/// Extract structural code tokens near the cursor without a grammar.
/// Works for Rust, Python, JS, C by looking for common patterns.
pub fn token_suggestions(ctx: &ScoringCtx) -> Vec<Suggestion> {
    let text = &ctx.text;
    let cursor = ctx.cursor.min(text.len());

    // Get the line containing the cursor
    let line_start = text[..cursor].rfind('\n').map(|i| i+1).unwrap_or(0);
    let line_end   = text[cursor..].find('\n').map(|i| cursor+i).unwrap_or(text.len());
    let current_line = &text[line_start..line_end];
    let col = cursor - line_start;

    // Partial token being typed (word before cursor on current line)
    let partial: String = current_line[..col.min(current_line.len())]
        .chars().rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<String>().chars().rev().collect();

    if partial.len() < 2 { return vec![]; }

    // Collect all identifiers from the buffer that start with `partial`
    let mut seen: HashMap<String, u32> = HashMap::new();
    let re_ident = regex::Regex::new(r"\b[a-zA-Z_][a-zA-Z0-9_]{2,40}\b").unwrap();
    for m in re_ident.find_iter(text) {
        let s = m.as_str();
        if s.starts_with(&partial) && s != partial {
            *seen.entry(s.to_string()).or_insert(0) += 1;
        }
    }

    // Also look for fn/struct/impl definitions that match
    let def_re = regex::Regex::new(
        r"(?m)^\s*(pub\s+)?(fn|struct|impl|enum|trait|type|const|let\s+mut|let)\s+([a-zA-Z_][a-zA-Z0-9_]*)"
    ).unwrap();
    for cap in def_re.captures_iter(text) {
        if let Some(name) = cap.get(3) {
            let s = name.as_str();
            if s.starts_with(&partial) {
                let kind = cap.get(2).map(|m| m.as_str()).unwrap_or("id");
                *seen.entry(format!("{s}  // {kind}")).or_insert(0) += 2;
            }
        }
    }

    let total = seen.values().sum::<u32>().max(1) as f32;
    let mut results: Vec<Suggestion> = seen.into_iter()
        .map(|(name, count)| {
            let label = name.split("//").next().unwrap_or(&name).trim().to_string();
            Suggestion {
                text:   label.clone(),
                label:  label.clone(),
                score:  (count as f32 / total).min(1.0) * 0.7 + 0.3,
                source: SuggestionSource::TokenParse,
                detail: format!("used {count}× in buffer"),
            }
        })
        .collect();

    results.sort_by(|a,b| b.score.partial_cmp(&a.score).unwrap());
    results.truncate(8);
    results
}

// ── Layer 3: Tree-sitter AST ──────────────────────────────────────────────────

/// Detect language from file extension or buffer content.
pub fn detect_ts_language(ctx: &ScoringCtx) -> Option<&'static str> {
    if let Some(path) = &ctx.file {
        match path.extension().and_then(|e| e.to_str()) {
            Some("rs")                     => return Some("rust"),
            Some("py")                     => return Some("python"),
            Some("js"|"jsx"|"mjs"|"cjs")   => return Some("javascript"),
            Some("ts"|"tsx")               => return Some("typescript"),
            Some("c"|"h")                  => return Some("c"),
            Some("cpp"|"cc"|"cxx"|"hpp")   => return Some("cpp"),
            _ => {}
        }
    }
    // Sniff from content
    let t = &ctx.text;
    if t.contains("fn ") && t.contains("let ") && t.contains("::") { return Some("rust"); }
    if t.contains("def ") && t.contains("import ") { return Some("python"); }
    if t.contains("const ") && t.contains("function") { return Some("javascript"); }
    None
}

/// Extract named nodes near the cursor using tree-sitter.
/// Returns (node_kind, node_text) pairs for use as suggestions.
pub fn tree_sitter_nodes(ctx: &ScoringCtx) -> Vec<(String, String)> {
    let lang_name = match detect_ts_language(ctx) {
        Some(l) => l,
        None    => return vec![],
    };

    let language: Option<tree_sitter::Language> = match lang_name {
        "rust"       => Some(tree_sitter_rust::LANGUAGE.into()),
        "python"     => Some(tree_sitter_python::LANGUAGE.into()),
        "javascript" => Some(tree_sitter_javascript::LANGUAGE.into()),
        _            => None,
    };

    let language = match language {
        Some(l) => l,
        None    => return vec![],
    };

    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() { return vec![]; }

    let tree = match parser.parse(&ctx.text, None) {
        Some(t) => t,
        None    => return vec![],
    };

    let cursor_point = {
        let before = &ctx.text[..ctx.cursor.min(ctx.text.len())];
        let line   = before.matches('\n').count();
        let col    = before.rfind('\n').map(|i| ctx.cursor - i - 1).unwrap_or(ctx.cursor);
        tree_sitter::Point::new(line, col)
    };

    // Walk the tree and collect interesting nodes near the cursor
    let mut results = Vec::new();
    let root = tree.root_node();
    collect_named_nodes(&root, &ctx.text, cursor_point, 0, &mut results);
    results
}

fn collect_named_nodes(
    node: &tree_sitter::Node,
    src: &str,
    cursor: tree_sitter::Point,
    depth: u32,
    out: &mut Vec<(String, String)>,
) {
    if depth > 6 { return; }
    let start = node.start_position();
    let _end  = node.end_position();

    // Only nodes within ~20 lines of cursor
    let near = (start.row as i64 - cursor.row as i64).abs() < 20;

    if near && node.is_named() {
        let kind = node.kind();
        // Collect semantically interesting nodes
        let interesting = matches!(kind,
            "function_item" | "function_definition" | "function_declaration" |
            "impl_item" | "struct_item" | "enum_item" | "trait_item" |
            "method_definition" | "class_definition" | "class_declaration" |
            "variable_declarator" | "let_declaration" | "const_item" |
            "call_expression" | "identifier" | "type_identifier" |
            "field_declaration" | "parameter" | "attribute_item"
        );
        if interesting {
            let text = node.utf8_text(src.as_bytes())
                .unwrap_or("")
                .lines().next().unwrap_or("")
                .trim()
                .chars().take(80)
                .collect::<String>();
            if !text.is_empty() && text.len() > 2 {
                out.push((kind.to_string(), text));
            }
        }
    }

    let mut c = node.walk();
    for child in node.children(&mut c) {
        collect_named_nodes(&child, src, cursor, depth + 1, out);
    }
}

pub fn tree_sitter_suggestions(ctx: &ScoringCtx) -> Vec<Suggestion> {
    let nodes = tree_sitter_nodes(ctx);
    nodes.into_iter().enumerate().map(|(i, (kind, text))| {
        let score = 0.9 - (i as f32 * 0.05);
        Suggestion {
            label:  text.clone(),
            text:   text,
            score:  score.max(0.3),
            source: SuggestionSource::TreeSitter,
            detail: format!("AST: {kind}"),
        }
    }).take(8).collect()
}

// ── Layer 4: LSP ──────────────────────────────────────────────────────────────

/// Detect available LSP servers on PATH.
pub fn detect_lsp_server(lang: Option<&str>) -> Option<&'static str> {
    let servers: &[(&str, &[&str])] = &[
        ("rust",       &["rust-analyzer"]),
        ("python",     &["pylsp", "pyright", "jedi-language-server"]),
        ("javascript", &["typescript-language-server", "deno"]),
        ("typescript", &["typescript-language-server", "deno"]),
        ("c",          &["clangd"]),
        ("cpp",        &["clangd"]),
    ];
    let candidates = lang
        .and_then(|l| servers.iter().find(|(k,_)| *k == l))
        .map(|(_,v)| *v)
        .unwrap_or(&["rust-analyzer", "pylsp", "clangd"]);

    for &cmd in candidates {
        if which_exists(cmd) { return Some(cmd); }
    }
    None
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get completion candidates from an LSP server for the current context.
///
/// Sessions are PERSISTENT and POOLED, one per server binary. The old code spawned a fresh
/// server per call and killed it (without reaping) after one request — with pylsp that means
/// a new ~100 MB python process per suggestion pass, several alive at once, plus a zombie
/// left behind each time. That was the "many memory-hungry python processes" bug. Now:
///   - one live server per language, initialized once, reused across requests;
///   - every kill is followed by wait() (no zombies), including in Drop;
///   - a failed/timed-out server is torn down and put on a 30s cooldown, so a broken
///     install can't respawn-storm either.
pub fn lsp_completions(ctx: &ScoringCtx) -> Vec<Suggestion> {
    let lang   = detect_ts_language(ctx);
    let server = match detect_lsp_server(lang) { Some(s) => s, None => return vec![] };
    let file   = match &ctx.file { Some(f) => f, None => return vec![] };
    let root   = ctx.project_root.clone()
        .or_else(|| file.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let cursor = ctx.cursor.min(ctx.text.len());
    let before = &ctx.text[..cursor];
    let line   = before.matches('\n').count();
    let col    = cursor - before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let file_uri = format!("file://{}", file.to_string_lossy());
    let root_uri = format!("file://{}", root.to_string_lossy());

    let pool = lsp_pool();
    let mut pool = match pool.lock() { Ok(p) => p, Err(_) => return vec![] };
    // Cooldown: a server that just failed doesn't get respawned for 30s.
    if let Some(t) = pool.cooldown.get(server) {
        if t.elapsed() < std::time::Duration::from_secs(30) { return vec![]; }
    }
    let sess = match pool.sessions.remove(server) {
        Some(s) => s,
        None => match LspSession::spawn(server, &root_uri) {
            Some(s) => s,
            None => { pool.cooldown.insert(server, std::time::Instant::now()); return vec![]; }
        }
    };
    let (sess, out) = sess.completion(&file_uri, lang.unwrap_or("text"), &ctx.text, line, col);
    match sess {
        Some(s) => { pool.sessions.insert(server, s); }              // healthy: keep for reuse
        None => { pool.cooldown.insert(server, std::time::Instant::now()); } // torn down inside
    }
    out.map(|raw| parse_lsp_completions(&raw)).unwrap_or_default()
}

struct LspPool {
    sessions: std::collections::HashMap<&'static str, LspSession>,
    cooldown: std::collections::HashMap<&'static str, std::time::Instant>,
}
fn lsp_pool() -> &'static std::sync::Mutex<LspPool> {
    static P: std::sync::OnceLock<std::sync::Mutex<LspPool>> = std::sync::OnceLock::new();
    P.get_or_init(|| std::sync::Mutex::new(LspPool {
        sessions: std::collections::HashMap::new(),
        cooldown: std::collections::HashMap::new(),
    }))
}

/// One live LSP server: child + writer handle + a reader thread that frames stdout by
/// Content-Length and forwards each message body over a channel.
struct LspSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    next_id: u32,
}

impl LspSession {
    fn spawn(server: &str, root_uri: &str) -> Option<Self> {
        let mut child = std::process::Command::new(server)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn().ok()?;
        let stdin = child.stdin.take()?;
        let mut stdout = child.stdout.take()?;
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf: Vec<u8> = Vec::new();
            let mut tmp = [0u8; 8192];
            loop {
                // frame out complete Content-Length messages
                loop {
                    let Some(hdr_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else { break };
                    let hdr = String::from_utf8_lossy(&buf[..hdr_end]);
                    let len: usize = hdr.lines()
                        .find_map(|l| l.strip_prefix("Content-Length:"))
                        .and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                    if buf.len() < hdr_end + 4 + len { break; }
                    let body = buf[hdr_end + 4..hdr_end + 4 + len].to_vec();
                    buf.drain(..hdr_end + 4 + len);
                    if tx.send(body).is_err() { return; }
                }
                match stdout.read(&mut tmp) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
        });
        let mut s = LspSession { child, stdin, rx, next_id: 1 };
        // initialize handshake, once per session lifetime
        let id = s.send_req("initialize", serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": { "completion": { "completionItem": { "snippetSupport": false } } }
            }
        }))?;
        s.wait_for(id, std::time::Duration::from_secs(3))?;
        s.send_notif("initialized", serde_json::json!({}))?;
        Some(s)
    }

    fn send(&mut self, msg: &str) -> Option<()> {
        use std::io::Write;
        self.stdin.write_all(msg.as_bytes()).ok()
    }
    fn send_req(&mut self, method: &str, params: serde_json::Value) -> Option<u32> {
        let id = self.next_id; self.next_id += 1;
        self.send(&lsp_msg(id, method, params))?; Some(id)
    }
    fn send_notif(&mut self, method: &str, params: serde_json::Value) -> Option<()> {
        self.send(&lsp_notif(method, params))
    }
    /// Drain frames until the response with `id` arrives (notifications are discarded).
    fn wait_for(&mut self, id: u32, dl: std::time::Duration) -> Option<Vec<u8>> {
        let needle = format!("\"id\":{id}");
        let end = std::time::Instant::now() + dl;
        loop {
            let left = end.checked_duration_since(std::time::Instant::now())?;
            let body = self.rx.recv_timeout(left).ok()?;
            if String::from_utf8_lossy(&body).contains(&needle) { return Some(body); }
        }
    }

    /// One completion round-trip. Consumes self; returns it only if the session stayed
    /// healthy — a failure tears the child down (kill + WAIT) and returns None instead,
    /// so a wedged server can never be reused or leak.
    fn completion(mut self, uri: &str, lang: &str, text: &str, line: usize, col: usize)
        -> (Option<Self>, Option<Vec<u8>>)
    {
        let r = (|| -> Option<Vec<u8>> {
            // didClose + didOpen: spec-clean full-document sync without tracking versions
            self.send_notif("textDocument/didClose", serde_json::json!({
                "textDocument": { "uri": uri } }))?;
            self.send_notif("textDocument/didOpen", serde_json::json!({
                "textDocument": { "uri": uri, "languageId": lang, "version": 1, "text": text } }))?;
            let id = self.send_req("textDocument/completion", serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": col } }))?;
            self.wait_for(id, std::time::Duration::from_secs(3))
        })();
        match r {
            Some(body) => (Some(self), Some(body)),
            None => { self.teardown(); (None, None) }
        }
    }

    fn teardown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait(); // REAP — kill without wait is where the zombies came from
    }
}
impl Drop for LspSession {
    fn drop(&mut self) { self.teardown(); }
}

fn lsp_msg(id: u32, method: &str, params: serde_json::Value) -> String {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": method, "params": params
    }).to_string();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

fn lsp_notif(method: &str, params: serde_json::Value) -> String {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "method": method, "params": params
    }).to_string();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

fn parse_lsp_completions(raw: &[u8]) -> Vec<Suggestion> {
    let text = std::str::from_utf8(raw).unwrap_or("");
    // Find the last JSON object that has "id":2
    let mut results = Vec::new();
    for line in text.split("\r\n\r\n") {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if val.get("id").and_then(|v| v.as_u64()) != Some(2) { continue }
        let items = val["result"]["items"].as_array()
            .or_else(|| val["result"].as_array());
        if let Some(items) = items {
            for (i, item) in items.iter().take(12).enumerate() {
                let label  = item["label"].as_str().unwrap_or("").to_string();
                let detail = item["detail"].as_str().unwrap_or("").to_string();
                let insert = item["insertText"].as_str()
                    .or_else(|| item["label"].as_str())
                    .unwrap_or("").to_string();
                if label.is_empty() { continue; }
                let score = 0.95 - (i as f32 * 0.04);
                results.push(Suggestion {
                    text:   insert,
                    label:  label,
                    score:  score.max(0.4),
                    source: SuggestionSource::Lsp,
                    detail,
                });
            }
            break;
        }
    }
    results
}

// ── Layer 5: AI inference ─────────────────────────────────────────────────────

/// Ask Ollama to predict the next code bloc given context.
/// Returns suggestions synchronously (call from rayon thread).
pub fn ai_suggestions(ctx: &ScoringCtx) -> Vec<Suggestion> {
    if ctx.ai_model.is_empty() || ctx.ai_host.is_empty() { return vec![]; }

    // Take the last ~40 lines as context window
    let context: String = ctx.text.lines()
        .rev().take(40).collect::<Vec<_>>()
        .into_iter().rev()
        .collect::<Vec<_>>().join("\n");

    let lang = detect_ts_language(ctx).unwrap_or("code");
    let prompt = format!(
        "You are a code completion engine. Given this {lang} code context, \
        predict the next 3-5 most likely code fragments the developer would type. \
        Respond ONLY with one fragment per line, prefixed with ITEM: \
        Keep each fragment under 60 chars. No explanations.\n\n\
        Context (cursor is at end):\n```\n{context}\n```\n\nNext fragments:",
    );

    let url = format!("{}/api/generate", ctx.ai_host);
    let body = serde_json::json!({
        "model": ctx.ai_model,
        "prompt": prompt,
        "stream": false,
        "options": { "num_predict": 200, "temperature": 0.3 }
    });

    let resp = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(8))
        .send_json(&body);

    let text = match resp {
        Ok(r) => r.into_json::<serde_json::Value>()
            .ok()
            .and_then(|v| v["response"].as_str().map(|s| s.to_owned()))
            .unwrap_or_default(),
        Err(_) => return vec![],
    };

    text.lines()
        .filter(|l| l.trim_start().starts_with("ITEM:"))
        .enumerate()
        .map(|(i, l)| {
            let item = l.trim_start_matches("ITEM:").trim().to_string();
            Suggestion {
                label:  item.chars().take(50).collect(),
                text:   item,
                score:  0.9 - (i as f32 * 0.05),
                source: SuggestionSource::Ai,
                detail: format!("AI ({lang})"),
            }
        })
        .take(6)
        .collect()
}

// ── Full pipeline ─────────────────────────────────────────────────────────────

/// Run all layers and return a merged, deduplicated, ranked list.
/// Layers 1-3 are fast (sync). Layer 4 (LSP) and 5 (AI) are slow;
/// call from a rayon::spawn and send results via channel.
pub fn score_text(
    ctx:         &ScoringCtx,
    pinned:      &[Suggestion],
    include_lsp: bool,
    include_ai:  bool,
    cancel:      Option<&std::sync::atomic::AtomicBool>,
) -> Vec<Suggestion> {
    // Bail early if cancelled
    if cancel.map(|c| c.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(false) {
        return vec![];
    }
    let mut all: Vec<Suggestion> = Vec::new();

    // Layer 1: char freq
    all.extend(char_freq_suggestions(ctx, pinned));

    // Layer 2: token parse
    all.extend(token_suggestions(ctx));

    // Layer 3: tree-sitter
    all.extend(tree_sitter_suggestions(ctx));

    // Check cancellation before slow layers
    if cancel.map(|c| c.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(false) {
        return finalize(all);
    }

    // Layer 4: LSP (optional, slow)
    if include_lsp {
        all.extend(lsp_completions(ctx));
    }

    if cancel.map(|c| c.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(false) {
        return finalize(all);
    }

    // Layer 5: AI (optional, slow)
    if include_ai {
        all.extend(ai_suggestions(ctx));
    }

    finalize(all)
}

fn finalize(all: Vec<Suggestion>) -> Vec<Suggestion> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut deduped: Vec<Suggestion> = Vec::new();
    for s in all {
        let key = s.text.trim().to_lowercase();
        if let Some(&idx) = seen.get(&key) {
            if s.score > deduped[idx].score { deduped[idx] = s; }
        } else {
            seen.insert(key, deduped.len());
            deduped.push(s);
        }
    }
    for s in &mut deduped {
        match s.source {
            SuggestionSource::Lsp        => s.score = (s.score * 1.3).min(1.0),
            SuggestionSource::TreeSitter => s.score = (s.score * 1.15).min(1.0),
            SuggestionSource::Ai         => s.score = (s.score * 1.2).min(1.0),
            _ => {}
        }
    }
    deduped.sort_by(|a,b| b.score.partial_cmp(&a.score).unwrap());
    deduped.truncate(16);
    deduped
}

// ── Template blocs and placeholder system ────────────────────────────────────

/// Placeholder marker inserted into template blocs.
/// Tap inserts the bloc; cursor jumps to first ‸, next tap moves to next ‸.
pub const PLACEHOLDER: &str = "‸";

/// A template bloc with named slots.
#[derive(Clone, Debug)]
pub struct TemplateBlocDef {
    pub category: &'static str,
    pub label:    &'static str,
    pub text:     &'static str,   // contains ‸ for insertion points
    pub lang:     Option<&'static str>,  // None = language-agnostic
}

/// Built-in template library, organized by category.
pub fn builtin_templates() -> Vec<TemplateBlocDef> {
    vec![
        // ── Control ────────────────────────────────────────────────────────────
        TemplateBlocDef { category: "Control",   label: "if",
            text: "if ‸condition‸ {
    ‸body‸
}", lang: None },
        TemplateBlocDef { category: "Control",   label: "if else",
            text: "if ‸condition‸ {
    ‸body‸
} else {
    ‸else_body‸
}", lang: None },
        TemplateBlocDef { category: "Control",   label: "for",
            text: "for ‸item‸ in ‸iterable‸ {
    ‸body‸
}", lang: None },
        TemplateBlocDef { category: "Control",   label: "while",
            text: "while ‸condition‸ {
    ‸body‸
}", lang: None },
        TemplateBlocDef { category: "Control",   label: "match",
            text: "match ‸expr‸ {
    ‸pattern‸ => ‸value‸,
    _ => ‸default‸,
}", lang: Some("rust") },
        TemplateBlocDef { category: "Control",   label: "loop",
            text: "loop {
    ‸body‸
    if ‸done‸ { break; }
}", lang: Some("rust") },

        // ── Variables ──────────────────────────────────────────────────────────
        TemplateBlocDef { category: "Variables", label: "let",
            text: "let ‸name‸ = ‸value‸;", lang: Some("rust") },
        TemplateBlocDef { category: "Variables", label: "let mut",
            text: "let mut ‸name‸ = ‸value‸;", lang: Some("rust") },
        TemplateBlocDef { category: "Variables", label: "const",
            text: "const ‸NAME‸: ‸Type‸ = ‸value‸;", lang: None },
        TemplateBlocDef { category: "Variables", label: "assign",
            text: "‸name‸ = ‸value‸;", lang: None },

        // ── Functions ──────────────────────────────────────────────────────────
        TemplateBlocDef { category: "Functions", label: "fn",
            text: "fn ‸name‸(‸args‸) -> ‸Return‸ {
    ‸body‸
}", lang: Some("rust") },
        TemplateBlocDef { category: "Functions", label: "pub fn",
            text: "pub fn ‸name‸(‸args‸) -> ‸Return‸ {
    ‸body‸
}", lang: Some("rust") },
        TemplateBlocDef { category: "Functions", label: "closure",
            text: "|‸args‸| ‸body‸", lang: Some("rust") },
        TemplateBlocDef { category: "Functions", label: "async fn",
            text: "async fn ‸name‸(‸args‸) -> ‸Return‸ {
    ‸body‸
}", lang: Some("rust") },
        TemplateBlocDef { category: "Functions", label: "def",
            text: "def ‸name‸(‸args‸):
    ‸body‸", lang: Some("python") },

        // ── Types ──────────────────────────────────────────────────────────────
        TemplateBlocDef { category: "Types",     label: "struct",
            text: "struct ‸Name‸ {
    ‸field‸: ‸Type‸,
}", lang: Some("rust") },
        TemplateBlocDef { category: "Types",     label: "enum",
            text: "enum ‸Name‸ {
    ‸Variant‸,
}", lang: Some("rust") },
        TemplateBlocDef { category: "Types",     label: "impl",
            text: "impl ‸Name‸ {
    pub fn new(‸args‸) -> Self {
        Self { ‸body‸ }
    }
}", lang: Some("rust") },
        TemplateBlocDef { category: "Types",     label: "trait",
            text: "trait ‸Name‸ {
    fn ‸method‸(‸args‸) -> ‸Return‸;
}", lang: Some("rust") },

        // ── Patterns ───────────────────────────────────────────────────────────
        TemplateBlocDef { category: "Patterns",  label: "Option",
            text: "if let Some(‸value‸) = ‸expr‸ {
    ‸body‸
}", lang: Some("rust") },
        TemplateBlocDef { category: "Patterns",  label: "Result",
            text: "match ‸expr‸ {
    Ok(‸v‸)  => ‸ok_body‸,
    Err(‸e‸) => ‸err_body‸,
}", lang: Some("rust") },
        TemplateBlocDef { category: "Patterns",  label: "unwrap_or",
            text: "‸expr‸.unwrap_or(‸default‸)", lang: Some("rust") },
        TemplateBlocDef { category: "Patterns",  label: "?",
            text: "‸expr‸?", lang: Some("rust") },
        TemplateBlocDef { category: "Patterns",  label: "map",
            text: "‸iter‸.map(|‸x‸| ‸body‸).collect::<Vec<_>>()", lang: Some("rust") },
        TemplateBlocDef { category: "Patterns",  label: "filter",
            text: "‸iter‸.filter(|‸x‸| ‸pred‸).collect::<Vec<_>>()", lang: Some("rust") },
        TemplateBlocDef { category: "Patterns",  label: "clone",
            text: "‸value‸.clone()", lang: Some("rust") },

        // ── Operators ──────────────────────────────────────────────────────────
        TemplateBlocDef { category: "Operators", label: "==",
            text: "‸a‸ == ‸b‸", lang: None },
        TemplateBlocDef { category: "Operators", label: "&&",
            text: "‸a‸ && ‸b‸", lang: None },
        TemplateBlocDef { category: "Operators", label: "||",
            text: "‸a‸ || ‸b‸", lang: None },
        TemplateBlocDef { category: "Operators", label: "format!",
            text: "format!(\"‸pattern‸\", ‸args‸)", lang: Some("rust") },
        TemplateBlocDef { category: "Operators", label: "println!",
            text: "println!(\"‸pattern‸\", ‸args‸)", lang: Some("rust") },
        TemplateBlocDef { category: "Operators", label: "vec!",
            text: "vec![‸items‸]", lang: Some("rust") },
    ]
}

/// Find the first placeholder ‸ at or after `from_byte` in `text`.
/// Returns the absolute byte offset of the placeholder in `text`.
pub fn advance_to_placeholder(text: &str, from_byte: usize) -> Option<usize> {
    text[from_byte..].find(PLACEHOLDER).map(|rel| from_byte + rel)
}

/// Replace the placeholder at the cursor with typed text and move to next ‸.
pub fn next_placeholder_offset(full_text: &str, after_byte: usize) -> Option<usize> {
    full_text[after_byte..].find(PLACEHOLDER).map(|rel| after_byte + rel)
}

