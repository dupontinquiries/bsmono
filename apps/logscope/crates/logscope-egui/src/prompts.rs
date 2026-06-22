//! prompts.rs — a library of small, SPECIALIZED prompts to pull from, plus the throwaway-model command
//! suggester.
//!
//! Local models get hitched when a prompt is broad ("help with this"). A tight, single-purpose prompt
//! ("cut this in half, keep every decision") keeps a small model on rails. This module centralizes those
//! prompts as a keyed registry so commands — and the suggester — can pull the right one, and so the set can
//! grow without scattering string literals across the codebase. The longer workflow prompts that are already
//! wired (`vimmode::STRATEGIST_PROMPT`, `refresh::TEACHER_PROMPT`, `race::DIAGNOSE_PROMPT`,
//! `vimmode::SEGMENTER_PROMPT`) stay where they're used; this is the expansion the small models reach for.

#[derive(Clone, Copy, Debug)]
pub struct Prompt {
    pub key: &'static str,    // stable id, e.g. "tighten"
    pub title: &'static str,  // short label for a menu
    pub summary: &'static str,// one line: what it's for
    pub body: &'static str,   // the instruction handed to the model
}

/// The specialized prompt library. Each body is deliberately narrow so a small model stays on task.
pub const PROMPTS: &[Prompt] = &[
    Prompt { key: "next-step", title: "Next step", summary: "propose the single most useful next action",
        body: "Read the conversation and propose the ONE most useful next action, in a single sentence, then a one-line reason. Be concrete and specific to what's here. Do not list options." },
    Prompt { key: "unstick", title: "Unstick", summary: "reframe a stuck/looping task and move it forward",
        body: "The work has stalled or is looping. Restate the actual goal in one sentence, name the single thing blocking progress, and give one concrete move that gets unstuck. No preamble, no apology." },
    Prompt { key: "tighten", title: "Tighten", summary: "cut length in half, keep every decision",
        body: "Rewrite the text to roughly HALF its length. Preserve every decision, constraint, number, and named entity; cut hedging, repetition, and throat-clearing. Return only the rewrite." },
    Prompt { key: "simplify", title: "Simplify", summary: "make it simpler without changing behavior",
        body: "Rewrite to be as simple and direct as possible WITHOUT changing meaning or behavior. Prefer plain words and short sentences. Return only the rewrite." },
    Prompt { key: "critique", title: "Critique", summary: "the 3 weakest points and their fixes",
        body: "Identify the THREE weakest points in what's above — gaps, risks, or errors — and for each give one concrete fix. Terse bullets. No praise." },
    Prompt { key: "steelman", title: "Steelman", summary: "the strongest case FOR the position",
        body: "Make the strongest, most charitable case FOR the position under discussion — the version its best advocate would give. Terse. Do not rebut it." },
    Prompt { key: "devil", title: "Devil's advocate", summary: "the strongest case AGAINST",
        body: "Make the strongest case AGAINST the current direction — the most serious objections a sharp critic would raise. Terse bullets. Do not soften them." },
    Prompt { key: "test-plan", title: "Test plan", summary: "the key tests this needs",
        body: "List the key tests the code/design above needs to be trustworthy: the happy path, the edges, and the failure modes. One terse line each, no scaffolding code." },
    Prompt { key: "explain-error", title: "Explain error", summary: "what the error means + smallest fix",
        body: "Explain what the error above actually means in one or two sentences, then give the SMALLEST change that fixes it. No general advice." },
    Prompt { key: "name", title: "Name things", summary: "suggest clear names",
        body: "Suggest clear, conventional names for the things above (functions, variables, modules, concepts). For each: the name and a 3–6 word reason. No prose." },
    Prompt { key: "extract-todos", title: "Extract TODOs", summary: "pull open items as terse bullets",
        body: "Scan the conversation and list every open task, decision still pending, or loose thread, as terse imperative bullets. Group nothing; just the list. Omit anything already resolved." },
    Prompt { key: "eli5", title: "Explain simply", summary: "explain to a smart non-expert",
        body: "Explain the topic above to a smart non-expert in a short paragraph, then one concrete analogy. No jargon unless you define it inline." },
    Prompt { key: "rubber-duck", title: "Rubber duck", summary: "ask the 3 questions that would unblock this",
        body: "Ask the THREE questions whose answers would most unblock the work above. Just the questions, ordered by leverage. Do not answer them." },
    Prompt { key: "decisions", title: "Decisions log", summary: "list decisions made + why",
        body: "List the decisions made so far in the conversation, each with the one-line reason it was chosen over the alternative. Terse. Facts only, no speculation." },
];

/// Look up a specialized prompt by key.
pub fn by_key(key: &str) -> Option<&'static Prompt> { PROMPTS.iter().find(|p| p.key == key) }
/// All prompt keys (for a menu / `:help`).
pub fn keys() -> Vec<&'static str> { PROMPTS.iter().map(|p| p.key).collect() }

// ---- the throwaway-model command suggester --------------------------------------------------------------

/// Instruction for the small, short-lived model that proposes context actions from what the user just looked
/// at. Kept narrow and format-locked so a tiny model produces parseable output.
pub const SUGGEST_COMMANDS: &str = "\
You suggest next commands for a branching-chat tool. Given the AVAILABLE COMMANDS and the user's RECENT \
MESSAGES, propose up to THREE commands the user most likely wants next. Rules: use ONLY commands from the \
list; pick what fits THIS context (e.g. a long thread → :compact; two diverging branches → :why; a vague \
result → :fuzz). Output one suggestion per line, exactly as `:command args — short reason`, nothing else — \
no numbering, no preamble.";

/// Assemble the suggester's input from the last few VIEWED messages (role, text) and the available command
/// names. Callers pass the three messages currently on screen.
pub fn suggester_input(recent: &[(String, String)], commands: &[&str]) -> String {
    let mut s = String::from(SUGGEST_COMMANDS);
    s.push_str("\n\nAVAILABLE COMMANDS:\n");
    s.push_str(&commands.iter().map(|c| format!(":{c}")).collect::<Vec<_>>().join("  "));
    s.push_str("\n\nRECENT MESSAGES:\n");
    for (role, text) in recent {
        let r = if role.eq_ignore_ascii_case("assistant") { "Assistant" } else { "User" };
        let t: String = text.chars().take(400).collect();
        s.push_str(&format!("\n[{r}] {}\n", t.trim()));
    }
    s
}

/// A parsed command suggestion from the suggester model.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion { pub command: String, pub rationale: String }

/// Parse the suggester's output (`:command args — reason` per line). Lines that don't start with `:` are
/// ignored, so a chatty model's stray prose is dropped. Both em-dash and " - " separate command from reason.
pub fn parse_suggestions(output: &str) -> Vec<Suggestion> {
    let mut out = Vec::new();
    for raw in output.lines() {
        let line = raw.trim().trim_start_matches(['-', '*', '•', ' ']).trim();
        if !line.starts_with(':') { continue; }
        let (cmd, why) = if let Some(i) = line.find('—') { (&line[..i], line[i + '—'.len_utf8()..].trim()) }
            else if let Some(i) = line.find(" - ") { (&line[..i], line[i + 3..].trim()) }
            else { (line, "") };
        let command = cmd.trim().to_string();
        if command.len() > 1 {
            out.push(Suggestion { command, rationale: why.to_string() });
        }
        if out.len() >= 3 { break; }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_is_keyed_and_nonempty() {
        assert!(PROMPTS.len() >= 12);
        assert!(by_key("tighten").is_some() && by_key("unstick").is_some());
        assert!(by_key("nope").is_none());
        for p in PROMPTS {
            assert!(!p.body.trim().is_empty() && !p.title.is_empty() && !p.summary.is_empty(), "{} fully populated", p.key);
        }
        // keys are unique
        let mut ks = keys(); ks.sort(); let n = ks.len(); ks.dedup();
        assert_eq!(ks.len(), n, "no duplicate keys");
    }

    #[test]
    fn suggester_input_carries_commands_and_messages() {
        let recent = vec![("user".into(), "this thread is getting really long".into())];
        let body = suggester_input(&recent, &["compact", "why", "fuzz"]);
        assert!(body.starts_with(SUGGEST_COMMANDS));
        assert!(body.contains(":compact") && body.contains(":why"));
        assert!(body.contains("really long"));
    }

    #[test]
    fn parse_suggestions_reads_lines_and_drops_prose() {
        let out = "Here are my picks:\n:compact 0.3 — the thread is long\n- :why -1,-2 — the branches diverge\nrandom chatter\n:fuzz 8 - try a few variations\n:extra 1 — should be capped out";
        let s = parse_suggestions(out);
        assert_eq!(s.len(), 3, "max three, prose ignored");
        assert_eq!(s[0], Suggestion { command: ":compact 0.3".into(), rationale: "the thread is long".into() });
        assert_eq!(s[1].command, ":why -1,-2");
        assert_eq!(s[2].command, ":fuzz 8");
        assert_eq!(s[2].rationale, "try a few variations");
    }
}
