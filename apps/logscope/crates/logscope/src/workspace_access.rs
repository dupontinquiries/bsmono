//! workspace_access.rs — what the model can see of a workspace, and when.
//!
//! A tmp workspace with twelve files and a coordinator running against it does not want all twelve in
//! the context. Most of them are irrelevant to any given step, and the ones that matter are usually
//! wanted in part. Attaching everything spends the Thinker's context on files it will not read and
//! crowds out the ones it will.
//!
//! ## Two policies
//!
//! * **Eager** — attach the contents. Right when the user has explicitly opened ONE document and is
//!   asking about it, which is the ordinary chat case.
//! * **OnDemand** — attach a MANIFEST and let the model ask. Right for the aggressive coordinator, the
//!   Liaison, and anything else working across a directory.
//!
//! ## The manifest has to say it is a manifest
//!
//! A list of filenames with sizes, dropped into a prompt without comment, reads as a description of
//! files the model has already been given. It will then answer about their contents from the names
//! alone. So the manifest states, in its own text, that the contents are NOT included and how to ask —
//! the same discipline as `repoquery::Evidence::caveat`, and for the same reason.

/// How much of the workspace goes into the prompt.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Policy {
    /// Contents attached.
    Eager,
    /// Manifest only; contents on request.
    OnDemand,
}

impl Policy {
    /// The default for a given mode.
    ///
    /// A coordinator runs many steps across many files and is exactly the case Eager handles worst.
    /// A single chat about a single opened log is exactly the case it handles best.
    pub fn for_coordinator() -> Self { Policy::OnDemand }
    pub fn for_single_document() -> Self { Policy::Eager }
}

/// One workspace entry, without its contents.
#[derive(Clone, PartialEq, Debug)]
pub struct Entry { pub path: String, pub bytes: u64, pub lines: usize }

/// A structured request for part of the workspace.
///
/// Same shape as `repoquery::Query` and for the same reason: the model picks a variant and fills typed
/// fields, so there is no string that could become a command or a path traversal.
#[derive(Clone, PartialEq, Debug)]
pub enum Request {
    /// Whole file, subject to the size cap.
    Read { path: String },
    /// Lines `start..=end`, 1-based.
    ReadLines { path: String, start: usize, end: usize },
    /// Lines containing a literal substring.
    Find { path: String, needle: String, limit: usize },
}

#[derive(Clone, PartialEq, Debug)]
pub enum Refusal { BadPath(String), NotInWorkspace(String), BadRange(String), EmptyNeedle }

impl Refusal {
    pub fn explain(&self) -> String {
        match self {
            Refusal::BadPath(p) => format!("{p:?} contains a traversal or a shell metacharacter"),
            Refusal::NotInWorkspace(p) => format!("{p:?} is not in the workspace manifest — the model \
asked for a file that does not exist, which usually means it inferred one from context"),
            Refusal::BadRange(r) => format!("line range {r} is empty or inverted"),
            Refusal::EmptyNeedle => "an empty search term would match every line".into(),
        }
    }
}

fn path_ok(p: &str) -> bool {
    !p.is_empty() && !p.starts_with('/') && !p.starts_with('-')
        && !p.split('/').any(|c| c == "..")
        && p.chars().all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
}

/// Largest single response, in bytes.
pub const MAX_RESPONSE: usize = 8000;

impl Request {
    pub fn path(&self) -> &str {
        match self { Request::Read { path } | Request::ReadLines { path, .. }
                     | Request::Find { path, .. } => path }
    }

    /// Validate against the manifest. A request for a file that is not listed is refused rather than
    /// attempted — a model that asks for `config.yaml` because most projects have one should be told it
    /// does not exist here, not handed an IO error.
    pub fn check(&self, manifest: &[Entry]) -> Result<(), Refusal> {
        let p = self.path();
        if !path_ok(p) { return Err(Refusal::BadPath(p.to_string())); }
        if !manifest.iter().any(|e| e.path == p) { return Err(Refusal::NotInWorkspace(p.to_string())); }
        match self {
            Request::ReadLines { start, end, .. } if *start == 0 || end < start =>
                Err(Refusal::BadRange(format!("{start}..{end}"))),
            Request::Find { needle, .. } if needle.trim().is_empty() => Err(Refusal::EmptyNeedle),
            _ => Ok(()),
        }
    }

    /// Serve the request from file text already in hand.
    pub fn serve(&self, text: &str) -> String {
        let out = match self {
            Request::Read { .. } => text.to_string(),
            Request::ReadLines { start, end, .. } => text.lines().enumerate()
                .filter(|(i, _)| i + 1 >= *start && i + 1 <= *end)
                .map(|(i, l)| format!("{:>5} | {l}", i + 1)).collect::<Vec<_>>().join("\n"),
            Request::Find { needle, limit, .. } => text.lines().enumerate()
                .filter(|(_, l)| l.contains(needle.as_str()))
                .take((*limit).clamp(1, 200))
                .map(|(i, l)| format!("{:>5} | {l}", i + 1)).collect::<Vec<_>>().join("\n"),
        };
        if out.len() <= MAX_RESPONSE { return out; }
        let head: String = out.chars().take(MAX_RESPONSE).collect();
        format!("{head}\n[truncated at {MAX_RESPONSE} characters — narrow the range or search instead]")
    }
}

/// The manifest block for a prompt.
///
/// States that contents are absent. A bare list of filenames reads as "here are the files you have",
/// and a model will answer about them from the names alone.
pub fn manifest_block(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return "WORKSPACE: empty. No files are available; do not refer to any.".to_string();
    }
    let mut s = format!("WORKSPACE MANIFEST — {} file(s). CONTENTS ARE NOT INCLUDED below; these are \
names and sizes only. Ask for what you need:\n  READ <path>\n  LINES <path> <start> <end>\n  \
FIND <path> <text> <max>\nRequest only what the question needs — every file you pull in costs context \
the answer could have used.\n", entries.len());
    for e in entries {
        s.push_str(&format!("  {} ({} bytes, {} lines)\n", e.path, e.bytes, e.lines));
    }
    s
}

/// Parse a model's one-line request.
pub fn parse_request(reply: &str) -> Option<Request> {
    let line = reply.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut it = line.split_whitespace();
    match it.next()?.to_ascii_uppercase().as_str() {
        "READ" => Some(Request::Read { path: it.next()?.to_string() }),
        "LINES" => {
            let p = it.next()?.to_string();
            Some(Request::ReadLines { path: p, start: it.next()?.parse().ok()?, end: it.next()?.parse().ok()? })
        }
        "FIND" => {
            let p = it.next()?.to_string();
            let n = it.next()?.to_string();
            Some(Request::Find { path: p, needle: n, limit: it.next().and_then(|v| v.parse().ok()).unwrap_or(20) })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn man() -> Vec<Entry> {
        vec![Entry { path: "app.log".into(), bytes: 400, lines: 12 },
             Entry { path: "src/main.rs".into(), bytes: 900, lines: 40 }]
    }

    #[test]
    fn a_coordinator_defaults_to_ON_DEMAND_and_a_single_doc_to_eager() {
        // A coordinator runs many steps across many files and is exactly what Eager handles worst;
        // one opened log is exactly what it handles best.
        assert_eq!(Policy::for_coordinator(), Policy::OnDemand);
        assert_eq!(Policy::for_single_document(), Policy::Eager);
    }

    #[test]
    fn the_manifest_says_the_CONTENTS_ARE_ABSENT() {
        // A bare list of filenames reads as "here are the files you have", and a model will answer
        // about them from the names alone — confidently, and without ever asking.
        let b = manifest_block(&man());
        assert!(b.contains("CONTENTS ARE NOT INCLUDED"), "{b}");
        assert!(b.contains("READ <path>"), "and how to ask for them");
        assert!(b.contains("costs context the answer could have used"),
            "with a reason not to ask for everything");
        assert!(b.contains("app.log") && b.contains("12 lines"));
    }

    #[test]
    fn an_empty_workspace_says_so_rather_than_showing_nothing() {
        // Silence reads as "the manifest was omitted", and a model may then assume files exist.
        let b = manifest_block(&[]);
        assert!(b.contains("empty") && b.contains("do not refer to any"));
    }

    #[test]
    fn a_file_not_in_the_manifest_is_refused_with_the_likely_reason() {
        // Models ask for config.yaml because most projects have one. Telling them it does not exist
        // here is more useful than an IO error, and stops the request being retried.
        let r = Request::Read { path: "config.yaml".into() }.check(&man()).unwrap_err();
        assert!(matches!(r, Refusal::NotInWorkspace(_)));
        assert!(r.explain().contains("inferred one from context"));
    }

    #[test]
    fn traversal_and_metacharacters_are_refused_before_the_manifest_check() {
        for bad in ["../etc/passwd", "/etc/passwd", "a;rm -rf b", "-oProxy"] {
            let e = Request::Read { path: bad.into() }.check(&man()).unwrap_err();
            assert!(matches!(e, Refusal::BadPath(_)), "{bad} -> {e:?}");
        }
    }

    #[test]
    fn a_response_is_capped_and_says_how_to_narrow_it() {
        // A cap that just truncates teaches nothing; the model repeats the same request.
        let big = (0..2000).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let out = Request::Read { path: "app.log".into() }.serve(&big);
        assert!(out.len() <= MAX_RESPONSE + 100);
        assert!(out.contains("narrow the range or search instead"));
    }

    #[test]
    fn line_and_find_responses_carry_line_numbers() {
        // So a follow-up request can name a range, and so an edit protocol can act on it.
        let text = "alpha\nbeta\ngamma\nbeta again\n";
        let l = Request::ReadLines { path: "app.log".into(), start: 2, end: 3 }.serve(text);
        assert!(l.contains("    2 | beta") && l.contains("    3 | gamma"));
        assert!(!l.contains("alpha"), "outside the range");
        let f = Request::Find { path: "app.log".into(), needle: "beta".into(), limit: 10 }.serve(text);
        assert!(f.contains("    2 | beta") && f.contains("    4 | beta again"));
    }

    #[test]
    fn the_parser_handles_the_documented_forms() {
        assert_eq!(parse_request("READ app.log"), Some(Request::Read { path: "app.log".into() }));
        assert_eq!(parse_request("LINES src/main.rs 10 20"),
                   Some(Request::ReadLines { path: "src/main.rs".into(), start: 10, end: 20 }));
        assert_eq!(parse_request("FIND app.log ERROR 5"),
                   Some(Request::Find { path: "app.log".into(), needle: "ERROR".into(), limit: 5 }));
        assert!(parse_request("please show me the log").is_none(), "prose is not a request");
    }
}
