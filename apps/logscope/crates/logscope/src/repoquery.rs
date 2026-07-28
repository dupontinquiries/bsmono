//! repoquery.rs — a small model fetching repository context for a large one, safely.
//!
//! The Liaison is well placed to answer "what changed here recently, and why" before the Thinker sees a
//! question: it is cheap, it is already running, and the answer is mechanical. But letting a model
//! produce the command that gets run is how a shell injection happens without anyone writing an exploit
//! — the model does not need to be adversarial, only wrong, and `git log --format=$(rm -rf ~)` is a
//! plausible hallucination.
//!
//! So the protocol is a fixed set of QUERY SHAPES. The model chooses a variant and fills typed fields;
//! it never writes a command. Every shape maps to a fixed argv with no shell, and arguments are
//! validated before they reach it.
//!
//! ## The second failure, which matters more
//!
//! A small model choosing what context the large one sees is choosing what it can conclude. If the
//! Liaison fetches three commits and the Thinker treats them as the complete history, it will answer
//! confidently about a repository it has seen a slice of. Every result therefore carries how it was
//! obtained and whether it was truncated, and `Evidence::caveat` says so in the prompt rather than in a
//! comment nobody reads.

/// A structured request. The model picks a variant; it does not write a command.
#[derive(Clone, PartialEq, Debug)]
pub enum Query {
    /// Recent commits touching a path.
    CommitsTouching { path: String, limit: usize },
    /// Commits whose subject mentions a term.
    CommitsMatching { term: String, limit: usize },
    /// One commit's message and stat summary. Never the full diff — that is unbounded.
    ShowCommit { sha: String },
    /// Which commit last changed a given line range.
    BlameRange { path: String, start: usize, end: usize },
}

/// Why a query was refused.
#[derive(Clone, PartialEq, Debug)]
pub enum Refusal { BadPath(String), BadSha(String), BadRange(String), EmptyTerm }

impl Refusal {
    pub fn explain(&self) -> String {
        match self {
            Refusal::BadPath(p) => format!("path {p:?} escapes the repository or contains a shell \
metacharacter — refused before argv, not sanitised into it"),
            Refusal::BadSha(s) => format!("{s:?} is not a hex object id"),
            Refusal::BadRange(r) => format!("line range {r} is empty or inverted"),
            Refusal::EmptyTerm => "an empty search term would match everything".into(),
        }
    }
}

fn path_ok(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('-')                      // would be read as a flag
        && !p.starts_with('/')                      // absolute: outside the repo
        && !p.split('/').any(|c| c == "..")         // traversal
        && p.chars().all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
}

fn sha_ok(s: &str) -> bool {
    (4..=40).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

impl Query {
    /// The exact argv to run. No shell, ever.
    ///
    /// Returning argv rather than a string is the whole safety argument: there is no parsing step where
    /// a quote or a semicolon could change the structure. `--` separates flags from paths so a filename
    /// can never be read as an option.
    pub fn argv(&self) -> Result<Vec<String>, Refusal> {
        let s = |x: &str| x.to_string();
        match self {
            Query::CommitsTouching { path, limit } => {
                if !path_ok(path) { return Err(Refusal::BadPath(path.clone())); }
                Ok(vec![s("git"), s("log"), format!("-{}", limit.clamp(&1, &50)),
                        s("--format=%h %ad %s"), s("--date=short"), s("--"), path.clone()])
            }
            Query::CommitsMatching { term, limit } => {
                if term.trim().is_empty() { return Err(Refusal::EmptyTerm); }
                Ok(vec![s("git"), s("log"), format!("-{}", limit.clamp(&1, &50)),
                        s("--format=%h %ad %s"), s("--date=short"),
                        s("--grep"), term.clone(), s("--fixed-strings")])
            }
            Query::ShowCommit { sha } => {
                if !sha_ok(sha) { return Err(Refusal::BadSha(sha.clone())); }
                // --stat, never a full diff: an unbounded patch would blow the Thinker's context and the
                // useful part is almost always the message plus which files moved.
                Ok(vec![s("git"), s("show"), s("--stat"), s("--format=%h %an %ad%n%n%B"),
                        s("--date=short"), sha.clone()])
            }
            Query::BlameRange { path, start, end } => {
                if !path_ok(path) { return Err(Refusal::BadPath(path.clone())); }
                if start == &0 || end < start { return Err(Refusal::BadRange(format!("{start}..{end}"))); }
                Ok(vec![s("git"), s("blame"), s("-L"), format!("{start},{end}"),
                        s("--porcelain"), s("--"), path.clone()])
            }
        }
    }

    /// A one-line description for the log, so an operator can see what the small model asked for.
    pub fn describe(&self) -> String {
        match self {
            Query::CommitsTouching { path, limit } => format!("last {limit} commits touching {path}"),
            Query::CommitsMatching { term, limit } => format!("last {limit} commits mentioning {term:?}"),
            Query::ShowCommit { sha } => format!("commit {sha}"),
            Query::BlameRange { path, start, end } => format!("blame {path}:{start}-{end}"),
        }
    }
}

/// A result, with how it was obtained.
#[derive(Clone, PartialEq, Debug)]
pub struct Evidence {
    pub query: String,
    pub body: String,
    /// True when the output was cut short.
    pub truncated: bool,
}

/// Longest body a single piece of evidence may contribute.
pub const MAX_BODY_CHARS: usize = 4000;

impl Evidence {
    pub fn new(query: &Query, raw: &str) -> Self {
        let truncated = raw.len() > MAX_BODY_CHARS;
        let body = if truncated { raw.chars().take(MAX_BODY_CHARS).collect() } else { raw.to_string() };
        Evidence { query: query.describe(), body, truncated }
    }

    /// The caveat the Thinker must see alongside the body.
    ///
    /// A small model choosing what the large one sees is choosing what it can conclude. Without this,
    /// three fetched commits read as the complete history and the Thinker answers confidently about a
    /// repository it has seen a slice of.
    pub fn caveat(&self) -> String {
        let base = format!("This is the result of `{}`, selected by a smaller model. It is a SLICE of \
the repository, not its history", self.query);
        if self.truncated { format!("{base}, and it was TRUNCATED at {MAX_BODY_CHARS} characters.") }
        else { format!("{base}.") }
    }

    /// The block to paste into the Thinker's prompt, framed as tool status.
    pub fn to_prompt_block(&self) -> String {
        format!("{}\n{}\n\n{}", crate::promptframe::Frame::ToolStatus.banner(), self.caveat(), self.body)
    }
}

/// The instruction given to the fetching model.
pub const FETCH_PROTOCOL: &str = "\
You may request repository context before the main model answers. Reply with ONE line, exactly:

    TOUCHING <path> <n>      recent commits touching a file
    MATCHING <term> <n>      recent commits whose message mentions a term
    SHOW <sha>               one commit's message and file stats
    BLAME <path> <a> <b>     which commit last changed lines a..b
    NONE                     no repository context would help

Do not write shell commands; they will not be run. Ask for NONE when the question is not about this \
repository's history — fetching something irrelevant costs the main model context it needs for the \
actual question.";

/// Parse the fetching model's one-line reply.
pub fn parse_request(reply: &str) -> Option<Query> {
    let line = reply.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut it = line.split_whitespace();
    let verb = it.next()?.to_ascii_uppercase();
    match verb.as_str() {
        "TOUCHING" => {
            let path = it.next()?.to_string();
            let limit = it.next().and_then(|v| v.parse().ok()).unwrap_or(10);
            Some(Query::CommitsTouching { path, limit })
        }
        "MATCHING" => {
            let term = it.next()?.to_string();
            let limit = it.next().and_then(|v| v.parse().ok()).unwrap_or(10);
            Some(Query::CommitsMatching { term, limit })
        }
        "SHOW" => Some(Query::ShowCommit { sha: it.next()?.to_string() }),
        "BLAME" => {
            let path = it.next()?.to_string();
            let (a, b) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?);
            Some(Query::BlameRange { path, start: a, end: b })
        }
        _ => None,   // includes NONE
    }
}

// ── Two-step negotiation ────────────────────────────────────────────────────────────────────────
//
// The Thinker asks for context; the Liaison runs the fetch and sees the result FIRST, then offers a
// PREVIEW — a truncated body plus how much was withheld. The Thinker answers "enough" or "send it all",
// and only the second case spends its context on the full text.
//
// The obvious failure is a preview that hides the useful part, so the Thinker says "enough" and answers
// from a slice it actively declined to complete. That is worse than no preview, because the Thinker has
// now made a decision it believes was informed. Two things guard it: the preview always states how many
// characters and lines were withheld, and `Preview::looks_sufficient` is advisory only — the Thinker
// decides, never the Liaison.
//
// The subtler failure is the Thinker always saying "send it all", which makes the preview pure overhead
// — a round trip and a small model's forward pass to learn nothing. `Negotiation::preview_is_paying`
// tracks the accept rate so a caller can stop previewing when it stops helping.

/// Where a context request has got to.
#[derive(Clone, PartialEq, Debug)]
pub enum Stage {
    /// The Thinker asked; nothing fetched yet.
    Requested(Query),
    /// Fetched, and a preview offered to the Thinker.
    Previewed { evidence: Evidence, preview: Preview },
    /// The Thinker accepted the preview. The full body was never sent.
    SettledOnPreview,
    /// The Thinker wanted everything, and got it.
    DeliveredFull,
    /// The fetch failed or was refused; the Thinker is told so rather than left waiting.
    Failed(String),
}

/// The cheap first offer.
#[derive(Clone, PartialEq, Debug)]
pub struct Preview {
    pub head: String,
    pub withheld_chars: usize,
    pub withheld_lines: usize,
}

/// Characters shown in a preview before the rest is withheld.
pub const PREVIEW_CHARS: usize = 600;

impl Preview {
    pub fn of(body: &str) -> Self {
        if body.len() <= PREVIEW_CHARS {
            return Preview { head: body.to_string(), withheld_chars: 0, withheld_lines: 0 };
        }
        // Cut on a line boundary: half a commit subject reads as a complete one and invites the Thinker
        // to accept a preview it has misread.
        let cut = body[..PREVIEW_CHARS].rfind('\n').unwrap_or(PREVIEW_CHARS);
        let head = body[..cut].to_string();
        Preview { head, withheld_chars: body.len() - cut,
                  withheld_lines: body[cut..].lines().count().saturating_sub(1) }
    }

    pub fn is_complete(&self) -> bool { self.withheld_chars == 0 }

    /// The message put to the Thinker. States the withholding in the same breath as the content.
    pub fn ask(&self) -> String {
        if self.is_complete() {
            return format!("{}\n\n(That is the complete result.)", self.head);
        }
        format!("{}\n\n[{} more lines / {} characters withheld]\n\nIs this enough to answer, or should \
I send the rest? Reply ENOUGH or SEND-ALL. Answering ENOUGH means you will not see the withheld part.",
            self.head, self.withheld_lines, self.withheld_chars)
    }

    /// Advisory only: a hint that the withheld part is probably immaterial.
    ///
    /// Never used to decide — the Thinker decides. A Liaison that could settle this on its own would be
    /// choosing what the Thinker knows, which is the thing this whole module exists to avoid.
    pub fn looks_sufficient(&self) -> bool { self.withheld_lines <= 2 }
}

/// The Thinker's answer to a preview.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict { Enough, SendAll }

pub fn parse_verdict(reply: &str) -> Option<Verdict> {
    let up = reply.trim().to_ascii_uppercase();
    // Checked in this order because a reply containing both ("not enough, send all") must resolve to
    // SEND-ALL — the expensive, safe direction. Preferring ENOUGH there would silently drop context the
    // Thinker asked for.
    if up.contains("SEND-ALL") || up.contains("SEND ALL") { return Some(Verdict::SendAll); }
    if up.contains("ENOUGH") { return Some(Verdict::Enough); }
    None
}

/// Tracks a request through the two steps, and whether previewing is worth continuing.
#[derive(Clone, Debug, Default)]
pub struct Negotiation { accepted: u32, escalated: u32 }

impl Negotiation {
    pub fn new() -> Self { Self::default() }

    /// Record a verdict.
    pub fn record(&mut self, v: Verdict) {
        match v { Verdict::Enough => self.accepted += 1, Verdict::SendAll => self.escalated += 1 }
    }

    /// Is previewing still saving anything?
    ///
    /// If the Thinker always escalates, the preview is a round trip and a small model's forward pass
    /// spent to learn nothing, and the caller should send the full body directly. Requires a few samples
    /// first: judging on one escalation would abandon the mechanism before it has been tried.
    pub fn preview_is_paying(&self) -> bool {
        let total = self.accepted + self.escalated;
        if total < 4 { return true; }
        self.accepted * 4 >= total     // paying if at least a quarter are accepted
    }

    pub fn stats(&self) -> String {
        format!("{} previews accepted, {} escalated to full", self.accepted, self.escalated)
    }
}

/// What the Thinker is told it may ask for.
pub const NEGOTIATION_PROTOCOL: &str = "When you need repository context, say so and name what you want. A smaller model will run the fetch and show you a PREVIEW first: the beginning of the result, plus how much was withheld.

Reply ENOUGH if the preview answers your question, or SEND-ALL to receive the whole result.

ENOUGH means you will not see the withheld part, so do not use it to save effort — use it when the preview genuinely settles the question. If you answer from a preview that was missing what you needed, nothing will tell you afterwards.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_picks_a_SHAPE_and_never_writes_a_command() {
        // Letting a model produce the command is how an injection happens with nobody writing an
        // exploit: it need not be adversarial, only wrong, and `--format=$(rm -rf ~)` is a plausible
        // hallucination. argv has no parsing step where a quote could change the structure.
        let q = Query::CommitsTouching { path: "src/lib.rs".into(), limit: 5 };
        let a = q.argv().unwrap();
        assert_eq!(a[0], "git");
        assert!(a.contains(&"--".to_string()), "flags and paths must be separated");
        assert_eq!(a.last().unwrap(), "src/lib.rs");
        assert!(!a.iter().any(|x| x.contains("sh") && x.contains("-c")));
    }

    #[test]
    fn hostile_paths_are_REFUSED_not_sanitised() {
        // Sanitising invites a rewrite that is still wrong. Refusing is checkable.
        for bad in ["../../etc/passwd", "/etc/passwd", "-oProxyCommand=x", "a;rm -rf b", "a$(x)b", "a|b"] {
            let e = Query::CommitsTouching { path: bad.into(), limit: 3 }.argv().unwrap_err();
            assert!(matches!(e, Refusal::BadPath(_)), "{bad} must be refused, got {e:?}");
            assert!(e.explain().contains("refused before argv"));
        }
        assert!(Query::CommitsTouching { path: "src/a-b_c.rs".into(), limit: 3 }.argv().is_ok());
    }

    #[test]
    fn a_sha_must_look_like_a_sha() {
        assert!(Query::ShowCommit { sha: "5b48358".into() }.argv().is_ok());
        for bad in ["HEAD;ls", "--upload-pack=x", "zzzz", ""] {
            assert!(Query::ShowCommit { sha: bad.into() }.argv().is_err(), "{bad}");
        }
    }

    #[test]
    fn show_never_asks_for_a_full_diff() {
        // An unbounded patch would blow the Thinker's context, and the useful part is almost always the
        // message plus which files moved.
        let a = Query::ShowCommit { sha: "abc123".into() }.argv().unwrap();
        assert!(a.contains(&"--stat".to_string()));
        assert!(!a.contains(&"-p".to_string()) && !a.contains(&"--patch".to_string()));
    }

    #[test]
    fn a_limit_is_clamped_so_a_model_cannot_ask_for_the_whole_history() {
        let a = Query::CommitsTouching { path: "x.rs".into(), limit: 100_000 }.argv().unwrap();
        assert!(a.contains(&"-50".to_string()), "{a:?}");
        let b = Query::CommitsTouching { path: "x.rs".into(), limit: 0 }.argv().unwrap();
        assert!(b.contains(&"-1".to_string()), "{b:?}");
    }

    #[test]
    fn evidence_tells_the_thinker_it_is_seeing_a_SLICE() {
        // The failure that matters more than injection: a small model choosing the context is choosing
        // what the large one can conclude. Three commits must not read as the complete history.
        let q = Query::CommitsTouching { path: "src/lib.rs".into(), limit: 3 };
        let e = Evidence::new(&q, "abc123 2026-07-28 fix a thing");
        assert!(e.caveat().contains("SLICE of the repository, not its history"));
        assert!(e.caveat().contains("selected by a smaller model"));
        assert!(e.to_prompt_block().contains("TOOL STATUS"), "framed, so it is not read as user data");
    }

    #[test]
    fn truncation_is_announced_in_the_caveat_not_just_the_flag() {
        let e = Evidence::new(&Query::ShowCommit { sha: "abc123".into() }, &"x".repeat(9000));
        assert!(e.truncated);
        assert!(e.body.len() <= MAX_BODY_CHARS);
        assert!(e.caveat().contains("TRUNCATED"), "a silently cut body reads as a complete one");
    }

    #[test]
    fn NONE_is_a_valid_answer_and_parses_to_no_query() {
        // Fetching something irrelevant costs the Thinker context it needs for the actual question, so
        // "nothing would help" has to be expressible and has to be the easy path.
        assert!(parse_request("NONE").is_none());
        assert!(parse_request("").is_none());
        assert!(FETCH_PROTOCOL.contains("Ask for NONE"));
        assert!(FETCH_PROTOCOL.contains("costs the main model context"));
    }

    fn long_body(lines: usize) -> String {
        (0..lines).map(|i| format!("abc{i:04} 2026-07-28 commit subject number {i}"))
            .collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_preview_states_what_it_WITHHELD_in_the_same_breath_as_the_content() {
        // The dangerous direction is a preview that hides the useful part, so the Thinker answers
        // ENOUGH from a slice it actively declined to complete — worse than no preview, because it has
        // now made a decision it believes was informed.
        let p = Preview::of(&long_body(60));
        assert!(!p.is_complete());
        let ask = p.ask();
        assert!(ask.contains("withheld"), "{ask}");
        assert!(ask.contains("you will not see the withheld part"),
            "the consequence of ENOUGH must be stated where the choice is made");
        assert!(p.withheld_lines > 0 && p.withheld_chars > 0);
    }

    #[test]
    fn a_preview_cuts_on_a_LINE_boundary() {
        // Half a commit subject reads as a whole one, and invites the Thinker to accept a preview it has
        // misread. Truncating mid-line is how a preview becomes actively misleading rather than partial.
        let p = Preview::of(&long_body(60));
        assert!(!p.head.is_empty());
        assert!(!p.head.ends_with(|c: char| c.is_ascii_digit() && p.head.len() == PREVIEW_CHARS),
            "must not stop mid-token");
        assert!(long_body(60).starts_with(&p.head));
    }

    #[test]
    fn a_short_result_is_offered_COMPLETE_rather_than_previewed() {
        // Previewing something that fits is a round trip to say "here is all of it, is that enough".
        let p = Preview::of("abc123 2026-07-28 one commit");
        assert!(p.is_complete());
        assert!(p.ask().contains("complete result"));
        assert_eq!(p.withheld_chars, 0);
    }

    #[test]
    fn an_ambiguous_verdict_resolves_to_the_SAFE_direction() {
        // "not enough, send all" contains both words. Preferring ENOUGH would silently drop context the
        // Thinker explicitly asked for; preferring SEND-ALL costs tokens and nothing else.
        assert_eq!(parse_verdict("not enough, SEND-ALL please"), Some(Verdict::SendAll));
        assert_eq!(parse_verdict("ENOUGH"), Some(Verdict::Enough));
        assert_eq!(parse_verdict("send all"), Some(Verdict::SendAll));
        assert_eq!(parse_verdict("hmm"), None, "an unparseable reply is not a silent ENOUGH");
    }

    #[test]
    fn the_liaison_never_DECIDES_sufficiency() {
        // looks_sufficient is a hint. A Liaison that could settle this alone would be choosing what the
        // Thinker knows, which is the thing this module exists to prevent.
        let p = Preview::of(&long_body(60));
        let _hint = p.looks_sufficient();
        // the ask always offers the choice, whatever the hint says
        assert!(p.ask().contains("ENOUGH or SEND-ALL"));
    }

    #[test]
    fn previewing_stops_when_it_stops_paying() {
        // If the Thinker always escalates, the preview is a round trip and a forward pass spent to learn
        // nothing. But judging on one escalation would abandon the mechanism before it was tried.
        let mut n = Negotiation::new();
        n.record(Verdict::SendAll);
        assert!(n.preview_is_paying(), "one sample is not evidence");
        for _ in 0..4 { n.record(Verdict::SendAll); }
        assert!(!n.preview_is_paying(), "5 escalations, 0 accepted: stop previewing");

        let mut m = Negotiation::new();
        for _ in 0..3 { m.record(Verdict::Enough); }
        for _ in 0..3 { m.record(Verdict::SendAll); }
        assert!(m.preview_is_paying(), "half accepted is clearly worth it");
        assert!(m.stats().contains("3 previews accepted"));
    }

    #[test]
    fn the_protocol_warns_against_using_ENOUGH_to_save_effort() {
        // The instruction a model most needs, because ENOUGH is the cheap answer and its cost is
        // invisible: nothing tells it afterwards that the withheld part mattered.
        assert!(NEGOTIATION_PROTOCOL.contains("do not use it to save effort"));
        assert!(NEGOTIATION_PROTOCOL.contains("nothing will tell you afterwards"));
    }

    #[test]
    fn the_parser_handles_the_documented_forms() {
        assert_eq!(parse_request("TOUCHING src/lib.rs 5"),
                   Some(Query::CommitsTouching { path: "src/lib.rs".into(), limit: 5 }));
        assert_eq!(parse_request("SHOW 5b48358"), Some(Query::ShowCommit { sha: "5b48358".into() }));
        assert_eq!(parse_request("BLAME src/a.rs 10 20"),
                   Some(Query::BlameRange { path: "src/a.rs".into(), start: 10, end: 20 }));
        // preamble is skipped, and a missing limit defaults rather than failing
        assert_eq!(parse_request("\n  MATCHING frame"),
                   Some(Query::CommitsMatching { term: "frame".into(), limit: 10 }));
    }
}
