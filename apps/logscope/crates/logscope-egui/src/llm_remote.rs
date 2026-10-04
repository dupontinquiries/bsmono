//! llm_remote — an `Llm` for the agent loop that lives on a llama-server, with the two things the
//! in-process path cannot give a small model:
//!
//!   * a GBNF grammar that only admits prose or the exact fenced ```tool { "tool": <one of the
//!     whitelist>, … } ``` block `workspace_exec::system_prompt` teaches, so a malformed fence, a
//!     JSON typo or an invented tool name cannot be emitted at all (missing FIELDS still can, and
//!     `parse_tools_reported` names them);
//!   * `cache_prompt`, so the system prompt and the growing transcript are prefilled once and each
//!     turn pays only for its own delta, which is most of the "faster than Ollama" in an agent loop.
//!
//! Also the tier budget: `for_params(billions)` sizes `n_predict` and the tool count from
//! `template_kit::budget_for`, which is the read_harness rule applied to the tool menu.
use std::time::Duration;

use crate::workspace_exec::{AgentConfig, Llm, RUNTIME_TOOLS};

#[derive(Debug, Clone, Default)]
pub struct Timings { pub prompt_n: u64, pub prompt_ms: f64, pub predicted_n: u64, pub predicted_ms: f64, pub cache_hit_n: Option<u64> }

pub struct RemoteLlm {
    pub base: String,
    pub token: Option<String>,
    pub temperature: f32,
    pub n_predict: i64,
    pub grammar: Option<String>,
    pub stops: Vec<String>,
    /// From the last call: prefill/decode tokens and ms, and how much of the prompt the server's
    /// cache already had, which is the number that says whether prefix reuse is working.
    pub last: Option<Timings>,
}

impl RemoteLlm {
    /// A llama-server at `base` (`http://ultra:8080`), plain: no grammar, model defaults.
    pub fn llama_server(base: &str) -> RemoteLlm {
        RemoteLlm { base: base.trim_end_matches('/').to_string(), token: std::env::var("LOGSCOPE_MINI_TOKEN").ok().filter(|t| !t.is_empty()),
            temperature: 0.0, n_predict: 1024, grammar: None, stops: Vec::new(), last: None }
    }

    /// Constrain output to prose or one well-formed tool block naming only these tools.
    pub fn with_tool_grammar(mut self, tools: &[String]) -> RemoteLlm {
        let names: Vec<&str> = if tools.is_empty() { RUNTIME_TOOLS.to_vec() } else { tools.iter().map(|s| s.as_str()).collect() };
        self.grammar = Some(tool_fence_grammar(&names));
        self
    }

    /// Size the call to the model: small models get short answers and few tools.
    pub fn for_params(mut self, billions: f32, cfg: &mut AgentConfig) -> RemoteLlm {
        let b = template_kit::budget_for(billions);
        self.n_predict = (b.system_prompt_tokens as i64 * 2).clamp(256, 4096);
        if cfg.tools.is_empty() || cfg.tools.len() > b.max_tools {
            // keep the harness's own order; the first tools in RUNTIME_TOOLS are the ones a small
            // model needs (write, edit, read, list, run), the rest are the essays it cannot afford.
            let pick: Vec<String> = if cfg.tools.is_empty() { RUNTIME_TOOLS.iter().take(b.max_tools).map(|s| s.to_string()).collect() } else { cfg.tools.iter().take(b.max_tools).cloned().collect() };
            cfg.tools = pick;
        }
        self.with_tool_grammar(&cfg.tools)
    }
}

impl Llm for RemoteLlm {
    fn generate(&mut self, prompt: &str) -> Result<String, String> {
        let mut body = serde_json::json!({ "prompt": prompt, "n_predict": self.n_predict, "temperature": self.temperature, "cache_prompt": true, "stream": false });
        if let Some(g) = &self.grammar { body["grammar"] = serde_json::Value::String(g.clone()); }
        if !self.stops.is_empty() { body["stop"] = serde_json::json!(self.stops); }
        let mut r = ureq::post(&format!("{}/completion", self.base)).timeout(Duration::from_secs(900));
        if let Some(t) = &self.token { r = r.set("Authorization", &format!("Bearer {t}")); }
        let v: serde_json::Value = r.send_json(body).map_err(|e| format!("llama-server: {e}"))?.into_json().map_err(|e| format!("llama-server reply: {e}"))?;
        if let Some(t) = v.get("timings") {
            let g = |k: &str| t.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
            self.last = Some(Timings { prompt_n: g("prompt_n") as u64, prompt_ms: g("prompt_ms"), predicted_n: g("predicted_n") as u64, predicted_ms: g("predicted_ms"), cache_hit_n: t.get("cache_n").and_then(|x| x.as_u64()) });
        }
        v.get("content").and_then(|c| c.as_str()).map(|s| s.to_string()).ok_or_else(|| "llama-server: no content in reply".into())
    }
}

/// GBNF for the harness's own contract: any prose (no triple backtick), then optionally ONE fenced
/// `tool` block whose object opens with `"tool": <name>` and continues as JSON. Names are the
/// whitelist; everything else about the call is checked by `parse_tools_reported` as before.
///
/// `nonce` is the taint gate's run token: when Some, the opener may carry ` nonce="…"` (the exact
/// token, optional — see template_kit::constrain's module note on why not required). Without it
/// the grammar forbade the attribute the prompt teaches and the gate demands, so under llama-server
/// every authored call was either forced token-less (and refused) or impossible.
pub fn tool_fence_grammar(names: &[&str]) -> String { tool_fence_grammar_nonced(names, None) }
pub fn tool_fence_grammar_nonced(names: &[&str], nonce: Option<&str>) -> String {
    let name = names.iter().map(|n| format!("\"\\\"{n}\\\"\"")).collect::<Vec<_>>().join(" | ");
    // The engine reads the token back off this line (backend::grammar_nonce), so its shape is a contract.
    let nonce_rule = match nonce {
        Some(n) => format!("nonce ::= \" nonce=\\\"{n}\\\"\"\n"),
        None => "nonce ::= \"\"\n".to_string(),
    };
    format!(r##"root ::= prose call? prose
prose ::= ( [^`] | "`" [^`] | "``" [^`] )*
call ::= "```tool" nonce? [ \n] ws "{{" ws "\"tool\"" ws ":" ws name ( ws "," ws pair )* ws "}}\n```"
{nonce_rule}name ::= {name}
pair ::= string ws ":" ws value
value ::= object | array | string | number | "true" | "false" | "null"
object ::= "{{" ws ( pair ( ws "," ws pair )* )? ws "}}"
array ::= "[" ws ( value ( ws "," ws value )* )? ws "]"
string ::= "\"" ( [^"\\] | "\\" . )* "\""
number ::= "-"? [0-9]+ ( "." [0-9]+ )?
ws ::= [ \t\n]*
"##)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_names_only_the_whitelist_and_keeps_the_fence() {
        let g = tool_fence_grammar(&["read_file", "run"]);
        assert!(g.contains("\"\\\"read_file\\\"\" | \"\\\"run\\\"\""), "{g}");
        assert!(!g.contains("write_file"));
        assert!(g.contains("```tool"));
        assert!(g.contains("nonce ::= \"\""), "no token → the attribute rule is empty: {g}");
    }

    #[test]
    fn the_run_token_rides_on_the_grammar_as_an_optional_literal() {
        let g = tool_fence_grammar_nonced(&["run"], Some("a25dbeef"));
        assert!(g.contains("nonce ::= \" nonce=\\\"a25dbeef\\\"\""), "{g}");
        assert!(g.contains("\"```tool\" nonce?"), "optional, on the opener: {g}");
    }

    #[test]
    fn tier_budget_trims_the_tool_menu_for_a_small_model() {
        let mut cfg = AgentConfig::default();
        let llm = RemoteLlm::llama_server("http://127.0.0.1:8080").for_params(3.0, &mut cfg);
        assert_eq!(cfg.tools.len(), template_kit::budget_for(3.0).max_tools);
        assert!(cfg.tools.iter().all(|t| RUNTIME_TOOLS.contains(&t.as_str())));
        assert!(llm.grammar.as_deref().unwrap().contains("write_file"));
        let mut big = AgentConfig::default();
        let _ = RemoteLlm::llama_server("http://x").for_params(35.0, &mut big);
        assert!(big.tools.len() >= 13, "a 35B keeps the whole menu: {}", big.tools.len());
    }
}
