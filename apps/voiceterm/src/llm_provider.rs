// llm_provider.rs — v0.1.160
//
// Abstraction over the LLM backend used for command parsing,
// summarization, and intent classification. Originally the codebase
// only supported Ollama (local LLMs via the `ollama` CLI). v0.1.160
// added Anthropic Claude as a second option, routed via HTTPS.
//
// DESIGN GOALS
//
//   - **Single dispatch point.** All LLM-bound queries go through
//     `provider_query()`. Adding a third provider later is a single
//     match-arm addition.
//
//   - **Default to Ollama.** The Claude path is opt-in via the
//     user's choice in Settings, and even when chosen, requires
//     an API key. If the key is empty, fall through to Ollama
//     rather than fail silently.
//
//   - **Pre-redact context.** Every prompt going to a network
//     provider gets pushed through `security::context_strip()`
//     first. Local Ollama gets the same treatment because the same
//     pipeline reaches it — defense in depth, no special case.
//
//   - **Conservative timeouts.** Claude API can be slow on long
//     contexts. 30s timeout matches the slowest local summarizer
//     case (the "summariz" path in system_info::ollama_run_raw).
//
//   - **Honest errors.** Network failures, auth failures, and
//     timeouts each return distinct error strings so the UI layer
//     can decide whether to surface or fall back.
//
//   - **No serde dependency.** The project hand-rolls JSON elsewhere
//     (see archive.rs). We follow the same pattern here to keep
//     the dependency surface flat.
//
// CONCERNS / LIMITATIONS
//
//   - The Anthropic API key lives in plaintext in the in-memory
//     Backend struct AND in the settings.conf file. The user
//     explicitly added this feature; we surface that they're
//     opting into key-on-disk storage via the UI label. A future
//     follow-up could move it to the OS keychain.
//
//   - No streaming. The Anthropic /v1/messages endpoint supports
//     SSE streaming for incremental output. We currently use the
//     non-streaming flavor for simplicity; long responses will
//     block until complete. Summarization (the dominant use case
//     here) produces short responses, so blocking is fine.
//
//   - JSON parsing is fragile. Hand-rolled extraction of the first
//     text block from a response works for the current API shape
//     but would break if Anthropic adds new structural sugar. Worth
//     a test suite (see bottom of this file) to catch regressions.

// ────────────────────────────────────────────────────────────────
// Feature gating
//
// The `claude` Cargo feature gates the network-calling code path.
// Rationale: if the Anthropic API or this module's hand-rolled JSON
// parser ever breaks, the user wants to be able to drop the feature
// and rebuild with Ollama-only, no API risk surface.
//
// What's gated:
//   - Anthropic API endpoint constants
//   - `claude_query`, `build_claude_request`, `json_str`,
//     `parse_claude_response`, `extract_text_field`,
//     `unescape_json_string`
//   - The `Provider::Claude` arm in `provider_query`
//
// What's NOT gated (still present even with `claude` off):
//   - The `Provider` enum itself (both variants). Keeps the i32
//     mapping stable across feature configs so QML can still set
//     numeric provider values.
//   - The `from_i32` constructor.
//   - The `provider_query` dispatcher. Just always routes to Ollama
//     when `claude` is off.
//
// ────────────────────────────────────────────────────────────────

#[cfg(feature = "claude")]
use std::time::Duration;

#[cfg(feature = "claude")]
const CLAUDE_API_URL: &str = "https://api.anthropic.com/v1/messages";
#[cfg(feature = "claude")]
const CLAUDE_API_VERSION: &str = "2023-06-01";

// Model choice. claude-haiku-4-5 is the current cheap-and-fast tier;
// matches the "local Ollama summarizer" use profile (short prompts,
// short answers, low latency wanted). Could be made user-configurable
// later. Using the dated alias rather than `-latest` so behavior is
// stable across Anthropic's release cycles.
#[cfg(feature = "claude")]
const CLAUDE_MODEL: &str = "claude-haiku-4-5";

// Per-request timeout. 30s matches the slowest local summarizer
// path. Tight enough that the UI doesn't appear hung; generous
// enough for a short Claude response on a typical context.
#[cfg(feature = "claude")]
const CLAUDE_TIMEOUT_SECS: u64 = 30;

// Max output tokens. Bounds the model's reply length — also bounds
// cost. Summarization replies are usually 50-200 tokens; 512 is
// roomy enough for command-explanation cases too.
#[cfg(feature = "claude")]
const CLAUDE_MAX_TOKENS: u32 = 512;

/// Selection of which LLM backend handles a query. Stored as i32
/// on the Qt Backend (no enum support in qmetaobject without
/// extra plumbing) — see the Backend struct in main.rs.
///
/// Variants exist regardless of feature flags, so the numeric
/// mapping is stable across feature configs. When a feature is
/// OFF, requesting that provider falls through at the dispatch
/// level rather than failing to compile.
///
/// v0.1.462: added `LlamaCpp = 2` for the in-process GGUF
/// backend. When the `llamacpp` feature is off — or on but with
/// no model loaded — it falls through to the network/local
/// path the same way Claude falls through to Ollama.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Ollama = 0,
    Claude = 1,
    LlamaCpp = 2,
}

impl Provider {
    pub fn from_i32(v: i32) -> Provider {
        match v {
            1 => Provider::Claude,
            2 => Provider::LlamaCpp,
            _ => Provider::Ollama,
        }
    }
}

/// Top-level query dispatcher. Pass the user's chosen provider,
/// the API key (only used for Claude; pass empty if none), and the
/// prompt text. Returns the model's reply on success or a string
/// describing the failure mode.
///
/// **Security.** The caller is responsible for redacting secrets
/// from `prompt` BEFORE invoking this function. We don't redact
/// here because some callers (intent classifier) already wrap
/// their prompt in a template — re-redacting could break the
/// template. The dispatch is dumb: send the prompt as-is.
///
/// **Feature gating.** When the `claude` feature is OFF, the
/// `Provider::Claude` arm degrades to Ollama with a log line.
/// `api_key` and `agent` are unused in that build but kept in
/// the signature so call sites in `main.rs` don't need to vary
/// by feature config. The `cfg_attr` below suppresses the
/// otherwise-correct unused-variable warning for that case.
/// v0.1.365: `op_tag` lets callers tell the token-log layer
/// what they were doing — block_summary, context_question,
/// voice_intent_followup, etc. The tag flows into the per-op
/// breakdown rendered at `web2qml://stats/tokens`. Empty tag
/// falls back to "claude_query" for back-compat.
#[cfg_attr(not(feature = "claude"), allow(unused_variables))]
pub fn provider_query(
    provider: Provider,
    api_key: &str,
    prompt: &str,
    agent: &ureq::Agent,
    op_tag: &str,
) -> Result<String, String> {
    match provider {
        Provider::Ollama => {
            crate::system_info::ollama_query_multi(prompt)
        }
        Provider::Claude => {
            #[cfg(feature = "claude")]
            {
                if api_key.trim().is_empty() {
                    eprintln!(
                        "[llm_provider] Claude selected but API key is empty — falling back to Ollama"
                    );
                    return crate::system_info::ollama_query_multi(prompt);
                }
                claude_query(api_key, prompt, agent, op_tag)
            }
            #[cfg(not(feature = "claude"))]
            {
                eprintln!(
                    "[llm_provider] Claude selected but built without `claude` feature — falling back to Ollama"
                );
                crate::system_info::ollama_query_multi(prompt)
            }
        }
        Provider::LlamaCpp => {
            // v0.1.462: this entry point has NO model handle, so
            // llama.cpp can't run here. Callers that have a loaded
            // model use `provider_query_with_local` instead. From
            // this path, fall through to Ollama (local default).
            eprintln!(
                "[llm_provider] LlamaCpp selected via handle-less path — falling back to Ollama"
            );
            crate::system_info::ollama_query_multi(prompt)
        }
    }
}

/// v0.1.462: dispatch variant that can carry a loaded in-process
/// llama.cpp model. Call sites that hold an `Arc<LoadedModel>`
/// (the async summary worker) use this; everyone else uses the
/// handle-less `provider_query`.
///
/// When `provider == LlamaCpp` and `local` is `Some`, runs the
/// generation in-process. When `local` is `None` (no model
/// loaded yet) or the `llamacpp` feature is off, falls through
/// to Ollama. All other providers behave exactly as in
/// `provider_query`.
///
/// `gen_params` controls max tokens / context / sampling for the
/// local path; ignored by the network/Ollama paths.
#[cfg_attr(not(feature = "claude"), allow(unused_variables))]
pub fn provider_query_with_local(
    provider: Provider,
    api_key: &str,
    prompt: &str,
    agent: &ureq::Agent,
    op_tag: &str,
    local: Option<&crate::llama_backend::LoadedModel>,
    gen_params: &crate::llama_backend::GenParams,
) -> Result<String, String> {
    match provider {
        Provider::LlamaCpp => {
            match local {
                Some(model) => {
                    crate::llama_backend::generate(model, prompt, gen_params)
                }
                None => {
                    eprintln!(
                        "[llm_provider] LlamaCpp selected but no model loaded — falling back to Ollama"
                    );
                    crate::system_info::ollama_query_multi(prompt)
                }
            }
        }
        // Everything else defers to the standard dispatcher.
        _ => provider_query(provider, api_key, prompt, agent, op_tag),
    }
}

/// Make a non-streaming request to the Anthropic Messages API.
/// Returns the model's text reply or a descriptive error.
///
/// Only compiled when the `claude` feature is enabled. With the
/// feature off, callers never reach this — `provider_query` falls
/// through to Ollama before this function would be referenced.
#[cfg(feature = "claude")]
fn claude_query(
    api_key: &str,
    prompt: &str,
    agent: &ureq::Agent,
    op_tag: &str,
) -> Result<String, String> {
    let body_json = build_claude_request(CLAUDE_MODEL, CLAUDE_MAX_TOKENS, prompt);

    // ureq's timeout API: we use the per-request `timeout` builder
    // on the request itself rather than configuring the Agent,
    // because callers may share the same agent for other (faster)
    // requests with different timeout needs.
    let response = agent
        .post(CLAUDE_API_URL)
        .set("content-type", "application/json")
        .set("x-api-key", api_key)
        .set("anthropic-version", CLAUDE_API_VERSION)
        .timeout(Duration::from_secs(CLAUDE_TIMEOUT_SECS))
        .send_string(&body_json);

    let resp = match response {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            // 401 / 403 → bad key. 429 → rate-limited. 5xx → server.
            // Surface the code so callers can decide whether to
            // fall back.
            let body = r.into_string().unwrap_or_default();
            return Err(format!(
                "claude: HTTP {} — {}",
                code,
                body.chars().take(200).collect::<String>()
            ));
        }
        Err(e) => {
            return Err(format!("claude: network error — {e}"));
        }
    };

    let text = resp
        .into_string()
        .map_err(|e| format!("claude: failed to read response body: {e}"))?;

    // v0.1.352: token accounting. Parse usage block from the
    // response and append to the local log. Best-effort — any
    // failure here is logged but doesn't affect the actual
    // response handling below.
    //
    // v0.1.365: op_tag arg lets callers say WHAT they were doing
    // (block_summary, context_question, voice_intent_followup,
    // etc.) so the per-op breakdown in web2qml://stats/tokens is
    // actually meaningful. Previously every call landed under
    // "claude_query" — useless for "where are my tokens going?"
    // analysis. Empty tag falls back to "claude_query" for
    // callers that haven't been updated yet (defensive default).
    let (in_tok, out_tok, cached) = crate::token_log::parse_anthropic_usage(&text);
    if in_tok > 0 || out_tok > 0 {
        let final_op = if op_tag.is_empty() { "claude_query" } else { op_tag };
        crate::token_log::append_event(crate::token_log::TokenEvent {
            ts: crate::token_log::now_iso(),
            op: final_op.to_string(),
            model: CLAUDE_MODEL.to_string(),
            input_tokens: in_tok,
            output_tokens: out_tok,
            cached_input_tokens: cached,
        });
    }

    // The Messages API returns:
    //   { "content": [ { "type": "text", "text": "..." }, ... ], ... }
    //
    // We pull out the first text block and return it. Tool-use
    // responses aren't used here (we never ask Claude to call
    // tools), so we only need to handle the text case.
    parse_claude_response(&text)
}

/// Hand-roll the request JSON. Matches the pattern used by
/// archive.rs::to_json for consistency.
///
/// Shape produced:
/// `{"model":"claude-haiku-4-5","max_tokens":512,`
/// `"messages":[{"role":"user","content":"<escaped prompt>"}]}`
#[cfg(feature = "claude")]
fn build_claude_request(model: &str, max_tokens: u32, prompt: &str) -> String {
    let mut o = String::with_capacity(prompt.len() + 128);
    o.push_str("{\"model\":");
    json_str(&mut o, model);
    o.push_str(",\"max_tokens\":");
    o.push_str(&max_tokens.to_string());
    o.push_str(",\"messages\":[{\"role\":\"user\",\"content\":");
    json_str(&mut o, prompt);
    o.push_str("}]}");
    o
}

/// Emit a JSON-escaped string literal (including surrounding quotes)
/// to `out`. Handles the seven characters JSON requires escaping:
/// `"`, `\`, and the C0 control characters. Other code points pass
/// through verbatim — the Anthropic API consumes UTF-8 directly.
#[cfg(feature = "claude")]
fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                // Other C0 controls: emit \u00XX
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Extract the first text block from an Anthropic Messages API
/// response. The parser is intentionally simple: find the
/// `"content":[...]` array, scan within for a block matching
/// `"type":"text"` and `"text":"..."`, and return the text.
///
/// This is fragile: if Anthropic adds new whitespace, reorders
/// keys, or changes the response shape, this parser may break.
/// The test suite at the bottom of this file pins the current
/// shape; rerun tests when bumping the API version constant.
#[cfg(feature = "claude")]
fn json_type_is_text(block: &str) -> bool {
    // True if `block` contains a `"type"` key whose string value is `"text"`,
    // tolerant of arbitrary whitespace around the colon (compact or pretty).
    let mut from = 0;
    while let Some(rel) = block[from..].find("\"type\"") {
        let after = from + rel + "\"type\"".len();
        let rest = block[after..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            if rest.trim_start().starts_with("\"text\"") {
                return true;
            }
        }
        from = after;
    }
    false
}

#[cfg(feature = "claude")]
fn parse_claude_response(json_text: &str) -> Result<String, String> {
    // Find the content array. The API always emits content as an
    // array, even for single-block responses.
    let content_idx = json_text.find("\"content\"").ok_or_else(|| {
        format!(
            "claude: response missing `content` field. First 200 chars: {}",
            json_text.chars().take(200).collect::<String>()
        )
    })?;

    // From the content index, find the opening `[`.
    let after_key = &json_text[content_idx..];
    let arr_start = after_key.find('[').ok_or_else(|| {
        "claude: `content` field present but no array follows".to_string()
    })?;
    let arr_body = &after_key[arr_start..];

    // Brace-depth walk to identify object boundaries within the
    // array. Necessary because tool_use blocks have nested objects
    // (their `input` field is an object) and a naive split-on-`}`
    // would tokenize wrong.
    let mut depth = 0i32;
    let mut obj_start: Option<usize> = None;
    let bytes = arr_body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'"' => {
                // Skip over a string literal. Handle backslash escapes.
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' { i += 1; }
                    i += 1;
                }
            }
            b'{' => {
                if depth == 1 {
                    obj_start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 1 {
                    if let Some(start) = obj_start.take() {
                        let block = &arr_body[start..=i];
                        // v0.1.772: match `"type" : "text"` tolerant of ANY
                        // whitespace around the colon. The old check only
                        // covered the no-space and space-both-sides forms and
                        // missed the standard pretty-printed `"type": "text"`
                        // (space only after the colon), so every multi-line
                        // response parsed as "no text block".
                        if json_type_is_text(block) {
                            if let Some(text) = extract_text_field(block) {
                                return Ok(text);
                            }
                        }
                    }
                }
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }

    Err("claude: no text block in response".into())
}

/// Extract the value of a `"text":"..."` key from a JSON object
/// fragment. Returns the unescaped string content. Returns None
/// if the field is missing or malformed.
#[cfg(feature = "claude")]
fn extract_text_field(obj: &str) -> Option<String> {
    let key_idx = obj.find("\"text\"")?;
    let after = &obj[key_idx + 6..]; // skip past "text"
    // Skip whitespace and the colon.
    let colon = after.find(':')?;
    let rest = &after[colon + 1..];
    // Skip whitespace, find opening quote.
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'"' {
        return None;
    }
    i += 1; // past opening quote
    let start = i;
    // Walk to matching close, handling backslash escapes.
    while i < bytes.len() && bytes[i] != b'"' {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
        } else {
            i += 1;
        }
    }
    if i >= bytes.len() {
        return None;
    }
    let raw = &rest[start..i];
    Some(unescape_json_string(raw))
}

/// Reverse the JSON string escaping for a sequence of characters
/// between two double-quotes. Handles `\"`, `\\`, `\n`, `\r`, `\t`,
/// `\b`, `\f`, and `\uXXXX` (BMP only). Unknown escapes pass
/// through verbatim — conservative, matches what Anthropic actually
/// emits.
#[cfg(feature = "claude")]
fn unescape_json_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('b') => out.push('\x08'),
            Some('f') => out.push('\x0c'),
            Some('u') => {
                // Read 4 hex digits.
                let mut hex = String::with_capacity(4);
                for _ in 0..4 {
                    if let Some(d) = chars.next() {
                        hex.push(d);
                    } else {
                        break;
                    }
                }
                if let Ok(code) = u32::from_str_radix(&hex, 16) {
                    if let Some(ch) = char::from_u32(code) {
                        out.push(ch);
                    }
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_from_i32() {
        assert_eq!(Provider::from_i32(0), Provider::Ollama);
        assert_eq!(Provider::from_i32(1), Provider::Claude);
        assert_eq!(Provider::from_i32(2), Provider::LlamaCpp);
        // Unknown values default to Ollama (safer fallback).
        assert_eq!(Provider::from_i32(99), Provider::Ollama);
        assert_eq!(Provider::from_i32(-1), Provider::Ollama);
    }

    // Tests below exercise the Claude-specific functions
    // (build_claude_request, json_str, parse_claude_response,
    // unescape_json_string). Those functions are themselves gated
    // on the `claude` feature, so the tests need the same gate to
    // avoid referencing names that don't exist when the feature
    // is OFF.
    #[cfg(feature = "claude")]
    mod claude_tests {
        use super::super::*;

        #[test]
        fn json_str_escapes_quotes_and_backslashes() {
            let mut o = String::new();
            json_str(&mut o, r#"hello "world" \ test"#);
            assert_eq!(o, r#""hello \"world\" \\ test""#);
        }

        #[test]
        fn json_str_escapes_newlines() {
            let mut o = String::new();
            json_str(&mut o, "line1\nline2\ttabbed");
            assert_eq!(o, r#""line1\nline2\ttabbed""#);
        }

        #[test]
        fn json_str_escapes_low_controls() {
            let mut o = String::new();
            json_str(&mut o, "x\x01y");
            assert_eq!(o, r#""x\u0001y""#);
        }

        #[test]
        fn build_claude_request_shape() {
            let json = build_claude_request("test-model", 100, "hello");
            assert!(json.starts_with(r#"{"model":"test-model""#));
            assert!(json.contains(r#""max_tokens":100"#));
            assert!(json.contains(r#""role":"user""#));
            assert!(json.contains(r#""content":"hello""#));
        }

        #[test]
        fn build_claude_request_escapes_prompt() {
            let json = build_claude_request("m", 10, r#"with "quotes" and \"#);
            assert!(json.contains(r#""content":"with \"quotes\" and \\""#));
        }

        #[test]
        fn parse_claude_response_simple() {
            let json = r#"{
                "id": "msg_01",
                "type": "message",
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "Hello, world!" }
                ],
                "stop_reason": "end_turn"
            }"#;
            let result = parse_claude_response(json).unwrap();
            assert_eq!(result, "Hello, world!");
        }

        #[test]
        fn parse_claude_response_picks_first_text() {
            let json = r#"{
                "content": [
                    { "type": "tool_use", "id": "x", "name": "foo", "input": {"x":1} },
                    { "type": "text", "text": "And the answer is 42" }
                ]
            }"#;
            let result = parse_claude_response(json).unwrap();
            assert_eq!(result, "And the answer is 42");
        }

        #[test]
        fn parse_claude_response_handles_escapes() {
            let json = r#"{ "content": [{ "type": "text", "text": "line1\nline2" }] }"#;
            let result = parse_claude_response(json).unwrap();
            assert_eq!(result, "line1\nline2");
        }

        #[test]
        fn parse_claude_response_missing_content() {
            let json = r#"{ "id": "x", "type": "error" }"#;
            let err = parse_claude_response(json).unwrap_err();
            assert!(err.contains("missing `content`"));
        }

        #[test]
        fn parse_claude_response_no_text_block() {
            let json = r#"{ "content": [{ "type": "tool_use", "name": "f", "input": {} }] }"#;
            let err = parse_claude_response(json).unwrap_err();
            assert!(err.contains("no text block"));
        }

        #[test]
        fn parse_claude_response_handles_nested_objects() {
            let json = r#"{ "content": [
                { "type": "tool_use", "name": "f", "input": { "a": {"b": 1} } },
                { "type": "text", "text": "after nested" }
            ] }"#;
            let result = parse_claude_response(json).unwrap();
            assert_eq!(result, "after nested");
        }

        #[test]
        fn unescape_json_handles_unicode_escape() {
            assert_eq!(unescape_json_string(r#"x\u00e9y"#), "x\u{e9}y");
        }
    }
}
