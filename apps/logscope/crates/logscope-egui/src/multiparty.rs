//! multiparty.rs — generalized conversation assembly. The conversation graph is no longer an
//! alternating user/assistant chain: it's a tree of messages each labelled with an AUTHOR (a
//! participant — a specific human or a specific agent), and a prompt is assembled from any
//! participant's PERSPECTIVE. This is the primitive under aggressive fan-out, multi-user rooms, and
//! multi-agent interaction: many participants can append after any node, consecutive same-role
//! messages are allowed (and merged), and each agent generates from its own point of view.
//!
//! Perspective rule: when participant `me` is the one about to speak, `me`'s own messages map to the
//! "assistant" role and everyone else's to "user". When more than one *other* participant appears in
//! the lineage, each non-me block is name-tagged ("Alice: …") so the model can attribute turns; with a
//! single other participant it's the ordinary two-party transcript and no tags are added. Consecutive
//! messages that map to the same (role, tag) are merged into one template block — that is what makes a
//! run of same-author messages read as one chained turn instead of N adjacent blocks.
//!
//! This module is the ASSEMBLY half only (pure, std-only, unit-tested). The lineage it takes is the
//! root→head path the existing tree already produces (`convograph::active_path`); wiring it in means
//! generalizing a message's `role` to an author id and calling `assemble` instead of the hardcoded
//! binary mapping in the per-template builders. The one structural change it does NOT cover is
//! multi-PARENT context (merging two separate branches into one node) — that needs `parent:
//! Option<usize>` to become a set, and is the same primitive as the consensus 2-parent merge.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Template { ChatML, Llama3, Plain }

/// A participant in the conversation: a stable id, a display name, and whether it's an agent (model)
/// or a human. `is_agent` doesn't affect assembly (only `me` vs not-me does) but the room/UI needs it
/// to decide who can be asked to generate and how to colour turns.
#[derive(Clone, Debug)]
pub struct Participant { pub id: u32, pub name: String, pub is_agent: bool }

/// Assemble a prompt from `lineage` (author id, message text) in order, from participant `me`'s view.
/// `name(id)` resolves a participant's display name (used only when tagging is needed). `sys` is the
/// system prompt. Pure: same inputs → same string.
pub fn assemble(lineage: &[(u32, &str)], me: u32, name: impl Fn(u32) -> String, sys: &str, tmpl: Template) -> String {
    // Tag turns only when the lineage genuinely involves more than one *other* participant — otherwise
    // it's the ordinary two-party case and tags would just be noise the model wasn't trained on.
    let others: std::collections::BTreeSet<u32> = lineage.iter().map(|&(a, _)| a).filter(|&a| a != me).collect();
    let tag = others.len() > 1;

    // Map each message to (is_assistant, tag_label), then merge consecutive messages sharing both.
    let mut blocks: Vec<(bool, String, String)> = Vec::new(); // (assistant, label, body)
    for &(author, text) in lineage {
        let assistant = author == me;
        let label = if tag && !assistant { name(author) } else { String::new() };
        match blocks.last_mut() {
            Some(b) if b.0 == assistant && b.1 == label => { b.2.push_str("\n\n"); b.2.push_str(text); }
            _ => blocks.push((assistant, label, text.to_string())),
        }
    }

    let labelled = |label: &str, body: &str| if label.is_empty() { body.to_string() } else { format!("{label}: {body}") };
    let mut s = String::new();
    match tmpl {
        Template::ChatML => {
            s.push_str(&format!("<|im_start|>system\n{sys}<|im_end|>\n"));
            for (assistant, label, body) in &blocks {
                let role = if *assistant { "assistant" } else { "user" };
                s.push_str(&format!("<|im_start|>{role}\n{}<|im_end|>\n", labelled(label, body)));
            }
            s.push_str("<|im_start|>assistant\n");
        }
        Template::Llama3 => {
            s.push_str(&format!("<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\n{sys}<|eot_id|>"));
            for (assistant, label, body) in &blocks {
                let role = if *assistant { "assistant" } else { "user" };
                s.push_str(&format!("<|start_header_id|>{role}<|end_header_id|>\n\n{}<|eot_id|>", labelled(label, body)));
            }
            s.push_str("<|start_header_id|>assistant<|end_header_id|>\n\n");
        }
        Template::Plain => {
            s.push_str(&format!("System: {sys}\n"));
            for (assistant, label, body) in &blocks {
                let tagword = if *assistant { "Assistant".to_string() } else if label.is_empty() { "User".to_string() } else { label.clone() };
                s.push_str(&format!("{tagword}: {body}\n"));
            }
            s.push_str("Assistant:");
        }
    }
    s
}

/// Convenience: the distinct non-`me` participants appearing in a lineage (who is "in the room" from
/// me's view), in id order — useful for UI and for deciding turn-taking.
pub fn other_participants(lineage: &[(u32, &str)], me: u32) -> Vec<u32> {
    let set: std::collections::BTreeSet<u32> = lineage.iter().map(|&(a, _)| a).filter(|&a| a != me).collect();
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn nm(id: u32) -> String { ["human", "alice", "bob", "carol"].get(id as usize).map(|s| s.to_string()).unwrap_or(format!("p{id}")) }

    // Two-party (human=0, agent=1), assembling from the agent's view: ordinary alternating transcript,
    // NO name tags, agent's turn is "assistant".
    #[test]
    fn two_party_no_tags_matches_plain_alternation() {
        let lin = [(0u32, "hi"), (1, "hello"), (0, "how are you")];
        let out = assemble(&lin, 1, nm, "SYS", Template::ChatML);
        let expect = "<|im_start|>system\nSYS<|im_end|>\n\
                      <|im_start|>user\nhi<|im_end|>\n\
                      <|im_start|>assistant\nhello<|im_end|>\n\
                      <|im_start|>user\nhow are you<|im_end|>\n\
                      <|im_start|>assistant\n";
        assert_eq!(out, expect);
    }

    // Non-alternating: two consecutive human messages merge into ONE user block (chained context).
    #[test]
    fn consecutive_same_role_merges() {
        let lin = [(0u32, "first thought"), (0, "second thought"), (1, "reply")];
        let out = assemble(&lin, 1, nm, "S", Template::ChatML);
        assert!(out.contains("<|im_start|>user\nfirst thought\n\nsecond thought<|im_end|>"), "got: {out}");
        // exactly one user block before the assistant reply
        assert_eq!(out.matches("<|im_start|>user").count(), 1);
    }

    // Multi-agent: human=0, alice=1, bob=2. From ALICE's view: her turns are assistant (untagged),
    // human's and bob's are user and tagged (since >1 other participant).
    #[test]
    fn multi_agent_perspective_tags_others() {
        let lin = [(0u32, "kick off"), (1, "alice idea"), (2, "bob idea"), (0, "human follow")];
        let out = assemble(&lin, 1, nm, "S", Template::ChatML);
        assert!(out.contains("<|im_start|>user\nhuman: kick off<|im_end|>"), "got: {out}");
        assert!(out.contains("<|im_start|>assistant\nalice idea<|im_end|>"), "alice untagged-assistant; got: {out}");
        assert!(out.contains("<|im_start|>user\nbob: bob idea<|im_end|>"), "bob tagged-user; got: {out}");
        // bob and human are both "user" but different authors → NOT merged (distinct tags)
        assert_eq!(out.matches("<|im_start|>user").count(), 3);
    }

    // Same lineage from BOB's view: bob's turn becomes the assistant; alice is now a tagged user.
    #[test]
    fn perspective_flips_with_me() {
        let lin = [(0u32, "kick off"), (1, "alice idea"), (2, "bob idea"), (0, "human follow")];
        let out = assemble(&lin, 2, nm, "S", Template::ChatML);
        assert!(out.contains("<|im_start|>assistant\nbob idea<|im_end|>"), "got: {out}");
        assert!(out.contains("<|im_start|>user\nalice: alice idea<|im_end|>"), "got: {out}");
    }

    // Single other participant → no tags even across templates (Llama3 here).
    #[test]
    fn llama3_two_party_untagged() {
        let lin = [(0u32, "q"), (1, "a")];
        let out = assemble(&lin, 1, nm, "S", Template::Llama3);
        assert!(out.starts_with("<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\nS<|eot_id|>"));
        assert!(out.contains("<|start_header_id|>user<|end_header_id|>\n\nq<|eot_id|>"));
        assert!(out.contains("<|start_header_id|>assistant<|end_header_id|>\n\na<|eot_id|>"));
        assert!(out.ends_with("<|start_header_id|>assistant<|end_header_id|>\n\n"));
        assert!(!out.contains("human:"));
    }

    // Plain template, multi-user (two humans + one agent). From the agent's (id=2) view both humans are
    // tagged users; consecutive turns from the SAME human merge, different humans don't.
    #[test]
    fn plain_multi_user_tags_and_merges() {
        let lin = [(0u32, "h0 a"), (0, "h0 b"), (1, "h1 a"), (2, "agent")];
        let out = assemble(&lin, 2, nm, "S", Template::Plain);
        assert!(out.contains("human: h0 a\n\nh0 b\n"), "same human merged; got: {out}");
        assert!(out.contains("alice: h1 a\n"), "other human tagged separately; got: {out}");
        assert!(out.contains("Assistant: agent\n"));
        assert!(out.ends_with("Assistant:"));
    }

    #[test]
    fn other_participants_lists_non_me() {
        let lin = [(0u32, "a"), (1, "b"), (2, "c"), (1, "d")];
        assert_eq!(other_participants(&lin, 1), vec![0, 2]);
    }

    // Empty lineage still yields a valid open prompt (just system + assistant cue).
    #[test]
    fn empty_lineage_opens_assistant() {
        let out = assemble(&[], 1, nm, "S", Template::ChatML);
        assert_eq!(out, "<|im_start|>system\nS<|im_end|>\n<|im_start|>assistant\n");
    }
}
