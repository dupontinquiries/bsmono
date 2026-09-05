//! The model seam.
//!
//! `voxel-tools` has no HTTP client and should not grow one — the game
//! already has an async runtime and its own opinions. What it needs is
//! a trait narrow enough that any client satisfies it, plus the parts
//! that are the same for everyone: request shape, response cleaning,
//! and a fake for tests.
//!
//! The cleaning matters more than it sounds. Models wrap plans in code
//! fences, prefix them with "Sure! Here's a plan:", and add a closing
//! remark. Every one of those lines becomes an "unknown op" error and
//! a rejected plan. Stripping them is nine lines here versus a prompt
//! that has to fight the model's instincts.

/// Anything that can turn (system, user) into a plan.
///
/// Same shape as `aperture_complete::Provider`: `&mut self`, a name,
/// and a `Result<String, String>`. Deliberately not async and
/// deliberately not HTTP -- see `CommandProvider` below for why.
pub trait Completer {
    fn name(&self) -> &str { "completer" }
    /// Blocking. Call it off the frame thread.
    fn complete(&mut self, system: &str, user: &str) -> Result<String, String>;
}

/// Any executable is a model.
///
/// Lifted from `aperture_complete::providers::CommandProvider`, which
/// exists for the same reason it is the right choice here: the game
/// should not grow an HTTP client, a TLS stack, or an opinion about
/// which server you run. A provider is any program that reads a prompt
/// on stdin and writes a plan to stdout. A shell script around
/// `llama-cli`, a curl to a private relay, an ollama wrapper, a
/// fixture for tests -- all the same interface.
///
/// Wire format, matching aperture's `prefix NUL suffix`:
///
/// ```text
/// <system prompt>\0<user turn>
/// ```
///
/// with `VOXEL_PASS` in the environment when the caller knows it.
/// Read to EOF, reply on stdout, exit 0. Non-zero exit means the
/// stderr text becomes the error the player sees.
pub struct CommandProvider {
    pub argv: Vec<String>,
    /// Extra environment for the child, e.g. the model path.
    pub env: Vec<(String, String)>,
}

impl CommandProvider {
    pub fn new(argv: Vec<String>) -> Self {
        Self { argv, env: Vec::new() }
    }
    /// The conventional shape: a script in the game's data dir, given
    /// the chosen model path so it can pick the weights.
    pub fn with_model(argv: Vec<String>, model_path: &std::path::Path) -> Self {
        Self { argv, env: vec![("VOXEL_MODEL".into(), model_path.display().to_string())] }
    }
}

impl Completer for CommandProvider {
    fn name(&self) -> &str { self.argv.first().map(String::as_str).unwrap_or("command") }

    fn complete(&mut self, system: &str, user: &str) -> Result<String, String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let (prog, args) = self.argv.split_first().ok_or("empty command")?;
        let mut cmd = Command::new(prog);
        cmd.args(args);
        for (k, v) in &self.env { cmd.env(k, v); }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn {prog}: {e}"))?;
        {
            let stdin = child.stdin.as_mut().ok_or("no stdin")?;
            // A provider that exits without draining stdin (crash, usage
            // error) closes the pipe under us; that is its report to
            // make. Swallow EPIPE so the child's exit status and stderr
            // surface below instead of a useless "Broken pipe" --
            // aperture learned this one the hard way.
            match stdin.write_all(format!("{system}\0{user}").as_bytes()) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                Err(e) => return Err(format!("write stdin: {e}")),
            }
        } // drop -> EOF
        let out = child.wait_with_output().map_err(|e| format!("wait: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!("{prog} exited {}: {}", out.status,
                        String::from_utf8_lossy(&out.stderr).trim()))
        }
    }
}

/// Strip everything that is not a plan line from a model reply.
///
/// Handles, in order: code fences (``` and ```text), a leading
/// conversational sentence, trailing commentary, and blank padding.
/// Lines that look like IR are kept verbatim -- including comments,
/// which the parser uses for the plan name.
pub fn clean_reply(raw: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut in_fence = false;
    let mut saw_fence = false;
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_fence = !in_fence;
            saw_fence = true;
            continue;
        }
        lines.push(line);
    }
    // If the reply had a fence, keep only what was inside it. Rebuild
    // rather than track state, so an unterminated fence still works.
    if saw_fence {
        let mut inside: Vec<&str> = Vec::new();
        let mut on = false;
        for line in raw.lines() {
            if line.trim().starts_with("```") { on = !on; continue; }
            if on { inside.push(line); }
        }
        if !inside.is_empty() { lines = inside; }
    }
    // Drop lines that cannot be IR: no known verb and not a comment.
    let keep: Vec<&str> = lines.into_iter()
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() { return false; }
            if t.starts_with('#') { return true; }
            let verb = t.split_whitespace().next().unwrap_or("");
            KNOWN_VERBS.contains(&verb)
        })
        .collect();
    let mut out = keep.join("\n");
    if !out.is_empty() { out.push('\n'); }
    out
}

/// Verbs the plan parser accepts. Kept here so `clean_reply` and the
/// parser cannot drift; there is a test asserting each one parses.
pub const KNOWN_VERBS: &[&str] = &[
    "terraform", "path", "enclose", "cylinder", "tower", "sphere", "dome",
    "tree", "scatter", "restyle", "fill", "box",
];

// ── in-process HTTP: no subprocess, no dependencies ─────────────────
//
// Adapted from `aperture_complete::providers::OllamaProvider`, which is
// itself lifted from logscope-fan's ollama client: raw `TcpStream`,
// HTTP/1.1 written by hand, targeted string-field extraction rather
// than a JSON parser, and de-chunking because a local server will use
// `Transfer-Encoding: chunked` as soon as a reply is non-trivial.
//
// Why this and not a subprocess: a script per request pays process
// start plus MODEL LOAD every time. A 3B reloads in seconds, an 8B in
// tens of them, and the game is frozen for all of it. A resident
// server loads once and answers in a fraction of that. The subprocess
// provider stays for anything exotic; this is the default worth using.

/// Which local server dialect to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// ollama: POST /api/generate, fields `system` + `prompt`.
    Ollama,
    /// llama.cpp server / any OpenAI-compatible: POST /v1/chat/completions.
    OpenAi,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpConfig {
    /// `host:port`, e.g. `127.0.0.1:11434` (ollama) or `127.0.0.1:8080`
    /// (llama-server).
    pub host: String,
    pub dialect: Dialect,
    /// Model name the server knows. Ignored by llama-server, which
    /// serves whatever it was started with.
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub timeout: std::time::Duration,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1:11434".into(),
            dialect: Dialect::Ollama,
            model: "qwen2.5-coder:7b".into(),
            temperature: 0.3,
            max_tokens: 512,
            // Generous: a cold model on CPU is slow, and a timeout that
            // fires mid-generation looks identical to a crash.
            timeout: std::time::Duration::from_secs(180),
        }
    }
}

impl HttpConfig {
    /// Parse `UNENDLICH_BUILD_HTTP`, e.g.
    /// `ollama:127.0.0.1:11434:qwen2.5-coder:7b` or
    /// `openai:127.0.0.1:8080`.
    pub fn parse(spec: &str) -> Option<Self> {
        let mut it = spec.splitn(3, ':');
        let dialect = match it.next()?.to_lowercase().as_str() {
            "ollama" => Dialect::Ollama,
            "openai" | "llama" | "llamacpp" => Dialect::OpenAi,
            _ => return None,
        };
        let host_port = it.next()?;
        let rest = it.next().unwrap_or("");
        // host may carry the port: `127.0.0.1:11434:model`
        let (host, model) = match rest.split_once(':') {
            Some((port, model)) if port.chars().all(|c| c.is_ascii_digit()) =>
                (format!("{host_port}:{port}"), model.to_string()),
            _ if rest.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty() =>
                (format!("{host_port}:{rest}"), String::new()),
            _ => (host_port.to_string(), rest.to_string()),
        };
        let mut cfg = HttpConfig { host, dialect, ..Default::default() };
        if !model.is_empty() { cfg.model = model; }
        Some(cfg)
    }
}

/// Minimal JSON string escaping. Hand-rolled for the same reason
/// aperture hand-rolls it: one function beats a dependency for the
/// three fields we send.
pub fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 16);
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

/// Build the HTTP/1.1 request text.
pub fn build_http(cfg: &HttpConfig, system: &str, user: &str) -> String {
    let (path, body) = match cfg.dialect {
        Dialect::Ollama => (
            "/api/generate",
            format!(
                "{{\"model\":\"{}\",\"stream\":false,\"system\":\"{}\",\"prompt\":\"{}\",\
                  \"options\":{{\"temperature\":{},\"num_predict\":{}}}}}",
                json_escape(&cfg.model), json_escape(system), json_escape(user),
                cfg.temperature, cfg.max_tokens),
        ),
        Dialect::OpenAi => (
            "/v1/chat/completions",
            format!(
                "{{\"model\":\"{}\",\"stream\":false,\"temperature\":{},\
                  \"max_tokens\":{},\"messages\":[\
                  {{\"role\":\"system\",\"content\":\"{}\"}},\
                  {{\"role\":\"user\",\"content\":\"{}\"}}]}}",
                json_escape(&cfg.model), cfg.temperature, cfg.max_tokens,
                json_escape(system), json_escape(user)),
        ),
    };
    format!(
        "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        cfg.host, body.len())
}

/// Strip HTTP headers and de-chunk. A local server switches to
/// `Transfer-Encoding: chunked` as soon as the reply is non-trivial,
/// so relying on the lucky Content-Length case truncates every real
/// answer.
pub fn split_body(raw: &str) -> String {
    let Some((head, body)) = raw.split_once("\r\n\r\n") else { return raw.to_string() };
    let chunked = head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    if !chunked { return body.to_string(); }
    // Work in BYTES. Chunk sizes are byte counts and a server may split
    // anywhere, including inside a multi-byte character -- slicing the
    // &str at such an index panics, which would crash the game mid-build
    // on any accented character in a model's reply. Reassemble the bytes
    // first, then decode once at the end.
    let mut out: Vec<u8> = Vec::new();
    let mut rest: &[u8] = body.as_bytes();
    loop {
        let Some(nl) = find_crlf(rest) else { break };
        let Ok(size_line) = std::str::from_utf8(&rest[..nl]) else { break };
        let Ok(size) = usize::from_str_radix(size_line.trim(), 16) else { break };
        let tail = &rest[nl + 2..];
        if size == 0 { break; }
        if tail.len() < size {
            out.extend_from_slice(tail);
            break;
        }
        out.extend_from_slice(&tail[..size]);
        let after = &tail[size..];
        rest = after.strip_prefix(b"\r\n").unwrap_or(after);
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn find_crlf(b: &[u8]) -> Option<usize> {
    b.windows(2).position(|w| w == b"\r\n")
}

/// Pull one string field out of a JSON object without a parser.
/// Handles escaped quotes; returns the unescaped value.
pub fn find_string_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut from = 0usize;
    while let Some(k) = json[from..].find(&needle) {
        let after = from + k + needle.len();
        let rest = &json[after..];
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix(':') else { from = after; continue };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('"') else { from = after; continue };
        let mut out = String::new();
        let mut esc = false;
        for c in rest.chars() {
            if esc {
                out.push(match c { 'n' => '\n', 't' => '\t', 'r' => '\r', o => o });
                esc = false;
            } else if c == '\\' { esc = true; }
            else if c == '"' { return Some(out); }
            else { out.push(c); }
        }
        from = after;
    }
    None
}

/// Extract the completion text from either dialect's reply.
pub fn parse_response(raw: &str) -> Result<String, String> {
    let body = split_body(raw);
    if let Some(e) = find_string_field(&body, "error") {
        return Err(format!("server: {e}"));
    }
    // ollama: {"response":"..."}   openai: {...,"message":{"content":"..."}}
    if let Some(r) = find_string_field(&body, "response") { return Ok(r); }
    if let Some(c) = find_string_field(&body, "content") { return Ok(c); }
    let head: String = body.chars().take(200).collect();
    Err(format!("no completion field in reply: {head}"))
}

/// In-process provider. Connects, writes, reads, closes.
pub struct HttpProvider {
    pub cfg: HttpConfig,
}

impl HttpProvider {
    pub fn new(cfg: HttpConfig) -> Self { Self { cfg } }
}

impl Completer for HttpProvider {
    fn name(&self) -> &str {
        match self.cfg.dialect { Dialect::Ollama => "ollama", Dialect::OpenAi => "openai" }
    }
    fn complete(&mut self, system: &str, user: &str) -> Result<String, String> {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        let req = build_http(&self.cfg, system, user);
        let mut stream = TcpStream::connect(&self.cfg.host)
            .map_err(|e| format!("cannot reach {} ({e}) -- is the server running?", self.cfg.host))?;
        stream.set_read_timeout(Some(self.cfg.timeout)).ok();
        stream.set_write_timeout(Some(self.cfg.timeout)).ok();
        stream.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).map_err(|e| format!("read: {e}"))?;
        parse_response(&String::from_utf8_lossy(&buf))
    }
}

/// A scripted completer for tests and for driving the game headlessly.
/// Returns replies in order; repeats the last one once exhausted, so a
/// loop that asks twice does not panic.
pub struct FakeCompleter {
    replies: Vec<String>,
    calls: std::cell::RefCell<usize>,
    /// Every (system, user) pair seen, for assertions.
    pub seen: std::cell::RefCell<Vec<(String, String)>>,
}

impl FakeCompleter {
    pub fn new(replies: Vec<String>) -> Self {
        Self { replies, calls: std::cell::RefCell::new(0), seen: Default::default() }
    }
    pub fn calls(&self) -> usize { *self.calls.borrow() }
}

impl Completer for FakeCompleter {
    fn name(&self) -> &str { "fake" }
    fn complete(&mut self, system: &str, user: &str) -> Result<String, String> {
        self.seen.borrow_mut().push((system.to_string(), user.to_string()));
        let mut n = self.calls.borrow_mut();
        let i = (*n).min(self.replies.len().saturating_sub(1));
        *n += 1;
        self.replies.get(i).cloned().ok_or_else(|| "no scripted reply".to_string())
    }
}

/// A completer that always fails, for exercising the error path. A
/// build loop that has never been tested against a dead server will
/// hang the first time llama-server is not running.
pub struct DeadCompleter(pub String);
impl Completer for DeadCompleter {
    fn name(&self) -> &str { "dead" }
    fn complete(&mut self, _s: &str, _u: &str) -> Result<String, String> { Err(self.0.clone()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parse_plan_at;

    #[test]
    fn fences_and_chatter_are_stripped() {
        let raw = "Sure! Here's a plan for a watchtower:\n\
                   ```text\n\
                   # watchtower\n\
                   terraform @-5,-5 @5,5 smooth block=stone\n\
                   cylinder @0,0 r=3 h=11 block=stone hollow\n\
                   ```\n\
                   Let me know if you'd like it taller!";
        let c = clean_reply(raw);
        assert!(c.starts_with("# watchtower"), "{c}");
        assert!(c.contains("cylinder @0,0"));
        assert!(!c.contains("Sure"));
        assert!(!c.contains("taller"));
        let (plan, issues) = parse_plan_at(&c, Some((0, 65, 0)));
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(plan.ops.len(), 2);
        assert_eq!(plan.name, "watchtower");
    }

    #[test]
    fn an_unfenced_reply_with_chatter_still_parses() {
        let raw = "Okay, placing a tower here.\ncylinder @0,0 r=2 h=8\nDone!";
        let c = clean_reply(raw);
        assert_eq!(c, "cylinder @0,0 r=2 h=8\n");
    }

    #[test]
    fn an_unterminated_fence_does_not_lose_the_plan() {
        let raw = "```\ntree @0,0 oak h=7\n";
        assert_eq!(clean_reply(raw), "tree @0,0 oak h=7\n");
    }

    #[test]
    fn a_reply_with_no_plan_cleans_to_nothing() {
        let c = clean_reply("I'm not sure what you mean by that.");
        assert!(c.is_empty(), "{c:?}");
        let (plan, _) = parse_plan_at(&c, Some((0, 65, 0)));
        assert!(plan.ops.is_empty(), "an empty plan is rejected downstream, not silently built");
    }

    #[test]
    fn every_known_verb_actually_parses() {
        // the guard against clean_reply and the parser drifting apart
        for verb in KNOWN_VERBS {
            let line = match *verb {
                "terraform" | "path" | "enclose" | "scatter" => format!("{verb} @0,0 @4,4\n"),
                "restyle" => "restyle @0,0,0 @4,4,4 stone oak\n".to_string(),
                "fill" | "box" => format!("{verb} @0,0,0 @2,2,2\n"),
                "tree" => "tree @0,0 oak\n".to_string(),
                _ => format!("{verb} @0,0 r=2 h=4\n"),
            };
            let (plan, issues) = parse_plan_at(&line, Some((0, 65, 0)));
            assert!(issues.is_empty() && plan.ops.len() == 1,
                    "verb '{verb}' in KNOWN_VERBS does not parse: {line:?} {issues:?}");
        }
    }

    #[test]
    fn the_fake_records_what_it_was_asked() {
        let mut f = FakeCompleter::new(vec!["tree @0,0 oak h=6\n".into()]);
        let r = f.complete("SYS", "USER").unwrap();
        assert_eq!(r, "tree @0,0 oak h=6\n");
        assert_eq!(f.calls(), 1);
        assert_eq!(f.seen.borrow()[0].0, "SYS");
        // exhausted: repeats rather than panicking
        assert!(f.complete("SYS", "AGAIN").is_ok());
    }


    #[test]
    fn a_shell_script_is_a_model() {
        // the whole point of the process style: no HTTP in this crate
        let mut p = CommandProvider::new(vec![
            "sh".into(), "-c".into(),
            // read stdin, ignore it, emit a plan
            "cat >/dev/null; printf 'cylinder @0,0 r=2 h=6\\n'".into()]);
        let out = p.complete("SYS", "USER").unwrap();
        assert_eq!(out, "cylinder @0,0 r=2 h=6\n");
        assert_eq!(p.name(), "sh");
    }

    #[test]
    fn the_prompt_arrives_as_system_nul_user() {
        let mut p = CommandProvider::new(vec![
            "sh".into(), "-c".into(),
            // split on NUL and echo the halves back as comments
            "tr '\\0' '\\n' | sed 's/^/# /'".into()]);
        let out = p.complete("SYSTEM_HALF", "USER_HALF").unwrap();
        assert!(out.contains("# SYSTEM_HALF"), "{out}");
        assert!(out.contains("# USER_HALF"), "{out}");
    }

    #[test]
    fn a_failing_provider_reports_its_stderr_not_a_pipe_error() {
        let mut p = CommandProvider::new(vec![
            "sh".into(), "-c".into(), "echo 'model file missing' >&2; exit 3".into()]);
        let e = p.complete("s", "u").unwrap_err();
        assert!(e.contains("model file missing"), "{e}");
        assert!(!e.contains("Broken pipe"), "EPIPE must not mask the real report: {e}");
    }

    #[test]
    fn a_provider_that_never_reads_stdin_still_reports_properly() {
        // exits immediately, closing the pipe mid-write
        let mut p = CommandProvider::new(vec![
            "sh".into(), "-c".into(), "exit 1".into()]);
        let e = p.complete(&"x".repeat(200_000), "u").unwrap_err();
        assert!(e.contains("exited"), "{e}");
    }

    #[test]
    fn a_missing_program_is_a_clear_message() {
        let mut p = CommandProvider::new(vec!["definitely-not-a-real-binary-xyz".into()]);
        let e = p.complete("s", "u").unwrap_err();
        assert!(e.starts_with("spawn definitely-not-a-real-binary-xyz"), "{e}");
    }

    #[test]
    fn model_path_reaches_the_child_environment() {
        let mut p = CommandProvider::with_model(
            vec!["sh".into(), "-c".into(), "cat >/dev/null; echo \"# $VOXEL_MODEL\"".into()],
            std::path::Path::new("/home/u/models/qwen.gguf"));
        let out = p.complete("s", "u").unwrap();
        assert!(out.contains("/home/u/models/qwen.gguf"), "{out}");
    }

    #[test]
    fn ollama_and_openai_requests_are_well_formed() {
        let mut cfg = HttpConfig::default();
        let r = build_http(&cfg, "SYS", "USER");
        assert!(r.starts_with("POST /api/generate HTTP/1.1"), "{r}");
        assert!(r.contains("Host: 127.0.0.1:11434"));
        assert!(r.contains(r#""system":"SYS""#) && r.contains(r#""prompt":"USER""#));
        // Content-Length must match the body exactly or the server hangs
        let (head, body) = r.split_once("\r\n\r\n").unwrap();
        let declared: usize = head.lines()
            .find_map(|l| l.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
        assert_eq!(declared, body.len(), "Content-Length must equal the body");

        cfg.dialect = Dialect::OpenAi;
        let r = build_http(&cfg, "SYS", "USER");
        assert!(r.starts_with("POST /v1/chat/completions"), "{r}");
        assert!(r.contains(r#""role":"system""#) && r.contains(r#""role":"user""#));
    }

    #[test]
    fn prompts_with_quotes_and_newlines_survive_escaping() {
        let cfg = HttpConfig::default();
        let r = build_http(&cfg, "a \"quoted\" line\nnext", "user");
        assert!(r.contains(r#"a \"quoted\" line\nnext"#), "{r}");
        let (head, body) = r.split_once("\r\n\r\n").unwrap();
        let declared: usize = head.lines()
            .find_map(|l| l.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
        assert_eq!(declared, body.len(), "escaping must not desync the length");
    }

    #[test]
    fn a_chunked_reply_is_reassembled() {
        // The case that truncates every real answer if ignored. Chunk
        // sizes are COMPUTED, not hand-written: my first version of this
        // test invented the hex lengths and failed against correct code,
        // which is the least useful kind of red.
        let parts = [r#"{"response":"tower @0,0 "#, r#"r=3 h=9"}"#];
        let mut raw = String::from("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
        for p in parts {
            raw.push_str(&format!("{:x}\r\n{p}\r\n", p.len()));
        }
        raw.push_str("0\r\n\r\n");
        let body = split_body(&raw);
        assert_eq!(body, r#"{"response":"tower @0,0 r=3 h=9"}"#, "reassembled");
        assert_eq!(parse_response(&raw).unwrap(), "tower @0,0 r=3 h=9");
    }

    #[test]
    fn a_chunk_boundary_inside_a_multibyte_char_does_not_panic() {
        // A server may split anywhere; slicing on a byte index inside a
        // UTF-8 sequence would panic, and a crash mid-build is worse
        // than a garbled character.
        let text = "caf\u{e9} \u{2014} tower";
        let bytes = text.as_bytes();
        let (a, b) = bytes.split_at(4); // lands inside the 2-byte 'é'
        let mut raw = String::from("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
        raw.push_str(&format!("{:x}\r\n{}\r\n", a.len(), String::from_utf8_lossy(a)));
        raw.push_str(&format!("{:x}\r\n{}\r\n", b.len(), String::from_utf8_lossy(b)));
        raw.push_str("0\r\n\r\n");
        let _ = split_body(&raw); // must not panic
    }

    #[test]
    fn both_dialects_reply_shapes_parse() {
        let ollama = "HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n{\"response\":\"cylinder @0,0 r=2 h=6\"}";
        assert_eq!(parse_response(ollama).unwrap(), "cylinder @0,0 r=2 h=6");
        let openai = "HTTP/1.1 200 OK\r\n\r\n{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"tree @0,0 oak\"}}]}";
        assert_eq!(parse_response(openai).unwrap(), "tree @0,0 oak");
    }

    #[test]
    fn a_server_error_is_reported_not_swallowed() {
        let e = parse_response("HTTP/1.1 404\r\n\r\n{\"error\":\"model 'nope' not found\"}").unwrap_err();
        assert!(e.contains("model 'nope' not found"), "{e}");
        let e2 = parse_response("HTTP/1.1 200 OK\r\n\r\n{\"unexpected\":1}").unwrap_err();
        assert!(e2.contains("no completion field"), "{e2}");
    }

    #[test]
    fn escaped_quotes_inside_a_value_do_not_end_it_early() {
        let body = r#"{"response":"say \"hi\" then stop"}"#;
        assert_eq!(find_string_field(body, "response").unwrap(), r#"say "hi" then stop"#);
    }

    #[test]
    fn config_specs_parse() {
        let c = HttpConfig::parse("ollama:127.0.0.1:11434:qwen2.5-coder:7b").unwrap();
        assert_eq!(c.host, "127.0.0.1:11434");
        assert_eq!(c.dialect, Dialect::Ollama);
        assert_eq!(c.model, "qwen2.5-coder:7b");
        let c = HttpConfig::parse("openai:127.0.0.1:8080").unwrap();
        assert_eq!(c.host, "127.0.0.1:8080");
        assert_eq!(c.dialect, Dialect::OpenAi);
        assert!(HttpConfig::parse("carrier-pigeon:x").is_none());
    }

    #[test]
    fn an_unreachable_server_says_so_plainly() {
        let mut p = HttpProvider::new(HttpConfig {
            host: "127.0.0.1:1".into(), ..Default::default() });
        let e = p.complete("s", "u").unwrap_err();
        assert!(e.contains("cannot reach 127.0.0.1:1"), "{e}");
        assert!(e.contains("is the server running"), "actionable: {e}");
    }
}
