//! cmdpalette.rs — metadata for the `:` command palette: what each command does, the shape of its argument,
//! and how to complete it. Pure + headless so the help and the suggestions are testable; the cmdline UI just
//! renders what `matches()` and `suggest_args()` return.

/// The shape of a command's argument — drives the usage hint and the argument suggestions.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ArgKind { None, Ratio, Refs, Model, Path, Count, Free }

#[derive(Clone, Copy, Debug)]
pub struct CmdSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub help: &'static str,
    pub example: &'static str,
    pub arg: ArgKind,
}

/// The palette. Ordered roughly by how often they're reached for; `matches()` re-sorts alphabetically.
pub const SPECS: &[CmdSpec] = &[
    CmdSpec { name: "help", aliases: &["h", "?"], usage: ":help [command]", arg: ArgKind::Free,
        help: "list commands, or show one command's usage", example: ":help compact" },
    CmdSpec { name: "suggest", aliases: &["sg"], usage: ":suggest", arg: ArgKind::None,
        help: "ask the smallest local model what to do next from your last messages", example: ":suggest" },
    CmdSpec { name: "prompt", aliases: &["pr"], usage: ":prompt <key>", arg: ArgKind::Free,
        help: "stage a specialized prompt (tighten, unstick, critique, …) over recent turns", example: ":pr tighten" },
    CmdSpec { name: "compact", aliases: &["refresh", "cp"], usage: ":compact [ratio]", arg: ArgKind::Ratio,
        help: "fold older turns into a shorter briefing (more context, faster decode, a pinned KV checkpoint)", example: ":compact 0.3" },
    CmdSpec { name: "race", aliases: &[], usage: ":race [ratio]", arg: ArgKind::Ratio,
        help: "show the full-vs-compacted token tradeoff before committing to a compaction", example: ":race" },
    CmdSpec { name: "why", aliases: &["diagnose"], usage: ":why [a,b]", arg: ArgKind::Refs,
        help: "flag a bad-jump compaction; a tiny model diagnoses what the briefing dropped", example: ":why -1,-2" },
    CmdSpec { name: "use", aliases: &["model"], usage: ":use <model>", arg: ArgKind::Model,
        help: "switch the active model in the pool", example: ":use smollm2-360m" },
    CmdSpec { name: "fuzz", aliases: &["f"], usage: ":fuzz [n]", arg: ArgKind::Count,
        help: "fan n varied retries of the last prompt off the same parent", example: ":fuzz 8" },
    CmdSpec { name: "fuzzd", aliases: &["fd"], usage: ":fuzzd [n]", arg: ArgKind::Count,
        help: "fan n branches off the focus, each carrying a different behavioral directive", example: ":fd 6" },
    CmdSpec { name: "fuzzy-all", aliases: &["fa"], usage: ":fuzzy-all [n]", arg: ArgKind::Count,
        help: "fan n varied continuations off every current leaf", example: ":fa 4" },
    CmdSpec { name: "continue-all", aliases: &["ca"], usage: ":continue-all", arg: ArgKind::None,
        help: "continue every current leaf one step (1 prefill, N decodes)", example: ":ca" },
    CmdSpec { name: "continue", aliases: &["cont"], usage: ":continue [n]", arg: ArgKind::Count,
        help: "deepen the focused line n times (single-line continue, follows this branch's tip)", example: ":continue 3" },
    CmdSpec { name: "map", aliases: &[], usage: ":map <glob> ['tmpl']", arg: ArgKind::Free,
        help: "fan one worker task per changed file in the /tmp workspace ({file} → the path)", example: ":map *.rs 'add tests to {file}'" },
    CmdSpec { name: "ws", aliases: &["workspace"], usage: ":ws", arg: ArgKind::None,
        help: "show the loaded /tmp .gitmind workspace and its changed files", example: ":ws" },
    CmdSpec { name: "sync", aliases: &[], usage: ":sync", arg: ArgKind::None,
        help: "rescan the /tmp workspace's changed files after edits", example: ":sync" },
    CmdSpec { name: "branch", aliases: &["b"], usage: ":branch", arg: ArgKind::None,
        help: "branch a new line from the focused node", example: ":b" },
    CmdSpec { name: "prune", aliases: &["p"], usage: ":prune", arg: ArgKind::None,
        help: "prune the focused turn from the tree", example: ":p" },
    CmdSpec { name: "unprune", aliases: &["up"], usage: ":unprune", arg: ArgKind::None,
        help: "restore the last pruned turn", example: ":up" },
    CmdSpec { name: "overview", aliases: &["fit"], usage: ":overview", arg: ArgKind::None,
        help: "refit the camera to the whole tree", example: ":fit" },
    CmdSpec { name: "goto", aliases: &[], usage: ":goto <id|-K|N>", arg: ArgKind::Refs,
        help: "focus a node by id prefix, -K from the end, or 1-based position", example: ":goto -1" },
    CmdSpec { name: "best", aliases: &[], usage: ":best", arg: ArgKind::None,
        help: "rank the sibling candidates and mark the strongest", example: ":best" },
    CmdSpec { name: "strategy", aliases: &["bird", "birdseye", "strat"], usage: ":strategy [refs] ['focus']", arg: ArgKind::Refs,
        help: "stage a bird's-eye summary over the referenced turns", example: ":strategy 1-9 'risks'" },
    CmdSpec { name: "mergectx", aliases: &["mc"], usage: ":mergectx [refs] ['instruction']", arg: ArgKind::Refs,
        help: "stage a merge of several branches' context into one", example: ":mc 3,7 'reconcile'" },
    CmdSpec { name: "segment", aliases: &["blocks", "seg"], usage: ":segment [refs]", arg: ArgKind::Refs,
        help: "stage a segmentation of the conversation into blocks", example: ":seg" },
    CmdSpec { name: "todo", aliases: &[], usage: ":todo [refs]", arg: ArgKind::Refs,
        help: "scan for open items and weak links (logscope-cli)", example: ":todo" },
    CmdSpec { name: "dump", aliases: &[], usage: ":dump [path]", arg: ArgKind::Path,
        help: "save this chat as a logscope/chats dump in a workspace", example: ":dump ~/proj" },
    CmdSpec { name: "loadchat", aliases: &["lc"], usage: ":loadchat <dir>", arg: ArgKind::Path,
        help: "reload a saved chat dump directory", example: ":lc ~/proj/logscope/chats/chat-..." },
    CmdSpec { name: "open", aliases: &["o"], usage: ":open <path>", arg: ArgKind::Path,
        help: "import a chat (## You / ## Assistant markdown) from a file", example: ":open notes.md" },
    CmdSpec { name: "attach", aliases: &["a", "add"], usage: ":attach <path>", arg: ArgKind::Path,
        help: "attach a file's text to the next message", example: ":a src/main.rs" },
    CmdSpec { name: "import-workspace", aliases: &["importws", "iw"], usage: ":import-workspace <dir>", arg: ArgKind::Path,
        help: "discover chat dumps under a directory of one or many workspaces", example: ":iw ~/code" },
    CmdSpec { name: "summarize-files", aliases: &["sf"], usage: ":summarize-files [glob]", arg: ArgKind::Free,
        help: "stage the chat's files for per-file summaries", example: ":sf" },
    CmdSpec { name: "quest", aliases: &["questions"], usage: ":quest [n]", arg: ArgKind::Count,
        help: "have the model pose the questions that would unblock it", example: ":quest 3" },
    CmdSpec { name: "ans", aliases: &["answer"], usage: ":ans [path]", arg: ArgKind::Path,
        help: "fold filled Q/A answers back into a focused, targeted generation", example: ":ans" },
    CmdSpec { name: "check", aliases: &["chk"], usage: ":check", arg: ArgKind::None,
        help: "verify the assembled result in the sandbox", example: ":check" },
    CmdSpec { name: "explore", aliases: &["xp"], usage: ":explore", arg: ArgKind::None,
        help: "exploratory compilation over the fan: compile each distinct candidate once (batched cache) and keep the best-building one per subtask", example: ":explore" },
    CmdSpec { name: "assemble", aliases: &["asm"], usage: ":assemble", arg: ArgKind::None,
        help: "assemble the selected branches into one artifact", example: ":asm" },
    CmdSpec { name: "fill", aliases: &[], usage: ":fill [path]", arg: ArgKind::Path,
        help: "decompose a todo!()-skeleton into KV-shared sub-tasks and fan them out", example: ":fill" },
    CmdSpec { name: "export", aliases: &["e"], usage: ":export <md|ascii|git>", arg: ArgKind::Free,
        help: "export the conversation tree", example: ":export md" },
    CmdSpec { name: "linear", aliases: &[], usage: ":linear", arg: ArgKind::None,
        help: "switch to the linear chat view", example: ":linear" },
    CmdSpec { name: "spatial", aliases: &[], usage: ":spatial", arg: ArgKind::None,
        help: "switch to the 3D spatial/branching view", example: ":spatial" },
    CmdSpec { name: "stop", aliases: &["st", "qa"], usage: ":stop", arg: ArgKind::None,
        help: "cancel the running generation", example: ":stop" },
    CmdSpec { name: "load-gitmind", aliases: &["lgm"], usage: ":load-gitmind <path>", arg: ArgKind::Path,
        help: "mirror a .gitmind project to an isolated /tmp work session for editing", example: ":load-gitmind ~/work/monorepo" },
    CmdSpec { name: "reapply", aliases: &["ra"], usage: ":reapply", arg: ArgKind::None,
        help: "reapply the loaded session's changes over the project as one new commit", example: ":reapply" },
    CmdSpec { name: "agent", aliases: &["ag"], usage: ":agent <task>", arg: ArgKind::Free,
        help: "run the local-LLM tool-calling loop in the loaded workspace (writes/reads/runs to build the task)", example: ":agent add a unit test for the parser" },
    CmdSpec { name: "fan-agent", aliases: &["faa"], usage: ":fan-agent <task>", arg: ArgKind::Free,
        help: "spin up several agent branches (one isolated workspace each), run them interleaved, then :pick the keeper", example: ":fan-agent try it with anyhow and thiserror" },
    CmdSpec { name: "pick", aliases: &[], usage: ":pick <n>", arg: ArgKind::Count,
        help: "land fan branch n's changes over the project (keep it, discard the other branches)", example: ":pick 2" },
    CmdSpec { name: "docs", aliases: &["doc-fan"], usage: ":docs <task>", arg: ArgKind::Free,
        help: "fan out agents to write DOCS under the organization convention (front-matter + section tree), each a slice; then :merge-docs", example: ":docs write install, config, and API reference" },
    CmdSpec { name: "merge-docs", aliases: &[], usage: ":merge-docs", arg: ArgKind::None,
        help: "merge all doc-fan branches into one organized tree, regenerate the index, and commit (collisions reported)", example: ":merge-docs" },
    // NOTE: `:longterm` (src/longterm.rs, the long-horizon planner) is deliberately NOT listed:
    // the module landed cfg'd dead (#[allow(dead_code)], no vimmode parser arm, no dispatcher),
    // and the parity test below rightly refuses to advertise a command the user can't run.
    // Restore the CmdSpec { name: "longterm", aliases: &["lt"], .. } entry when the parser arm
    // and dispatch land.
    // NOTE: `:interject` (src/interject.rs) is in the same condition — module landed cfg'd dead,
    // no parser arm; restore CmdSpec { name: "interject", aliases: &["ij"], .. } with it.
];

/// Look up a command by its name or any alias (a leading `:` is tolerated).
pub fn spec(name: &str) -> Option<&'static CmdSpec> {
    let n = name.trim().trim_start_matches(':');
    SPECS.iter().find(|s| s.name == n || s.aliases.contains(&n))
}

/// Commands whose name or an alias starts with `partial` (a leading `:` is tolerated). Empty → all, sorted.
pub fn matches(partial: &str) -> Vec<&'static CmdSpec> {
    let p = partial.trim().trim_start_matches(':');
    let mut v: Vec<&CmdSpec> = if p.is_empty() {
        SPECS.iter().collect()
    } else {
        SPECS.iter().filter(|s| s.name.starts_with(p) || s.aliases.iter().any(|a| a.starts_with(p))).collect()
    };
    v.sort_by_key(|s| s.name);
    v
}

/// Context for argument suggestions — what the live app can offer (model names, recent message refs).
#[derive(Clone, Debug, Default)]
pub struct ArgCtx { pub models: Vec<String>, pub refs: Vec<String> }

#[derive(Clone, Debug, PartialEq)]
pub struct ArgSuggestion { pub value: String, pub hint: String }

/// Suggest values for a command's argument given a partial token and the live context.
pub fn suggest_args(spec: &CmdSpec, partial: &str, ctx: &ArgCtx) -> Vec<ArgSuggestion> {
    let p = partial.trim();
    let stat = |vals: &[(&str, &str)]| -> Vec<ArgSuggestion> {
        vals.iter().filter(|(v, _)| p.is_empty() || v.starts_with(p))
            .map(|(v, h)| ArgSuggestion { value: (*v).into(), hint: (*h).into() }).collect()
    };
    match spec.arg {
        ArgKind::Ratio => stat(&[("0.2", "keep ~20% — aggressive"), ("0.3", "keep ~30% — balanced"), ("0.5", "keep ~50% — light")]),
        ArgKind::Count => stat(&[("4", "a few"), ("8", "several"), ("16", "many")]),
        ArgKind::Model => ctx.models.iter().filter(|m| p.is_empty() || m.starts_with(p))
            .map(|m| ArgSuggestion { value: m.clone(), hint: "switch to this model".into() }).collect(),
        ArgKind::Refs => ctx.refs.iter().filter(|r| p.is_empty() || r.starts_with(p))
            .map(|r| ArgSuggestion { value: r.clone(), hint: "message".into() }).collect(),
        ArgKind::Path => vec![ArgSuggestion { value: "~/".into(), hint: "a folder on disk".into() }],
        ArgKind::None | ArgKind::Free => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_resolves_by_name_and_alias() {
        assert_eq!(spec("compact").unwrap().name, "compact");
        assert_eq!(spec(":cp").unwrap().name, "compact", "alias + leading colon");
        assert_eq!(spec("refresh").unwrap().name, "compact");
        assert!(spec("nonesuch").is_none());
        // every alias is unique across the table and never collides with another command's name
        let mut seen = std::collections::HashSet::new();
        for s in SPECS {
            assert!(seen.insert(s.name), "duplicate name {}", s.name);
            for a in s.aliases { assert!(seen.insert(*a), "alias {a} collides"); }
        }
    }

    #[test]
    fn matches_completes_a_prefix() {
        let m = matches("co"); // compact, continue-all
        let names: Vec<_> = m.iter().map(|s| s.name).collect();
        assert!(names.contains(&"compact") && names.contains(&"continue-all"));
        assert!(matches("").len() == SPECS.len(), "empty lists all");
        // alias prefix works
        assert!(matches("lc").iter().any(|s| s.name == "loadchat"));
    }

    #[test]
    fn arg_suggestions_match_the_kind() {
        let ctx = ArgCtx { models: vec!["smollm2-360m".into(), "llama-1b".into()], refs: vec!["-1".into(), "-2".into()] };
        // ratio presets, filtered by partial
        let r = suggest_args(spec("compact").unwrap(), "0.3", &ctx);
        assert_eq!(r, vec![ArgSuggestion { value: "0.3".into(), hint: "keep ~30% — balanced".into() }]);
        // model names from context
        let m = suggest_args(spec("use").unwrap(), "sm", &ctx);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].value, "smollm2-360m");
        // refs from context
        assert_eq!(suggest_args(spec("why").unwrap(), "", &ctx).len(), 2);
        // none-arg commands suggest nothing
        assert!(suggest_args(spec("best").unwrap(), "", &ctx).is_empty());
    }

    // A dummy argument that the PARSER accepts for each arg kind (a bare positive int is valid for both
    // counts and ref-lists — including :goto N — so those share one).
    fn sample_arg(k: ArgKind) -> &'static str {
        match k {
            ArgKind::Ratio => "0.3",
            ArgKind::Model => "m",
            ArgKind::Path => "x",
            ArgKind::Free => "x",
            ArgKind::None => "",
            ArgKind::Count | ArgKind::Refs => "1",
        }
    }

    #[test]
    fn fan_and_longhorizon_commands_are_discoverable() {
        // The fan commands and the long-horizon quest→ans / fill→assemble loops must be reachable from
        // :help and offered to :suggest — not just parseable if you happen to know them.
        for n in ["fuzzd", "fd", "fill", "ans", "answer", "prune", "unprune", "overview", "fit", "open", "goto", "attach"] {
            assert!(spec(n).is_some(), ":help / :suggest cannot see '{n}'");
        }
    }

    #[test]
    fn palette_stays_in_parity_with_the_parser() {
        use crate::vimmode::{parse_command, Command};
        // Every command this palette advertises (in :help + :suggest) must map to a real parser arm —
        // otherwise :help would list a command the user can't actually run.
        for s in SPECS {
            let line = format!(":{} {}", s.name, sample_arg(s.arg));
            assert!(!matches!(parse_command(line.trim()), Command::Unknown(_)),
                    "palette lists ':{}' but the parser rejects it (tables drifted)", s.name);
        }
        // …and conversely every command in the LIVE completion catalog (the `:` grid) must be in this
        // palette, so :help/:suggest never fall behind what the grid already offers. This is exactly the
        // drift that hid the fan commands (:fuzzd, :fill, :ans) from help while the grid showed them.
        for c in crate::vimmode::command_catalog() {
            assert!(spec(c.name).is_some(),
                    "'{}' is in the live command grid but missing from :help/:suggest", c.name);
        }
    }
}
