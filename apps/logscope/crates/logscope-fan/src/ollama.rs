//! ollama.rs — a dependency-free ollama HTTP client that implements [`crate::dispatch::Executor`], so a
//! joined fleet member can run an assigned slice against a local or remote ollama server.
//!
//! A slice's `payload` is the prompt; the executor is configured with the server `host` ("ip:port"), the
//! `model`, and a token budget. `run` POSTs to `/api/generate` (non-streaming) and returns the generated
//! text. The request builder and the JSON response parser are PURE and unit-tested; only `http_roundtrip`
//! touches a socket (integration-tested on a box with ollama running). std-only, like `netserve.rs`.
#![allow(dead_code)]

use crate::dispatch::Executor;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Runs slices against an ollama server. `host` is "ip:port" (default ollama is 127.0.0.1:11434).
pub struct OllamaExecutor {
    pub host: String,
    pub model: String,
    pub num_predict: u32,
    pub timeout: Duration,
}

impl OllamaExecutor {
    pub fn new(host: &str, model: &str, num_predict: u32) -> Self {
        Self { host: host.into(), model: model.into(), num_predict, timeout: Duration::from_secs(120) }
    }
    pub fn with_timeout(mut self, t: Duration) -> Self { self.timeout = t; self }
}

impl Executor for OllamaExecutor {
    fn run(&mut self, _task: u64, payload: &str) -> Result<String, String> {
        let req = build_request(&self.host, &self.model, payload, self.num_predict);
        let raw = http_roundtrip(&self.host, &req, self.timeout).map_err(|e| format!("io: {e}"))?;
        parse_response(&raw)
    }
}

// ── pure: request building ──────────────────────────────────────────────────────────────────────--

/// Build the raw HTTP/1.1 request for ollama's non-streaming `/api/generate`.
pub fn build_request(host: &str, model: &str, prompt: &str, num_predict: u32) -> String {
    let body = format!(
        "{{\"model\":\"{}\",\"prompt\":\"{}\",\"stream\":false,\"options\":{{\"num_predict\":{}}}}}",
        json_escape(model), json_escape(prompt), num_predict);
    format!(
        "POST /api/generate HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len())
}

/// JSON-escape a string for embedding in a request body.
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

// ── pure: response parsing ──────────────────────────────────────────────────────────────────────--

/// Pull the generated text out of ollama's JSON reply (the top-level `"response"` field), or surface its
/// `"error"` field if the server returned one.
pub fn parse_response(raw: &str) -> Result<String, String> {
    let body = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or(raw);
    if let Some(err) = find_string_field(body, "error") { return Err(format!("ollama error: {err}")); }
    find_string_field(body, "response").ok_or_else(|| "ollama reply had no 'response' field".to_string())
}

/// Targeted extractor for a top-level JSON string field. Scans the object reading whole string tokens, so
/// the key is never matched inside another field's value; returns the (unescaped) value of the first
/// `"key": "..."` it finds. Not a full JSON parser — enough for ollama's flat reply shape.
fn find_string_field(json: &str, key: &str) -> Option<String> {
    let c: Vec<char> = json.chars().collect();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '"' {
            let (s, next) = read_json_string(&c, i)?;
            let mut j = next;
            while j < c.len() && c[j].is_whitespace() { j += 1; }
            if j < c.len() && c[j] == ':' {            // this string was an object key
                j += 1;
                while j < c.len() && c[j].is_whitespace() { j += 1; }
                if s == key {
                    return if j < c.len() && c[j] == '"' { read_json_string(&c, j).map(|(v, _)| v) } else { None };
                }
                i = j;                                 // skip to the value; it'll be scanned next
            } else {
                i = next;                              // string was a value/element
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Read a JSON string literal starting at the opening quote `c[i]`; returns (unescaped, index-after-close).
fn read_json_string(c: &[char], mut i: usize) -> Option<(String, usize)> {
    if c.get(i) != Some(&'"') { return None; }
    i += 1;
    let mut out = String::new();
    while i < c.len() {
        match c[i] {
            '"' => return Some((out, i + 1)),
            '\\' => {
                i += 1;
                match c.get(i)? {
                    '"' => out.push('"'), '\\' => out.push('\\'), '/' => out.push('/'),
                    'n' => out.push('\n'), 'r' => out.push('\r'), 't' => out.push('\t'),
                    'b' => out.push('\u{8}'), 'f' => out.push('\u{C}'),
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
            ch => { out.push(ch); i += 1; }
        }
    }
    None
}

// ── integration seam: the socket round-trip ────────────────────────────────────────────────────--

fn http_roundtrip(host: &str, req: &str, timeout: Duration) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(host)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(req.as_bytes())?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_has_method_path_and_correct_content_length() {
        let req = build_request("10.0.0.5:11434", "qwen2", "hi \"there\"\n", 64);
        assert!(req.starts_with("POST /api/generate HTTP/1.1\r\n"));
        assert!(req.contains("Host: 10.0.0.5:11434\r\n"));
        assert!(req.contains("Content-Type: application/json\r\n"));
        // body is everything after the blank line; Content-Length must equal its byte length
        let (head, body) = req.split_once("\r\n\r\n").unwrap();
        let cl: usize = head.lines().find_map(|l| l.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
        assert_eq!(cl, body.len());
        assert!(body.contains("\"num_predict\":64"));
        assert!(body.contains("\\\"there\\\"") && body.contains("hi \\\"there\\\"\\n"), "prompt JSON-escaped: {body}");
    }

    #[test]
    fn json_escape_covers_specials() {
        assert_eq!(json_escape("a\"b\\c\nd\te"), "a\\\"b\\\\c\\nd\\te");
        assert_eq!(json_escape("\u{1}"), "\\u0001");
    }

    #[test]
    fn parse_extracts_response_field() {
        // "model" string precedes "response"; the parser must not be fooled by it, and must unescape.
        let body = r#"{"model":"qwen2","created_at":"t","response":"hello\nworld \"q\"","done":true}"#;
        let raw = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{body}");
        assert_eq!(parse_response(&raw).unwrap(), "hello\nworld \"q\"");
    }

    #[test]
    fn parse_surfaces_error_field() {
        let body = r#"{"error":"model 'zzz' not found"}"#;
        assert!(parse_response(body).unwrap_err().contains("not found"));
    }

    #[test]
    fn extractor_not_fooled_by_key_inside_a_value() {
        // the literal text "response": appears INSIDE another field's string value
        let body = r#"{"note":"see response: nothing","response":"real"}"#;
        assert_eq!(find_string_field(body, "response").as_deref(), Some("real"));
    }
}
