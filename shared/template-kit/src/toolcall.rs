//! toolcall — the other half of `--jinja`: parse a model's reply back into structured tool calls.
//!
//! `Renderer` turns messages+tools into the prompt the model trained on. This turns what comes back
//! into `ParsedReply { content, reasoning, calls }`. llama.cpp does this with a hand-written parser
//! per family; the format here is DETECTED FROM THE TEMPLATE the model shipped, so a fine-tune with
//! a slightly different template still lands in the right parser and an unknown one falls back to a
//! generic scan rather than to nothing.
//!
//! What it handles, in the shapes models actually emit:
//!   * ChatML / Hermes / Qwen / Ornith: `<tool_call>{"name":…,"arguments":{…}}</tool_call>`, repeated
//!   * Llama 3.1/3.2: `<|python_tag|>{"name":…,"parameters":{…}}` and bare `{"name":…}` JSON replies
//!   * Mistral / Devstral: `[TOOL_CALLS][{"name":…,"arguments":{…}}]`
//!   * DeepSeek R1: `<｜tool▁call▁begin｜>function<｜tool▁sep｜>name\n```json\n{…}\n```<｜tool▁call▁end｜>`
//!   * the fenced ```tool {"tool":…} block logscope's own harness teaches
//!   * reasoning: `<think>…</think>` (Qwen3.x/Ornith/R1) split off before any of the above
//!
//! Partial output is handled the way a streaming caller needs: a call whose closing marker has not
//! arrived is reported as `truncated`, not as content and not as a call.
use serde::{Deserialize, Serialize};

use crate::ToolSyntax;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedCall {
    pub name: String,
    /// Always an object; a model that emitted a scalar or a string of JSON is normalised here.
    pub arguments: serde_json::Value,
    /// Which syntax it was found in, for diagnostics and for the harness's counters.
    pub via: Format,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedReply {
    /// The answer text, with reasoning and every call removed.
    pub content: String,
    pub reasoning: Option<String>,
    pub calls: Vec<ParsedCall>,
    /// A call marker opened and never closed: the reply was cut off mid-call. Nothing was parsed
    /// from it, and a caller streaming should wait rather than act.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// `<tool_call>…</tool_call>` — ChatML, Hermes 2/3, Qwen 2.5/3.x, Ornith.
    ChatMlToolCall,
    /// `<|python_tag|>{…}` or a bare top-level JSON object — Llama 3.1/3.2.
    Llama3Python,
    /// `[TOOL_CALLS][ … ]` — Mistral, Devstral.
    MistralToolCalls,
    /// DeepSeek R1's unicode-delimited form.
    DeepSeekR1,
    /// The fenced ```tool {"tool": …} block logscope's harness teaches small models.
    FencedTool,
    /// Nothing recognised; content only.
    None,
}

/// Which format a template implies. Detected from the template SOURCE (the strings it emits for a
/// tool call), which is how a fine-tune inherits its base model's parser for free.
pub fn detect_format(template_src: &str) -> Format {
    let t = template_src;
    if t.contains("tool▁call▁begin") || t.contains("tool_call_begin") { return Format::DeepSeekR1; }
    if t.contains("[TOOL_CALLS]") { return Format::MistralToolCalls; }
    if t.contains("<tool_call>") { return Format::ChatMlToolCall; }
    if t.contains("<|python_tag|>") { return Format::Llama3Python; }
    Format::None
}

/// The format a probed syntax implies, when the template was probed rather than read.
pub fn format_of(syntax: &ToolSyntax) -> Format { detect_format(&format!("{}{}", syntax.call_open, syntax.call_close)) }

/// Split `<think>…</think>` off the front. An unterminated block is all reasoning: the model was
/// still thinking when the reply ended, so nothing in it is an answer or an action.
pub fn split_reasoning(text: &str) -> (Option<String>, String, bool) {
    let Some(a) = text.find("<think>") else { return (None, text.to_string(), false) };
    let after = &text[a + 7..];
    match after.find("</think>") {
        Some(b) => (Some(after[..b].trim().to_string()), format!("{}{}", &text[..a], &after[b + 8..]), false),
        None => (Some(after.trim().to_string()), text[..a].to_string(), true),
    }
}

/// Parse a reply in a known format. `Format::None` still scans for every shape, because a model
/// asked for one syntax sometimes answers in the one it was pretrained on.
pub fn parse(text: &str, format: Format) -> ParsedReply {
    let (reasoning, body, cut_in_think) = split_reasoning(text);
    let mut r = ParsedReply { reasoning, truncated: cut_in_think, ..Default::default() };
    if cut_in_think { r.content = body.trim().to_string(); return r; }
    let mut content = body;
    // The declared format first, then every other shape: a model asked for one syntax sometimes
    // answers in the one it was pretrained on, and a reply understood is better than a reply lost.
    const ALL: [Format; 5] = [Format::ChatMlToolCall, Format::FencedTool, Format::MistralToolCalls, Format::DeepSeekR1, Format::Llama3Python];
    let mut tried: Vec<Format> = Vec::with_capacity(ALL.len());
    if format != Format::None { tried.push(format); }
    for f in ALL { if !tried.contains(&f) { tried.push(f); } }
    for f in tried {
        let (calls, rest, trunc) = match f {
            Format::ChatMlToolCall => scan_delimited(&content, "<tool_call>", "</tool_call>", f),
            Format::MistralToolCalls => scan_mistral(&content),
            Format::DeepSeekR1 => scan_deepseek(&content),
            Format::FencedTool => scan_fenced(&content),
            Format::Llama3Python => scan_llama3(&content),
            Format::None => (Vec::new(), content.clone(), false),
        };
        r.truncated |= trunc;
        if !calls.is_empty() { r.calls.extend(calls); content = rest; }
        else if trunc { content = rest; }
    }
    r.content = content.trim().to_string();
    r
}

/// The generic path: parse using a probed syntax's own markers, for a template none of the named
/// formats matched. Falls back to `parse` with `Format::None` when the markers are not delimiters.
pub fn parse_with_syntax(text: &str, syntax: &ToolSyntax) -> ParsedReply {
    let open = syntax.call_open.trim();
    let close = syntax.call_close.trim();
    let open_tag = open.split('{').next().unwrap_or("").trim();
    let close_tag = close.rsplit('}').last().unwrap_or("").trim();
    if open_tag.is_empty() || close_tag.is_empty() { return parse(text, format_of(syntax)); }
    let (reasoning, body, cut) = split_reasoning(text);
    let (calls, rest, trunc) = scan_delimited(&body, open_tag, close_tag, Format::None);
    ParsedReply { content: rest.trim().to_string(), reasoning, calls, truncated: cut || trunc }
}

// ── scanners ──────────────────────────────────────────────────────────────────────────────────────

/// `open … close`, repeated; the JSON between them is the call. Returns (calls, text with the calls
/// removed, truncated).
fn scan_delimited(text: &str, open: &str, close: &str, via: Format) -> (Vec<ParsedCall>, String, bool) {
    let (mut calls, mut rest, mut truncated) = (Vec::new(), String::new(), false);
    let mut s = text;
    while let Some(a) = s.find(open) {
        rest.push_str(&s[..a]);
        let after = &s[a + open.len()..];
        match after.find(close) {
            Some(b) => {
                if let Some(c) = call_from_json(after[..b].trim(), via) { calls.push(c); } else { rest.push_str(&s[a..a + open.len() + b + close.len()]); }
                s = &after[b + close.len()..];
            }
            None => { truncated = true; s = ""; }
        }
    }
    rest.push_str(s);
    (calls, rest, truncated)
}

/// `[TOOL_CALLS][{…},{…}]` — one JSON array of calls.
fn scan_mistral(text: &str) -> (Vec<ParsedCall>, String, bool) {
    const TAG: &str = "[TOOL_CALLS]";
    let Some(a) = text.find(TAG) else { return (Vec::new(), text.to_string(), false) };
    let after = text[a + TAG.len()..].trim_start();
    let Some(end) = balanced_end(after, '[', ']') else { return (Vec::new(), text[..a].to_string(), true) };
    let calls = serde_json::from_str::<Vec<serde_json::Value>>(&after[..end]).ok()
        .map(|v| v.iter().filter_map(|x| call_from_value(x, Format::MistralToolCalls)).collect::<Vec<_>>()).unwrap_or_default();
    let rest = format!("{}{}", &text[..a], &after[end..]);
    (calls, rest, false)
}

/// DeepSeek R1: `…begin｜>function…sep｜>NAME\n```json\n{…}\n```…end｜>`
fn scan_deepseek(text: &str) -> (Vec<ParsedCall>, String, bool) {
    let (mut calls, mut rest, mut truncated) = (Vec::new(), String::new(), false);
    let mut s = text;
    while let Some(a) = s.find("tool▁call▁begin").or_else(|| s.find("tool_call_begin")) {
        let head = s[..a].rfind('<').unwrap_or(a);
        rest.push_str(&s[..head]);
        let after = &s[a..];
        let Some(sep) = after.find("tool▁sep").or_else(|| after.find("tool_sep")) else { truncated = true; break };
        let tail = &after[sep..];
        let Some(nl) = tail.find('\n') else { truncated = true; break };
        let name = tail[..nl].rsplit('>').next().unwrap_or("").trim().to_string();
        let Some(js) = tail.find("```json").map(|i| i + 7).or_else(|| tail.find('{').map(|i| i)) else { truncated = true; break };
        let json_part = &tail[js..];
        let Some(end) = balanced_end(json_part.trim_start(), '{', '}') else { truncated = true; break };
        let args: serde_json::Value = serde_json::from_str(json_part.trim_start()[..end].trim()).unwrap_or(serde_json::json!({}));
        calls.push(ParsedCall { name, arguments: args, via: Format::DeepSeekR1 });
        match tail.find("tool▁call▁end").or_else(|| tail.find("tool_call_end")) {
            Some(e) => { let abs = a + sep + e; s = &s[abs..]; s = s.split_once('>').map(|(_, r)| r).unwrap_or(""); }
            None => { truncated = true; s = ""; }
        }
    }
    rest.push_str(s);
    (calls, rest, truncated)
}

/// ```tool\n{"tool":"name", …}\n``` — logscope's own harness format; the whole object is the call,
/// with the name under `"tool"` and the remaining keys as the arguments.
fn scan_fenced(text: &str) -> (Vec<ParsedCall>, String, bool) {
    let (mut calls, mut rest, mut truncated) = (Vec::new(), String::new(), false);
    let mut s = text;
    while let Some(a) = s.find("```tool") {
        rest.push_str(&s[..a]);
        let after = &s[a + 7..];
        match after.find("```") {
            Some(b) => {
                let body = after[..b].trim();
                match serde_json::from_str::<serde_json::Value>(body) {
                    Ok(v) => match v.get("tool").and_then(|t| t.as_str()) {
                        Some(name) => {
                            let mut args = v.clone();
                            if let Some(o) = args.as_object_mut() { o.remove("tool"); }
                            calls.push(ParsedCall { name: name.to_string(), arguments: args, via: Format::FencedTool });
                        }
                        None => { if let Some(c) = call_from_value(&v, Format::FencedTool) { calls.push(c); } else { rest.push_str(&s[a..a + 7 + b + 3]); } }
                    },
                    Err(_) => rest.push_str(&s[a..a + 7 + b + 3]),
                }
                s = &after[b + 3..];
            }
            None => { truncated = true; s = ""; }
        }
    }
    rest.push_str(s);
    (calls, rest, truncated)
}

/// `<|python_tag|>{…}` or a reply that is nothing but a JSON object with a name.
fn scan_llama3(text: &str) -> (Vec<ParsedCall>, String, bool) {
    const TAG: &str = "<|python_tag|>";
    if let Some(a) = text.find(TAG) {
        let after = &text[a + TAG.len()..];
        let trimmed = after.trim_start();
        let Some(end) = balanced_end(trimmed, '{', '}') else { return (Vec::new(), text[..a].to_string(), true) };
        let call = call_from_json(&trimmed[..end], Format::Llama3Python);
        // an <|eom_id|> after the call is a turn marker, not content
        let tail = trimmed[end..].trim_start().trim_start_matches("<|eom_id|>").trim_start_matches("<|eot_id|>");
        return (call.into_iter().collect(), format!("{}{}", &text[..a], tail), false);
    }
    // bare JSON: the whole reply is one object with a name and arguments/parameters
    let t = text.trim();
    if t.starts_with('{') && t.ends_with('}') {
        if let Some(c) = call_from_json(t, Format::Llama3Python) { return (vec![c], String::new(), false); }
    }
    (Vec::new(), text.to_string(), false)
}

/// Index just past the matching close, honouring strings and escapes, so a brace inside a string
/// value does not end the object.
fn balanced_end(s: &str, open: char, close: char) -> Option<usize> {
    let b = s.as_bytes();
    if b.first().copied() != Some(open as u8) { return None; }
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, ch) in s.char_indices() {
        if in_str {
            if esc { esc = false; } else if ch == '\\' { esc = true; } else if ch == '"' { in_str = false; }
            continue;
        }
        match ch {
            '"' => in_str = true,
            c if c == open => depth += 1,
            c if c == close => { depth -= 1; if depth == 0 { return Some(i + ch.len_utf8()); } }
            _ => {}
        }
    }
    None
}

fn call_from_json(s: &str, via: Format) -> Option<ParsedCall> {
    serde_json::from_str::<serde_json::Value>(s).ok().and_then(|v| call_from_value(&v, via))
}

/// Normalise the several shapes a call object comes in: `{name, arguments}`, `{name, parameters}`,
/// `{function:{name, arguments}}`, and arguments given as a JSON *string* rather than an object.
fn call_from_value(v: &serde_json::Value, via: Format) -> Option<ParsedCall> {
    let v = v.get("function").unwrap_or(v);
    let name = v.get("name").and_then(|n| n.as_str())?.to_string();
    let raw = v.get("arguments").or_else(|| v.get("parameters")).cloned().unwrap_or(serde_json::json!({}));
    let arguments = match raw {
        serde_json::Value::String(s) => serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s)),
        other => other,
    };
    let arguments = if arguments.is_object() { arguments } else { serde_json::json!({ "value": arguments }) };
    Some(ParsedCall { name, arguments, via })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(r: &ParsedReply) -> &ParsedCall { assert_eq!(r.calls.len(), 1, "{r:?}"); &r.calls[0] }

    #[test]
    fn chatml_qwen_ornith() {
        let t = "Let me look.\n<tool_call>\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.rs\"}}\n</tool_call>";
        let r = parse(t, Format::ChatMlToolCall);
        assert_eq!(one(&r).name, "read_file");
        assert_eq!(one(&r).arguments["path"], "a.rs");
        assert_eq!(r.content, "Let me look.");
        assert!(!r.truncated);
    }

    #[test]
    fn two_calls_in_one_reply_is_fan_out() {
        let t = "<tool_call>{\"name\":\"a\",\"arguments\":{}}</tool_call>\n<tool_call>{\"name\":\"b\",\"arguments\":{\"n\":2}}</tool_call>";
        let r = parse(t, Format::ChatMlToolCall);
        assert_eq!(r.calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(r.calls[1].arguments["n"], 2);
    }

    #[test]
    fn llama3_python_tag_and_bare_json_and_parameters_key() {
        let r = parse("<|python_tag|>{\"name\": \"get\", \"parameters\": {\"q\": \"x\"}}<|eom_id|>", Format::Llama3Python);
        assert_eq!(one(&r).name, "get");
        assert_eq!(one(&r).arguments["q"], "x");
        assert_eq!(r.content, "");
        let bare = parse("{\"name\":\"ls\",\"arguments\":{}}", Format::Llama3Python);
        assert_eq!(one(&bare).name, "ls");
    }

    #[test]
    fn mistral_array_form() {
        let r = parse("[TOOL_CALLS][{\"name\":\"run\",\"arguments\":{\"argv\":[\"cargo\",\"check\"]}}]", Format::MistralToolCalls);
        assert_eq!(one(&r).name, "run");
        assert_eq!(one(&r).arguments["argv"][1], "check");
    }

    #[test]
    fn deepseek_r1_unicode_delimiters() {
        let t = "thinking done<｜tool▁call▁begin｜>function<｜tool▁sep｜>search\n```json\n{\"q\": \"rust\"}\n```<｜tool▁call▁end｜>";
        let r = parse(t, Format::DeepSeekR1);
        assert_eq!(one(&r).name, "search");
        assert_eq!(one(&r).arguments["q"], "rust");
    }

    #[test]
    fn the_harness_fenced_block() {
        let r = parse("ok\n```tool\n{\"tool\":\"write_file\",\"path\":\"a\",\"contents\":\"b\"}\n```", Format::FencedTool);
        let c = one(&r);
        assert_eq!(c.name, "write_file");
        assert_eq!(c.arguments["path"], "a");
        assert!(c.arguments.get("tool").is_none(), "the name key is not also an argument");
    }

    #[test]
    fn reasoning_is_split_and_a_call_inside_it_is_not_a_call() {
        let t = "<think>maybe <tool_call>{\"name\":\"rm\",\"arguments\":{}}</tool_call> no</think>real\n<tool_call>{\"name\":\"ls\",\"arguments\":{}}</tool_call>";
        let r = parse(t, Format::ChatMlToolCall);
        assert_eq!(one(&r).name, "ls");
        assert!(r.reasoning.as_deref().unwrap().contains("maybe"));
        assert_eq!(r.content, "real");
    }

    #[test]
    fn truncation_is_reported_not_guessed() {
        let r = parse("working<tool_call>{\"name\":\"read_file\"", Format::ChatMlToolCall);
        assert!(r.truncated && r.calls.is_empty(), "{r:?}");
        let think = parse("<think>still going", Format::ChatMlToolCall);
        assert!(think.truncated && think.calls.is_empty() && think.content.is_empty());
    }

    #[test]
    fn braces_inside_strings_do_not_end_the_object() {
        let r = parse("<tool_call>{\"name\":\"write\",\"arguments\":{\"c\":\"fn main(){ }\"}}</tool_call>", Format::ChatMlToolCall);
        assert_eq!(one(&r).arguments["c"], "fn main(){ }");
    }

    #[test]
    fn arguments_as_a_json_string_are_normalised() {
        let r = parse("<tool_call>{\"name\":\"f\",\"arguments\":\"{\\\"k\\\":1}\"}</tool_call>", Format::ChatMlToolCall);
        assert_eq!(one(&r).arguments["k"], 1);
    }

    #[test]
    fn a_known_format_still_understands_another_one() {
        // Ornith asked for a fenced block by the harness, answering in its native ChatML shape,
        // and the reverse. Both must parse.
        let r = parse("```tool\n{\"tool\":\"ls\"}\n```", Format::ChatMlToolCall);
        assert_eq!(one(&r).name, "ls");
        let r2 = parse("<tool_call>{\"name\":\"ls\",\"arguments\":{}}</tool_call>", Format::FencedTool);
        assert_eq!(one(&r2).name, "ls");
    }

    #[test]
    fn detection_comes_from_the_template_source() {
        assert_eq!(detect_format("… <tool_call>\\n{\"name\": …"), Format::ChatMlToolCall);
        assert_eq!(detect_format("… <|python_tag|>{\"name\": …"), Format::Llama3Python);
        assert_eq!(detect_format("… [TOOL_CALLS][ …"), Format::MistralToolCalls);
        assert_eq!(detect_format("plain template, no tools"), Format::None);
    }

    #[test]
    fn prose_only_replies_are_untouched() {
        let r = parse("just an answer, no calls.", Format::ChatMlToolCall);
        assert!(r.calls.is_empty() && !r.truncated);
        assert_eq!(r.content, "just an answer, no calls.");
    }
}
