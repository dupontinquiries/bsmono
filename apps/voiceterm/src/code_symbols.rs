//! code_symbols: the identifier dictionary that makes code dictation
//! bearable, plus the whisper prompt derived from it.
//!
//! Two problems, one table.
//!
//! **Speaking identifiers.** Without this, every name needs an explicit
//! casing directive — you say "snake read to string" to get
//! `read_to_string`. That is too much ceremony to sustain. But the file
//! in front of you already contains the identifiers you are about to
//! say. Index them by their SPOKEN form (`read_to_string` →
//! "read to string", `UserStore` → "user store") and a bare run of words
//! resolves to the real spelling, casing and all.
//!
//! **Hearing identifiers.** voice.rs sets one global whisper
//! `initial_prompt`, and it is a list of shell commands ("run cargo
//! test", "scroll down", "open github"). While dictating code that
//! prompt is actively harmful — it anchors the decoder on a command
//! vocabulary. `prompt()` here produces the replacement: the spoken
//! forms of the identifiers actually present in the buffer. Spoken
//! forms, not literal spellings, because the prompt biases what whisper
//! EMITS, and whisper will never emit `read_to_string`.
//!
//! So the same extraction pass improves both the recognition and the
//! rendering, which is the reason these live in one module.
//!
//! ## Cost
//!
//! Measured (x86_64, release, this sandbox — ARM will be slower but the
//! shape holds). Full rebuild from source text:
//!
//! ```text
//!   8000 lines, realistic name reuse   (15 forms)    1.9 ms
//!   8000 lines, every name unique   (32005 forms)   28.5 ms
//!  32000 lines, every name unique  (128005 forms)  164.0 ms
//!   resolve_longest                                  0.08 us
//!   prompt(600)                                      0.001 ms
//! ```
//!
//! Against whisper's ~1200 ms a full rebuild per utterance is noise
//! outside the pathological case, so there is no incremental-diff
//! machinery here — it would be complexity bought with a rounding
//! error. `merge_source` exists for the one case that IS worth it:
//! during dictation the buffer grows by the text just inserted, so
//! merging that text costs microseconds instead of rescanning the file.
//!
//! PURE: std only. No IO — the caller supplies the source text.

use std::collections::HashMap;

/// An identifier as it is actually spelled, with how often it occurs in
/// the source. Frequency is the tie-break when two spellings share a
/// spoken form (`read_to_string` vs `readToString`): the one this file
/// uses more is the one you meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub text: String,
    pub count: usize,
}

/// Spoken-form → candidate spellings, built from a buffer.
#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    by_spoken: HashMap<String, Vec<Ident>>,
}

/// Keywords excluded from the whisper prompt. They still resolve (as
/// no-ops), but spending prompt budget on "let" and "fn" is waste — the
/// budget should go to the names whisper is likely to mangle.
const KEYWORDS: &[&str] = &[
    "let", "mut", "fn", "pub", "use", "mod", "impl", "self", "super", "crate", "struct", "enum",
    "trait", "type", "const", "static", "match", "if", "else", "for", "while", "loop", "return",
    "break", "continue", "true", "false", "none", "some", "ok", "err", "where", "as", "in", "ref",
    "def", "class", "import", "from", "var", "function", "const", "public", "private", "static",
    "void", "int", "new", "this", "null", "async", "await", "try", "catch", "throw",
];

impl SymbolTable {
    /// Build from buffer text. Extraction is deliberately crude — a
    /// regex-shaped scan for identifier characters, no parsing. A
    /// syntax-aware pass would be more precise, but this runs on every
    /// keystroke-adjacent refresh and precision is not what it buys:
    /// a spurious entry costs one wrong resolution that the user
    /// immediately sees and corrects, while a parser costs a dependency
    /// per language.
    pub fn from_source(src: &str) -> Self {
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut cur = String::new();
        for ch in src.chars() {
            if ch.is_alphanumeric() || ch == '_' {
                cur.push(ch);
            } else if !cur.is_empty() {
                bump(&mut counts, &cur);
                cur.clear();
            }
        }
        if !cur.is_empty() {
            bump(&mut counts, &cur);
        }

        let mut by_spoken: HashMap<String, Vec<Ident>> = HashMap::new();
        for (text, count) in counts {
            let spoken = spoken_form(&text).join(" ");
            if spoken.is_empty() {
                continue;
            }
            by_spoken.entry(spoken).or_default().push(Ident { text, count });
        }
        // Deterministic order: most frequent first, then lexicographic.
        // Without the second key the winner would depend on HashMap
        // iteration order, which is randomized per process — the kind of
        // bug that reproduces once a week.
        for v in by_spoken.values_mut() {
            v.sort_by(|a, b| b.count.cmp(&a.count).then(a.text.cmp(&b.text)));
        }
        SymbolTable { by_spoken }
    }

    /// Fold additional source text into the table.
    ///
    /// For the dictation loop: after inserting an utterance, merge just
    /// that text rather than rescanning the buffer. Counts accumulate,
    /// so a name the user keeps dictating climbs the frequency ranking
    /// and starts winning collisions — which is the behaviour you want.
    pub fn merge_source(&mut self, src: &str) {
        let add = SymbolTable::from_source(src);
        for (spoken, idents) in add.by_spoken {
            let slot = self.by_spoken.entry(spoken).or_default();
            for id in idents {
                match slot.iter_mut().find(|e| e.text == id.text) {
                    Some(e) => e.count += id.count,
                    None => slot.push(id),
                }
            }
            slot.sort_by(|a, b| b.count.cmp(&a.count).then(a.text.cmp(&b.text)));
        }
    }

    /// Merge only tokens that LOOK like identifiers: containing an
    /// underscore, an internal capital, or a digit.
    ///
    /// This is what the dictation loop should use on its own output.
    /// Plain lowercase English words are not identifiers, add nothing
    /// (single words are no longer resolvable), and actively harm:
    /// dictating "match state {" put a lowercase `state` in the table,
    /// where it then beat the buffer's `State` in path position and
    /// produced `state::Idle`. They also waste whisper-prompt budget.
    pub fn merge_identifiers(&mut self, src: &str) {
        let kept: String = src
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|t| looks_like_identifier(t))
            .collect::<Vec<_>>()
            .join(" ");
        if !kept.is_empty() {
            self.merge_source(&kept);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_spoken.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_spoken.len()
    }

    /// Resolve a run of spoken words to a real identifier.
    ///
    /// **Multi-word runs only.** "user store" can only plausibly mean
    /// the `UserStore` in the buffer, so that rewrite is safe.
    ///
    /// Single words are NOT resolved, and that is a reversal. The
    /// original rule allowed capitalization recovery — "vec" -> `Vec` —
    /// restricted to cases where no candidate was spelled exactly as
    /// spoken. A fixture run over realistic dictation showed why that is
    /// not enough: a buffer containing `fn read(p: &Path)` has `Path`
    /// and not `path`, so saying "path" produced `Path`. Same for
    /// "match state" against a buffer declaring `enum State`. Having the
    /// TYPE in scope without the lowercase variable is the normal case
    /// in typed languages, so the rule fired exactly where it was most
    /// wrong.
    ///
    /// `Vec` is still reachable with "pascal vec", which is two words
    /// and explicit. The convenience was not worth corrupting the
    /// commonest identifiers in the file.
    pub fn resolve(&self, words: &[&str]) -> Option<&str> {
        if words.len() < 2 {
            return None;
        }
        let key = words.join(" ").to_lowercase();
        Some(&self.by_spoken.get(&key)?.first()?.text)
    }

    /// Resolve a SINGLE word, but only in path position — immediately
    /// before or after a `::`.
    ///
    /// The general single-word rule was removed because a buffer holding
    /// `Path` but not `path` made every spoken "path" wrong. Path
    /// position is different: `x::y` is unambiguously naming code, not
    /// prose, so "state double colon idle" against a buffer declaring
    /// `enum State { Idle }` should give `State::Idle`.
    ///
    /// An exact-spelling candidate still wins, so `std::fs` stays
    /// lowercase.
    pub fn resolve_path_segment(&self, word: &str) -> Option<&str> {
        let w = word.to_lowercase();
        let cands = self.by_spoken.get(&w)?;
        if let Some(exact) = cands.iter().find(|c| c.text == word) {
            return Some(&exact.text);
        }
        Some(&cands.first()?.text)
    }

    /// Longest run of `words` starting at 0 that resolves, with the
    /// spelling. Returns (run_length, identifier).
    pub fn resolve_longest(&self, words: &[&str], max_span: usize) -> Option<(usize, &str)> {
        let upper = max_span.min(words.len());
        for len in (2..=upper).rev() {
            if let Some(id) = self.resolve(&words[..len]) {
                return Some((len, id));
            }
        }
        None
    }

    /// A whisper `initial_prompt` for dictation into this buffer:
    /// comma-separated SPOKEN forms of the most frequent identifiers,
    /// truncated to `budget` chars.
    ///
    /// `budget` is in characters, not tokens, on purpose — whisper's
    /// prompt limit is n_text_ctx/2 tokens (224 for the standard
    /// models) and there is no tokenizer on this side of the FFI.
    /// ~600 chars is a safe stand-in for 224 tokens of English; be
    /// conservative, because whisper silently truncates from the FRONT
    /// and you would lose your most frequent symbols.
    ///
    /// Imperfection worth knowing: identifiers containing digits render
    /// their digits as digits, so `u64` becomes "u 64" while whisper
    /// would actually emit "u sixty four". Harmless — the prompt is a
    /// bias, not a constraint — and fixing it needs a digits-to-words
    /// table that does not otherwise earn its place.
    pub fn prompt(&self, budget: usize) -> String {
        let mut entries: Vec<(&str, usize)> = self
            .by_spoken
            .iter()
            .filter(|(spoken, _)| !KEYWORDS.contains(&spoken.as_str()))
            // single-word entries carry almost no recognition value;
            // whisper gets ordinary words right already.
            .filter(|(spoken, _)| spoken.contains(' '))
            .map(|(spoken, ids)| (spoken.as_str(), ids.iter().map(|i| i.count).sum()))
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));

        let mut out = String::new();
        for (spoken, _) in entries {
            let add = if out.is_empty() { spoken.len() } else { spoken.len() + 2 };
            if out.len() + add > budget {
                continue;
            }
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push_str(spoken);
        }
        out
    }
}

/// Which `initial_prompt` family the currently-active voice grammar
/// wants. The decision, separated from the prompt text and from the
/// FFI, so it can be tested without whisper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    /// The existing command-vocabulary prompt in voice.rs. Correct when
    /// the user is issuing commands.
    Commands,
    /// Buffer-derived spoken forms. Correct when the user is dictating
    /// INTO something, where the command prompt actively misleads the
    /// decoder.
    Dictation(String),
    /// Set no prompt at all. Non-English locales get this because the
    /// English command prompt anchors whisper on the wrong language —
    /// voice.rs already makes that call, preserved here.
    None,
}

/// Choose the prompt for `grammar_label` (as produced by
/// `grammar_context::grammar_label`) in language `lang`.
///
/// The bug this fixes: voice.rs sets ONE global prompt, full of shell
/// commands, regardless of what the user is doing. Dictating
/// `impl Iterator for` while primed on "run cargo test" is fighting the
/// decoder. `grammar_label` already knows which grammar is live and is
/// derived from the same detectors the intent dispatcher uses, so the
/// prompt can never disagree with what a command would actually do.
///
/// Falls back to `Commands` when dictating with no usable table — an
/// empty prompt is worse than a mildly wrong one, since whisper's
/// default behaviour without a prompt is to punctuate and capitalize
/// like prose.
pub fn prompt_for(
    grammar_label: &str,
    lang: &str,
    syms: Option<&SymbolTable>,
    budget: usize,
) -> PromptKind {
    if lang != "en" {
        return PromptKind::None;
    }
    let dictating = grammar_label == "dictation" || grammar_label.starts_with("dictation:");
    if !dictating {
        return PromptKind::Commands;
    }
    match syms {
        Some(t) => {
            let p = t.prompt(budget);
            if p.is_empty() {
                PromptKind::Commands
            } else {
                PromptKind::Dictation(p)
            }
        }
        None => PromptKind::Commands,
    }
}

/// Underscore, an internal capital, or a digit. Any of the three means
/// someone chose this spelling deliberately.
fn looks_like_identifier(t: &str) -> bool {
    if t.chars().count() < 2 {
        return false;
    }
    t.contains('_')
        || t.chars().any(|c| c.is_ascii_digit())
        || t.chars().skip(1).any(|c| c.is_uppercase())
        || (t.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
            && t.chars().skip(1).all(|c| c.is_lowercase()))
}

fn bump(counts: &mut HashMap<String, usize>, tok: &str) {
    // Pure numbers are not identifiers. Single chars carry no spoken
    // form worth indexing (and `i`/`x` would collide with real words).
    if tok.chars().all(|c| c.is_ascii_digit()) || tok.chars().count() < 2 {
        return;
    }
    *counts.entry(tok.to_string()).or_insert(0) += 1;
}

/// Split an identifier into the words a person would say for it.
///
/// Handles snake_case, kebab-case, camelCase, PascalCase, and acronym
/// runs (`HTTPServer` → http, server — the last capital of a run starts
/// the next word, not ends the current one). Digit runs split off as
/// their own token (`utf8Decode` → utf, 8, decode).
pub fn spoken_form(ident: &str) -> Vec<String> {
    let chars: Vec<char> = ident.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, words: &mut Vec<String>| {
        if !cur.is_empty() {
            words.push(cur.to_lowercase());
            cur.clear();
        }
    };
    for i in 0..chars.len() {
        let c = chars[i];
        if c == '_' || c == '-' {
            flush(&mut cur, &mut words);
            continue;
        }
        let prev = if i > 0 { Some(chars[i - 1]) } else { None };
        let next = chars.get(i + 1).copied();
        let boundary = match prev {
            None => false,
            Some(p) => {
                // lower/digit → upper: camelCase boundary
                (!p.is_uppercase() && c.is_uppercase())
                    // UPPER → Upper+lower: end of an acronym run
                    || (p.is_uppercase()
                        && c.is_uppercase()
                        && next.map(|n| n.is_lowercase()).unwrap_or(false))
                    // alpha ↔ digit either way
                    || (p.is_ascii_digit() != c.is_ascii_digit())
            }
        };
        if boundary {
            flush(&mut cur, &mut words);
        }
        cur.push(c);
    }
    flush(&mut cur, &mut words);
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sf(s: &str) -> Vec<String> {
        spoken_form(s)
    }

    #[test]
    fn spoken_form_splits_every_convention() {
        assert_eq!(sf("read_to_string"), ["read", "to", "string"]);
        assert_eq!(sf("UserStore"), ["user", "store"]);
        assert_eq!(sf("maxRetries"), ["max", "retries"]);
        assert_eq!(sf("kebab-case-name"), ["kebab", "case", "name"]);
        assert_eq!(sf("MAX_SIZE"), ["max", "size"]);
        assert_eq!(sf("plain"), ["plain"]);
    }

    #[test]
    fn spoken_form_handles_acronym_runs() {
        assert_eq!(sf("HTTPServer"), ["http", "server"]);
        assert_eq!(sf("parseHTTP"), ["parse", "http"]);
        assert_eq!(sf("utf8Decode"), ["utf", "8", "decode"]);
    }

    #[test]
    fn multi_word_run_resolves_to_real_spelling() {
        let t = SymbolTable::from_source("fn read_to_string(p: &Path) { let s = UserStore::new(); }");
        assert_eq!(t.resolve(&["read", "to", "string"]), Some("read_to_string"));
        assert_eq!(t.resolve(&["user", "store"]), Some("UserStore"));
        assert_eq!(t.resolve(&["not", "in", "file"]), None);
    }

    #[test]
    fn single_words_are_never_resolved() {
        // The reversal: a buffer holding the TYPE but not the lowercase
        // variable is the normal case in typed code, so single-word
        // capitalization recovery fired exactly where it was worst.
        let t = SymbolTable::from_source("fn read(p: &Path) {} enum State { Idle }");
        assert_eq!(t.resolve(&["path"]), None, "would have produced Path");
        assert_eq!(t.resolve(&["state"]), None, "would have produced State");
        let v = SymbolTable::from_source("let v: Vec<u8> = Vec::new();");
        assert_eq!(v.resolve(&["vec"]), None, "reachable as \"pascal vec\" instead");
    }

    #[test]
    fn frequency_breaks_spoken_form_collisions() {
        let src = "readToString readToString readToString read_to_string";
        let t = SymbolTable::from_source(src);
        assert_eq!(t.resolve(&["read", "to", "string"]), Some("readToString"));
    }

    #[test]
    fn collision_tiebreak_is_deterministic() {
        // equal counts → lexicographic, so the answer never depends on
        // HashMap iteration order
        let src = "readToString read_to_string";
        let first = SymbolTable::from_source(src).resolve(&["read", "to", "string"]).unwrap().to_string();
        for _ in 0..50 {
            assert_eq!(
                SymbolTable::from_source(src).resolve(&["read", "to", "string"]).unwrap(),
                first
            );
        }
    }

    #[test]
    fn resolve_longest_prefers_the_longer_name() {
        let t = SymbolTable::from_source("read_to_string read_to");
        let (n, id) = t.resolve_longest(&["read", "to", "string", "extra"], 4).unwrap();
        assert_eq!((n, id), (3, "read_to_string"));
    }

    #[test]
    fn numbers_and_single_chars_are_not_indexed() {
        let t = SymbolTable::from_source("let x = 42; let i = 0;");
        assert!(t.resolve(&["42"]).is_none());
        assert!(t.resolve(&["x"]).is_none());
    }

    #[test]
    fn resolve_longest_will_not_fall_back_to_one_word() {
        let t = SymbolTable::from_source("read_to_string path");
        assert_eq!(t.resolve_longest(&["path"], 5), None);
        assert_eq!(
            t.resolve_longest(&["read", "to", "string"], 5),
            Some((3, "read_to_string"))
        );
    }

    #[test]
    fn prompt_is_spoken_forms_not_spellings() {
        let t = SymbolTable::from_source("fn read_to_string() { UserStore::new(); }");
        let p = t.prompt(600);
        assert!(p.contains("read to string"), "{p}");
        assert!(p.contains("user store"), "{p}");
        assert!(!p.contains("read_to_string"), "whisper will never emit that: {p}");
    }

    #[test]
    fn prompt_respects_budget_and_drops_keywords() {
        let src = "let mut return_value = 1; fn compute_average_score() {} fn tiny_fn() {}";
        let t = SymbolTable::from_source(src);
        assert!(t.prompt(600).len() <= 600);
        let tight = t.prompt(20);
        assert!(tight.len() <= 20, "got {} chars: {tight}", tight.len());
        // bare keywords never spend budget
        assert!(!t.prompt(600).split(", ").any(|e| e == "let" || e == "fn"));
    }

    #[test]
    fn prompt_orders_by_frequency() {
        let src = "hot_path hot_path hot_path cold_path";
        let p = SymbolTable::from_source(src).prompt(600);
        assert!(p.starts_with("hot path"), "{p}");
    }

    #[test]
    fn prompt_for_keeps_commands_outside_dictation() {
        let t = SymbolTable::from_source("fn read_to_string() {}");
        assert_eq!(prompt_for("editor:helix", "en", Some(&t), 600), PromptKind::Commands);
        assert_eq!(prompt_for("terminal", "en", Some(&t), 600), PromptKind::Commands);
    }

    #[test]
    fn prompt_for_swaps_in_buffer_symbols_when_dictating() {
        let t = SymbolTable::from_source("fn read_to_string() { UserStore::new(); }");
        match prompt_for("dictation:rust", "en", Some(&t), 600) {
            PromptKind::Dictation(p) => {
                assert!(p.contains("read to string"), "{p}");
                assert!(!p.contains("cargo"), "{p}");
            }
            other => panic!("expected Dictation, got {other:?}"),
        }
    }

    #[test]
    fn prompt_for_falls_back_rather_than_going_empty() {
        assert_eq!(prompt_for("dictation", "en", None, 600), PromptKind::Commands);
        let empty = SymbolTable::from_source("");
        assert_eq!(prompt_for("dictation", "en", Some(&empty), 600), PromptKind::Commands);
    }

    #[test]
    fn prompt_for_stays_out_of_the_way_in_other_languages() {
        let t = SymbolTable::from_source("fn read_to_string() {}");
        assert_eq!(prompt_for("dictation", "fr", Some(&t), 600), PromptKind::None);
        assert_eq!(prompt_for("terminal", "de", Some(&t), 600), PromptKind::None);
    }

    #[test]
    fn merge_adds_new_names_and_accumulates_counts() {
        let mut t = SymbolTable::from_source("read_to_string");
        assert_eq!(t.resolve(&["user", "store"]), None);
        t.merge_source("UserStore UserStore");
        assert_eq!(t.resolve(&["user", "store"]), Some("UserStore"));
        assert_eq!(t.resolve(&["read", "to", "string"]), Some("read_to_string"));
    }

    #[test]
    fn merge_can_flip_a_collision_winner() {
        let mut t = SymbolTable::from_source("read_to_string read_to_string");
        assert_eq!(t.resolve(&["read", "to", "string"]), Some("read_to_string"));
        // the user keeps dictating the camelCase spelling; it takes over
        t.merge_source("readToString readToString readToString");
        assert_eq!(t.resolve(&["read", "to", "string"]), Some("readToString"));
    }

    #[test]
    fn merge_into_empty_matches_a_fresh_build() {
        let src = "fn compute_average_score(userStore: &UserStore) {}";
        let mut merged = SymbolTable::default();
        merged.merge_source(src);
        let fresh = SymbolTable::from_source(src);
        assert_eq!(merged.len(), fresh.len());
        assert_eq!(merged.prompt(600), fresh.prompt(600));
    }

    #[test]
    fn merge_identifiers_ignores_plain_words() {
        let mut t = SymbolTable::from_source("enum State { Idle }");
        // the engine's own output, fed back in
        t.merge_identifiers("match state {");
        // the lowercase "state" must not have been learned, or it would
        // beat State in path position
        assert_eq!(t.resolve_path_segment("state"), Some("State"));
        // but a real identifier in the output IS learned
        t.merge_identifiers("let max_retries = 3;");
        assert_eq!(t.resolve(&["max", "retries"]), Some("max_retries"));
    }

    #[test]
    fn identifier_shape_test() {
        assert!(looks_like_identifier("max_retries"));
        assert!(looks_like_identifier("userStore"));
        assert!(looks_like_identifier("UserStore"));
        assert!(looks_like_identifier("State"));
        assert!(looks_like_identifier("f64"));
        assert!(!looks_like_identifier("match"));
        assert!(!looks_like_identifier("the"));
        assert!(!looks_like_identifier("x"));
        assert!(!looks_like_identifier(""));
    }

    #[test]
    fn empty_source_is_harmless() {
        let t = SymbolTable::from_source("");
        assert!(t.is_empty());
        assert_eq!(t.prompt(600), "");
        assert_eq!(t.resolve(&["anything"]), None);
    }
}
