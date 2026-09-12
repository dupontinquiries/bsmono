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
/// `Send` because generation runs on a worker thread: a 1B took 22.5
/// seconds in the field, and doing that on the frame thread is a dead
/// window, not a pause. Costs nothing in practice — aperture's
/// `Backend` is already `Send + Sync`, a subprocess handle is Send, and
/// a TcpStream is Send. The compiler asked for this the moment the work
/// moved off-thread.
/// Live control of a turn in flight: a cancel flag the generator
/// checks between steps, and a progress slot it updates so the chat
/// can show tokens and rate during a wait that may be minutes long.
#[derive(Debug, Default)]
pub struct TurnControl {
    pub cancel: std::sync::atomic::AtomicBool,
    /// (tokens so far, text so far). Replaced whole each update, so a
    /// reader never sees a torn string.
    pub progress: std::sync::Mutex<(usize, String)>,
    /// A per-line validator. A provider that drives its own decode loop
    /// rejects and resamples a line this refuses, so a small model
    /// cannot emit a line that does not parse. A fn pointer, not a
    /// closure: nothing to box, nothing to send.
    pub line_check: Option<fn(&str) -> bool>,
    /// Lines the provider took back and resampled, for the log.
    pub retried: std::sync::atomic::AtomicUsize,
}

impl TurnControl {
    pub fn cancelled(&self) -> bool { self.cancel.load(std::sync::atomic::Ordering::Relaxed) }
    pub fn cancel(&self) { self.cancel.store(true, std::sync::atomic::Ordering::Relaxed); }
    pub fn report(&self, tokens: usize, text: &str) {
        if let Ok(mut p) = self.progress.lock() { *p = (tokens, text.to_string()); }
    }
    pub fn snapshot(&self) -> (usize, String) {
        self.progress.lock().map(|p| p.clone()).unwrap_or_default()
    }
}

pub trait Completer: Send {
    fn name(&self) -> &str { "completer" }
    /// Blocking. Call it off the frame thread.
    fn complete(&mut self, system: &str, user: &str) -> Result<String, String>;

    /// `complete_with` for a CONVERSATION: prior (user, reply) exchanges
    /// before this user turn. A provider with a KV cache keeps the
    /// chain's checkpoints and prefills only what is new. The default
    /// folds the history into the system text, which is correct and
    /// merely unpaced by any cache.
    fn complete_chain(&mut self, system: &str, prior: &[(String, String)], user: &str,
                      ctl: &TurnControl) -> Result<String, String> {
        if prior.is_empty() { return self.complete_with(system, user, ctl); }
        let mut sys = system.to_string();
        for (u, a) in prior { sys.push_str(&format!("\n\nUser: {u}\nAssistant: {a}")); }
        self.complete_with(&sys, user, ctl)
    }

    /// `complete` with live control. The default ignores the control
    /// and reports once at the end -- correct for a provider that
    /// cannot stream. A provider that can should check `ctl.cancelled()`
    /// between steps and `ctl.report()` as text arrives, so /cancel
    /// actually stops the GPU rather than letting it finish a reply
    /// nobody will read.
    fn complete_with(&mut self, system: &str, user: &str, ctl: &TurnControl)
        -> Result<String, String> {
        let r = self.complete(system, user);
        if let Ok(t) = &r { ctl.report(t.split_whitespace().count(), t); }
        if ctl.cancelled() { return Err("cancelled".into()); }
        r
    }
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
/// Strip code fences and obvious prose, WITHOUT the verb filter.
///
/// `clean_reply` keeps only lines whose first word is a known IR verb,
/// which is right for a plan and destroys a program: `part`, `rel`,
/// `pass`, `asset`, `frame` and `end` are not verbs, and a program run
/// through it comes back empty. A program's own parser reports what it
/// cannot read, so the job here is only to remove the wrapper a model
/// puts around code.
/// Did this reply run out of room rather than finish?
///
/// A reply that hits its token cap stops MID-LINE, and nothing
/// detected it: the half-written program failed to parse and the
/// player was told "produced no plan lines", which is true and
/// useless. The tells are structural -- an unclosed block, a line with
/// no newline after it, a trailing key with no value.
pub fn looks_truncated(raw: &str) -> bool {
    let t = raw.trim_end();
    if t.is_empty() { return false; }
    // ended in the middle of a word or a key=value
    if t.ends_with('=') || t.ends_with(',') || t.ends_with('{') { return true; }
    // a block was opened and never closed
    let opens = t.lines().filter(|l| {
        let h = l.trim().split_whitespace().next().unwrap_or("").to_lowercase();
        ["asset", "frame", "pass", "flair"].contains(&h.as_str())
    }).count();
    let closes = t.lines().filter(|l| l.trim().eq_ignore_ascii_case("end")).count();
    if opens > closes { return true; }
    // the last line looks like the start of a statement that needs more
    let last = t.lines().last().unwrap_or("").trim().to_lowercase();
    ["part", "rel", "repeat", "space", "adj", "section"].iter()
        .any(|k| last == *k || (last.starts_with(k) && last.split_whitespace().count() < 3))
}

pub fn strip_fences(raw: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    let mut saw_fence = false;
    for line in raw.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            saw_fence = true;
            continue;
        }
        if in_fence { out.push_str(line); out.push('\n'); }
    }
    if saw_fence && !out.trim().is_empty() { return out; }
    // no fence: keep the lines that could be program source, which is
    // anything starting with one of its keywords or indented under one
    raw.lines()
        .filter(|l| {
            let t = l.trim().to_lowercase();
            t.is_empty() || t.starts_with('#')
                || ["asset", "frame", "pass", "part", "rel", "repeat", "stamp", "end"]
                    .iter().any(|k| t.starts_with(&format!("{k} ")) || t == *k)
        })
        .map(|l| format!("{l}\n")).collect()
}

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
    // Drop lines that cannot be IR: no known verb, or a verb with
    // placeholder arguments.
    let keep: Vec<&str> = lines.into_iter()
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() { return false; }
            if t.starts_with('#') { return true; }
            let verb = t.split_whitespace().next().unwrap_or("");
            KNOWN_VERBS.contains(&verb) && !is_grammar_echo(t)
        })
        .collect();
    let mut out = keep.join("\n");
    if !out.is_empty() { out.push('\n'); }
    out
}

/// Is this line the GRAMMAR echoed back rather than a plan?
///
/// A small model handed a long instruction prompt will often continue
/// it by repeating it. The echoed lines start with a real verb, so a
/// verb check alone keeps them — and a 1B produced exactly this:
/// `terraform A B [delta=N] [smooth] [block=B]`, eight of them, which
/// reached the parser as eight ops and came back as eight "needs two
/// x,z pairs" errors. The errors were right; the input was never a
/// plan.
///
/// Signatures of the reference form, none of which appear in real IR:
/// square brackets or angle brackets (optional-argument notation), and
/// bare uppercase placeholders standing in for coordinates.
pub fn is_grammar_echo(line: &str) -> bool {
    if line.contains('[') || line.contains(']') || line.contains('<') || line.contains('>') {
        return true;
    }
    const PLACEHOLDERS: &[&str] = &["A", "B", "N", "X", "Y", "Z", "SPECIES", "FROM", "TO",
                                    "BLOCK", "PALETTE", "CMD"];
    line.split_whitespace().skip(1).any(|tok| {
        let bare = tok.trim_end_matches(',');
        PLACEHOLDERS.contains(&bare)
            // `block=B`, `r=N` -- a placeholder on the right of a key
            || bare.split_once('=').is_some_and(|(_, v)| PLACEHOLDERS.contains(&v))
    })
}

/// Verbs the plan parser accepts. Kept here so `clean_reply` and the
/// parser cannot drift; there is a test asserting each one parses.
/// Every verb the parser accepts.
///
/// THIS LIST WAS TWELVE VERBS OLD. `clean_reply` drops any line whose
/// verb is not here, so every op added since -- form, spec, plant,
/// mood, populate, greeble, road, river, lake, brush, layer, weather,
/// detail, def/use, place -- was being silently deleted from every
/// model reply before the plan ever saw it. The spec line in
/// particular: the tuner had nothing to chase and the run tracked no
/// distance, which is how it was found.
///
/// Kept in one place with a test that every verb the parser knows is
/// in it, so the next op cannot go missing the same way.
pub const KNOWN_VERBS: &[&str] = &[
    // geometry
    "terraform", "path", "enclose", "cylinder", "tower", "sphere", "dome",
    "tree", "scatter", "restyle", "fill", "box", "roof", "floor", "bridge",
    "stairs", "mirror",
    // the dictionary and its axes
    "form", "build", "place", "stamp", "spec", "properties",
    // living things and atmosphere
    "plant", "grow", "mood", "atmosphere", "populate",
    // surfaces
    "detail", "weather", "greeble", "decorate", "views",
    // landscape
    "brush", "layer", "river", "road", "lake", "world",
    // carved terrain: what a heightfield cannot say
    "canyon", "cave", "mesa", "waterfall", "overhang", "gorge",
    // composite places
    "feature",
    // vertical access: the answer to a cliff is not a road
    "cliff_stair", "stair", "switchback", "shaft", "terrace",
    // substructures
    "def", "end", "use",
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
                // keep_alive holds the model resident between turns, and
                // ollama reuses the prompt context when the prefix matches;
                // both are what make the second turn faster than the first.
                "{{\"model\":\"{}\",\"stream\":false,\"keep_alive\":\"30m\",\
                  \"system\":\"{}\",\"prompt\":\"{}\",\
                  \"options\":{{\"temperature\":{},\"num_predict\":{}}}}}",
                json_escape(&cfg.model), json_escape(system), json_escape(user),
                cfg.temperature, cfg.max_tokens),
        ),
        Dialect::OpenAi => (
            "/v1/chat/completions",
            format!(
                // cache_prompt is llama.cpp's server flag for reusing the
                // KV of a matching prefix; other OpenAI-compatible servers
                // ignore an unknown field.
                "{{\"model\":\"{}\",\"stream\":false,\"cache_prompt\":true,\"temperature\":{},\
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
        // THE GUARD against clean_reply and the parser drifting apart --
        // and it caught the drift the other way too: the list was
        // twelve verbs old, so form, spec, plant and the rest were
        // being deleted from every reply. A sample per verb, because a
        // generic `@0,0 r=2 h=4` is not valid for most of them.
        for verb in KNOWN_VERBS {
            let line = match *verb {
                "terraform" | "path" | "enclose" => format!("{verb} @0,0 @4,4\n"),
                "scatter" => "scatter @-4,-4 @4,4 block=flower clusters=3 per=4\n".into(),
                "restyle" => "restyle @0,0,0 @4,4,4 stone oak\n".into(),
                "fill" | "box" => format!("{verb} @0,0,0 @2,2,2\n"),
                "tree" => "tree @0,0 oak\n".into(),
                "cylinder" | "tower" => format!("{verb} @0,0 r=3 h=9\n"),
                "sphere" | "dome" => format!("{verb} @0,0 r=3\n"),
                "roof" => "roof @-3,-3 @3,3 gable\n".into(),
                "floor" => "floor @-3,-3 @3,3 y=4\n".into(),
                "bridge" | "stairs" => format!("{verb} @0,0 @9,0\n"),
                "mirror" => "mirror @0,0,0 @4,4,4 x\n".into(),
                "form" | "build" => format!("{verb} house @-4,-4 @4,4\n"),
                "place" | "stamp" => continue,          // needs a captured template
                "spec" | "properties" => format!("{verb} height=5-7 durability=high\n"),
                "plant" | "grow" => format!("{verb} @0,0 oak tall\n"),
                "mood" | "atmosphere" => format!("{verb} eerie @-4,-4 @4,4\n"),
                "populate" => "populate deer @-9,-9 @9,9 count=3\n".into(),
                "detail" => "detail speckle @-3,-3,-3 @3,3,3\n".into(),
                "weather" => "weather @-3,-3,-3 @3,3,3 worn\n".into(),
                "greeble" | "decorate" => format!("{verb} @-3,-1,-3 @3,5,3 gothic\n"),
                "views" => continue,                    // a multi-line block
                "brush" => "brush raise @0,0 by=6 r=20\n".into(),
                "layer" => "layer boreal @-20,-20 @20,20\n".into(),
                "river" => "river @0,0 len=40\n".into(),
                "road" => "road @0,0 @40,40\n".into(),
                "lake" => "lake @0,0 r=10 level=66\n".into(),
                "canyon" | "gorge" => format!("{verb} @0,0 @60,0 w=12 depth=18\n"),
                "cave" => "cave @0,0 dir=180 len=60 r=4 branches=2\n".into(),
                "mesa" => "mesa @0,0 r=24 drop=16\n".into(),
                "waterfall" => "waterfall @0,0 dir=180 w=6 h=14\n".into(),
                "overhang" => "overhang @0,0 dir=180 w=14 depth=8 h=5\n".into(),
                "feature" => "feature badlands @0,0 r=120\n".into(),
                "cliff_stair" | "stair" => format!("{verb} @0,0 dir=180 to=64 w=3\n"),
                "switchback" => "switchback @0,0 dir=0 to=100 w=3 leg=20\n".into(),
                "shaft" => "shaft @0,0 to=40 r=2\n".into(),
                "terrace" => "terrace @0,0 @60,60 step=4\n".into(),
                "strata" => continue,        // a k=v on carves, not a verb
                "world" => continue,                    // gated on the player's intent
                "def" | "end" | "use" => continue,      // substructure syntax, not ops
                other => panic!("KNOWN_VERBS gained '{other}' with no sample line here"),
            };
            let (plan, issues) = parse_plan_at(&line, Some((0, 65, 0)));
            let errs: Vec<_> = issues.iter().filter(|i| i.severity == crate::plan::Severity::Error).collect();
            assert!(errs.is_empty() && !plan.ops.is_empty(),
                    "verb '{verb}' in KNOWN_VERBS does not parse: {line:?} {errs:?}");
            assert!(clean_reply(line.trim()).contains(*verb), "clean_reply drops '{verb}'");
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

    #[test]
    fn the_grammar_echoed_back_is_not_a_plan() {
        // Exactly what a 1B returned, verbatim from the field log.
        let raw = "terraform A B [delta=N] [smooth] [block=B]\n\
                   path A B [w=N] [block=B]\n\
                   enclose A B [h=N] [block=B]\n\
                   cylinder A [r=N] [h=N] [block=B] [hollow]\n\
                   sphere A [r=N] [block=B] [hollow]\n\
                   tree A SPECIES [h=N] [lean=dx,dz]\n\
                   scatter A B [block=B] [clusters=N] [per=N]\n\
                   restyle A B FROM TO\n";
        assert_eq!(clean_reply(raw), "", "grammar echo must not reach the parser");
    }

    #[test]
    fn real_ir_survives_the_echo_filter() {
        let raw = "# hut\n\
                   terraform @-5,-5 @5,5 smooth block=stone\n\
                   cylinder @0,0 r=3 h=9 block=stone_bricks hollow\n\
                   tree @8,2 oak h=7\n\
                   restyle @-4,-1,-4 @4,15,4 stone ruined\n";
        let c = clean_reply(raw);
        assert_eq!(c.lines().count(), 5, "{c}");
        let (plan, issues) = crate::plan::parse_plan_at(&c, Some((0, 65, 0)));
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(plan.ops.len(), 4);
    }

    #[test]
    fn a_half_echo_keeps_the_real_lines_only() {
        // models mix the two: some real ops, then it drifts into the reference
        let raw = "cylinder @0,0 r=3 h=9 block=stone\n\
                   path A B [w=N] [block=B]\n\
                   tree @5,5 oak\n";
        assert_eq!(clean_reply(raw), "cylinder @0,0 r=3 h=9 block=stone\ntree @5,5 oak\n");
    }

    #[test]
    fn http_bodies_ask_for_prefix_reuse() {
        let o = build_http(&HttpConfig { dialect: Dialect::Ollama, ..HttpConfig::default() }, "SYS", "USER");
        assert!(o.contains("\"keep_alive\":\"30m\""), "{o}");
        let l = build_http(&HttpConfig { dialect: Dialect::OpenAi, ..HttpConfig::default() }, "SYS", "USER");
        assert!(l.contains("\"cache_prompt\":true"), "{l}");
    }


    #[test]
    fn strip_fences_keeps_program_source_where_clean_reply_would_destroy_it() {
        let reply = "Here is the program:\n\
                     ```\n\
                     asset t\n\
                       part x box w=2 material=stone\n\
                     end\n\
                     pass a add\n\
                       repeat t ring radius=10 count=6\n\
                     end\n\
                     ```\n\
                     Hope that helps!";
        let s = strip_fences(reply);
        assert!(s.contains("asset t") && s.contains("pass a add") && s.contains("repeat t"), "{s}");
        assert!(!s.contains("Hope that helps"));
        let (prog, errs) = crate::program::parse(&s);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(prog.passes.len(), 1);
        // And the verb filter really would have destroyed it: `end` is
        // the only line it keeps, because `end` happens to collide with
        // an IR verb. Two orphan `end`s parse to no passes at all.
        let wrecked = clean_reply(reply);
        assert!(!wrecked.contains("asset") && !wrecked.contains("pass") && !wrecked.contains("part"),
                "clean_reply keeps no structural line: {wrecked:?}");
        assert!(crate::program::parse(&wrecked).0.passes.is_empty());
    }

    #[test]
    fn strip_fences_works_without_a_fence_and_drops_the_prose() {
        let s = strip_fences("Sure! I will build it like this.\n                              pass a add\n  part x box w=4 material=stone\nend\n                              Let me know if you want changes.");
        assert!(s.contains("pass a add") && s.contains("part x"), "{s}");
        assert!(!s.contains("Sure!") && !s.contains("Let me know"), "{s}");
        let (prog, _) = crate::program::parse(&s);
        assert_eq!(prog.passes.len(), 1);
    }

    #[test]
    fn a_reply_that_ran_out_of_room_is_detectable() {
        // it stops MID-LINE, and the half-written program fails to
        // parse: the player was told "produced no plan lines", which is
        // true and useless
        for cut in ["pass mass add\n  part body box w=20 h=10 d=",
                    "asset merlon\n  part m box w=2 h=3 d=2 material=stone\nend\npass a add\n  part x",
                    "pass a add\n  part x box w=4 material=stone\n",
                    "part"] {
            assert!(looks_truncated(cut), "{cut:?} should look cut off");
        }
        // and a complete one does not
        for whole in ["pass a add\n  part x box w=4 material=stone\n  rel x on ground\nend\n",
                      "IT IS: a giant tree\nMATERIALS: oak\n",
                      "done\n", ""] {
            assert!(!looks_truncated(whole), "{whole:?} is complete");
        }
    }
}
