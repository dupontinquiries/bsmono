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
    CmdSpec { name: "tour", aliases: &["fly"], usage: ":tour [speed|off]", arg: ArgKind::Free,
        help: "toggle the auto-flying camera along the conversation strand; a number sets the pace",
        example: ":tour 1.5" },
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
    CmdSpec { name: "ws", aliases: &["workspace"], usage: ":ws [path]", arg: ArgKind::Path,
        help: "with no argument, show the loaded workspace and its changed files; with a path, OPEN that cargo workspace",
        example: ":ws ~/src/monorepo" },
    CmdSpec { name: "replicate", aliases: &["rep"], usage: ":replicate [n]", arg: ArgKind::Count,
        help: "open replicator mode: n independent chats side by side over the workspace, each with its own job — \
message all or aim at one, and pair any two so each sees the other's latest",
        example: ":replicate 3" },
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
/// One addressable message, as the ref grammar sees it.
///
/// `resolve_ref` accepts three forms — `-K` (K back from the end), `N` (1-based position), and a
/// PREFIX of the message's `short_id`, which resolves to nothing when two messages share the prefix.
/// So completing a message means offering its FULL short_id: that is the only form guaranteed to
/// resolve, and the guarantee is what makes an autocomplete worth having rather than a hint.
#[derive(Clone, Debug, PartialEq)]
pub struct MsgRef {
    /// 0-based position in the conversation.
    pub idx: usize,
    /// The stable short id (`vimmode::short_id`) — what the user should end up typing.
    pub short_id: String,
    /// "you" / "assistant" / a participant name.
    pub role: String,
    /// Enough of the text to recognise the message by.
    pub preview: String,
}

#[derive(Clone, Debug, Default)]
pub struct ArgCtx {
    pub models: Vec<String>,
    pub refs: Vec<String>,
    /// The conversation, newest LAST. Empty means the caller has nothing to offer and only the
    /// static relative refs are suggested, which is the old behaviour.
    pub msgs: Vec<MsgRef>,
}

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
        ArgKind::Refs => suggest_refs(p, ctx),
        ArgKind::Path => vec![ArgSuggestion { value: "~/".into(), hint: "a folder on disk".into() }],
        ArgKind::None | ArgKind::Free => Vec::new(),
    }
}

/// Message-id completion for the ref-taking commands (`:goto`, `:why`, `:mergectx`, `:segment`, …).
///
/// Three ways a person reaches for a message, all supported because all three get typed:
///   * BY ID — they can see `3df` in the transcript and start typing it.
///   * BY POSITION — `-1` for the last message, `4` for the fourth. Kept, and now annotated with
///     WHICH message it lands on, since a bare `-2` tells you nothing about what you are about to
///     operate on.
///   * BY CONTENT — they remember the message said "auth" but not where it was. Substring search
///     over the preview, which is the case a plain prefix-matcher cannot serve at all.
///
/// Newest first throughout: in a conversation the recent end is where the work is.
pub fn suggest_refs(p: &str, ctx: &ArgCtx) -> Vec<ArgSuggestion> {
    const MAX: usize = 8;
    let n = ctx.msgs.len();
    let label = |m: &MsgRef| -> String {
        let back = n - m.idx;                       // 1 = last message
        let text: String = m.preview.chars().take(48).collect();
        format!("-{back} · {} · {}", m.role, text.trim())
    };
    // No context to offer: fall back to whatever static refs the caller passed.
    if ctx.msgs.is_empty() {
        return ctx.refs.iter().filter(|r| p.is_empty() || r.starts_with(p))
            .map(|r| ArgSuggestion { value: r.clone(), hint: "message".into() }).collect();
    }
    let mut out: Vec<ArgSuggestion> = Vec::new();
    let newest_first = || ctx.msgs.iter().rev();

    if p.is_empty() {
        out.extend(newest_first().take(MAX)
            .map(|m| ArgSuggestion { value: m.short_id.clone(), hint: label(m) }));
        return out;
    }

    // A positional ref: show it, and say what it resolves to so the choice is informed.
    if p.parse::<i64>().is_ok() {
        if let Some(m) = resolve_positional(p, ctx) {
            out.push(ArgSuggestion { value: p.to_string(), hint: format!("{} (position)", label(m)) });
        }
    }

    let lower = p.to_ascii_lowercase();
    // By id prefix. An id that is still ambiguous would resolve to NOTHING, so completing to the
    // full short_id is the point rather than a nicety.
    out.extend(newest_first().filter(|m| m.short_id.starts_with(&lower))
        .take(MAX).map(|m| ArgSuggestion { value: m.short_id.clone(), hint: label(m) }));
    // By content, for the ones the id prefix did not already offer.
    if lower.chars().any(|c| !c.is_ascii_digit()) {
        let already: std::collections::HashSet<String> = out.iter().map(|s| s.value.clone()).collect();
        out.extend(newest_first()
            .filter(|m| !already.contains(&m.short_id)
                     && m.preview.to_ascii_lowercase().contains(&lower))
            .take(MAX)
            .map(|m| ArgSuggestion { value: m.short_id.clone(), hint: format!("{} (match)", label(m)) }));
    }
    out.truncate(MAX * 2);
    out
}

/// Which message a `-K` / `N` ref lands on, mirroring `vimmode::resolve_ref` for those two forms.
/// Duplicated deliberately rather than depended on: this crate's palette must not gain a dependency
/// on the mode layer just to render a hint, and the shapes are pinned by a test.
fn resolve_positional<'a>(r: &str, ctx: &'a ArgCtx) -> Option<&'a MsgRef> {
    let n = ctx.msgs.len();
    let v: i64 = r.trim().parse().ok()?;
    let idx = if r.trim_start().starts_with('-') {
        let k = (-v) as usize;
        (k >= 1 && k <= n).then(|| n - k)?
    } else {
        let k = v as usize;
        (k >= 1 && k <= n).then(|| k - 1)?
    };
    ctx.msgs.get(idx)
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
        let ctx = ArgCtx { models: vec!["smollm2-360m".into(), "llama-1b".into()], refs: vec!["-1".into(), "-2".into()], msgs: Vec::new() };
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

#[cfg(test)]
mod msg_id_completion {
    use super::*;

    fn convo() -> ArgCtx {
        // short_ids taken from vimmode::short_id so the test pins the REAL ids, not invented ones.
        let ids: Vec<String> = (0..5).map(crate::vimmode::short_id).collect();
        let texts = ["hello there", "let us fix the auth bug", "which auth bug",
                     "the token refresh one", "on it"];
        let msgs = ids.iter().zip(texts).enumerate().map(|(i, (id, t))| MsgRef {
            idx: i, short_id: id.clone(),
            role: if i % 2 == 0 { "you".into() } else { "assistant".into() },
            preview: t.to_string(),
        }).collect();
        ArgCtx { models: Vec::new(), refs: Vec::new(), msgs }
    }
    fn goto() -> &'static CmdSpec { spec("goto").unwrap() }

    /// Empty partial offers the recent end first — that is where the work is.
    #[test]
    fn empty_partial_lists_newest_first_with_a_readable_hint() {
        let c = convo();
        let s = suggest_args(goto(), "", &c);
        assert_eq!(s[0].value, c.msgs[4].short_id, "the LAST message should come first");
        assert!(s[0].hint.starts_with("-1 · you · on it"), "hint should say how far back and what it says: {}", s[0].hint);
        assert_eq!(s[1].value, c.msgs[3].short_id);
    }

    /// The completed value must RESOLVE. A prefix that is still ambiguous resolves to nothing in
    /// vimmode::resolve_ref, so completing to the full short_id is the whole point.
    #[test]
    fn every_suggested_value_resolves_through_the_real_resolver() {
        let c = convo();
        for partial in ["", "a", "-2", "auth"] {
            for sg in suggest_args(goto(), partial, &c) {
                assert!(crate::vimmode::resolve_ref(c.msgs.len(), &sg.value).is_some(),
                    "suggested {:?} for partial {partial:?} but the resolver rejects it", sg.value);
            }
        }
    }

    /// Positional refs keep working AND now say what they land on.
    #[test]
    fn a_positional_ref_is_annotated_with_its_target() {
        let c = convo();
        let s = suggest_args(goto(), "-2", &c);
        assert!(!s.is_empty(), "-2 should suggest something");
        assert_eq!(s[0].value, "-2");
        assert!(s[0].hint.contains("the token refresh one"), "should name the message: {}", s[0].hint);
        assert!(s[0].hint.contains("(position)"));
        // out of range offers nothing rather than a ref that fails later
        assert!(suggest_args(goto(), "-99", &c).is_empty());
    }

    /// Content search — the case a prefix matcher cannot serve.
    #[test]
    fn a_word_finds_the_messages_that_contain_it() {
        let c = convo();
        let s = suggest_args(goto(), "auth", &c);
        let vals: Vec<&str> = s.iter().map(|x| x.value.as_str()).collect();
        assert!(vals.contains(&c.msgs[2].short_id.as_str()), "should find 'which auth bug'");
        assert!(vals.contains(&c.msgs[1].short_id.as_str()), "should find 'let us fix the auth bug'");
        assert!(!vals.contains(&c.msgs[4].short_id.as_str()), "'on it' does not mention auth");
        assert!(s.iter().all(|x| x.hint.contains("(match)")), "content hits should say why they matched");
    }

    #[test]
    fn an_id_prefix_completes_to_the_full_id() {
        let c = convo();
        let target = c.msgs[3].short_id.clone();
        let s = suggest_args(goto(), &target[..2], &c);
        assert!(s.iter().any(|x| x.value == target), "prefix {:?} should complete to {target}", &target[..2]);
    }

    /// With no conversation supplied the old static behaviour survives — callers that have not been
    /// updated must not lose what they had.
    #[test]
    fn without_messages_the_static_refs_still_work() {
        let c = ArgCtx { models: Vec::new(), refs: vec!["-1".into(), "-2".into()], msgs: Vec::new() };
        let s = suggest_args(goto(), "-", &c);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].value, "-1");
    }
}
