//! liaison_fan.rs — the Liaison's voice for a running fan. Composes human sentences STRICTLY from
//! reported fan events (the HUD contract: never say anything the machinery did not report), so the
//! strip under the newest message can narrate a fan the way it narrates the engine.

#[derive(Clone, Debug, PartialEq)]
pub enum FanEvent {
    RoundStarted { round: usize, width: usize, styled: bool },
    BranchDone { label: String, chars: usize, reflexes: Vec<&'static str> },
    ReflexFollowUp { why: &'static str },
    PathsFanned { feature_preview: String, n: usize },
    FanFinished { branches: usize },
}

/// One sentence per event, deterministic, no speculation. The caller feeds these to the HUD with
/// source "liaison".
pub fn narrate(e: &FanEvent) -> String {
    match e {
        FanEvent::RoundStarted { round, width, styled } => format!(
            "fan round {round}: {width} branches{}", if *styled { ", each with its own work style" } else { "" }),
        FanEvent::BranchDone { label, chars, reflexes } => {
            if reflexes.is_empty() { format!("branch '{label}' done ({chars} chars)") }
            else { format!("branch '{label}' done ({chars} chars) — reflexes: {}", reflexes.join(", ")) }
        }
        FanEvent::ReflexFollowUp { why } => format!("reflex queued a follow-up ({why})"),
        FanEvent::PathsFanned { feature_preview, n } => format!(
            "exploring {n} feature-creation paths for '{feature_preview}'"),
        FanEvent::FanFinished { branches } => format!("fan finished — {branches} branches to compare (:pick / :merge-docs)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sentences_come_only_from_the_event() {
        assert_eq!(narrate(&FanEvent::RoundStarted { round: 1, width: 5, styled: true }),
                   "fan round 1: 5 branches, each with its own work style");
        assert_eq!(narrate(&FanEvent::ReflexFollowUp { why: "compile error" }),
                   "reflex queued a follow-up (compile error)");
        let s = narrate(&FanEvent::BranchDone { label: "explorer".into(), chars: 812,
                                                reflexes: vec!["test failure"] });
        assert!(s.contains("explorer") && s.contains("812") && s.contains("test failure"));
    }
}
