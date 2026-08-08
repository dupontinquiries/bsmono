//! providers — three ways to answer "what comes next".
//!
//! [`OllamaProvider`] is adapted from `apps/logscope/crates/logscope-fan/src/ollama.rs`
//! (the fleet's dependency-free ollama executor): same raw-`TcpStream` HTTP/1.1,
//! same targeted string-field extractor, same pure/tested request builder.
//! What's new here for the editor use: the **`suffix` field** (ollama's
//! fill-in-the-middle path, honored by qwen2.5-coder / codellama / starcoder2 /
//! deepseek-coder base models), generation options (`temperature`, `stop`),
//! and de-chunking of `Transfer-Encoding: chunked` replies — an editor asks
//! constantly, so the parse can't rely on the lucky Content-Length case.
//! When the fan's client and this one next need the same fix, that's the cue
//! to lift a shared `shared/infer-kit` (noted in TODO.md) — two call sites is
//! the threshold, per the kit rule.
//!
//! [`CommandProvider`] is the escape hatch and the steering wheel: any
//! executable that reads `prefix NUL suffix` on stdin and writes a completion
//! to stdout is a provider — a shell script around `llm`, a curl to a private
//! relay, a fixture for tests. The editor never needs to grow TLS for you to
//! use a cloud model.

use crate::{CompletionRequest, Provider};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

// ── static: deterministic answers for tests and demos ───────────────────────

pub struct StaticProvider {
    /// Longest matching prefix-tail wins. Keys are compared against the END
    /// of the request prefix, which is how a human thinks about "after I type
    /// `fn ma`, complete `in() {`".
    pub table: HashMap<String, String>,
}

impl StaticProvider {
    pub fn new(pairs: &[(&str, &str)]) -> Self {
        StaticProvider { table: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }
}

impl Provider for StaticProvider {
    fn name(&self) -> &str {
        "static"
    }
    fn complete(&mut self, req: &CompletionRequest) -> Result<String, String> {
        let mut best: Option<(&String, &String)> = None;
        for (k, v) in &self.table {
            if req.prefix.ends_with(k.as_str()) && best.map(|(bk, _)| k.len() > bk.len()).unwrap_or(true) {
                best = Some((k, v));
            }
        }
        best.map(|(_, v)| v.clone()).ok_or_else(|| "no static completion for this prefix".into())
    }
}

// ── subprocess: any executable is a provider ────────────────────────────────

pub struct CommandProvider {
    pub argv: Vec<String>,
    pub timeout: Duration,
}

impl CommandProvider {
    pub fn new(argv: Vec<String>) -> Self {
        CommandProvider { argv, timeout: Duration::from_secs(60) }
    }
}

impl Provider for CommandProvider {
    fn name(&self) -> &str {
        self.argv.first().map(String::as_str).unwrap_or("command")
    }
    fn complete(&mut self, req: &CompletionRequest) -> Result<String, String> {
        let (prog, args) = self.argv.split_first().ok_or("empty command")?;
        let mut child = Command::new(prog)
            .args(args)
            .env("APERTURE_LANG", &req.lang)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn {prog}: {e}"))?;
        {
            let stdin = child.stdin.as_mut().ok_or("no stdin")?;
            // A provider that exits without draining stdin (crash, usage
            // error) closes the pipe under us; that's its *report* to make —
            // swallow EPIPE here so the child's exit status + stderr surface
            // below instead of a useless "Broken pipe".
            match stdin.write_all(format!("{}\0{}", req.prefix, req.suffix).as_bytes()) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                Err(e) => return Err(format!("write stdin: {e}")),
            }
        } // drop → EOF
        let out = child.wait_with_output().map_err(|e| format!("wait: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!(
                "{prog} exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

// ── ollama: local models over a raw socket ──────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct OllamaConfig {
    /// "ip:port" — ollama's default is 127.0.0.1:11434.
    pub host: String,
    pub model: String,
    pub num_predict: u32,
    pub temperature: f32,
    /// Use ollama's native `suffix` (FIM). Turn off for models without a FIM
    /// template — the prefix then rides alone as a plain continuation.
    pub use_suffix: bool,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        OllamaConfig {
            host: "127.0.0.1:11434".into(),
            model: "qwen2.5-coder:1.5b-base".into(),
            num_predict: 96,
            temperature: 0.2,
            use_suffix: true,
        }
    }
}

pub struct OllamaProvider {
    pub cfg: OllamaConfig,
    pub timeout: Duration,
}

impl OllamaProvider {
    pub fn new(cfg: OllamaConfig) -> Self {
        OllamaProvider { cfg, timeout: Duration::from_secs(120) }
    }
}

impl Provider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }
    fn complete(&mut self, req: &CompletionRequest) -> Result<String, String> {
        let http = build_request(&self.cfg, &req.prefix, &req.suffix);
        let raw = http_roundtrip(&self.cfg.host, &http, self.timeout).map_err(|e| format!("io: {e}"))?;
        parse_response(&raw)
    }
}

/// Build the raw HTTP/1.1 POST for `/api/generate` (non-streaming).
/// Pure — the tests below pin the wire shape.
pub fn build_request(cfg: &OllamaConfig, prefix: &str, suffix: &str) -> String {
    let mut body = format!(
        "{{\"model\":\"{}\",\"prompt\":\"{}\",\"stream\":false",
        json_escape(&cfg.model),
        json_escape(prefix)
    );
    if cfg.use_suffix {
        body.push_str(&format!(",\"suffix\":\"{}\"", json_escape(suffix)));
    }
    body.push_str(&format!(
        ",\"options\":{{\"num_predict\":{},\"temperature\":{}}}}}",
        cfg.num_predict, cfg.temperature
    ));
    format!(
        "POST /api/generate HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        cfg.host,
        body.len()
    )
}

/// JSON-escape for embedding in a request body. (logscope-fan's, verbatim.)
pub fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// Split an HTTP reply into (headers, body), de-chunking the body when the
/// headers say `Transfer-Encoding: chunked`. logscope-fan reads to EOF and
/// scans the raw bytes, which works until a chunk boundary lands inside the
/// completion string; an editor rolls those dice hundreds of times an hour,
/// so we reassemble properly.
pub fn split_body(raw: &str) -> String {
    let Some((head, body)) = raw.split_once("\r\n\r\n") else {
        return raw.to_string();
    };
    let chunked = head
        .lines()
        .any(|l| {
            let l = l.to_ascii_lowercase();
            l.starts_with("transfer-encoding:") && l.contains("chunked")
        });
    if !chunked {
        return body.to_string();
    }
    let mut out = String::new();
    let mut rest = body;
    loop {
        let Some((size_line, tail)) = rest.split_once("\r\n") else { break };
        let Ok(size) = usize::from_str_radix(size_line.trim(), 16) else { break };
        if size == 0 {
            break;
        }
        // Chunk sizes are bytes; index bytes, then re-validate UTF-8 lossily
        // at the very end via the fact we only slice on chunk boundaries the
        // server chose. If a boundary splits a code point, fall back to the
        // raw tail rather than panic.
        let bytes = tail.as_bytes();
        if bytes.len() < size {
            out.push_str(tail);
            break;
        }
        match std::str::from_utf8(&bytes[..size]) {
            Ok(s) => out.push_str(s),
            Err(_) => out.push_str(&String::from_utf8_lossy(&bytes[..size])),
        }
        rest = std::str::from_utf8(&bytes[size..]).unwrap_or("");
        rest = rest.strip_prefix("\r\n").unwrap_or(rest);
    }
    out
}

/// Extract the completion (`"response"`) or surface the server's `"error"`.
pub fn parse_response(raw: &str) -> Result<String, String> {
    let body = split_body(raw);
    if let Some(err) = find_string_field(&body, "error") {
        return Err(format!("ollama error: {err}"));
    }
    find_string_field(&body, "response").ok_or_else(|| "ollama reply had no 'response' field".to_string())
}

/// Targeted top-level string-field extractor — logscope-fan's scanner: reads
/// whole string tokens, so a key inside another field's *value* can't fool it.
fn find_string_field(json: &str, key: &str) -> Option<String> {
    let c: Vec<char> = json.chars().collect();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '"' {
            let (s, next) = read_json_string(&c, i)?;
            let mut j = next;
            while j < c.len() && c[j].is_whitespace() {
                j += 1;
            }
            if j < c.len() && c[j] == ':' {
                j += 1;
                while j < c.len() && c[j].is_whitespace() {
                    j += 1;
                }
                if s == key {
                    return if j < c.len() && c[j] == '"' {
                        read_json_string(&c, j).map(|(v, _)| v)
                    } else {
                        None
                    };
                }
                i = j;
            } else {
                i = next;
            }
        } else {
            i += 1;
        }
    }
    None
}

fn read_json_string(c: &[char], mut i: usize) -> Option<(String, usize)> {
    if c.get(i) != Some(&'"') {
        return None;
    }
    i += 1;
    let mut out = String::new();
    while i < c.len() {
        match c[i] {
            '"' => return Some((out, i + 1)),
            '\\' => {
                i += 1;
                match c.get(i)? {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{C}'),
                    'u' => {
                        let h: String = c.get(i + 1..i + 5)?.iter().collect();
                        let cp = u32::from_str_radix(&h, 16).ok()?;
                        out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        i += 4;
                    }
                    _ => return None,
                }
                i += 1;
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    None
}

fn http_roundtrip(host: &str, req: &str, timeout: Duration) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(host)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(req.as_bytes())?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(prefix: &str, suffix: &str) -> CompletionRequest {
        CompletionRequest { generation: 0, prefix: prefix.into(), suffix: suffix.into(), lang: "rust".into() }
    }

    #[test]
    fn static_provider_prefers_the_longest_matching_tail() {
        let mut p = StaticProvider::new(&[("ma", "?"), ("fn ma", "in() {}")]);
        assert_eq!(p.complete(&req("xx fn ma", "")).unwrap(), "in() {}");
        assert!(p.complete(&req("zzz", "")).is_err());
    }

    #[test]
    fn command_provider_round_trips_stdin_contract() {
        // `cat` echoes stdin: the completion is exactly "prefix NUL suffix".
        let mut p = CommandProvider::new(vec!["/bin/cat".into()]);
        let out = p.complete(&req("before", "after")).unwrap();
        assert_eq!(out, "before\0after");
    }

    #[test]
    fn command_provider_surfaces_failure_with_stderr() {
        let mut p = CommandProvider::new(vec![
            "/bin/sh".into(),
            "-c".into(),
            "echo boom >&2; exit 3".into(),
        ]);
        let err = p.complete(&req("", "")).unwrap_err();
        assert!(err.contains("boom") && err.contains('3'), "{err}");
    }

    #[test]
    fn ollama_request_carries_fim_suffix_and_options() {
        let cfg = OllamaConfig { model: "m".into(), ..OllamaConfig::default() };
        let http = build_request(&cfg, "let x = ", "\n}");
        let (head, body) = http.split_once("\r\n\r\n").unwrap();
        let cl: usize = head
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(cl, body.len());
        assert!(body.contains("\"suffix\":\"\\n}\""));
        assert!(body.contains("\"num_predict\":96"));
        let no_fim = build_request(&OllamaConfig { use_suffix: false, ..cfg }, "p", "s");
        assert!(!no_fim.contains("suffix"), "{no_fim}");
    }

    #[test]
    fn parse_handles_content_length_and_chunked_replies() {
        let body = r#"{"model":"m","response":"hi()","done":true}"#;
        let plain = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        assert_eq!(parse_response(&plain).unwrap(), "hi()");

        // Chunk boundary deliberately inside the "response" value.
        let (a, b) = body.split_at(25);
        let chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
            a.len(),
            b.len()
        );
        assert_eq!(parse_response(&chunked).unwrap(), "hi()");
    }

    #[test]
    fn parse_surfaces_server_error_and_survives_key_in_value() {
        assert!(parse_response(r#"{"error":"model 'z' not found"}"#).unwrap_err().contains("not found"));
        let tricky = r#"{"note":"the response: is elsewhere","response":"real"}"#;
        assert_eq!(parse_response(tricky).unwrap(), "real");
    }
}
