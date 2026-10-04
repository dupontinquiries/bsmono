//! constrain — constrained decoding in Rust, for an engine that has no llama.cpp grammar sampler.
//!
//! llama.cpp masks the vocabulary with a GBNF automaton every token, which is why a 3B there cannot
//! emit a malformed tool call. The wgpu engine samples in Rust and had no equivalent: `GenParams
//! .grammar` was honoured by llama.cpp and llama-server and ignored by it. This is the equivalent,
//! in Rust, and it is deliberately NOT a general GBNF engine — it is an incremental acceptor for the
//! one shape the harness needs, which is small enough to be obviously correct:
//!
//! ```text
//! <prose, no backtick-fence> [ ```tool[ nonce="<run token>"]\n{ "tool": "<whitelisted>", ...json... }\n``` ] <prose>
//! ```
//!
//! The optional ` nonce="…"` attribute on the opener is the taint gate's run token
//! (logscope-egui `taint.rs`): the prompt tells the model to put it on every fence it authors, and
//! the gate refuses a fence without it. This acceptor used to allow only whitespace between `tool`
//! and `{`, so with both on — the default — every character of the attribute was vetoed, and the
//! call went through only when the model's whole top-k was attribute text (exhaustion, the override
//! path) or was lost when a newline was among the candidates. When a nonce is configured
//! (`with_nonce`) the attribute is accepted exactly, and still OPTIONAL: forcing it would stamp the
//! token onto a fence the model is merely quoting, which is the one thing the gate exists to tell
//! apart. With no nonce configured the attribute is rejected, as before.
//!
//! It works on CHARACTERS, not tokens, so it is tokenizer-independent. The sampler asks
//! `would_accept(candidate_text)` for each of its top-k candidates and drops the ones that cannot
//! continue a legal string; filtering the top-k rather than the whole 248k vocabulary is what keeps
//! it cheap (the wgpu sampler already reduces to top-k before softmax). If every candidate is
//! rejected the caller keeps its unconstrained pick — a constraint that can deadlock a decode is
//! worse than one that occasionally lets a bad token through, and `violated()` reports it.
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum S {
    /// Ordinary text. A partial fence opener may be in progress (`fence` counts backticks seen).
    Prose { ticks: u8 },
    /// Inside "```tool" after the ticks: matching the word.
    Word { matched: usize },
    /// One space after the word: either the ` nonce="…"` attribute starts, or more whitespace / the object.
    AfterWord,
    /// Matching the attribute text `nonce="<token>"` character by character.
    Attr { matched: usize },
    /// After ```tool, before the object opens.
    PreObject,
    /// Inside the JSON object. `depth` counts braces/brackets, `in_str`/`esc` track strings.
    Json { depth: u32, in_str: bool, esc: bool },
    /// Object closed; expecting the closing fence.
    PostObject { ticks: u8 },
    /// One call has been emitted; back to prose, and no second call is allowed.
    Done { ticks: u8 },
}

/// The acceptor. Feed it the text as it is decoded; ask it whether a candidate continuation is legal.
#[derive(Debug, Clone)]
pub struct ToolFence {
    state: S,
    /// Tool names allowed after `"tool":`. Empty = any name.
    names: BTreeSet<String>,
    /// The JSON text of the object so far, for checking the name prefix as it is written.
    json: String,
    violated: bool,
    /// Only one call per reply; a second fence is rejected. Set false to allow several.
    pub single_call: bool,
    /// The attribute text accepted after the word (`nonce="…"`), when a run token is configured.
    attr: Option<String>,
}

const WORD: &str = "tool";

impl ToolFence {
    pub fn new(names: &[&str]) -> ToolFence {
        ToolFence { state: S::Prose { ticks: 0 }, names: names.iter().map(|s| s.to_string()).collect(), json: String::new(), violated: false, single_call: true, attr: None }
    }
    /// Accept ` nonce="<token>"` on the opener (optional, exact). See the module note.
    pub fn with_nonce(mut self, token: &str) -> ToolFence {
        self.attr = Some(format!("nonce=\"{token}\""));
        self
    }
    /// The configured run token, if any.
    pub fn nonce(&self) -> Option<&str> {
        self.attr.as_deref().and_then(|a| a.strip_prefix("nonce=\"")).and_then(|a| a.strip_suffix('"'))
    }

    /// True once a token was forced through that the grammar rejected.
    pub fn violated(&self) -> bool { self.violated }
    pub fn note_violation(&mut self) { self.violated = true; }

    /// Has a complete, well-formed call been emitted?
    pub fn complete(&self) -> bool { matches!(self.state, S::Done { .. }) }

    /// Would this text be legal from the current state? Does not advance.
    pub fn would_accept(&self, text: &str) -> bool { self.clone().push_str(text) }

    /// Advance over `text`. Returns false at the first illegal character (state is then unspecified;
    /// callers use `would_accept` on a clone to test, and `push_str` only on text they commit to).
    pub fn push_str(&mut self, text: &str) -> bool {
        for c in text.chars() { if !self.push(c) { return false; } }
        true
    }

    fn push(&mut self, c: char) -> bool {
        self.state = match self.state {
            S::Prose { ticks } | S::Done { ticks } => {
                let done = matches!(self.state, S::Done { .. });
                if c == '`' {
                    let n = ticks + 1;
                    if n == 3 {
                        // a second call is refused when single_call
                        if done && self.single_call { return false; }
                        S::Word { matched: 0 }
                    } else if done { S::Done { ticks: n } } else { S::Prose { ticks: n } }
                } else if ticks > 0 {
                    // backticks that did not become a fence are ordinary prose, unless they were the
                    // start of one: a lone ` or `` is fine.
                    if done { S::Done { ticks: 0 } } else { S::Prose { ticks: 0 } }
                } else if done { S::Done { ticks: 0 } } else { S::Prose { ticks: 0 } }
            }
            S::Word { matched } => {
                let w = WORD.as_bytes();
                if matched < w.len() {
                    if c as u32 == w[matched] as u32 { S::Word { matched: matched + 1 } } else { return false; }
                } else if c == ' ' { S::AfterWord }
                else if c == '\n' || c == '\r' { S::PreObject }
                else { return false; }
            }
            S::AfterWord => match c {
                // the attribute can only start here, right after the single space
                'n' if self.attr.is_some() => S::Attr { matched: 1 },
                '{' => { self.json.clear(); self.json.push('{'); S::Json { depth: 1, in_str: false, esc: false } }
                ' ' | '\n' | '\r' | '\t' => S::PreObject,
                _ => return false,
            },
            S::Attr { matched } => {
                let a = self.attr.as_deref().unwrap_or("");
                let want = a.chars().nth(matched);
                match want {
                    Some(w) if w == c => {
                        if matched + 1 == a.chars().count() { S::PreObject } else { S::Attr { matched: matched + 1 } }
                    }
                    _ => return false,
                }
            }
            S::PreObject => match c {
                '{' => { self.json.clear(); self.json.push('{'); S::Json { depth: 1, in_str: false, esc: false } }
                ' ' | '\n' | '\r' | '\t' => S::PreObject,
                _ => return false,
            },
            S::Json { depth, in_str, esc } => {
                self.json.push(c);
                if in_str {
                    if esc { S::Json { depth, in_str: true, esc: false } }
                    else if c == '\\' { S::Json { depth, in_str: true, esc: true } }
                    else if c == '"' {
                        // a string just closed: if it was the tool name, it must BE a whitelisted name,
                        // not merely a prefix of one ("read" alone is not "read_file").
                        if !self.names.is_empty() && !self.name_ok() { return false; }
                        S::Json { depth, in_str: false, esc: false }
                    } else {
                        // while the tool name is being written, reject the moment no allowed name has
                        // it as a prefix — this is what makes an invented name impossible rather than
                        // merely detectable afterwards.
                        if let Some(partial) = self.tool_name_partial() {
                            if !self.name_prefix_ok(partial) { return false; }
                        }
                        S::Json { depth, in_str: true, esc: false }
                    }
                } else {
                    match c {
                        '"' => S::Json { depth, in_str: true, esc: false },
                        '{' | '[' => S::Json { depth: depth + 1, in_str: false, esc: false },
                        '}' | ']' => {
                            if depth == 1 && c == '}' {
                                if !self.names.is_empty() && self.tool_name().is_none() { return false; }
                                S::PostObject { ticks: 0 }
                            } else if depth == 0 { return false; } else { S::Json { depth: depth - 1, in_str: false, esc: false } }
                        }
                        _ => S::Json { depth, in_str, esc: false },
                    }
                }
            }
            S::PostObject { ticks } => {
                if c == '`' { if ticks == 2 { S::Done { ticks: 0 } } else { S::PostObject { ticks: ticks + 1 } } }
                else if ticks == 0 && (c == '\n' || c == ' ' || c == '\r') { S::PostObject { ticks: 0 } }
                else { return false; }
            }
        };
        true
    }

    /// The tool-name text written so far, when the string currently open IS the `"tool"` value.
    /// `self.json` already includes the character just pushed.
    fn tool_name_partial(&self) -> Option<&str> {
        let i = self.json.rfind("\"tool\"")?;
        let rest = &self.json[i + 6..];
        let c = rest.find(':')?;
        let after = rest[c + 1..].trim_start();
        let after = after.strip_prefix('"')?;
        // the value string is still open only if no unescaped quote has closed it
        let mut esc = false;
        for (k, ch) in after.char_indices() {
            if esc { esc = false; continue; }
            match ch { '\\' => esc = true, '"' => return None, _ => { let _ = k; } }
        }
        Some(after)
    }

    /// The `"tool"` value in the object written so far, if it is complete.
    fn tool_name(&self) -> Option<String> {
        let i = self.json.find("\"tool\"")?;
        let rest = &self.json[i + 6..];
        let c = rest.find(':')?;
        let after = rest[c + 1..].trim_start();
        let after = after.strip_prefix('"')?;
        let end = after.find('"')?;
        Some(after[..end].to_string())
    }

    /// While the name string is being written, reject as soon as no whitelisted name has it as a
    /// prefix — the check that makes an invented tool name impossible rather than merely detectable.
    fn name_ok(&self) -> bool {
        match self.tool_name() {
            Some(n) => self.names.contains(&n),
            // the closed string was some other value (a path, contents): nothing to check
            None => true,
        }
    }

    /// While inside the name string, is `partial` still a prefix of something allowed?
    pub fn name_prefix_ok(&self, partial: &str) -> bool {
        self.names.is_empty() || self.names.iter().any(|n| n.starts_with(partial))
    }
}

/// Filter a sampler's candidate list: keep only the candidates whose text the acceptor would accept.
/// `text_of` maps a token id to its string. Returns the kept indices into `candidates`; empty means
/// the caller should keep its unconstrained choice and call `note_violation`.
pub fn filter_candidates<F: Fn(u32) -> String>(acc: &ToolFence, candidates: &[u32], text_of: F) -> Vec<usize> {
    candidates.iter().enumerate().filter(|(_, &t)| acc.would_accept(&text_of(t))).map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc() -> ToolFence { ToolFence::new(&["read_file", "write_file"]) }

    #[test]
    fn a_legal_call_walks_all_the_way_to_done() {
        let mut a = acc();
        assert!(a.push_str("Let me look.\n```tool\n{\"tool\": \"read_file\", \"path\": \"a.rs\"}\n```"));
        assert!(a.complete() && !a.violated());
    }

    #[test]
    fn prose_alone_is_legal_and_never_completes() {
        let mut a = acc();
        assert!(a.push_str("just an answer with a `backtick` and ``two``."));
        assert!(!a.complete());
    }

    #[test]
    fn an_invented_tool_name_is_refused_at_the_closing_quote() {
        let mut a = acc();
        assert!(!a.push_str("```tool\n{\"tool\": \"rm_rf\"}"), "a name outside the whitelist cannot be completed");
        // …and a prefix of a real one is still open while it is being written
        let b = acc();
        assert!(b.would_accept("```tool\n{\"tool\": \"read_f"));
        assert!(b.name_prefix_ok("read_f") && !b.name_prefix_ok("rm"));
    }

    #[test]
    fn the_fence_must_say_tool_and_open_an_object() {
        let mut a = acc();
        assert!(!a.push_str("```bash\n"), "only a tool fence is allowed");
        let mut b = acc();
        assert!(!b.push_str("```tool\nnot json"));
    }

    #[test]
    fn braces_and_fences_inside_strings_do_not_close_anything() {
        let mut a = acc();
        assert!(a.push_str("```tool\n{\"tool\": \"write_file\", \"contents\": \"fn main(){ } ``` \\\" done\"}\n```"));
        assert!(a.complete());
    }

    #[test]
    fn a_second_call_is_refused_when_single() {
        let mut a = acc();
        assert!(a.push_str("```tool\n{\"tool\": \"read_file\"}\n```"));
        assert!(a.push_str(" and then"));
        assert!(!a.push_str("```tool\n{\"tool\": \"read_file\"}\n```"));
        let mut b = acc(); b.single_call = false;
        assert!(b.push_str("```tool\n{\"tool\": \"read_file\"}\n```\n```tool\n{\"tool\": \"write_file\"}\n```"));
    }

    /// The gate and the grammar used to contradict each other: the prompt teaches
    /// ```tool nonce="…" and the gate refuses a fence without it, while this acceptor allowed only
    /// whitespace between `tool` and `{`. With a token configured the exact attribute is legal —
    /// and nothing else is: a wrong token is refused at its first wrong character, and the
    /// attribute cannot start anywhere but after the single space.
    #[test]
    fn the_run_token_attribute_is_accepted_exactly_when_configured() {
        let mut a = acc().with_nonce("a25dbeef");
        assert!(a.push_str("```tool nonce=\"a25dbeef\"\n{\"tool\": \"read_file\", \"path\": \"a.rs\"}\n```"));
        assert!(a.complete() && !a.violated());
        // the one-line form, object right after the attribute
        let mut b = acc().with_nonce("a25dbeef");
        assert!(b.push_str("```tool nonce=\"a25dbeef\" {\"tool\": \"read_file\"}\n```"));
        assert!(b.complete());
        // a wrong token dies at the first wrong character, not at the end
        let c = acc().with_nonce("a25dbeef");
        assert!(c.would_accept("```tool nonce=\"a25d"));
        assert!(!c.would_accept("```tool nonce=\"a25X"));
        assert!(!c.would_accept("```tool nonce=\"a25dbeef\"x"), "nothing but whitespace or the object may follow it");
        // and the plain form stays legal: the attribute is optional (forcing it would stamp the token
        // onto a quoted fence, which is what the token exists to distinguish)
        let mut d = acc().with_nonce("a25dbeef");
        assert!(d.push_str("```tool\n{\"tool\": \"read_file\"}\n```"));
        assert!(d.complete());
    }

    #[test]
    fn without_a_token_the_attribute_is_still_refused() {
        let a = acc();
        assert!(!a.would_accept("```tool nonce=\"a25dbeef\"\n{"));
        assert!(a.would_accept("```tool \n{"), "a space then newline was always fine");
        assert_eq!(acc().with_nonce("ff00").nonce(), Some("ff00"));
    }

    #[test]
    fn candidate_filtering_keeps_only_legal_continuations() {
        let mut a = acc();
        a.push_str("```tool\n{\"tool\": \"");
        // token ids 0..4 with these strings
        let text = |t: u32| ["read", "rm", "write", "\"", "_file"][t as usize].to_string();
        let keep = filter_candidates(&a, &[0, 1, 2, 3, 4], text);
        // "read" and "write" are prefixes of allowed names; "rm" is not; a bare closing quote would
        // finish an empty name, which is not whitelisted
        assert_eq!(keep, vec![0, 2], "kept: {keep:?}");
    }

    #[test]
    fn no_whitelist_means_any_name() {
        let mut a = ToolFence::new(&[]);
        assert!(a.push_str("```tool\n{\"tool\": \"anything\"}\n```"));
        assert!(a.complete());
    }
}
