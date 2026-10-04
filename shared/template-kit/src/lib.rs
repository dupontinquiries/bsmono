//! template-kit — the model's own chat template, rendered in Rust, plus what the harness needs to
//! know about it.
//!
//! Why: logscope's engines hand-build five template families (`Llama3, ChatML, Gemma, Llama4,
//! Bailing2`) and repair mismatches at the worker. Every GGUF already carries the template the model
//! was trained on, as Jinja, in `tokenizer.chat_template` (sometimes a second one under
//! `tokenizer.chat_template.tool_use`). Rendering that text is the whole fix: no family detection,
//! no repair pass, no "TEMPLATE MISMATCH" alarm. llama.cpp does this with `--jinja` (its own minja);
//! this does it with minijinja, so the wgpu engine and the harness share one renderer.
//!
//! The three functions chat templates call that plain Jinja lacks — `raise_exception`,
//! `strftime_now`, `tojson` — are provided, and `pycompat` gives the Python method calls.
pub mod gguf;
pub mod constrain;   // constrained decoding in Rust, for engines with no GBNF sampler
pub mod toolcall;   // the parse half of --jinja: model output → structured calls

use minijinja::{Environment, Error, ErrorKind, Value};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall { pub id: Option<String>, pub name: String, pub arguments: serde_json::Value }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// OpenAI shape, which is what the templates index into: `tool_call.function.name`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallEnvelope { #[serde(default)] pub id: Option<String>, #[serde(rename = "type", default = "call_type")] pub kind: String, pub function: ToolCall }
fn call_type() -> String { "function".into() }

impl Message {
    pub fn system(s: &str) -> Message { Message { role: "system".into(), content: s.into(), tool_calls: vec![], tool_call_id: None, name: None } }
    pub fn user(s: &str) -> Message { Message { role: "user".into(), content: s.into(), tool_calls: vec![], tool_call_id: None, name: None } }
    pub fn assistant(s: &str) -> Message { Message { role: "assistant".into(), content: s.into(), tool_calls: vec![], tool_call_id: None, name: None } }
    pub fn assistant_call(name: &str, args: serde_json::Value) -> Message {
        Message { role: "assistant".into(), content: String::new(), tool_call_id: None, name: None,
            tool_calls: vec![ToolCallEnvelope { id: Some("call_0".into()), kind: "function".into(), function: ToolCall { id: None, name: name.into(), arguments: args } }] }
    }
    pub fn tool_result(call_id: &str, name: &str, content: &str) -> Message { Message { role: "tool".into(), content: content.into(), tool_calls: vec![], tool_call_id: Some(call_id.into()), name: Some(name.into()) } }
}

/// One tool as the template sees it (OpenAI shape).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tool { #[serde(rename = "type", default = "call_type")] pub kind: String, pub function: ToolSpec }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSpec { pub name: String, pub description: String, pub parameters: serde_json::Value }

impl Tool {
    pub fn new(name: &str, description: &str, parameters: serde_json::Value) -> Tool { Tool { kind: "function".into(), function: ToolSpec { name: name.into(), description: description.into(), parameters } } }
}

/// Whether thinking is on for rendered prompts: `LOGSCOPE_THINKING=0` turns it off (the only
/// place the app reads it; `render_thinking` takes it explicitly).
pub fn thinking_enabled() -> bool { std::env::var("LOGSCOPE_THINKING").as_deref() != Ok("0") }

pub struct Renderer { env: Environment<'static> }

impl Renderer {
    /// Compile a template as it comes out of the GGUF (`tokenizer.chat_template`).
    pub fn new(template: &str) -> Result<Renderer, String> {
        let mut env = Environment::new();
        minijinja_contrib::add_to_environment(&mut env);
        env.add_function("raise_exception", |msg: String| -> Result<Value, Error> { Err(Error::new(ErrorKind::InvalidOperation, msg)) });
        env.add_function("strftime_now", |fmt: String| -> String { strftime_now(&fmt) });
        env.set_trim_blocks(false); env.set_lstrip_blocks(false);
        env.add_template_owned("chat", template.to_string()).map_err(|e| format!("template: {e}"))?;
        Ok(Renderer { env })
    }

    /// The prompt the model was trained to see. `tools` may be empty; `add_generation_prompt` puts
    /// the assistant header on the end (true for every turn the model must answer).
    pub fn render(&self, messages: &[Message], tools: &[Tool], add_generation_prompt: bool) -> Result<String, String> {
        self.render_thinking(messages, tools, add_generation_prompt, thinking_enabled())
    }

    /// `render` with the Qwen3 / Qwen3.5 / Ornith family's `enable_thinking` switch explicit. Their
    /// templates read it to decide whether the generation prompt ends in `<think>\n` (the model
    /// continues a thought) or a closed empty block (`<think>\n\n</think>\n\n` — no thinking). It was
    /// never passed, so it was undefined and the templates took their default (thinking ON).
    /// Templates that do not read it are unaffected by its presence.
    pub fn render_thinking(&self, messages: &[Message], tools: &[Tool], add_generation_prompt: bool, enable_thinking: bool) -> Result<String, String> {
        let t = self.env.get_template("chat").map_err(|e| e.to_string())?;
        let ctx = minijinja::context! { messages => messages, tools => if tools.is_empty() { Value::from(()) } else { Value::from_serialize(tools) },
            add_generation_prompt => add_generation_prompt, bos_token => "", eos_token => "", enable_thinking => enable_thinking };
        t.render(ctx).map_err(|e| format!("render: {e}"))
    }

    /// What THIS model's tool-call syntax is, found by rendering twice and diffing: an assistant turn
    /// with a call minus the same turn without one leaves the call's wrapper. Also how a tool result
    /// is framed. The harness reads these instead of hard-coding `<tool_call>` vs `<|python_tag|>`.
    pub fn probe_tool_syntax(&self) -> Result<ToolSyntax, String> {
        let base = vec![Message::user("q")];
        let tool = Tool::new("probe_fn", "probe", serde_json::json!({"type":"object","properties":{"x":{"type":"string"}},"required":["x"]}));
        let mut with_call = base.clone(); with_call.push(Message::assistant_call("probe_fn", serde_json::json!({"x": "PROBEARG"})));
        let mut without = base.clone(); without.push(Message::assistant("PROBETEXT"));
        let a = self.render(&with_call, std::slice::from_ref(&tool), false)?;
        let b = self.render(&without, std::slice::from_ref(&tool), false)?;
        // common prefix of the two renders = everything up to the assistant turn's body
        let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
        let a_tail: String = a.chars().skip(common).collect();
        let name_at = a_tail.find("probe_fn").ok_or("the template did not render the tool call name; it may not support tools")?;
        let call_open = a_tail[..name_at].trim_start().to_string();
        let after_args = a_tail.find("PROBEARG").map(|i| i + "PROBEARG".len()).unwrap_or(name_at);
        let call_close = a_tail[after_args..].chars().take_while(|c| *c != '\n' || true).collect::<String>().trim_end().to_string();
        let tools_visible = a.contains("probe_fn") && a.contains("\"x\"");
        let mut with_result = with_call.clone(); with_result.push(Message::tool_result("call_0", "probe_fn", "PROBERESULT"));
        let c = self.render(&with_result, std::slice::from_ref(&tool), true)?;
        let result_frame = c.find("PROBERESULT").map(|i| c[i.saturating_sub(40)..i].trim().to_string()).unwrap_or_default();
        Ok(ToolSyntax { call_open, call_close, tools_in_prompt: tools_visible, result_frame, json_arguments: a_tail.contains("\"x\"") && a_tail.contains("PROBEARG") })
    }
}

/// Separate Qwen3.x/Ornith reasoning from the answer: `(thinking, content)`. An unterminated block
/// is all thinking (the reply was cut off before it acted).
pub fn split_thinking(text: &str) -> (Option<String>, String) {
    let Some(a) = text.find("<think>") else { return (None, text.to_string()) };
    let after = &text[a + 7..];
    match after.find("</think>") {
        Some(b) => (Some(after[..b].trim().to_string()), format!("{}{}", &text[..a], &after[b + 8..]).trim().to_string()),
        None => (Some(after.trim().to_string()), text[..a].trim().to_string()),
    }
}

/// The strftime subset chat templates use (`%d %b %Y`, `%Y-%m-%d`, `%H:%M`), from the UTC clock, no
/// dependency: templates use the date for "Today Date:" lines and nothing time-zone sensitive.
pub fn strftime_now(fmt: &str) -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil-from-days (Howard Hinnant), valid for the years that matter
    let z = days + 719_468; let era = z.div_euclid(146_097); let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400; let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1; let m = if mp < 10 { mp + 3 } else { mp - 9 }; let y = if m <= 2 { y + 1 } else { y };
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    const MONL: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let mut out = String::new(); let mut it = fmt.chars();
    while let Some(c) = it.next() {
        if c != '%' { out.push(c); continue; }
        match it.next() {
            Some('Y') => out.push_str(&y.to_string()), Some('m') => out.push_str(&format!("{m:02}")), Some('d') => out.push_str(&format!("{d:02}")),
            Some('e') => out.push_str(&format!("{d:>2}")), Some('b') => out.push_str(MON[(m - 1) as usize]), Some('B') => out.push_str(MONL[(m - 1) as usize]),
            Some('H') => out.push_str(&format!("{hh:02}")), Some('M') => out.push_str(&format!("{mm:02}")), Some('S') => out.push_str(&format!("{ss:02}")),
            Some('%') => out.push('%'), Some(o) => { out.push('%'); out.push(o); } None => out.push('%'),
        }
    }
    out
}

/// The observed syntax, as text the harness can show and a grammar generator can use.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSyntax {
    /// Text the model emits before the call (e.g. `<tool_call>\n{"name": "` or `<|python_tag|>{"name": "`).
    pub call_open: String,
    /// Text after the arguments (e.g. `}\n</tool_call>`).
    pub call_close: String,
    /// Whether tool schemas appear in the rendered prompt at all.
    pub tools_in_prompt: bool,
    /// The framing around a tool result (the 40 chars before it), e.g. `<|start_header_id|>ipython<|end_header_id|>`.
    pub result_frame: String,
    /// Arguments are rendered as JSON (the normal case) rather than something bespoke.
    pub json_arguments: bool,
}

/// The size-tier policy from logscope's read_harness, made a number the harness can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessBudget { pub max_tools: usize, pub system_prompt_tokens: usize, pub few_shot: usize, pub max_turns_before_compact: usize }

pub fn budget_for(params_billions: f32) -> HarnessBudget {
    if params_billions <= 4.0 { HarnessBudget { max_tools: 4, system_prompt_tokens: 600, few_shot: 1, max_turns_before_compact: 6 } }
    else if params_billions <= 12.0 { HarnessBudget { max_tools: 8, system_prompt_tokens: 1500, few_shot: 1, max_turns_before_compact: 12 } }
    else if params_billions <= 40.0 { HarnessBudget { max_tools: 14, system_prompt_tokens: 4000, few_shot: 0, max_turns_before_compact: 24 } }
    else { HarnessBudget { max_tools: 24, system_prompt_tokens: 12000, few_shot: 0, max_turns_before_compact: 40 } }
}

/// A GBNF grammar that only admits `call_open <json object> call_close` (or plain text), built from a
/// probe. Hand it to llama-server as the request's `grammar`, and a 3B cannot emit a malformed call.
pub fn tool_call_grammar(s: &ToolSyntax, tool_names: &[&str]) -> String {
    let esc = |t: &str| t.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    let names = tool_names.iter().map(|n| format!("\"\\\"{}\\\"\"", esc(n))).collect::<Vec<_>>().join(" | ");
    format!(r##"root ::= text | call
text ::= [^<]+
call ::= "{}" "{{" ws "\"name\"" ws ":" ws name ws "," ws "\"arguments\"" ws ":" ws object "}}" "{}"
name ::= {}
object ::= "{{" ws ( pair ( ws "," ws pair )* )? ws "}}"
pair ::= string ws ":" ws value
value ::= object | array | string | number | "true" | "false" | "null"
array ::= "[" ws ( value ( ws "," ws value )* )? ws "]"
string ::= "\"" ( [^"\\] | "\\" . )* "\""
number ::= "-"? [0-9]+ ( "." [0-9]+ )?
ws ::= [ \t\n]*
"##, esc(s.call_open.trim_end_matches(|c: char| c == '{' || c.is_whitespace()).trim_end_matches("\"name\": \"").trim_end_matches("\"name\":\"")), esc(&s.call_close), names)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A ChatML/Qwen-shaped template: tools listed in the system turn, calls wrapped in <tool_call>.
    pub(crate) const QWEN_LIKE: &str = r##"{%- if tools %}{{- '<|im_start|>system\n' }}{%- if messages[0].role == 'system' %}{{- messages[0].content + '\n\n' }}{%- endif %}{{- "# Tools\n\nYou may call one or more functions.\n\n<tools>" }}{%- for tool in tools %}{{- "\n" }}{{- tool | tojson }}{%- endfor %}{{- "\n</tools>\n\nFor each function call, return a json object with function name and arguments within <tool_call></tool_call> XML tags:\n<tool_call>\n{\"name\": <function-name>, \"arguments\": <args-json-object>}\n</tool_call><|im_end|>\n" }}{%- else %}{%- if messages[0].role == 'system' %}{{- '<|im_start|>system\n' + messages[0].content + '<|im_end|>\n' }}{%- endif %}{%- endif %}{%- for message in messages %}{%- if (message.role == "user") or (message.role == "system" and not loop.first) %}{{- '<|im_start|>' + message.role + '\n' + message.content + '<|im_end|>' + '\n' }}{%- elif message.role == "assistant" %}{{- '<|im_start|>' + message.role }}{%- if message.content %}{{- '\n' + message.content }}{%- endif %}{%- for tool_call in message.tool_calls %}{%- if tool_call.function is defined %}{%- set tool_call = tool_call.function %}{%- endif %}{{- '\n<tool_call>\n{"name": "' }}{{- tool_call.name }}{{- '", "arguments": ' }}{{- tool_call.arguments | tojson }}{{- '}\n</tool_call>' }}{%- endfor %}{{- '<|im_end|>\n' }}{%- elif message.role == "tool" %}{%- if (loop.index0 == 0) or (messages[loop.index0 - 1].role != "tool") %}{{- '<|im_start|>user' }}{%- endif %}{{- '\n<tool_response>\n' }}{{- message.content }}{{- '\n</tool_response>' }}{%- if loop.last or (messages[loop.index0 + 1].role != "tool") %}{{- '<|im_end|>\n' }}{%- endif %}{%- endif %}{%- endfor %}{%- if add_generation_prompt %}{{- '<|im_start|>assistant\n' }}{%- endif %}"##;

    // A Llama-3.1-shaped template: python_tag calls, ipython results, strip() and dates.
    pub(crate) const LLAMA31_LIKE: &str = r##"{{- bos_token }}{%- if messages[0]['role'] == 'system' %}{%- set system_message = messages[0]['content'] | trim %}{%- set messages = messages[1:] %}{%- else %}{%- set system_message = "" %}{%- endif %}{{- "<|start_header_id|>system<|end_header_id|>\n\n" }}{{- "Cutting Knowledge Date: December 2023\nToday Date: " + strftime_now("%d %b %Y") + "\n\n" }}{%- if tools is not none %}{{- "You have access to the following functions. To call a function, respond with JSON for a function call.\n\n" }}{%- for t in tools %}{{- t | tojson(indent=4) }}{{- "\n\n" }}{%- endfor %}{%- endif %}{{- system_message }}{{- "<|eot_id|>" }}{%- for message in messages %}{%- if not (message.role == 'ipython' or message.role == 'tool' or 'tool_calls' in message) %}{{- '<|start_header_id|>' + message['role'] + '<|end_header_id|>\n\n' + message['content'] | trim + '<|eot_id|>' }}{%- elif 'tool_calls' in message and message.tool_calls %}{%- set tool_call = message.tool_calls[0].function %}{{- '<|start_header_id|>assistant<|end_header_id|>\n\n' }}{{- '<|python_tag|>{"name": "' + tool_call.name + '", "parameters": ' }}{{- tool_call.arguments | tojson }}{{- "}" }}{{- "<|eom_id|>" }}{%- elif message.role == "tool" or message.role == "ipython" %}{{- "<|start_header_id|>ipython<|end_header_id|>\n\n" }}{{- message.content | trim }}{{- "<|eot_id|>" }}{%- endif %}{%- endfor %}{%- if add_generation_prompt %}{{- '<|start_header_id|>assistant<|end_header_id|>\n\n' }}{%- endif %}"##;

    fn tools() -> Vec<Tool> { vec![Tool::new("read_file", "Read a file", serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}))] }

    #[test]
    fn qwen_like_renders_and_probes() {
        let r = Renderer::new(QWEN_LIKE).unwrap();
        let out = r.render(&[Message::system("be brief"), Message::user("open main.rs")], &tools(), true).unwrap();
        assert!(out.contains("<tools>") && out.contains("\"read_file\"") && out.ends_with("<|im_start|>assistant\n"), "{out}");
        let s = r.probe_tool_syntax().unwrap();
        assert!(s.call_open.starts_with("<tool_call>"), "{s:?}");
        assert!(s.call_close.contains("</tool_call>"), "{s:?}");
        assert!(s.tools_in_prompt && s.json_arguments);
        assert!(s.result_frame.contains("<tool_response>"), "{s:?}");
        let g = tool_call_grammar(&s, &["read_file", "run"]);
        assert!(g.contains("<tool_call>") && g.contains("\\\"read_file\\\""), "{g}");
    }

    /// Qwen3.5 / Ornith-1.5 shape: the generation prompt depends on `enable_thinking`. Before this
    /// variable was passed the templates saw it undefined and always took the thinking branch.
    #[test]
    fn enable_thinking_reaches_the_template() {
        const QWEN35_TAIL: &str = r##"{%- for message in messages %}{{- '<|im_start|>' + message.role + '
' + message.content + '<|im_end|>
' }}{%- endfor %}{%- if add_generation_prompt %}{{- '<|im_start|>assistant
' }}{%- if enable_thinking is defined and enable_thinking is false %}{{- '<think>

</think>

' }}{%- else %}{{- '<think>
' }}{%- endif %}{%- endif %}"##;
        let r = Renderer::new(QWEN35_TAIL).unwrap();
        let on = r.render_thinking(&[Message::user("q")], &[], true, true).unwrap();
        assert!(on.ends_with("<|im_start|>assistant
<think>
"), "thinking on → the opener is in the PROMPT (so the reply carries only </think>): {on:?}");
        let off = r.render_thinking(&[Message::user("q")], &[], true, false).unwrap();
        assert!(off.ends_with("<think>

</think>

"), "thinking off → a closed empty block: {off:?}");
        // a template that never reads it renders the same either way
        let r2 = Renderer::new(QWEN_LIKE).unwrap();
        assert_eq!(r2.render_thinking(&[Message::user("q")], &[], true, true).unwrap(), r2.render_thinking(&[Message::user("q")], &[], true, false).unwrap());
    }

    #[test]
    fn llama31_like_renders_and_probes() {
        let r = Renderer::new(LLAMA31_LIKE).unwrap();
        let out = r.render(&[Message::user("open main.rs")], &tools(), true).unwrap();
        assert!(out.contains("Today Date:") && out.contains("\"read_file\""), "{out}");
        let s = r.probe_tool_syntax().unwrap();
        assert!(s.call_open.starts_with("<|python_tag|>"), "{s:?}");
        assert!(s.result_frame.contains("ipython"), "{s:?}");
    }

    #[test]
    fn thinking_splits_cleanly() {
        assert_eq!(split_thinking("<think>hmm</think>\nanswer"), (Some("hmm".into()), "answer".into()));
        assert_eq!(split_thinking("plain"), (None, "plain".into()));
        assert_eq!(split_thinking("<think>cut off"), (Some("cut off".into()), String::new()));
    }

    #[test]
    fn budgets_are_monotonic() {
        let (a, b, c) = (budget_for(3.0), budget_for(9.0), budget_for(35.0));
        assert!(a.max_tools < b.max_tools && b.max_tools < c.max_tools);
        assert!(a.system_prompt_tokens < b.system_prompt_tokens && b.system_prompt_tokens < c.system_prompt_tokens);
    }
}

#[cfg(test)]
mod show {
    use super::*;
    #[test]
    fn print_probes() {
        for (name, t) in [("qwen-like", tests::QWEN_LIKE), ("llama31-like", tests::LLAMA31_LIKE)] {
            let s = Renderer::new(t).unwrap().probe_tool_syntax().unwrap();
            eprintln!("{name}: open={:?} close={:?} result_frame={:?}", s.call_open, s.call_close, s.result_frame);
        }
    }
}
