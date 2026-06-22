//! v0.1.658: session auto-management (R2 + the kernel-dev "this session
//! needs a new tab for this python script with helix" insight).
//!
//! Hector (the Asahi kernel-dev in the corpus) works LINEARLY — one long
//! terminal, manually juggling tracer scripts, reboots, and edits. A real
//! engineering approach would split that into clean per-task
//! sessions/tabs, but that costs keyboard throughput he's already maxed on,
//! and thought-stream keepup he has no voice for. So: detect context shifts
//! from the activity stream and PROPOSE the structure he'd make if he had
//! the spare hands — "new tab for foo.py, helix, in this workspace".
//!
//! PURE + testable. The detector takes a stream of Activity events
//! (commands / cd / edits / editor-opens / reboots) and emits:
//!   - segment boundaries (where a new session is warranted),
//!   - a Proposal at the boundary (new tab / new session / re-entry),
//!   - an auto-generated name per segment.
//! The wiring layer feeds it real pty/editor events and surfaces proposals
//! via the assist-bubble (transient, dismissable — never auto-acts).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    /// changed working directory
    Cd,
    /// ran a shell command
    Run,
    /// opened a file in an editor
    EditorOpen,
    /// created/started a new file (not seen before)
    NewFile,
    /// a build step
    Build,
    /// a test run
    Test,
    /// device/system reboot — a hard context boundary + re-entry point
    Reboot,
}

#[derive(Debug, Clone)]
pub struct Activity {
    pub kind: ActivityKind,
    /// path / command / directory, depending on kind
    pub target: String,
    /// working directory at the time
    pub cwd: String,
}

impl Activity {
    pub fn new(kind: ActivityKind, target: &str, cwd: &str) -> Self {
        Activity { kind, target: target.to_string(), cwd: cwd.to_string() }
    }
    /// language guessed from a file extension, if target is a path.
    pub fn lang(&self) -> Option<&'static str> { lang_of(&self.target) }
}

/// Guess a language label from a file path's extension.
pub fn lang_of(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next().filter(|e| *e != path)?;
    Some(match ext {
        "rs" => "rust", "py" => "python", "c" | "h" => "c",
        "cpp" | "cc" | "hpp" => "cpp", "js" | "ts" => "js",
        "go" => "go", "sh" => "shell", "toml" => "toml",
        "md" => "markdown", "rb" => "ruby", _ => return None,
    })
}

/// repo root guess: the last path component of the cwd (cheap stand-in for
/// a real VCS-root walk, which the wiring layer can supply instead).
fn repo_of(cwd: &str) -> &str {
    cwd.trim_end_matches('/').rsplit('/').next().unwrap_or(cwd)
}

/// A proposed session-management action, surfaced (never auto-applied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proposal {
    /// open `path` in a new tab/pane with `editor`, in the current workspace
    NewTabForFile { path: String, editor: String },
    /// start a new named session for a different repo/dir
    NewSessionForRepo { repo: String },
    /// after a reboot, re-present the prior context
    ReentryAfterReboot { restore: String },
}

/// A detected session segment over a slice of the activity stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub start: usize,
    pub end: usize, // exclusive
    pub name: String,
    pub repo: String,
}

/// Rolling context the detector carries while scanning.
#[derive(Debug, Clone, Default)]
struct Ctx {
    repo: String,
    dominant_lang: Option<String>,
    open_files: Vec<String>,
}

/// Decide whether `next` starts a NEW session relative to `ctx`. The rules
/// mirror what a clean engineering workflow would split out:
///  - a cd into a DIFFERENT repo,
///  - a reboot (hard boundary),
/// Editing a new file of a different language is a TAB proposal, not
/// necessarily a new session — handled separately by `propose`.
fn is_boundary(ctx: &Ctx, next: &Activity) -> bool {
    match next.kind {
        ActivityKind::Reboot => true,
        ActivityKind::Cd => {
            let r = repo_of(&next.target);
            !ctx.repo.is_empty() && r != ctx.repo
        }
        _ => {
            // a cwd that belongs to a different repo than the session's
            let r = repo_of(&next.cwd);
            !ctx.repo.is_empty() && !r.is_empty() && r != ctx.repo
        }
    }
}

/// Segment an activity stream into sessions at context boundaries, naming
/// each from its dominant signal.
pub fn segment(acts: &[Activity]) -> Vec<Segment> {
    let mut out = Vec::new();
    if acts.is_empty() { return out; }
    let mut ctx = Ctx::default();
    let mut start = 0usize;
    for (i, a) in acts.iter().enumerate() {
        if i == start {
            // initialize the segment's context from its first activity
            ctx = Ctx::default();
            ctx.repo = repo_of(if a.kind == ActivityKind::Cd { &a.target } else { &a.cwd }).to_string();
            ctx.dominant_lang = a.lang().map(|s| s.to_string());
            continue;
        }
        if is_boundary(&ctx, a) {
            out.push(finish_segment(&acts[start..i], start, i, &ctx));
            start = i;
            // re-init context for the new segment from this activity
            ctx = Ctx::default();
            ctx.repo = repo_of(if a.kind == ActivityKind::Cd { &a.target } else { &a.cwd }).to_string();
            ctx.dominant_lang = a.lang().map(|s| s.to_string());
        } else {
            // accumulate context
            if ctx.dominant_lang.is_none() {
                ctx.dominant_lang = a.lang().map(|s| s.to_string());
            }
            if matches!(a.kind, ActivityKind::EditorOpen | ActivityKind::NewFile)
                && !ctx.open_files.contains(&a.target) {
                ctx.open_files.push(a.target.clone());
            }
        }
    }
    out.push(finish_segment(&acts[start..], start, acts.len(), &ctx));
    out
}

fn finish_segment(slice: &[Activity], start: usize, end: usize, ctx: &Ctx) -> Segment {
    Segment { start, end, name: name_segment(slice), repo: ctx.repo.clone() }
}

/// Auto-name a segment from its dominant signal: repo + dominant language +
/// the most-edited file (basename), falling back gracefully.
pub fn name_segment(acts: &[Activity]) -> String {
    if acts.is_empty() { return "session".into(); }
    let repo = acts.iter().map(|a| repo_of(&a.cwd))
        .find(|r| !r.is_empty()).unwrap_or("").to_string();
    // dominant language
    let mut lang_counts: std::collections::HashMap<&str, usize> = Default::default();
    for a in acts { if let Some(l) = a.lang() { *lang_counts.entry(l).or_insert(0) += 1; } }
    let lang = lang_counts.into_iter().max_by_key(|(_, n)| *n).map(|(l, _)| l);
    // most-touched file basename
    let mut file_counts: std::collections::HashMap<&str, usize> = Default::default();
    for a in acts {
        if matches!(a.kind, ActivityKind::EditorOpen | ActivityKind::NewFile) {
            let base = a.target.rsplit('/').next().unwrap_or(&a.target);
            *file_counts.entry(base).or_insert(0) += 1;
        }
    }
    let file = file_counts.into_iter().max_by_key(|(_, n)| *n).map(|(f, _)| f);
    let mut parts: Vec<String> = Vec::new();
    if !repo.is_empty() { parts.push(repo); }
    if let Some(f) = file { parts.push(f.to_string()); }
    else if let Some(l) = lang { parts.push(l.to_string()); }
    if parts.is_empty() { "session".into() } else { parts.join(" · ") }
}

/// Given the current session's open files + dominant language, decide
/// whether `next` warrants a PROPOSAL (new tab / new session / re-entry).
/// `editor` is the user's preferred editor for NewTab proposals.
pub fn propose(open_files: &[String], dominant_lang: Option<&str>,
               current_repo: &str, next: &Activity, editor: &str) -> Option<Proposal> {
    match next.kind {
        ActivityKind::Reboot => Some(Proposal::ReentryAfterReboot {
            restore: if open_files.is_empty() { current_repo.to_string() }
                     else { open_files.join(", ") },
        }),
        ActivityKind::Cd => {
            let r = repo_of(&next.target);
            if !current_repo.is_empty() && r != current_repo {
                Some(Proposal::NewSessionForRepo { repo: r.to_string() })
            } else { None }
        }
        ActivityKind::NewFile | ActivityKind::EditorOpen => {
            // propose a NEW TAB when starting a file that isn't open yet AND
            // something is already open (linear-workflow split). A brand-new
            // file of a different language than the session's dominant one is
            // an especially strong signal.
            if open_files.contains(&next.target) { return None; }
            let already_working = !open_files.is_empty();
            let lang_shift = match (dominant_lang, next.lang()) {
                (Some(d), Some(n)) => d != n,
                _ => false,
            };
            if already_working || lang_shift {
                Some(Proposal::NewTabForFile {
                    path: next.target.clone(),
                    editor: editor.to_string(),
                })
            } else { None }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn act(k: ActivityKind, t: &str, cwd: &str) -> Activity { Activity::new(k, t, cwd) }

    #[test]
    fn lang_detection() {
        assert_eq!(lang_of("src/main.rs"), Some("rust"));
        assert_eq!(lang_of("tracer.py"), Some("python"));
        assert_eq!(lang_of("Makefile"), None);
        assert_eq!(lang_of("noext"), None);
    }

    #[test]
    fn single_repo_one_segment() {
        let acts = vec![
            act(ActivityKind::Cd, "/home/h/asahi", ""),
            act(ActivityKind::EditorOpen, "tracer.py", "/home/h/asahi"),
            act(ActivityKind::Run, "python tracer.py", "/home/h/asahi"),
        ];
        let segs = segment(&acts);
        assert_eq!(segs.len(), 1, "one repo → one session");
        assert!(segs[0].name.contains("asahi"));
    }

    #[test]
    fn repo_change_splits_session() {
        let acts = vec![
            act(ActivityKind::Cd, "/home/h/asahi", ""),
            act(ActivityKind::Run, "make", "/home/h/asahi"),
            act(ActivityKind::Cd, "/home/h/m1n1", ""),  // different repo → boundary
            act(ActivityKind::Run, "make", "/home/h/m1n1"),
        ];
        let segs = segment(&acts);
        assert_eq!(segs.len(), 2, "repo change → two sessions");
        assert_eq!(segs[0].repo, "asahi");
        assert_eq!(segs[1].repo, "m1n1");
    }

    #[test]
    fn reboot_is_a_boundary() {
        let acts = vec![
            act(ActivityKind::EditorOpen, "dcp.py", "/home/h/asahi"),
            act(ActivityKind::Reboot, "", "/home/h/asahi"),
            act(ActivityKind::Run, "dmesg", "/home/h/asahi"),
        ];
        let segs = segment(&acts);
        assert_eq!(segs.len(), 2, "reboot splits the timeline");
    }

    #[test]
    fn propose_new_tab_for_second_file() {
        // already editing tracer.py; opening a new script → propose a tab
        let open = vec!["tracer.py".to_string()];
        let next = act(ActivityKind::EditorOpen, "decoder.py", "/home/h/asahi");
        let p = propose(&open, Some("python"), "asahi", &next, "hx");
        assert_eq!(p, Some(Proposal::NewTabForFile { path: "decoder.py".into(), editor: "hx".into() }));
    }

    #[test]
    fn propose_lang_shift_tab_even_first_file() {
        // session is python; opening a .rs is a strong shift → propose a tab
        let open: Vec<String> = vec![];
        let next = act(ActivityKind::EditorOpen, "build.rs", "/home/h/asahi");
        let p = propose(&open, Some("python"), "asahi", &next, "vim");
        assert!(matches!(p, Some(Proposal::NewTabForFile { .. })));
    }

    #[test]
    fn no_proposal_for_already_open_file() {
        let open = vec!["tracer.py".to_string()];
        let next = act(ActivityKind::EditorOpen, "tracer.py", "/home/h/asahi");
        assert_eq!(propose(&open, Some("python"), "asahi", &next, "hx"), None);
    }

    #[test]
    fn propose_reentry_after_reboot() {
        let open = vec!["dcp.py".to_string(), "mailbox.py".to_string()];
        let next = act(ActivityKind::Reboot, "", "/home/h/asahi");
        let p = propose(&open, Some("python"), "asahi", &next, "hx");
        assert!(matches!(p, Some(Proposal::ReentryAfterReboot { .. })));
    }

    #[test]
    fn propose_new_session_on_repo_cd() {
        let open = vec!["a.py".to_string()];
        let next = act(ActivityKind::Cd, "/home/h/other", "/home/h/asahi");
        assert_eq!(propose(&open, Some("python"), "asahi", &next, "hx"),
                   Some(Proposal::NewSessionForRepo { repo: "other".into() }));
    }

    #[test]
    fn name_from_dominant_file() {
        let acts = vec![
            act(ActivityKind::EditorOpen, "/home/h/asahi/tracer.py", "/home/h/asahi"),
            act(ActivityKind::EditorOpen, "/home/h/asahi/tracer.py", "/home/h/asahi"),
            act(ActivityKind::EditorOpen, "/home/h/asahi/other.py", "/home/h/asahi"),
        ];
        let n = name_segment(&acts);
        assert!(n.contains("asahi") && n.contains("tracer.py"), "got {}", n);
    }
}
