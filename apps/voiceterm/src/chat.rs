// src/chat.rs
// LLM chat client: Claude API + OpenAI API
// Stores conversations locally, supports export to markdown/JSON.
//
// Usage: set API keys via env vars:
//   ANTHROPIC_API_KEY=sk-ant-...
//   OPENAI_API_KEY=sk-...

use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: String,  // "user" or "assistant"
    pub content: String,
    pub timestamp: String,
}

#[derive(Clone, Debug)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub provider: String, // "claude" or "chatgpt"
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub created_at: String,
}

pub struct ChatClient {
    pub conversations: Vec<Conversation>,
    pub active_id: Option<String>,
    claude_key: Option<String>,
    openai_key: Option<String>,
}

impl ChatClient {
    pub fn new() -> Self {
        let claude_key = std::env::var("ANTHROPIC_API_KEY").ok();
        let openai_key = std::env::var("OPENAI_API_KEY").ok();
        if claude_key.is_some() { eprintln!("Chat: Claude API key found"); }
        if openai_key.is_some() { eprintln!("Chat: OpenAI API key found"); }
        let conversations = Self::load_conversations();
        ChatClient { conversations, active_id: None, claude_key, openai_key }
    }

    pub fn has_claude(&self) -> bool { self.claude_key.is_some() }
    pub fn has_openai(&self) -> bool { self.openai_key.is_some() }

    /// Start a new conversation
    pub fn new_conversation(&mut self, provider: &str) -> String {
        let id = format!("conv_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
        let model = match provider {
            "claude" => "claude-sonnet-4-20250514",
            "chatgpt" => "gpt-4o",
            _ => "unknown",
        };
        self.conversations.push(Conversation {
            id: id.clone(), title: "New conversation".into(), provider: provider.into(),
            model: model.into(), messages: Vec::new(), created_at: crate::archive::chrono_now(),
        });
        self.active_id = Some(id.clone());
        id
    }

    /// Send a message and get a response
    pub fn send_message(&mut self, message: &str, agent: &ureq::Agent) -> Result<String, String> {
        let conv_idx = if let Some(id) = &self.active_id {
            self.conversations.iter().position(|c| c.id == *id).ok_or("No active conversation")?
        } else { return Err("No active conversation".into()); };

        // Add user message and update title (mutable borrow scope)
        {
            let conv = &mut self.conversations[conv_idx];
            conv.messages.push(ChatMessage {
                role: "user".into(), content: message.into(), timestamp: crate::archive::chrono_now(),
            });
            if conv.messages.len() == 1 {
                conv.title = message.chars().take(50).collect::<String>();
                if message.len() > 50 { conv.title.push_str("..."); }
            }
        }

        // Clone conversation for API call (releases mutable borrow)
        let conv_clone = self.conversations[conv_idx].clone();
        let response = match conv_clone.provider.as_str() {
            "claude" => self.call_claude(&conv_clone, agent)?,
            "chatgpt" => self.call_openai(&conv_clone, agent)?,
            _ => return Err("Unknown provider".into()),
        };

        // Add assistant response
        let conv = &mut self.conversations[conv_idx];
        conv.messages.push(ChatMessage {
            role: "assistant".into(), content: response.clone(), timestamp: crate::archive::chrono_now(),
        });

        self.save_conversations();
        Ok(response)
    }

    fn call_claude(&self, conv: &Conversation, agent: &ureq::Agent) -> Result<String, String> {
        let key = self.claude_key.as_ref().ok_or("No ANTHROPIC_API_KEY set")?;

        // Build messages array
        let mut msgs = String::from("[");
        for (i, m) in conv.messages.iter().enumerate() {
            if i > 0 { msgs.push(','); }
            msgs.push_str("{\"role\":\""); msgs.push_str(&m.role);
            msgs.push_str("\",\"content\":\""); msgs.push_str(&json_esc(&m.content));
            msgs.push_str("\"}");
        }
        msgs.push(']');

        let body = format!(
            "{{\"model\":\"{}\",\"max_tokens\":4096,\"messages\":{}}}",
            conv.model, msgs
        );

        let resp = agent.post("https://api.anthropic.com/v1/messages")
            .set("x-api-key", key)
            .set("anthropic-version", "2023-06-01")
            .set("content-type", "application/json")
            .send_string(&body)
            .map_err(|e| format!("Claude API error: {}", e))?;

        let text = resp.into_string().map_err(|e| format!("Read error: {}", e))?;

        // v0.1.352: token accounting (see token_log.rs). Best-effort
        // — failures here don't break the response. Op tag is "chat"
        // to distinguish from the lower-level "claude_query" used by
        // llm_provider.
        let (in_tok, out_tok, cached) = crate::token_log::parse_anthropic_usage(&text);
        if in_tok > 0 || out_tok > 0 {
            crate::token_log::append_event(crate::token_log::TokenEvent {
                ts: crate::token_log::now_iso(),
                op: "chat".to_string(),
                model: conv.model.clone(),
                input_tokens: in_tok,
                output_tokens: out_tok,
                cached_input_tokens: cached,
            });
        }

        // Extract content from response
        // {"content":[{"type":"text","text":"..."}],...}
        if let Some(pos) = text.find("\"text\":\"") {
            let start = pos + 8;
            let rest = &text[start..];
            let mut end = 0;
            let mut esc = false;
            for c in rest.chars() {
                if esc { esc = false; end += c.len_utf8(); continue; }
                if c == '\\' { esc = true; end += 1; continue; }
                if c == '"' { break; }
                end += c.len_utf8();
            }
            let content = rest[..end].replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\");
            Ok(content)
        } else if text.contains("\"error\"") {
            Err(format!("Claude error: {}", crate::archive::safe_trunc(&text, 200)))
        } else {
            Err("Could not parse Claude response".into())
        }
    }

    fn call_openai(&self, conv: &Conversation, agent: &ureq::Agent) -> Result<String, String> {
        let key = self.openai_key.as_ref().ok_or("No OPENAI_API_KEY set")?;

        let mut msgs = String::from("[");
        for (i, m) in conv.messages.iter().enumerate() {
            if i > 0 { msgs.push(','); }
            msgs.push_str("{\"role\":\""); msgs.push_str(&m.role);
            msgs.push_str("\",\"content\":\""); msgs.push_str(&json_esc(&m.content));
            msgs.push_str("\"}");
        }
        msgs.push(']');

        let body = format!(
            "{{\"model\":\"{}\",\"messages\":{},\"max_tokens\":4096}}",
            conv.model, msgs
        );

        let resp = agent.post("https://api.openai.com/v1/chat/completions")
            .set("Authorization", &format!("Bearer {}", key))
            .set("content-type", "application/json")
            .send_string(&body)
            .map_err(|e| format!("OpenAI API error: {}", e))?;

        let text = resp.into_string().map_err(|e| format!("Read error: {}", e))?;

        // Extract content: {"choices":[{"message":{"content":"..."}}]}
        if let Some(pos) = text.find("\"content\":\"") {
            let start = pos + 11;
            let rest = &text[start..];
            let mut end = 0;
            let mut esc = false;
            for c in rest.chars() {
                if esc { esc = false; end += c.len_utf8(); continue; }
                if c == '\\' { esc = true; end += 1; continue; }
                if c == '"' { break; }
                end += c.len_utf8();
            }
            let content = rest[..end].replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\");
            Ok(content)
        } else if text.contains("\"error\"") {
            Err(format!("OpenAI error: {}", crate::archive::safe_trunc(&text, 200)))
        } else {
            Err("Could not parse OpenAI response".into())
        }
    }

    /// Export conversation to markdown
    pub fn export_markdown(&self, conv_id: &str) -> Option<String> {
        let conv = self.conversations.iter().find(|c| c.id == conv_id)?;
        let mut out = String::with_capacity(conv.messages.len() * 200);
        out.push_str("# "); out.push_str(&conv.title); out.push('\n');
        out.push_str("*Provider: "); out.push_str(&conv.provider);
        out.push_str(" | Model: "); out.push_str(&conv.model);
        out.push_str(" | Created: "); out.push_str(&conv.created_at); out.push_str("*\n\n");
        out.push_str("---\n\n");
        for msg in &conv.messages {
            if msg.role == "user" { out.push_str("## You\n"); }
            else { out.push_str("## Assistant\n"); }
            out.push_str(&msg.content); out.push_str("\n\n---\n\n");
        }
        Some(out)
    }

    /// Export conversation to JSON
    pub fn export_json(&self, conv_id: &str) -> Option<String> {
        let conv = self.conversations.iter().find(|c| c.id == conv_id)?;
        let mut out = String::with_capacity(conv.messages.len() * 200);
        out.push_str("{\"id\":\""); out.push_str(&conv.id);
        out.push_str("\",\"title\":\""); out.push_str(&json_esc(&conv.title));
        out.push_str("\",\"provider\":\""); out.push_str(&conv.provider);
        out.push_str("\",\"model\":\""); out.push_str(&conv.model);
        out.push_str("\",\"created_at\":\""); out.push_str(&conv.created_at);
        out.push_str("\",\"messages\":[");
        for (i, msg) in conv.messages.iter().enumerate() {
            if i > 0 { out.push(','); }
            out.push_str("{\"role\":\""); out.push_str(&msg.role);
            out.push_str("\",\"content\":\""); out.push_str(&json_esc(&msg.content));
            out.push_str("\",\"timestamp\":\""); out.push_str(&msg.timestamp);
            out.push_str("\"}");
        }
        out.push_str("]}");
        Some(out)
    }

    /// List conversations for QML display
    /// Returns "C\x1Fid\x1Ftitle\x1Fprovider\x1Fdate\x1Fmsg_count\n" per conversation
    pub fn list_conversations(&self) -> String {
        let mut out = String::with_capacity(self.conversations.len() * 80);
        for conv in self.conversations.iter().rev() {
            out.push_str("C\x1F"); out.push_str(&conv.id);
            out.push('\x1F'); out.push_str(crate::archive::safe_trunc(&conv.title, 50));
            out.push('\x1F'); out.push_str(&conv.provider);
            out.push('\x1F'); out.push_str(&conv.created_at);
            out.push('\x1F'); out.push_str(&conv.messages.len().to_string());
            out.push('\n');
        }
        out
    }

    /// Get active conversation messages for display
    pub fn active_messages(&self) -> String {
        let conv = match &self.active_id {
            Some(id) => self.conversations.iter().find(|c| c.id == *id),
            None => None,
        };
        let conv = match conv { Some(c) => c, None => return String::new() };
        let mut out = String::with_capacity(conv.messages.len() * 200);
        for msg in &conv.messages {
            out.push_str("M\x1F"); out.push_str(&msg.role);
            out.push('\x1F'); out.push_str(&msg.content);
            out.push('\x1F'); out.push_str(&msg.timestamp);
            out.push('\n');
        }
        out
    }

    /// v0.1.551: local-model chat support. Append a user message to the
    /// active conversation (creating one if needed) and return a flat
    /// prompt string built from the full history, suitable for a local
    /// llama.cpp completion. Cloud providers keep their own structured
    /// request builders (call_claude/call_openai); this is the plain-
    /// text path for local models that take a single prompt.
    ///
    /// Returns None if there's no active conversation and one can't be
    /// made. The caller runs the returned prompt through the local
    /// model, then calls `push_assistant_response`.
    pub fn push_user_and_build_prompt(&mut self, message: &str) -> Option<String> {
        let conv_idx = match &self.active_id {
            Some(id) => self.conversations.iter().position(|c| c.id == *id)?,
            None => return None,
        };
        {
            let conv = &mut self.conversations[conv_idx];
            conv.messages.push(ChatMessage {
                role: "user".into(),
                content: message.into(),
                timestamp: crate::archive::chrono_now(),
            });
            if conv.messages.len() == 1 {
                conv.title = message.chars().take(50).collect::<String>();
                if message.len() > 50 { conv.title.push_str("..."); }
            }
        }
        Some(Self::build_flat_prompt(&self.conversations[conv_idx].messages))
    }

    /// Build a flat chat-style prompt from a message list. Pure — no
    /// self, no I/O — so it's unit-testable. Uses a simple, model-
    /// agnostic role-tagged format that small instruct models follow.
    pub fn build_flat_prompt(messages: &[ChatMessage]) -> String {
        let mut p = String::with_capacity(messages.len() * 128);
        for m in messages {
            let tag = if m.role == "assistant" { "Assistant" } else { "User" };
            p.push_str(tag);
            p.push_str(": ");
            p.push_str(&m.content);
            p.push('\n');
        }
        // Cue the model to produce the next assistant turn.
        p.push_str("Assistant: ");
        p
    }

    /// Append an assistant response to the active conversation (after a
    /// local-model completion). Mirrors the cloud path's bookkeeping.
    pub fn push_assistant_response(&mut self, response: &str) {
        let conv_idx = match &self.active_id {
            Some(id) => match self.conversations.iter().position(|c| c.id == *id) {
                Some(i) => i, None => return,
            },
            None => return,
        };
        let conv = &mut self.conversations[conv_idx];
        conv.messages.push(ChatMessage {
            role: "assistant".into(),
            content: response.into(),
            timestamp: crate::archive::chrono_now(),
        });
    }

    // Persistence
    fn conv_dir() -> PathBuf {
        let dir = PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into())).join(".config/web2qml/chats");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn load_conversations() -> Vec<Conversation> {
        let dir = Self::conv_dir();
        let mut convs = Vec::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                    if let Ok(content) = fs::read_to_string(entry.path()) {
                        if let Some(conv) = Self::parse_conversation(&content) {
                            convs.push(conv);
                        }
                    }
                }
            }
        }
        convs.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        eprintln!("Chat: {} conversations loaded", convs.len());
        convs
    }

    fn save_conversations(&self) {
        let dir = Self::conv_dir();
        for conv in &self.conversations {
            let json = self.conv_to_json(conv);
            let path = dir.join(format!("{}.json", conv.id));
            let _ = fs::write(path, json);
        }
    }

    fn conv_to_json(&self, conv: &Conversation) -> String {
        self.export_json(&conv.id).unwrap_or_default()
    }

    fn parse_conversation(json: &str) -> Option<Conversation> {
        let id = json_get(json, "id")?;
        let title = json_get(json, "title").unwrap_or_default();
        let provider = json_get(json, "provider").unwrap_or("claude".into());
        let model = json_get(json, "model").unwrap_or_default();
        let created_at = json_get(json, "created_at").unwrap_or_default();

        let mut messages = Vec::new();
        if let Some(msgs_start) = json.find("\"messages\":[") {
            let msgs_str = &json[msgs_start + 12..];
            if let Some(end) = find_matching_bracket(&format!("[{}", msgs_str)) {
                let arr = &msgs_str[..end - 1];
                for obj in arr.split("},{") {
                    let obj = obj.trim().trim_matches(|c| c == '{' || c == '}' || c == '[' || c == ']');
                    if let (Some(role), Some(content)) = (json_get(obj, "role"), json_get(obj, "content")) {
                        messages.push(ChatMessage {
                            role, content,
                            timestamp: json_get(obj, "timestamp").unwrap_or_default(),
                        });
                    }
                }
            }
        }

        Some(Conversation { id, title, provider, model, messages, created_at })
    }
}

fn json_esc(s: &str) -> String { crate::util::json_esc(s) }

fn json_get(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\":\"", key);
    let start = json.find(&pat)? + pat.len();
    let rest = &json[start..];
    let mut end = 0; let mut esc = false;
    for c in rest.chars() {
        if esc { esc = false; end += c.len_utf8(); continue; }
        if c == '\\' { esc = true; end += 1; continue; }
        if c == '"' { break; }
        end += c.len_utf8();
    }
    Some(rest[..end].replace("\\\"", "\"").replace("\\n", "\n").replace("\\\\", "\\"))
}

fn find_matching_bracket(s: &str) -> Option<usize> {
    let open = *s.as_bytes().first()?;
    let close = match open { b'[' => b']', b'{' => b'}', _ => return None };
    let mut depth = 0; let mut in_str = false; let mut esc = false;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if esc { esc = false; continue; }
        if b == b'\\' && in_str { esc = true; continue; }
        if b == b'"' { in_str = !in_str; continue; }
        if in_str { continue; }
        if b == open { depth += 1; }
        if b == close { depth -= 1; if depth == 0 { return Some(i); } }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── build_flat_prompt tests (v0.1.551 local-model chat) ───

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage { role: role.into(), content: content.into(), timestamp: "t".into() }
    }

    #[test]
    fn flat_prompt_empty_ends_with_assistant_cue() {
        // Even with no history, the prompt must cue the assistant turn.
        assert_eq!(ChatClient::build_flat_prompt(&[]), "Assistant: ");
    }

    #[test]
    fn flat_prompt_single_user_turn() {
        let p = ChatClient::build_flat_prompt(&[msg("user", "hello")]);
        assert_eq!(p, "User: hello\nAssistant: ");
    }

    #[test]
    fn flat_prompt_alternating_roles() {
        let p = ChatClient::build_flat_prompt(&[
            msg("user", "hi"),
            msg("assistant", "hello there"),
            msg("user", "how are you"),
        ]);
        assert_eq!(p, "User: hi\nAssistant: hello there\nUser: how are you\nAssistant: ");
    }

    #[test]
    fn flat_prompt_unknown_role_treated_as_user() {
        // Defensive: any non-"assistant" role maps to User.
        let p = ChatClient::build_flat_prompt(&[msg("system", "x")]);
        assert_eq!(p, "User: x\nAssistant: ");
    }

    // ─── json_esc tests ───

    #[test]
    fn json_esc_simple() {
        assert_eq!(json_esc("hello"), "hello");
    }

    #[test]
    fn json_esc_quotes() {
        assert_eq!(json_esc("he said \"hi\""), r#"he said \"hi\""#);
    }

    #[test]
    fn json_esc_backslash() {
        assert_eq!(json_esc("path\\to\\file"), "path\\\\to\\\\file");
    }

    #[test]
    fn json_esc_newline() {
        assert_eq!(json_esc("line1\nline2"), "line1\\nline2");
    }

    #[test]
    fn json_esc_strips_carriage_return() {
        // \r is dropped (per impl); \n stays as escape.
        assert_eq!(json_esc("line1\r\nline2"), "line1\\nline2");
    }

    #[test]
    fn json_esc_tab() {
        assert_eq!(json_esc("col1\tcol2"), "col1\\tcol2");
    }

    #[test]
    fn json_esc_strips_other_control_chars() {
        // Control chars other than \n, \r, \t are stripped.
        assert_eq!(json_esc("a\x07b"), "ab");
        assert_eq!(json_esc("a\x01b"), "ab");
    }

    // ─── json_get tests ───

    #[test]
    fn json_get_simple() {
        let json = r#"{"name":"Alice"}"#;
        assert_eq!(json_get(json, "name"), Some("Alice".to_string()));
    }

    #[test]
    fn json_get_missing_key() {
        let json = r#"{"name":"Alice"}"#;
        assert_eq!(json_get(json, "age"), None);
    }

    #[test]
    fn json_get_with_escaped_quote() {
        // Value contains an escaped quote.
        let json = r#"{"msg":"he said \"hi\""}"#;
        assert_eq!(json_get(json, "msg"), Some(r#"he said "hi""#.to_string()));
    }

    #[test]
    fn json_get_with_escaped_newline() {
        let json = r#"{"text":"line1\nline2"}"#;
        assert_eq!(json_get(json, "text"), Some("line1\nline2".to_string()));
    }

    // ─── find_matching_bracket tests ───

    #[test]
    fn matching_bracket_simple_object() {
        assert_eq!(find_matching_bracket("{}"), Some(1));
    }

    #[test]
    fn matching_bracket_simple_array() {
        assert_eq!(find_matching_bracket("[]"), Some(1));
    }

    #[test]
    fn matching_bracket_nested() {
        assert_eq!(find_matching_bracket("[[],[]]"), Some(6));
        assert_eq!(find_matching_bracket("{\"a\":[1,2]}"), Some(10));
    }

    #[test]
    fn matching_bracket_string_with_brackets() {
        // Brackets inside strings should not be counted as nesting.
        let s = r#"{"key":"[not nested]"}"#;
        assert_eq!(find_matching_bracket(s), Some(s.len() - 1));
    }

    #[test]
    fn matching_bracket_invalid_open() {
        // First char isn't a bracket → None
        assert_eq!(find_matching_bracket("abc"), None);
    }

    #[test]
    fn matching_bracket_unbalanced() {
        // Missing close
        assert_eq!(find_matching_bracket("{a:1"), None);
    }
}
