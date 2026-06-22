//! coach.rs — "training wheels" for the branching paradigm. Pure, headless, testable.
//!
//! Why this exists: Logscope's power (forking a conversation, fanning N branches off a leaf, letting the
//! toolchain GRADE them, then merging the winner) is invisible to a new user who types into the box and
//! reads one reply like every other chat app. The wizard frames the vision once on first run; this is the
//! other half — progressive disclosure DURING use. Each capability is taught exactly when it first becomes
//! relevant (never tell someone about fan-out before they've made a single branch), then never again once
//! they've done it. The wheels come off on their own.
//!
//! Design: this module decides ONLY *which lesson is eligible now*. It owns no UI and no nagging policy —
//! it emits a lesson, the caller offers it through the existing `cards::CardQueue` (CardKind::Note), and the
//! queue's content-keyed dedup + dismissal memory handle "don't stack" and "user said no, never again". So
//! the coach can be re-run every frame for free: eligibility is cheap and the queue suppresses repeats.
//!
//! Persistence: `LearnState` is serde-serializable so learned skills survive across sessions — a returning
//! user doesn't get re-taught branching. Persist it next to the other per-user UI state.

use serde::{Deserialize, Serialize};

/// The capabilities a user discovers, roughly in the order they become useful. Each is a bit in
/// `LearnState.learned`; a lesson for a skill stops being offered once its bit is set (the user did it)
/// or once the card queue records a dismissal for its key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Skill {
    Branch = 0,   // fork the conversation (edit-&-rerun, or interject at a node) → :branch
    Spatial = 1,  // see the tree instead of a linear strand → :spatial
    FanOut = 2,   // expand every leaf at once → :continue-all (:ca)
    Fuzz = 3,     // N branches that differ by STRATEGY, not wording → :fuzz N
    Verify = 4,   // let the compiler/tests grade branches, pick the proven one → :check / :best
    Compact = 5,  // keep a long conversation inside the window → :compact
    FilesIn = 6,  // bring real files in → :import-workspace / :summarize-files
    Decompose = 7,// split a long-horizon task into subtasks → :quest / :todo
    Ship = 8,     // turn the winning branch into files/a patch → :export / :assemble
}

impl Skill {
    fn bit(self) -> u32 { 1 << (self as u32) }
    /// All skills, in teaching order. Iteration order here defines lesson priority.
    pub const ALL: [Skill; 9] = [
        Skill::Branch, Skill::Spatial, Skill::FanOut, Skill::Fuzz, Skill::Verify,
        Skill::Compact, Skill::FilesIn, Skill::Decompose, Skill::Ship,
    ];
}

/// A lightweight snapshot of what the user has done — enough to decide eligibility without reaching into
/// the chat state. The caller fills this each time it asks for a lesson.
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub model_loaded: bool,
    pub assistant_replies: usize, // completed assistant turns seen
    pub branches: usize,          // nodes with an explicit parent (forks the user made)
    pub leaves: usize,            // current branch tips
    pub total_msgs: usize,
    pub has_code_block: bool,     // a reply produced a fenced block (a candidate for verify/ship)
    pub files_present: bool,      // something has been mounted / imported / attached
}

/// Persistent record of which skills the user has demonstrated. Serde so the wheels stay off across
/// runs. `dismissed` lets the coach stand alone without `cards::CardQueue` (which isn't wired into the
/// app yet) — a lesson the user waves off is suppressed here, permanently, the same way the queue's
/// dismissal memory would have.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct LearnState {
    #[serde(default)] learned: u32,
    #[serde(default)] dismissed: u32,
    /// Master switch — a user can turn coaching off entirely (e.g. an expert importing a session).
    #[serde(default)] pub enabled_off: bool,
}

impl LearnState {
    pub fn new() -> Self { Self::default() }
    pub fn has(&self, s: Skill) -> bool { self.learned & s.bit() != 0 }
    pub fn is_dismissed(&self, s: Skill) -> bool { self.dismissed & s.bit() != 0 }
    /// Mark a skill demonstrated. Call from the command/UI paths the moment the user actually does the
    /// thing (e.g. on a successful `:branch`, mark Skill::Branch) — that's what retires its lesson.
    pub fn mark(&mut self, s: Skill) { self.learned |= s.bit(); }
    /// User waved the lesson off → never offer it again (distinct from learning it).
    pub fn dismiss(&mut self, s: Skill) { self.dismissed |= s.bit(); }
    /// A skill is settled once it's either been done or dismissed — either way, stop teaching it.
    pub fn settled(&self, s: Skill) -> bool { self.has(s) || self.is_dismissed(s) }
    /// How far along the ladder they are (0.0–1.0) — handy for a tiny progress pip in the UI.
    pub fn fraction(&self) -> f32 {
        Skill::ALL.iter().filter(|s| self.has(**s)).count() as f32 / Skill::ALL.len() as f32
    }
    pub fn all_settled(&self) -> bool { Skill::ALL.iter().all(|s| self.settled(*s)) }
}

/// A teachable moment, ready to hand to `CardQueue::suggest`.
#[derive(Clone, Debug)]
pub struct Lesson {
    pub skill: Skill,
    /// Stable content key for the card queue (dedup + dismissal). Unique per skill so a dismissed lesson
    /// never returns even though the coach re-evaluates every frame.
    pub key: &'static str,
    pub title: &'static str,
    pub body: &'static str,
    pub action: &'static str,       // accept-button label
    /// The command to drop on the cmdline when the user accepts (None → the lesson is informational).
    pub command: Option<&'static str>,
    pub score: f32,                 // queue priority — deliberately modest so real suggestions outrank tips
}

/// Is this skill's PREREQUISITE met — i.e. has the user reached the point where the tip is useful (and
/// not premature)? This is the whole "training wheels" feel: lessons surface in sequence as earned.
fn eligible(skill: Skill, p: &Progress) -> bool {
    match skill {
        // First real reply on screen → show them a conversation can fork.
        Skill::Branch => p.model_loaded && p.assistant_replies >= 1,
        // They've forked once → now the tree is worth seeing.
        Skill::Spatial => p.branches >= 1,
        // They understand a leaf → expanding all leaves at once makes sense.
        Skill::FanOut => p.branches >= 1 && p.leaves >= 2,
        // They've multiplied branches → teach behavioural (not sampling) diversity.
        Skill::Fuzz => p.branches >= 2 || p.leaves >= 3,
        // Several candidates exist AND at least one is gradeable → let the toolchain judge.
        Skill::Verify => p.leaves >= 2 && p.has_code_block,
        // The conversation is long enough to start straining the window.
        Skill::Compact => p.total_msgs >= 16,
        // They're clearly doing code work → bringing a real project in is the next lever.
        Skill::FilesIn => p.has_code_block && !p.files_present,
        // Real files are in and the task is substantial → split it for parallel workers.
        Skill::Decompose => p.files_present && p.total_msgs >= 6,
        // There's a verified/winning branch worth materializing.
        Skill::Ship => p.files_present && p.has_code_block && p.leaves >= 2,
    }
}

fn lesson_for(skill: Skill) -> Lesson {
    use Skill::*;
    match skill {
        Branch => Lesson { skill, key: "coach:branch", score: 0.40,
            title: "You can fork this conversation",
            body: "Don't start over to try a different angle. Edit any earlier message and re-run, or interject at a node — the old path is kept as a branch. This is the core move: explore alternatives without losing the thread.",
            action: "Show me", command: Some(":help branch") },
        Spatial => Lesson { skill, key: "coach:spatial", score: 0.36,
            title: "See the whole tree",
            body: "You've made a branch — the conversation is now a tree, not a line. The spatial view lays every branch out side by side so you can navigate and compare them.",
            action: "Open spatial view", command: Some(":spatial") },
        FanOut => Lesson { skill, key: "coach:fanout", score: 0.33,
            title: "Expand every branch at once",
            body: "Instead of continuing one tip at a time, fan all current leaves in parallel. One shared prefill, many continuations — the engine reuses the cached context so it's cheap.",
            action: "Try continue-all", command: Some(":continue-all") },
        Fuzz => Lesson { skill, key: "coach:fuzz", score: 0.30,
            title: "Make branches genuinely different",
            body: "Fanning with the same prompt just gives you N rewordings. :fuzz N gives each branch a different STRATEGY (incremental, hardest-first, a skeptic, a constraint) so you explore real approaches, not noise.",
            action: "Try fuzz", command: Some(":fuzz 4") },
        Verify => Lesson { skill, key: "coach:verify", score: 0.34,
            title: "Let the toolchain pick the winner",
            body: "You have branches that produced code. Don't eyeball them — :check builds/tests each in an isolated sandbox and :best surfaces the branch that actually passes. Best-of-K only means something when something grades K.",
            action: "Grade the branches", command: Some(":check") },
        Compact => Lesson { skill, key: "coach:compact", score: 0.22,
            title: "Keep the long conversation in-window",
            body: "This thread is getting long. :compact folds older turns into a summary while preserving the branch structure, so you stay under the context limit without losing the history.",
            action: "Compact", command: Some(":compact") },
        FilesIn => Lesson { skill, key: "coach:filesin", score: 0.28,
            title: "Bring a real project in",
            body: "You're writing code in chat — you can work on actual files instead. Import a workspace (or summarize a folder) and the assistant operates on a /tmp copy, never your real tree.",
            action: "Import a workspace", command: Some(":import-workspace") },
        Decompose => Lesson { skill, key: "coach:decompose", score: 0.26,
            title: "Split a big task across workers",
            body: "For a long-horizon job, decompose it: :quest / :todo break the goal into subtasks that branches can each take on, then the verify-and-merge loop assembles the proven pieces.",
            action: "Plan the work", command: Some(":quest") },
        Ship => Lesson { skill, key: "coach:ship", score: 0.24,
            title: "Turn the winning branch into files",
            body: "Once a branch is proven, :export materializes its fenced blocks into a workspace and :assemble bundles the result as a patch/zip you can apply on your own terms.",
            action: "Export the result", command: Some(":export") },
    }
}

/// The single most useful lesson to offer right now, or None when nothing is eligible (or coaching is off,
/// or everything's been learned). Offer the returned lesson via `CardQueue::suggest(lesson.key, ...)`; the
/// queue's dismissal memory means a lesson the user rejected won't come back even though we re-evaluate.
///
/// "Most useful" = earliest unlearned skill on the ladder whose prerequisite is met. Returning one at a
/// time (not a backlog) is deliberate: training wheels guide the next step, they don't dump a manual.
pub fn next_lesson(state: &LearnState, p: &Progress) -> Option<Lesson> {
    if state.enabled_off || state.all_settled() { return None; }
    Skill::ALL.iter().copied()
        .find(|&s| !state.settled(s) && eligible(s, p))
        .map(lesson_for)
}

impl Lesson {
    /// One-line form for the transient status surface (`self.note`): a tip marker, the title, and the
    /// command to run. Plain text — no emoji — to match the rest of the status line.
    pub fn status_line(&self) -> String {
        match self.command {
            Some(c) => format!("tip — {} · try {c}  (:help to dismiss coaching)", self.title),
            None => format!("tip — {}", self.title),
        }
    }
}

// ── stateful driver ─────────────────────────────────────────────────────────────────────────────
//
// `Coach` owns the persisted `LearnState` so the app can mark/dismiss and ask for the next lesson
// without managing serialization or depending on `cards::CardQueue`. Surface the returned lesson
// through the status line (`Lesson::status_line`) today, or hand it to a card queue if one gets wired.

fn coach_path() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("LOGSCOPE_COACH_FILE") { return Some(std::path::PathBuf::from(p)); }
    std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share/logscope").join("coach.json"))
}

fn load_state() -> LearnState {
    coach_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(st: &LearnState) -> std::io::Result<()> {
    let Some(path) = coach_path() else { return Ok(()); };
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let body = serde_json::to_string(st).unwrap_or_else(|_| "{}".into());
    // atomic-ish: write a tmp then rename, matching the rest of the store's write discipline.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)
}

/// The app holds one of these. `load()` on startup, then `learn`/`dismiss` from command/UI success
/// paths (each persists best-effort), and `lesson(&progress)` each frame to get the hint to show.
pub struct Coach { state: LearnState }

impl Coach {
    pub fn load() -> Self { Self { state: load_state() } }
    pub fn state(&self) -> LearnState { self.state }
    pub fn fraction(&self) -> f32 { self.state.fraction() }
    /// Turn coaching on/off (persisted). Wire to a settings toggle and to `:help` for a quick "stop".
    pub fn set_enabled(&mut self, on: bool) { self.state.enabled_off = !on; let _ = save_state(&self.state); }
    pub fn enabled(&self) -> bool { !self.state.enabled_off }
    /// The lesson to show now, or None.
    pub fn lesson(&self, p: &Progress) -> Option<Lesson> { next_lesson(&self.state, p) }
    /// Mark a skill demonstrated (retires its lesson) and persist.
    pub fn learn(&mut self, s: Skill) {
        if !self.state.has(s) { self.state.mark(s); let _ = save_state(&self.state); }
    }
    /// User waved the current lesson off — suppress it permanently and persist.
    pub fn dismiss(&mut self, s: Skill) {
        if !self.state.is_dismissed(s) { self.state.dismiss(s); let _ = save_state(&self.state); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog() -> Progress { Progress { model_loaded: true, ..Default::default() } }

    #[test]
    fn nothing_before_a_reply() {
        let s = LearnState::new();
        assert!(next_lesson(&s, &Progress::default()).is_none(), "no model, no reply → no coaching");
        assert!(next_lesson(&s, &prog()).is_none(), "model loaded but no reply yet → still nothing");
    }

    #[test]
    fn branch_is_taught_first() {
        let s = LearnState::new();
        let p = Progress { assistant_replies: 1, ..prog() };
        assert_eq!(next_lesson(&s, &p).unwrap().skill, Skill::Branch);
    }

    #[test]
    fn ladder_advances_as_skills_are_marked() {
        let mut s = LearnState::new();
        let mut p = Progress { assistant_replies: 1, ..prog() };
        assert_eq!(next_lesson(&s, &p).unwrap().skill, Skill::Branch);
        // user branches → Branch retires, and now the tree is worth seeing
        s.mark(Skill::Branch);
        p.branches = 1;
        assert_eq!(next_lesson(&s, &p).unwrap().skill, Skill::Spatial);
        // they open spatial and add another leaf → fan-out becomes eligible
        s.mark(Skill::Spatial);
        p.leaves = 2;
        assert_eq!(next_lesson(&s, &p).unwrap().skill, Skill::FanOut);
    }

    #[test]
    fn premature_lessons_do_not_fire() {
        let s = LearnState::new();
        // long thread but never branched: Compact is eligible, but Branch (earlier on the ladder) wins
        let p = Progress { assistant_replies: 1, total_msgs: 40, ..prog() };
        assert_eq!(next_lesson(&s, &p).unwrap().skill, Skill::Branch,
            "earliest eligible unlearned skill is taught first");
    }

    #[test]
    fn verify_needs_both_branches_and_code() {
        let s = LearnState::new();
        let only_branches = Progress { assistant_replies: 1, leaves: 2, ..prog() };
        assert!(!eligible(Skill::Verify, &only_branches), "no code block → don't push verify yet");
        let with_code = Progress { has_code_block: true, ..only_branches };
        assert!(eligible(Skill::Verify, &with_code));
    }

    #[test]
    fn off_switch_and_completion_silence_coaching() {
        let mut s = LearnState::new();
        let p = Progress { assistant_replies: 1, ..prog() };
        s.enabled_off = true;
        assert!(next_lesson(&s, &p).is_none(), "coaching disabled → nothing");
        s.enabled_off = false;
        for sk in Skill::ALL { s.mark(sk); }
        assert!(s.all_settled() && next_lesson(&s, &p).is_none(), "all learned → wheels off");
        assert!((s.fraction() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn dismiss_suppresses_a_lesson_without_marking_it_learned() {
        let mut s = LearnState::new();
        let p = Progress { assistant_replies: 1, ..prog() };
        assert_eq!(next_lesson(&s, &p).unwrap().skill, Skill::Branch);
        s.dismiss(Skill::Branch);
        assert!(s.settled(Skill::Branch) && !s.has(Skill::Branch), "dismissed, not learned");
        // Branch is suppressed; nothing later is eligible yet (no branch made), so coaching goes quiet.
        assert!(next_lesson(&s, &p).is_none(), "dismissed lesson is skipped");
        assert!(s.fraction() < 1e-6, "dismissing doesn't count as progress");
    }

    #[test]
    fn status_line_includes_the_command() {
        let l = lesson_for(Skill::Spatial);
        assert!(l.status_line().contains(":spatial"), "the tip tells the user what to type");
    }

    #[test]
    fn coach_persists_across_loads() {
        let tmp = std::env::temp_dir().join(format!("ls-coach-test-{}.json", std::process::id()));
        std::env::set_var("LOGSCOPE_COACH_FILE", &tmp);
        let _ = std::fs::remove_file(&tmp);
        {
            let mut c = Coach::load();
            c.learn(Skill::Branch);
            c.dismiss(Skill::Spatial);
        }
        let c2 = Coach::load(); // fresh load reads the file back
        assert!(c2.state().has(Skill::Branch), "learned skill survived");
        assert!(c2.state().is_dismissed(Skill::Spatial), "dismissal survived");
        let _ = std::fs::remove_file(&tmp);
        std::env::remove_var("LOGSCOPE_COACH_FILE");
    }
}
