//! One turn, end to end.
//!
//! `ChatSession` owns the state machine and `Completer` owns the
//! model; this owns the *sequence* so the game does not have to. A
//! turn is: build the context, ask the model, clean the reply, parse
//! and validate, dry-run, and hand back either a preview or a reason
//! it was rejected. Commit stays separate and explicit, because the
//! whole point of the preview is that the player gets to say no.
//!
//! The game calls `turn(...)` off the frame thread and `commit(...)`
//! on it. Everything between is deterministic and tested.

use crate::chat::{ChatSession, Reply};
use crate::focus::Gaze;
use crate::llm::{clean_reply, Completer};
use crate::prompt::{system_prompt, Pass};
use crate::{VoxelTools, VoxelWorld};

/// What a turn produced, with enough detail for the chat log.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    /// A plan is pending. Show the echo; wait for /commit.
    Preview { echo: String, blocks: usize, pass: Pass },
    /// The model answered but nothing usable came back.
    NoPlan { raw_len: usize },
    /// The plan was parsed but rejected (errors, bad arithmetic).
    Rejected(String),
    /// The model itself failed: server down, timeout, bad response.
    ModelError(String),
}

impl TurnOutcome {
    /// One line for the chat log.
    pub fn message(&self) -> String {
        match self {
            TurnOutcome::Preview { blocks, pass, .. } =>
                format!("{} pass: {blocks} blocks planned -- /commit to build, /plan to review",
                        pass.name()),
            TurnOutcome::NoPlan { raw_len } =>
                format!("the model replied ({raw_len} chars) but produced no plan lines; \
                         try rephrasing, or /focus bigger if the request is larger than the view"),
            TurnOutcome::Rejected(why) => why.clone(),
            TurnOutcome::ModelError(e) =>
                format!("model unavailable: {e}  (is llama-server running? /model to check)"),
        }
    }
}

/// Run one build turn. `force_pass` overrides the inferred pass, for a
/// UI button or an explicit "expand this".
pub fn turn<W: VoxelWorld, C: Completer>(
    session: &mut ChatSession,
    tools: &mut VoxelTools<W>,
    completer: &mut C,
    gaze: &Gaze,
    surface_name: Option<&str>,
    prose: &str,
    player_feet: Option<(i32, i32, i32)>,
    force_pass: Option<Pass>,
) -> TurnOutcome {
    let (user, pass) = session.user_turn(tools, gaze, surface_name, prose, force_pass);
    let raw = match completer.complete(&system_prompt(), &user) {
        Ok(r) => r,
        Err(e) => return TurnOutcome::ModelError(e),
    };
    let ir = clean_reply(&raw);
    if ir.trim().is_empty() {
        return TurnOutcome::NoPlan { raw_len: raw.len() };
    }
    match session.receive_plan_for(tools, gaze, &ir, player_feet) {
        Reply::Preview { echo, blocks, .. } => TurnOutcome::Preview { echo, blocks, pass },
        Reply::Say(why) => TurnOutcome::Rejected(why),
        other => TurnOutcome::Rejected(format!("unexpected: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::focus::GazeHit;
    use crate::llm::{DeadCompleter, FakeCompleter};
    use std::collections::BTreeMap;

    struct Flat { cells: BTreeMap<(i32, i32, i32), u16> }
    impl Flat { fn new() -> Self { Self { cells: BTreeMap::new() } } }
    impl VoxelWorld for Flat {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.cells.insert((x, y, z), b); true }
    }
    fn gaze() -> Gaze {
        Gaze { eye: (0.0, 70.0, -10.0), dir: (0.0, -0.3, 1.0),
               hit: Some(GazeHit { block: (0, 64, 0), face_normal: (0, 1, 0), distance: 12.0 }) }
    }

    #[test]
    fn a_good_turn_previews_then_commits() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let mut c = FakeCompleter::new(vec![
            "Here you go:\n```\n# tower\ncylinder @0,0 r=3 h=9 block=stone hollow\n```\nEnjoy!".into()]);
        let out = turn(&mut s, &mut t, &mut c, &gaze(), Some("grass"), "a small tower", None, None);
        match &out {
            TurnOutcome::Preview { blocks, pass, .. } => {
                assert!(*blocks > 0);
                assert_eq!(*pass, Pass::Massing, "empty site -> massing");
            }
            o => panic!("{o:?}"),
        }
        assert!(out.message().contains("/commit"));
        // the model saw the context, not just the prose
        let (sys, user) = c.seen.borrow()[0].clone();
        assert!(sys.contains("BUILD IN PASSES"));
        assert!(user.contains("READ FIRST") && user.contains("player: a small tower"));
        // and nothing was built yet
        assert_eq!(t.world.get(3, 66, 0), 0);
        match s.commit(&mut t, &gaze()) {
            Reply::Committed { blocks, .. } => assert!(blocks > 0),
            o => panic!("{o:?}"),
        }
        assert_ne!(t.world.get(3, 66, 0), 0, "committed");
    }

    #[test]
    fn a_dead_server_is_a_message_not_a_hang() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let mut c = DeadCompleter("connection refused".into());
        let out = turn(&mut s, &mut t, &mut c, &gaze(), None, "a tower", None, None);
        assert_eq!(out, TurnOutcome::ModelError("connection refused".into()));
        assert!(out.message().contains("llama-server"), "tells the player what to check");
    }

    #[test]
    fn a_chatty_reply_with_no_plan_is_reported_not_silently_dropped() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let mut c = FakeCompleter::new(vec!["I'd love to help, but could you be more specific?".into()]);
        let out = turn(&mut s, &mut t, &mut c, &gaze(), None, "something nice", None, None);
        assert!(matches!(out, TurnOutcome::NoPlan { .. }), "{out:?}");
        assert!(out.message().contains("no plan lines"));
    }

    #[test]
    fn an_impossible_plan_is_rejected_with_the_reason() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        // `r=0` is now CLAMPED, not rejected -- a fumbled number should
        // build. What still fails is input with no recoverable meaning.
        let mut c = FakeCompleter::new(vec!["frobnicate the widget\n".into()]);
        let out = turn(&mut s, &mut t, &mut c, &gaze(), None, "a tower", None, None);
        match &out {
            // no known verb at all -> clean_reply drops it -> no plan
            TurnOutcome::NoPlan { .. } => {}
            TurnOutcome::Rejected(why) => assert!(why.contains("rejected"), "{why}"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn a_second_turn_becomes_a_detail_pass_on_the_last_build() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let mut c = FakeCompleter::new(vec![
            "# tower\ncylinder @0,0 r=3 h=9 block=stone hollow\n".into(),
            "sphere @3,0 r=1 block=air\n".into(),
        ]);
        turn(&mut s, &mut t, &mut c, &gaze(), None, "a tower", None, None);
        s.commit(&mut t, &gaze());
        let out = turn(&mut s, &mut t, &mut c, &gaze(), None, "add a door", None, None);
        match out {
            TurnOutcome::Preview { pass, .. } => assert_eq!(pass, Pass::Detail),
            o => panic!("{o:?}"),
        }
        // the second prompt carried the referent
        let (_, user) = c.seen.borrow()[1].clone();
        assert!(user.contains("LAST BUILD 'tower'"), "{user}");
        assert!(user.contains("PASS: detail"));
    }
}
