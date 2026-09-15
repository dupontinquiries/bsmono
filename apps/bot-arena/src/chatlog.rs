//! chatlog — Claude conversation exports as a structured corpus.
//!
//! An export is one JSON file per conversation: metadata plus `chat_messages`,
//! each message a `sender` and a list of typed content blocks. The blocks are
//! where the value is — a turn is not a wall of prose but an alternation of
//!
//!   human ask -> assistant *thinking* -> *tool_use* -> *tool_result* -> text
//!
//! and flattening that to "user said X, assistant said Y" throws away the part
//! worth training on. So the model here keeps the block structure and the
//! renderer decides what to emit, rather than the parser deciding for it.
//!
//! ## Measured shape of the input
//!
//! Across the 219-conversation sample this was built against: 19,931 messages,
//! 147k `tool_use`, 147k `tool_result`, 87k `text`, 52k `thinking`, and 1,086
//! `token_budget`. Tool calls outnumber prose 2:1, and `bash_tool` alone is
//! 101k of the 147k. A corpus that drops tool blocks keeps under a third of
//! the record; one that includes raw tool output is dominated by build logs.
//! Both are wrong, which is what [`Policy`] exists to tune.
//!
//! ## Unknown block types are kept, not dropped
//!
//! `token_budget` was not in the first sample and appeared in the full scan.
//! An unrecognised type becomes [`Block::Other`] carrying its type name, so a
//! new export format shows up as a visible count in the summary rather than as
//! silently missing turns.

/// Which stage-0 converter turns raw input into a corpus.
///
/// Lives here rather than in either shell's own config struct because it is
/// plain data both shells need identically — the GTK bin and the egui
/// front-end each keep their own `StudioConfig` (deliberately duplicated per
/// this crate's convention: `see corpus_studio.rs`'s "field-for-field the GTK
/// X, verbatim" comments), but a converter CHOICE drifting between the two
/// would mean the same settings file's `pp` key means something different
/// depending which shell reads it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preprocessor {
    /// Bundled `corpus_md`: a directory tree of HTML into one Markdown corpus.
    Html,
    /// Bundled `corpus_chat`: Claude conversation exports into one corpus,
    /// optionally correlated against a checkout.
    Chat,
    /// The user's own tool, invoked with a `{in}`/`{out}` argument template.
    External,
}

impl Preprocessor {
    /// The bundled binary's name, or `None` for [`Preprocessor::External`].
    pub fn builtin_bin(self) -> Option<&'static str> {
        match self {
            Preprocessor::Html => Some("corpus_md"),
            Preprocessor::Chat => Some("corpus_chat"),
            Preprocessor::External => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Preprocessor::Html => "Built-in HTML→MD (corpus_md)",
            Preprocessor::Chat => "Built-in chat exports (corpus_chat)",
            Preprocessor::External => "My own tool",
        }
    }

    /// The settings-file token. Kept stable and explicit rather than derived
    /// from the variant name, so renaming a variant cannot silently orphan
    /// everyone's saved choice.
    pub fn key(self) -> &'static str {
        match self {
            Preprocessor::Html => "html",
            Preprocessor::Chat => "chat",
            Preprocessor::External => "external",
        }
    }

    pub fn from_key(s: &str) -> Option<Preprocessor> {
        match s {
            "html" => Some(Preprocessor::Html),
            "chat" => Some(Preprocessor::Chat),
            "external" => Some(Preprocessor::External),
            _ => None,
        }
    }

    /// The bundled converter's path, next to the given executable (pass
    /// `std::env::current_exe()`'s directory). `None` for
    /// [`Preprocessor::External`] — there is no bundled binary for the user's
    /// own tool, and returning a bogus path would turn a programming mistake
    /// into a confusing "no such file" at run time.
    pub fn builtin_path_near(self, exe_dir: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
        let name = self.builtin_bin()?;
        Some(match exe_dir {
            Some(d) => d.join(name),
            None => std::path::PathBuf::from(name),
        })
    }
}

use crate::json::{self, J};

/// Who produced a turn. The export's `sender` field is `"human"`/`"assistant"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Human,
    Assistant,
}

impl Role {
    fn parse(s: &str) -> Option<Role> {
        match s {
            "human" => Some(Role::Human),
            "assistant" => Some(Role::Assistant),
            _ => None,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Role::Human => "HUMAN",
            Role::Assistant => "ASSISTANT",
        }
    }
}

/// One content block inside a turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// Prose the other party sees.
    Text(String),
    /// The assistant's reasoning. Present on ~half the assistant turns.
    Thinking(String),
    ToolUse(ToolUse),
    ToolResult(ToolResult),
    /// A block type this parser does not model. Carries the `type` string so
    /// an unexpected export format is countable instead of invisible.
    Other(String),
}

/// A tool invocation.
///
/// `input` is kept as ordered key/value pairs rather than a typed per-tool
/// struct: there are 17 distinct tool names in the sample and they change
/// between product releases, so a closed enum would need editing every time
/// the tool list moves. `path` is lifted out because it is the one field the
/// correlation layer needs and it means the same thing across `view`,
/// `str_replace` and `create_file`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolUse {
    pub name: String,
    pub description: Option<String>,
    pub path: Option<String>,
    pub input: Vec<(String, String)>,
}

/// What a tool returned.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolResult {
    pub name: String,
    pub is_error: bool,
    pub text: String,
}

/// One message.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub role: Role,
    pub blocks: Vec<Block>,
}

/// One conversation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Episode {
    pub title: String,
    pub uuid: String,
    pub model: String,
    pub created_at: String,
    pub turns: Vec<Turn>,
}

impl Episode {
    /// Block counts by kind, for the summary line. Returns
    /// `(text, thinking, tool_use, tool_result, other)`.
    pub fn block_counts(&self) -> (usize, usize, usize, usize, usize) {
        let (mut t, mut th, mut tu, mut tr, mut o) = (0, 0, 0, 0, 0);
        for turn in &self.turns {
            for b in &turn.blocks {
                match b {
                    Block::Text(_) => t += 1,
                    Block::Thinking(_) => th += 1,
                    Block::ToolUse(_) => tu += 1,
                    Block::ToolResult(_) => tr += 1,
                    Block::Other(_) => o += 1,
                }
            }
        }
        (t, th, tu, tr, o)
    }

    /// Every path any tool touched, in first-seen order, deduplicated. The
    /// input to correlation.
    pub fn touched_paths(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for turn in &self.turns {
            for b in &turn.blocks {
                if let Block::ToolUse(tu) = b {
                    if let Some(p) = tu.path.as_deref() {
                        if !out.contains(&p) {
                            out.push(p);
                        }
                    }
                }
            }
        }
        out
    }
}

/// Parse one conversation export.
///
/// Tolerant by design: a message with no recognisable `sender` is skipped
/// rather than failing the file, because one malformed turn in a
/// three-thousand-turn export should cost that turn. A file that is not JSON
/// at all, or has no `chat_messages` array, IS an error — that is a wrong
/// input rather than a damaged one, and silently producing an empty episode
/// would look like a conversation that happened to be empty.
pub fn parse_episode(src: &str) -> Result<Episode, String> {
    let v = json::parse(src)?;
    let msgs = v.get("chat_messages").ok_or("no chat_messages array")?;
    if !matches!(msgs, J::Arr(_)) {
        return Err("chat_messages is not an array".into());
    }
    let mut ep = Episode {
        title: v.str_field("name").unwrap_or_default().to_string(),
        uuid: v.str_field("uuid").unwrap_or_default().to_string(),
        model: v.str_field("model").unwrap_or_default().to_string(),
        created_at: v.str_field("created_at").unwrap_or_default().to_string(),
        turns: Vec::new(),
    };
    for m in msgs.arr() {
        let Some(role) = m.str_field("sender").and_then(Role::parse) else { continue };
        let blocks: Vec<Block> = m.get("content").map(|c| c.arr()).unwrap_or(&[])
            .iter()
            .filter_map(parse_block)
            .collect();
        if blocks.is_empty() {
            continue;
        }
        ep.turns.push(Turn { role, blocks });
    }
    Ok(ep)
}

fn parse_block(b: &J) -> Option<Block> {
    let ty = b.str_field("type")?;
    Some(match ty {
        "text" => Block::Text(b.str_field("text").unwrap_or_default().to_string()),
        "thinking" => Block::Thinking(b.str_field("thinking").unwrap_or_default().to_string()),
        "tool_use" => {
            let input = b.get("input");
            let mut kv: Vec<(String, String)> = Vec::new();
            if let Some(J::Obj(o)) = input {
                for (k, val) in o {
                    kv.push((k.clone(), scalar_to_string(val)));
                }
            }
            Block::ToolUse(ToolUse {
                name: b.str_field("name").unwrap_or_default().to_string(),
                description: input
                    .and_then(|i| i.str_field("description"))
                    .map(|s| s.to_string()),
                path: input.and_then(|i| i.str_field("path")).map(|s| s.to_string()),
                input: kv,
            })
        }
        "tool_result" => Block::ToolResult(ToolResult {
            name: b.str_field("name").unwrap_or_default().to_string(),
            is_error: b.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false),
            text: tool_result_text(b),
        }),
        other => Block::Other(other.to_string()),
    })
}

/// A `tool_result`'s payload is a list of inner blocks, each `{type, text}`.
/// Concatenates the text ones; anything else (an image, say) contributes
/// nothing rather than a placeholder, since a corpus line reading
/// `[image]` teaches nothing.
fn tool_result_text(b: &J) -> String {
    let Some(c) = b.get("content") else { return String::new() };
    let mut out = String::new();
    for inner in c.arr() {
        if inner.str_field("type") == Some("text") {
            if let Some(t) = inner.str_field("text") {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
    }
    out
}

/// Flatten a JSON value to one line for the `input` map.
///
/// Arrays and objects are rendered compactly rather than pretty-printed: these
/// become single corpus lines like `filepaths = [...]`, and a multi-line value
/// inside a key/value listing makes the block boundaries unreadable.
fn scalar_to_string(v: &J) -> String {
    match v {
        J::Str(s) => s.clone(),
        J::Num(n) => {
            if n.fract() == 0.0 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        J::Bool(b) => b.to_string(),
        J::Null => String::new(),
        J::Arr(_) | J::Obj(_) => json::pretty(v, 0).replace('\n', " "),
    }
}

// ── rendering ────────────────────────────────────────────────────────────────

/// What to put in the corpus.
///
/// The defaults are the ones the measured shape argues for: keep reasoning
/// (it is the scarce half of the signal), keep tool calls (they are the
/// action), and cap tool OUTPUT hard — 101k bash results are mostly build
/// logs, and an uncapped corpus is a corpus of compiler warnings.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub include_thinking: bool,
    pub include_tool_use: bool,
    pub include_tool_results: bool,
    /// Max characters of a single tool result. 0 disables the cap.
    pub tool_result_cap: usize,
    /// Max characters of a single `tool_use` input value (a `create_file`
    /// `file_text` can be tens of KB and is usually already in the repo).
    pub tool_input_cap: usize,
    /// Annotate tool paths that resolve to a real file in the monorepo.
    pub correlate: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            include_thinking: true,
            include_tool_use: true,
            include_tool_results: true,
            tool_result_cap: 2000,
            tool_input_cap: 4000,
            correlate: true,
        }
    }
}

/// Resolves a sandbox path to a path in the repository. Implemented by
/// [`crate::repo_index::RepoIndex`].
///
/// A trait rather than a concrete type so the renderer stays pure and
/// testable: rendering does not need to know that correlation is a suffix
/// search over `git ls-files`, and a test can resolve from a two-entry stub.
pub trait Correlate {
    /// The canonical repo-relative path, or `None` when the path is not in
    /// this repository or is ambiguous.
    fn resolve(&self, path: &str) -> Option<String>;
}

/// Nothing correlates. Used when the feature is off or no repo was given.
pub struct NoCorrelation;

impl Correlate for NoCorrelation {
    fn resolve(&self, _path: &str) -> Option<String> {
        None
    }
}

/// Truncate on a char boundary, with a marker naming what was dropped.
///
/// The marker carries the DROPPED COUNT rather than just an ellipsis: a
/// reader (or a later filter) can tell a 20-character trim from a 200 KB one,
/// and the difference decides whether the cap is set wrong.
fn cap(s: &str, limit: usize) -> String {
    if limit == 0 || s.chars().count() <= limit {
        return s.to_string();
    }
    let kept: String = s.chars().take(limit).collect();
    let dropped = s.chars().count() - limit;
    format!("{kept}\n[... {dropped} more characters truncated]")
}

/// Render one episode to corpus text.
///
/// The format is line-oriented and explicitly delimited (`=== ROLE ===`,
/// `--- kind ---`) rather than markdown, because the downstream BPE tokenizer
/// sees this as a flat stream: consistent rare delimiters give it stable token
/// boundaries between turns, where markdown headings collide with the markdown
/// that is already inside the conversation content.
pub fn render(ep: &Episode, policy: &Policy, corr: &dyn Correlate) -> String {
    let mut out = String::new();
    out.push_str(&format!("# EPISODE: {}\n", ep.title));
    if !ep.model.is_empty() {
        out.push_str(&format!("# model: {}\n", ep.model));
    }
    if !ep.created_at.is_empty() {
        out.push_str(&format!("# created: {}\n", ep.created_at));
    }
    out.push('\n');

    for turn in &ep.turns {
        let mut body = String::new();
        for b in &turn.blocks {
            match b {
                Block::Text(t) => {
                    if !t.trim().is_empty() {
                        body.push_str(t.trim_end());
                        body.push_str("\n\n");
                    }
                }
                Block::Thinking(t) => {
                    if policy.include_thinking && !t.trim().is_empty() {
                        body.push_str("--- thinking ---\n");
                        body.push_str(t.trim_end());
                        body.push_str("\n\n");
                    }
                }
                Block::ToolUse(tu) => {
                    if policy.include_tool_use {
                        body.push_str(&render_tool_use(tu, policy, corr));
                    }
                }
                Block::ToolResult(tr) => {
                    if policy.include_tool_results {
                        body.push_str(&render_tool_result(tr, policy));
                    }
                }
                // Counted in the summary; contributes no corpus text, since
                // the type name alone is not language.
                Block::Other(_) => {}
            }
        }
        if body.trim().is_empty() {
            continue;
        }
        out.push_str(&format!("=== {} ===\n", turn.role.tag()));
        out.push_str(body.trim_end());
        out.push_str("\n\n");
    }
    out
}

fn render_tool_use(tu: &ToolUse, policy: &Policy, corr: &dyn Correlate) -> String {
    let mut s = format!("--- tool_use: {} ---\n", tu.name);
    if let Some(p) = tu.path.as_deref() {
        // The correlation line is what ties this corpus to the monorepo: the
        // sandbox path is per-session scratch (`/home/claude/work/...`) and
        // means nothing outside that session, while the repo path is stable
        // and is the key a reader or a downstream tool can actually look up.
        if policy.correlate {
            if let Some(real) = corr.resolve(p) {
                s.push_str(&format!("repo_path: {real}\n"));
            }
        }
    }
    for (k, v) in &tu.input {
        // `description` is already the human-readable intent and is printed
        // as-is; the rest are shown as key = value.
        if k == "description" {
            continue;
        }
        s.push_str(&format!("{k}: {}\n", cap(v, policy.tool_input_cap)));
    }
    if let Some(d) = tu.description.as_deref() {
        s.push_str(&format!("intent: {d}\n"));
    }
    s.push('\n');
    s
}

fn render_tool_result(tr: &ToolResult, policy: &Policy) -> String {
    let status = if tr.is_error { " (error)" } else { "" };
    let text = cap(tr.text.trim(), policy.tool_result_cap);
    if text.is_empty() {
        return String::new();
    }
    format!("--- tool_result: {}{} ---\n{}\n\n", tr.name, status, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "uuid": "abc",
      "name": "a conversation",
      "model": "claude-x",
      "created_at": "2026-08-28T00:00:00Z",
      "chat_messages": [
        {"sender":"human","content":[{"type":"text","text":"fix the parser"}]},
        {"sender":"assistant","content":[
          {"type":"thinking","thinking":"the escape arm is wrong"},
          {"type":"tool_use","name":"str_replace","input":{
             "path":"/home/claude/work/src/json.rs",
             "description":"fix the escape",
             "old_str":"a","new_str":"b"}},
          {"type":"tool_result","name":"str_replace","is_error":false,
           "content":[{"type":"text","text":"ok"}]},
          {"type":"text","text":"done"}
        ]},
        {"sender":"system","content":[{"type":"text","text":"ignored"}]}
      ]
    }"#;

    struct StubRepo;
    impl Correlate for StubRepo {
        fn resolve(&self, p: &str) -> Option<String> {
            p.ends_with("src/json.rs").then(|| "apps/bot-arena/src/json.rs".to_string())
        }
    }

    #[test]
    fn parses_metadata_and_turns() {
        let ep = parse_episode(SAMPLE).unwrap();
        assert_eq!(ep.title, "a conversation");
        assert_eq!(ep.model, "claude-x");
        // The `system` sender is not a modelled role, so that message is
        // skipped -- but the two real turns survive.
        assert_eq!(ep.turns.len(), 2);
        assert_eq!(ep.turns[0].role, Role::Human);
        assert_eq!(ep.turns[1].role, Role::Assistant);
    }

    #[test]
    fn keeps_block_structure_rather_than_flattening_to_prose() {
        let ep = parse_episode(SAMPLE).unwrap();
        let (text, thinking, tool_use, tool_result, other) = ep.block_counts();
        assert_eq!((text, thinking, tool_use, tool_result, other), (2, 1, 1, 1, 0));
    }

    /// An unmodelled type must be counted, not dropped: `token_budget` was
    /// absent from the first sample and present in the full scan.
    #[test]
    fn unknown_block_types_survive_as_Other() {
        let src = r#"{"chat_messages":[{"sender":"assistant","content":[
            {"type":"token_budget","budget":5}]}]}"#;
        let ep = parse_episode(src).unwrap();
        assert_eq!(ep.block_counts().4, 1);
        assert_eq!(ep.turns[0].blocks[0], Block::Other("token_budget".into()));
    }

    #[test]
    fn a_missing_chat_messages_array_is_an_error_not_an_empty_episode() {
        assert!(parse_episode(r#"{"name":"x"}"#).is_err());
        assert!(parse_episode(r#"{"chat_messages":{}}"#).is_err());
        assert!(parse_episode("not json").is_err());
    }

    #[test]
    fn touched_paths_are_deduplicated_in_first_seen_order() {
        let ep = parse_episode(SAMPLE).unwrap();
        assert_eq!(ep.touched_paths(), vec!["/home/claude/work/src/json.rs"]);
    }

    #[test]
    fn render_correlates_paths_to_the_repo() {
        let ep = parse_episode(SAMPLE).unwrap();
        let out = render(&ep, &Policy::default(), &StubRepo);
        assert!(out.contains("repo_path: apps/bot-arena/src/json.rs"), "{out}");
        assert!(out.contains("=== HUMAN ==="));
        assert!(out.contains("=== ASSISTANT ==="));
        assert!(out.contains("--- thinking ---"));
        assert!(out.contains("--- tool_use: str_replace ---"));
        assert!(out.contains("intent: fix the escape"));
    }

    #[test]
    fn policy_switches_actually_remove_content() {
        let ep = parse_episode(SAMPLE).unwrap();
        let p = Policy {
            include_thinking: false,
            include_tool_use: false,
            include_tool_results: false,
            correlate: false,
            ..Policy::default()
        };
        let out = render(&ep, &p, &StubRepo);
        assert!(!out.contains("thinking"), "{out}");
        assert!(!out.contains("tool_use"), "{out}");
        assert!(!out.contains("repo_path"), "{out}");
        // The prose survives.
        assert!(out.contains("fix the parser"));
        assert!(out.contains("done"));
    }

    /// The cap must name how much it dropped, so a wrong cap is visible.
    #[test]
    fn truncation_reports_the_dropped_count() {
        let long = "x".repeat(5000);
        let out = cap(&long, 100);
        assert!(out.contains("[... 4900 more characters truncated]"), "{out}");
        assert_eq!(cap("short", 100), "short");
        assert_eq!(cap(&long, 0).len(), 5000, "0 disables the cap");
    }

    /// Multi-byte content must not panic the truncator, which is why it counts
    /// chars rather than slicing bytes.
    #[test]
    fn truncation_is_char_safe() {
        let s = "→".repeat(50);
        let out = cap(&s, 10);
        assert!(out.starts_with(&"→".repeat(10)));
        assert!(out.contains("40 more characters"));
    }

    /// A turn whose every block is filtered out must not emit a bare header.
    #[test]
    fn a_fully_filtered_turn_emits_nothing() {
        let src = r#"{"chat_messages":[{"sender":"assistant","content":[
            {"type":"thinking","thinking":"hidden"}]}]}"#;
        let ep = parse_episode(src).unwrap();
        let p = Policy { include_thinking: false, ..Policy::default() };
        let out = render(&ep, &p, &NoCorrelation);
        assert!(!out.contains("=== ASSISTANT ==="), "{out}");
    }
}
