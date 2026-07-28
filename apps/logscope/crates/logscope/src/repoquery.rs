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
