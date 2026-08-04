//! linearize.rs — a conversation tree is not a chat transcript, and the difference bites.
//!
//! `branchtree::path_to_root` walks a leaf back to the root correctly. What it returns is a PATH, and
//! every chat model expects something stricter: **a strictly alternating user/assistant sequence
//! beginning with a system or user turn.**
//!
//! A tree produces paths that violate that routinely, without any bug in the walk:
//!
//! * a regenerate leaves two assistant nodes in a row
//! * an edited user turn leaves two user nodes in a row
//! * a branch taken from mid-conversation can start on an assistant turn
//! * a cancelled generation leaves an EMPTY assistant node
//!
//! The model's response to a malformed sequence is not an error. **It is a plausible reply to a
//! prompt that means something different from what the tree showed** — which is why this has to be
//! checked rather than trusted.
//!
//! ## On graph-native models
//!
//! There are none in production. Diffusion LLMs like LLaDA denoise in parallel but still over a
//! LINEAR position sequence; graph transformers exist in research and are not chat models. **The tree
//! must be flattened, so the flattening is where correctness lives.**

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role { System, User, Assistant }

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Turn { pub id: u64, pub role: Role, pub text: String }

/// What is wrong with a flattened path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Defect {
    /// Two turns of the same role in a row.
    Consecutive { role: Role, first_id: u64, second_id: u64 },
    /// An empty assistant turn — a cancelled or failed generation left in the tree.
    EmptyAssistant { id: u64 },
    /// The sequence begins on an assistant turn.
    StartsOnAssistant { id: u64 },
    /// A system turn somewhere other than the front.
    LateSystem { id: u64, position: usize },
    /// Two turns sharing an id. **The tree's own invariant broken**, and the one defect that means
    /// something upstream is wrong rather than merely unusual.
    DuplicateId { id: u64 },
}

impl Defect {
    pub fn explain(&self) -> String {
        match self {
            Defect::Consecutive { role, first_id, second_id } => format!(
                "two {role:?} turns in a row ({first_id} then {second_id}) — a regenerate or an \
edited branch. The model will read them as one turn and answer something the tree never showed"),
            Defect::EmptyAssistant { id } => format!(
                "assistant turn {id} is empty — a cancelled generation left in the tree. Sent as-is \
it teaches the model that empty replies are acceptable here"),
            Defect::StartsOnAssistant { id } => format!(
                "the sequence begins on assistant turn {id}, which most chat templates cannot \
represent at all"),
            Defect::LateSystem { id, position } => format!(
                "system turn {id} at position {position} — most templates only honour a system turn \
at the front, and elsewhere it is silently treated as user text"),
            Defect::DuplicateId { id } => format!(
                "turn id {id} appears twice — this is the TREE's invariant broken, not a formatting \
quirk, and nothing downstream can repair it safely"),
        }
    }

    /// Can this be repaired without inventing content?
    ///
    /// A duplicate id cannot: repairing it means choosing which turn is real, and a wrong choice is
    /// worse than a refusal.
    pub fn repairable(&self) -> bool { !matches!(self, Defect::DuplicateId { .. }) }
}

/// Check a flattened path.
pub fn check(turns: &[Turn]) -> Vec<Defect> {
    let mut v = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for (i, t) in turns.iter().enumerate() {
        if !ids.insert(t.id) { v.push(Defect::DuplicateId { id: t.id }); }
        if t.role == Role::Assistant && t.text.trim().is_empty() {
            v.push(Defect::EmptyAssistant { id: t.id });
        }
        if t.role == Role::System && i > 0 {
            v.push(Defect::LateSystem { id: t.id, position: i });
        }
        if i == 0 && t.role == Role::Assistant {
            v.push(Defect::StartsOnAssistant { id: t.id });
        }
        if i > 0 && turns[i - 1].role == t.role && t.role != Role::System {
            v.push(Defect::Consecutive { role: t.role, first_id: turns[i - 1].id, second_id: t.id });
        }
    }
    v
}

/// Repair what can be repaired, and report what was dropped.
///
/// **Drops rather than merges.** Merging two assistant turns invents a reply neither model produced;
/// dropping the earlier one is the truthful choice, because the later turn is the one the user saw
/// and responded to.
pub fn repair(turns: &[Turn]) -> (Vec<Turn>, Vec<String>) {
    let mut out: Vec<Turn> = Vec::new();
    let mut notes = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for t in turns {
        if !seen.insert(t.id) {
            notes.push(format!("dropped duplicate id {}", t.id));
            continue;
        }
        if t.role == Role::Assistant && t.text.trim().is_empty() {
            notes.push(format!("dropped empty assistant turn {}", t.id));
            continue;
        }
        if t.role == Role::System && !out.is_empty() {
            notes.push(format!("dropped late system turn {} — templates only honour it at the front",
                t.id));
            continue;
        }
        if let Some(prev) = out.last() {
            if prev.role == t.role && t.role != Role::System {
                // The LATER turn wins: it is the one the user saw and responded to. Keeping the
                // earlier would replay a superseded generation as though it were current.
                notes.push(format!("replaced superseded {:?} turn {} with {}", t.role, prev.id, t.id));
                out.pop();
            }
        }
        out.push(t.clone());
    }
    while out.first().is_some_and(|t| t.role == Role::Assistant) {
        notes.push(format!("dropped leading assistant turn {}", out[0].id));
        out.remove(0);
    }
    (out, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u64, role: Role, text: &str) -> Turn { Turn { id, role, text: text.into() } }

    #[test]
    fn a_REGENERATE_leaves_two_assistant_turns_and_is_caught() {
        // The common case, and no bug in the tree walk produces it — the tree is correct and the
        // linear form it yields is not.
        let d = check(&[t(1, Role::User, "hi"), t(2, Role::Assistant, "a"), t(3, Role::Assistant, "b")]);
        assert!(matches!(d[0], Defect::Consecutive { role: Role::Assistant, .. }));
        assert!(d[0].explain().contains("answer something the tree never showed"));
    }

    #[test]
    fn repair_keeps_the_LATER_of_two_same_role_turns() {
        // The later one is what the user saw and responded to; keeping the earlier replays a
        // superseded generation as though it were current.
        let (out, notes) = repair(&[t(1, Role::User, "hi"), t(2, Role::Assistant, "old"),
                                    t(3, Role::Assistant, "new")]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].id, 3);
        assert!(notes[0].contains("superseded"));
    }

    #[test]
    fn an_EMPTY_assistant_turn_is_dropped() {
        // A cancelled generation left in the tree. Sent as-is it teaches the model that empty replies
        // are acceptable here.
        let (out, notes) = repair(&[t(1, Role::User, "hi"), t(2, Role::Assistant, "   ")]);
        assert_eq!(out.len(), 1);
        assert!(notes[0].contains("empty assistant"));
    }

    #[test]
    fn a_DUPLICATE_ID_is_reported_as_a_TREE_bug_and_not_merely_a_quirk() {
        // The one defect that means something upstream is wrong. Repair drops it, but the check says
        // plainly that nothing downstream can fix it safely.
        let d = check(&[t(1, Role::User, "a"), t(1, Role::Assistant, "b")]);
        let dup = d.iter().find(|x| matches!(x, Defect::DuplicateId { .. })).unwrap();
        assert!(!dup.repairable());
        assert!(dup.explain().contains("TREE's invariant broken"));
    }

    #[test]
    fn a_path_STARTING_on_an_assistant_turn_is_caught_and_trimmed() {
        // A branch taken from mid-conversation. Most chat templates cannot represent it at all.
        assert!(matches!(check(&[t(1, Role::Assistant, "x")])[0], Defect::StartsOnAssistant { .. }));
        let (out, notes) = repair(&[t(1, Role::Assistant, "x"), t(2, Role::User, "hi")]);
        assert_eq!(out[0].role, Role::User);
        assert!(notes.iter().any(|n| n.contains("leading assistant")));
    }

    #[test]
    fn a_LATE_SYSTEM_turn_is_dropped_rather_than_silently_becoming_user_text() {
        // Most templates only honour a system turn at the front; elsewhere it is treated as user text
        // and quietly changes what the model was asked.
        let (out, notes) = repair(&[t(1, Role::User, "hi"), t(2, Role::System, "be terse")]);
        assert_eq!(out.len(), 1);
        assert!(notes[0].contains("only honour it at the front"));
    }

    #[test]
    fn a_WELL_FORMED_path_is_left_alone() {
        let good = vec![t(0, Role::System, "s"), t(1, Role::User, "a"),
                        t(2, Role::Assistant, "b"), t(3, Role::User, "c")];
        assert!(check(&good).is_empty());
        let (out, notes) = repair(&good);
        assert_eq!(out, good);
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn repair_REPORTS_everything_it_changed() {
        // A silent repair is a lie about what the model was sent, which is the failure this module
        // exists to prevent rather than commit.
        let (_, notes) = repair(&[t(1, Role::Assistant, ""), t(2, Role::User, "a"),
                                  t(3, Role::User, "b")]);
        assert_eq!(notes.len(), 2, "{notes:?}");
    }
}
