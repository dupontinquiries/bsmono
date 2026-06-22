// gitmind-core/src/provenance.rs
//
// "Who changed this file?" — attribute a change to the process behind it so
// shared-file conflicts can be resolved with context: was that edit from an
// AI agent, a build tool, an editor, or a human shell? Two sources:
//
//   1. Self-report (unprivileged, reliable): a process sets GITMIND_AGENT
//      (e.g. an AI coding tool exports GITMIND_AGENT=claude). Snapshots made
//      while that's set are tagged with it. This needs no special perms.
//
//   2. PID attribution (privileged): a fanotify watcher reports the PID that
//      wrote a file; we resolve it via /proc to a command name and classify
//      it. fanotify needs CAP_SYS_ADMIN, so this is the opportunistic path;
//      plain inotify (what watch.rs uses) sees the change but NOT the PID.
//
// The classifier + /proc resolution are pure/testable; the fanotify loop is
// a thin privileged layer over them.

/// What kind of actor produced a change. Drives conflict policy: e.g. prefer
/// to auto-resolve BuildTool churn, but flag AiAgent vs Human collisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    AiAgent,
    BuildTool,
    Editor,
    VersionControl,
    Shell,
    Unknown,
}

impl Origin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Origin::AiAgent => "ai-agent",
            Origin::BuildTool => "build-tool",
            Origin::Editor => "editor",
            Origin::VersionControl => "vcs",
            Origin::Shell => "shell",
            Origin::Unknown => "unknown",
        }
    }
}

/// Classify a process by its command name (basename of comm/exe).
pub fn classify(comm: &str) -> Origin {
    let c = comm.trim().to_ascii_lowercase();
    let c = c.rsplit('/').next().unwrap_or(&c);
    const AI: &[&str] = &["claude", "aider", "copilot", "cursor", "codeium", "continue", "cody", "tabby", "ollama", "llama"];
    const BUILD: &[&str] = &["cargo", "rustc", "gcc", "clang", "cc", "ld", "make", "ninja", "cmake", "go", "node", "npm", "pnpm", "yarn", "python", "python3", "javac", "tsc", "webpack"];
    const EDITOR: &[&str] = &["vim", "nvim", "emacs", "nano", "code", "codium", "hx", "helix", "kate", "gedit", "micro", "subl"];
    const VCS: &[&str] = &["git", "jj", "hg", "gitmind"];
    const SHELL: &[&str] = &["bash", "zsh", "sh", "fish", "dash", "ksh"];
    if AI.iter().any(|k| c.contains(k)) { return Origin::AiAgent; }
    if BUILD.contains(&c) { return Origin::BuildTool; }
    if EDITOR.contains(&c) { return Origin::Editor; }
    if VCS.contains(&c) { return Origin::VersionControl; }
    if SHELL.contains(&c) { return Origin::Shell; }
    Origin::Unknown
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub comm: String,
    pub origin: Origin,
}

impl ProcessInfo {
    /// Resolve a PID via /proc (Linux). None if the process is gone or /proc
    /// isn't available.
    pub fn from_pid(pid: u32) -> Option<ProcessInfo> {
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        let comm = comm.trim().to_string();
        if comm.is_empty() {
            return None;
        }
        let origin = classify(&comm);
        Some(ProcessInfo { pid, comm, origin })
    }
}

/// The origin of the current snapshot, for tagging checkpoints. Prefers the
/// self-reported GITMIND_AGENT; falls back to classifying this process.
pub fn current_origin() -> (Origin, String) {
    if let Ok(agent) = std::env::var("GITMIND_AGENT") {
        if !agent.is_empty() {
            return (classify(&agent), agent);
        }
    }
    match ProcessInfo::from_pid(std::process::id()) {
        Some(p) => (p.origin, p.comm),
        None => (Origin::Unknown, "unknown".to_string()),
    }
}

/// Compose a checkpoint label that records provenance, e.g.
/// "[ai-agent:claude] fix parser".
pub fn tag_label(label: &str) -> String {
    let (origin, name) = current_origin();
    format!("[{}:{}] {}", origin.as_str(), name, label)
}

/// Tag only when a process self-reports via GITMIND_AGENT; otherwise return
/// the label unchanged (avoids tagging every change with the watcher's own
/// identity when no real attribution is available).
pub fn tag_label_if_agent(label: &str) -> String {
    if std::env::var("GITMIND_AGENT").map(|v| !v.is_empty()).unwrap_or(false) {
        tag_label(label)
    } else {
        label.to_string()
    }
}

/// Conflict hint for two changes to the same file by different origins.
/// AI-vs-Human is the case worth surfacing to a person; build-tool churn is
/// usually safe to take the newer side automatically.
pub fn conflict_needs_human(a: Origin, b: Origin) -> bool {
    let involves_ai = a == Origin::AiAgent || b == Origin::AiAgent;
    let involves_human = matches!(a, Origin::Editor | Origin::Shell) || matches!(b, Origin::Editor | Origin::Shell);
    involves_ai && involves_human
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_actors() {
        assert_eq!(classify("claude"), Origin::AiAgent);
        assert_eq!(classify("/usr/bin/cargo"), Origin::BuildTool);
        assert_eq!(classify("nvim"), Origin::Editor);
        assert_eq!(classify("git"), Origin::VersionControl);
        assert_eq!(classify("zsh"), Origin::Shell);
        assert_eq!(classify("some-random-daemon"), Origin::Unknown);
    }

    #[test]
    fn resolves_own_pid() {
        let me = ProcessInfo::from_pid(std::process::id());
        // On Linux /proc exists; comm should be non-empty.
        if let Some(info) = me {
            assert_eq!(info.pid, std::process::id());
            assert!(!info.comm.is_empty());
        }
    }

    #[test]
    fn env_self_report_wins() {
        std::env::set_var("GITMIND_AGENT", "claude");
        let (origin, name) = current_origin();
        assert_eq!(origin, Origin::AiAgent);
        assert_eq!(name, "claude");
        assert!(tag_label("fix").starts_with("[ai-agent:claude]"));
        std::env::remove_var("GITMIND_AGENT");
    }

    #[test]
    fn ai_vs_human_flags_for_review() {
        assert!(conflict_needs_human(Origin::AiAgent, Origin::Editor));
        assert!(!conflict_needs_human(Origin::BuildTool, Origin::BuildTool));
        assert!(!conflict_needs_human(Origin::AiAgent, Origin::BuildTool));
    }
}
