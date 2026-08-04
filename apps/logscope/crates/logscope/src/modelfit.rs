//! modelfit.rs — which roles a model is SUITED for, rather than which one it was assigned.
//!
//! `deploy::Role` names six jobs and a `Slot` binds one model to one of them. That is the right
//! runtime shape and the wrong authoring shape: **a model is not a role, it has aptitudes**, and the
//! same 3B can be the Draft, the Delegate and the Embed at once while a 109B can only sensibly be the
//! Target.
//!
//! Forcing one choice per model means the operator assigns by hand, gets it wrong in ways that are
//! invisible (a Draft larger than its Target makes speculation a net loss), and has to redo it when
//! the task changes.
//!
//! ## What "suited" means here
//!
//! Not a benchmark. Three observable properties — parameter count, whether it fits resident, and its
//! modality — decide most of it, and the cases they do not decide are the ones worth asking about.

use crate::deploy::Role;

/// What is known about a model without running it.
#[derive(Clone, PartialEq, Debug)]
pub struct ModelFacts {
    pub name: String,
    /// Billions of parameters.
    pub params_b: f32,
    /// GiB it would occupy resident.
    pub resident_gib: f32,
    /// Text, or a separate modality.
    pub modality: Modality,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Modality { Text, Image, Speech, Embedding }

/// How well a model suits a role, 0..1. **Zero means unsuitable, not merely worse** — a speech model
/// is not a bad Target, it cannot be one.
pub fn fitness(m: &ModelFacts, role: Role, ceiling_gib: f32) -> f32 {
    let fits = m.resident_gib <= ceiling_gib;
    match (role, m.modality) {
        (Role::Voice, Modality::Speech) => 1.0,
        (Role::Image, Modality::Image) => 1.0,
        (Role::Embed, Modality::Embedding) => 1.0,
        // A modality mismatch is disqualifying, not a penalty.
        (Role::Voice | Role::Image, _) => 0.0,
        (_, Modality::Speech | Modality::Image) => 0.0,
        // An EMBEDDING model has no decoder, so it cannot generate at all — the compiler caught this
        // gap and it is a real one: a retrieval model is not a weak Target, it is not a Target.
        (_, Modality::Embedding) => 0.0,
        (Role::Embed, Modality::Text) => if m.params_b < 3.0 { 0.5 } else { 0.1 },
        (Role::Target, Modality::Text) => {
            // Capability dominates, but a Target that does not fit runs streamed, and this project
            // has measured what that costs: 99.9% of the step spent waiting.
            let cap = (m.params_b / 30.0).min(1.0);
            if fits { cap } else { cap * 0.4 }
        }
        (Role::Draft, Modality::Text) => {
            // A Draft must be SMALL. Speculation wins only if drafting is much cheaper than
            // verifying, so a large draft is worse than none — and that failure is invisible at
            // assignment time, which is why it is scored rather than left to the operator.
            if m.params_b > 8.0 { 0.0 } else { 1.0 - (m.params_b / 10.0) }
        }
        (Role::Delegate, Modality::Text) => {
            // Summarising a file window: small and resident beats capable. This is the readahead
            // role, where latency hides but throughput does not.
            if !fits { 0.2 } else if m.params_b <= 8.0 { 1.0 - (m.params_b / 20.0) } else { 0.3 }
        }
    }
}

/// A role filled, with the reason.
#[derive(Clone, PartialEq, Debug)]
pub struct Assignment {
    pub role: Role,
    pub model: Option<String>,
    /// Why this model, or why none — shown instead of "model loaded".
    pub because: String,
}

/// Fill every role from the available models.
///
/// **A model may take several roles.** That is the change: the same small model can draft, delegate
/// and embed, and refusing to let it do so leaves roles empty for no reason.
pub fn assign(models: &[ModelFacts], ceiling_gib: f32) -> Vec<Assignment> {
    [Role::Target, Role::Draft, Role::Delegate, Role::Embed, Role::Image, Role::Voice]
        .into_iter().map(|role| {
            let best = models.iter()
                .map(|m| (m, fitness(m, role, ceiling_gib)))
                .filter(|(_, f)| *f > 0.0)
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            match best {
                Some((m, f)) => Assignment {
                    role, model: Some(m.name.clone()),
                    because: format!("{} — {:.0}B, {:.1} GiB, best fit at {:.0}%",
                        m.name, m.params_b, m.resident_gib, f * 100.0),
                },
                None => Assignment {
                    role, model: None,
                    because: match role {
                        Role::Draft => "no model under 8B — speculation needs a draft much cheaper \
than the target, and a large one is worse than none".into(),
                        Role::Voice => "no speech model loaded".into(),
                        Role::Image => "no image model loaded".into(),
                        _ => "no suitable model among those loaded".into(),
                    },
                },
            }
        }).collect()
}

/// What changed between two assignments, in words.
///
/// **This replaces "model loaded".** That message says something happened and nothing about what — so
/// an operator who has just switched task cannot tell whether the roles they care about moved.
pub fn describe_change(before: &[Assignment], after: &[Assignment]) -> Vec<String> {
    let mut out = Vec::new();
    for a in after {
        let prev = before.iter().find(|b| b.role == a.role);
        match (prev.and_then(|p| p.model.as_ref()), a.model.as_ref()) {
            (Some(p), Some(n)) if p != n =>
                out.push(format!("{:?}: {p} → {n} ({})", a.role, a.because)),
            (None, Some(n)) => out.push(format!("{:?}: now {n} ({})", a.role, a.because)),
            // A role losing its model is the one an operator most needs told, because everything
            // still appears to work — it is just quietly slower or less capable.
            (Some(p), None) => out.push(format!(
                "{:?}: LOST {p} — {}. Work in this role now falls back to the target.",
                a.role, a.because)),
            _ => {}
        }
    }
    if out.is_empty() { out.push("no role changed".into()); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, b: f32, gib: f32) -> ModelFacts {
        ModelFacts { name: name.into(), params_b: b, resident_gib: gib, modality: Modality::Text }
    }

    #[test]
    fn ONE_MODEL_can_hold_SEVERAL_roles() {
        // The change being asked for. Refusing this leaves roles empty for no reason.
        let a = assign(&[m("qwen3-3b", 3.0, 2.0)], 31.0);
        let held: Vec<_> = a.iter().filter(|x| x.model.is_some()).map(|x| x.role).collect();
        assert!(held.len() >= 3, "one small model should cover target, draft and delegate: {held:?}");
    }

    #[test]
    fn a_DRAFT_larger_than_useful_is_scored_ZERO_not_merely_worse() {
        // Speculation wins only if drafting is much cheaper than verifying, so a large draft is worse
        // than none — and that failure is invisible at assignment time, which is why it is scored
        // rather than left to the operator.
        assert_eq!(fitness(&m("big", 30.0, 20.0), Role::Draft, 31.0), 0.0);
        assert!(fitness(&m("small", 1.5, 1.0), Role::Draft, 31.0) > 0.8);
    }

    #[test]
    fn a_TARGET_that_does_not_fit_is_penalised_but_not_disqualified() {
        // Streaming works and this project has measured what it costs — 99.9% of the step waiting —
        // so it is a heavy penalty and not a refusal.
        let f = fitness(&m("scout", 109.0, 56.6), Role::Target, 31.0);
        assert!(f > 0.0 && f < 0.5, "got {f}");
    }

    #[test]
    fn a_MODALITY_mismatch_is_disqualifying() {
        // A speech model is not a bad Target, it cannot be one.
        let whisper = ModelFacts { name: "whisper".into(), params_b: 1.5, resident_gib: 1.0,
                                   modality: Modality::Speech };
        assert_eq!(fitness(&whisper, Role::Target, 31.0), 0.0);
        assert_eq!(fitness(&whisper, Role::Voice, 31.0), 1.0);
    }

    #[test]
    fn an_UNFILLED_role_says_WHY_rather_than_going_blank() {
        let a = assign(&[m("scout", 109.0, 56.6)], 31.0);
        let draft = a.iter().find(|x| x.role == Role::Draft).unwrap();
        assert!(draft.model.is_none());
        assert!(draft.because.contains("worse than none"));
    }

    #[test]
    fn the_change_message_says_WHAT_and_WHY_not_model_loaded() {
        // The explicit replacement for a message that says something happened and nothing about what.
        let before = assign(&[m("qwen3-3b", 3.0, 2.0)], 31.0);
        let after = assign(&[m("qwen3-3b", 3.0, 2.0), m("scout", 109.0, 56.6)], 31.0);
        let msgs = describe_change(&before, &after);
        assert!(msgs.iter().any(|s| s.contains("Target") && s.contains("scout")), "{msgs:?}");
        assert!(msgs.iter().any(|s| s.contains("best fit at")));
    }

    #[test]
    fn LOSING_a_role_is_reported_most_loudly() {
        // Everything still appears to work; it is just quietly slower or less capable, which is
        // exactly the kind of change an operator needs told.
        let before = assign(&[m("qwen3-3b", 3.0, 2.0)], 31.0);
        let after = assign(&[], 31.0);
        let msgs = describe_change(&before, &after);
        assert!(msgs.iter().any(|s| s.contains("LOST") && s.contains("falls back to the target")));
    }

    #[test]
    fn NO_CHANGE_says_so_rather_than_printing_the_whole_table() {
        let a = assign(&[m("qwen3-3b", 3.0, 2.0)], 31.0);
        assert_eq!(describe_change(&a, &a), vec!["no role changed".to_string()]);
    }
}


#[cfg(test)]
mod modality_tests {
    use super::*;

    #[test]
    fn an_EMBEDDING_model_cannot_generate() {
        // Caught by the compiler rather than by me: I wrote arms for speech and image and forgot that
        // an embedding model has no decoder either. A retrieval model is not a weak Target.
        let e = ModelFacts { name: "bge".into(), params_b: 0.3, resident_gib: 0.2,
                             modality: Modality::Embedding };
        assert_eq!(fitness(&e, Role::Target, 31.0), 0.0);
        assert_eq!(fitness(&e, Role::Draft, 31.0), 0.0);
        assert_eq!(fitness(&e, Role::Delegate, 31.0), 0.0);
        assert_eq!(fitness(&e, Role::Embed, 31.0), 1.0, "and it is perfect at the one it is for");
    }
}
