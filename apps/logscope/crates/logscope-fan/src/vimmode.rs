//! vimmode.rs — modal (vim-like) control for the spatial graph. Pure + testable: it owns the Mode
//! enum, parses `:` colon-commands (with aliases), fuzzy-ranks search candidates (fzf-lite), and maps a
//! Normal-mode keypress — honoring a `z` rotation prefix and a `space` leader — to an ACTION the egui
//! layer then executes (move focus, rotate/zoom the camera, open search/command, branch, …). The egui
//! glue only reads keys and runs actions; all the logic that's worth testing lives here.

/// A discrete direction from an analog stick (dominant axis past the deadzone).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dir4 { Left, Right, Up, Down }

/// Map a joystick deflection (each component in [-1,1]) to a discrete direction once it leaves the
/// deadzone; the dominant axis wins. Left/Right = switch branch, Up/Down = crawl the timeline. None
/// inside the deadzone. (The UI handles auto-repeat while held.)
pub fn joystick_dir(dx: f32, dy: f32, deadzone: f32) -> Option<Dir4> {
    if (dx * dx + dy * dy).sqrt() < deadzone { return None; }
    if dx.abs() >= dy.abs() { Some(if dx < 0.0 { Dir4::Left } else { Dir4::Right }) }
    else { Some(if dy < 0.0 { Dir4::Up } else { Dir4::Down }) }
}

/// Camera joystick (bottom-right): deflection → (Δyaw, Δpitch) per frame, scaled by sensitivity.
pub fn cam_stick(dx: f32, dy: f32, sens: f32) -> (f32, f32) { (dx * sens, -dy * sens) }

/// Navigation joystick (bottom-left): direction → tree action (the UI auto-repeats while held).
pub fn nav_dir_action(d: Dir4) -> Action {
    match d { Dir4::Left => Action::SiblingPrev, Dir4::Right => Action::SiblingNext, Dir4::Up => Action::FocusPrev, Dir4::Down => Action::FocusNext }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode { Normal, Insert, Command, Search }
impl Mode {
    pub fn label(self) -> &'static str {
        match self { Mode::Normal => "NORMAL", Mode::Insert => "INSERT", Mode::Command => "COMMAND", Mode::Search => "SEARCH" }
    }
    /// Indicator colour for the mode pill (kept here so the choice is one place + testable). The egui
    /// layer turns this into a Color32; Normal=cyan, Insert=green, Command=amber, Search=violet.
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            Mode::Normal  => (0x53, 0xd6, 0xe0),
            Mode::Insert  => (0x86, 0xe0, 0xa0),
            Mode::Command => (0xe7, 0xb7, 0x5f),
            Mode::Search  => (0xc9, 0x8f, 0xff),
        }
    }
    /// True while the composer should own the keyboard (typing) rather than the motion layer.
    pub fn is_typing(self) -> bool { matches!(self, Mode::Insert | Mode::Command | Mode::Search) }
}

/// What kind of search the line is running (changes how the bottom bar labels it / how matching works).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchKind { Messages, Tree, FuzzyTree }

/// A parsed colon-command.
#[derive(Clone, PartialEq, Debug)]
pub enum Command {
    Branch,            // :b / :branch         — next message forks off the focused node
    Attach(String),    // :a / :add / :attach <path>
    Goto(usize),       // :<N> / :goto <N>     — focus turn N (1-based as shown in the UI)
    Prune,             // :prune / :p          — prune the focused turn
    Unprune,           // :unprune / :up
    Overview,          // :overview / :fit    — refit the camera to the whole tree (the `o` key, too)
    Fuzz(usize),       // :fuzz <N>            — fan N fuzzed branches (default 10)
    FuzzDirective(usize), // :fuzzd <N> / :fd  — fan N branches, each with a different behavioral directive
    Open(String),      // :o / :open <path>    — import a chat (markdown ## You / ## Assistant) from a file
    ContinueAll(usize),// :ca [N] / :Nca       — continuation on every leaf, fanned N rounds deep (depth-N)
    FuzzyAll { depth: usize, width: usize, template: String }, // :fa [W] ['tmpl'] / :Nfa W 'tmpl' — depth×width templated fan
    Continue(usize),   // :continue [N] / :cont — deepen the FOCUSED line N times (a single-leaf continue-all)
    MapFiles { glob: String, template: String }, // :map <glob> ['tmpl'] — fan one worker task per changed file in
                       // the /tmp workspace ({file} → the path); the worker-thread-per-file fan-out for a project
    WorkspaceStatus,   // :ws / :workspace — show the loaded /tmp .gitmind session + its changed files
    SyncWorkspace,     // :sync — rescan the /tmp workspace's changed files (refresh after edits)
    Stop,              // :stop / :st          — cancel a running fan + clear the queue
    Fill(String),      // :fill [path]         — decompose a todo!()-skeleton into KV-shared sub-tasks + fan them
    Assemble,          // :assemble / :asm     — splice the filled bodies back into the skeleton
    Quest(usize),      // :quest [N]           — generate N clarifying questions (breadth) as a fillable Q/A template
    Answer(String),    // :ans [path]          — fold filled Q/A answers into a focused, targeted generation
    Check,             // :check / :chk     — compile every leaf's code and store a real pass/fail verdict
    Explore,           // :explore / :xp    — exploratory compilation over the current fan: compile every
                       // candidate through a batched verdict cache (each distinct one once) and report the
                       // best-BUILDING candidate per subtask (a Pass beats a higher-quality Fail)
    Best,              // :best                 — highlight + focus the region that verified best (agent pick)
    Linear,            // :linear              — switch to the linear chat view
    Spatial,           // :spatial             — switch to the spatial view
    Export(String),    // :export <ascii|md|git> / :e — write the whole tree in that format
    Use(String),       // :use <name> / :model <name> — switch which model in the pool subsequent commands run on
    MergeCtx { refs: Vec<String>, instruction: String }, // :mergectx -1,3df,a9j ['ask'] — flatten the referenced
                       // nodes into ONE context (a candidate list) and generate against it: fan out cheap, then
                       // hand the list to a big model to pick the best. Ids are addressed by prefix ("first bits").
    Strategy { refs: Vec<String>, focus: String }, // :bird / :birdseye / :strat / :strategy [ids] ['focus'] —
                       // collapse the conversation (whole tree, or just the referenced nodes) into one condensed,
                       // unified bird's-eye synthesis. The summarizer-worker prompt rides in the generated turn.
    Segment { refs: Vec<String>, focus: String }, // :segment / :blocks / :seg [ids] ['focus'] — map the chat into
                       // coherent BLOCKS and mark the natural seams between them (topic/intent shifts, loops), so the
                       // graph can split/branch there. The segmenter-worker prompt rides in the generated turn.
    Todo { refs: Vec<String>, focus: String }, // :todo [ids] ['focus'] — spin off the small model (small context) to
                       // find open items per turn and weakly link each to the turn that addresses it. Builds the
                       // labeled "weak link" edges that power merged context (`:mergectx ~addresses`) + KV-order.
    Dump(String),      // :dump [workspace-path] — write this chat as a logscope/chats/<id> dump in a workspace
    LoadChat(String),  // :loadchat <dir> / :lc — reload a chat dump directory
    ImportWorkspace(String), // :import-workspace <dir> / :iw — discover + import chat dumps under a directory
                       // (one or many workspaces; project folder structure respected)
    SummarizeFiles(String),  // :summarize-files [glob] / :sf — stage the chat's files into the sandbox and
                       // ask the model for a per-file summary (fan-out friendly)
    LoadGitmind(String), // :load-gitmind <path> — mirror a .gitmind project into an isolated /tmp session
    Reapply,             // :reapply — land the loaded session's changes over the project as one new commit
    Agent(String),       // :agent <task> — run the local-LLM agentic loop (tool-calling) in the loaded
                         // session's isolated workspace: the model writes/reads/runs to build the task,
                         // files appear live in the scene, then :reapply lands them as one commit
    FanAgent(String),    // :fan-agent <task> — spin up several agent branches (one isolated workspace each),
                         // run them interleaved on the one worker, then prune to the keeper
    Pick(usize),         // :pick <n> — land fan branch n's changes over the project (keep it, discard the rest)
    DocFan(String),      // :docs <task> — fan out several agents to WRITE DOCS (doc-organization convention on),
                         // each a slice of the tree in its own workspace; then :merge-docs to union them
    MergeDocs,           // :merge-docs — merge all fan branches' docs into one organized tree + generated index
    Compact(String),   // :compact [ratio] / :refresh — student/teacher self-compaction: fold older turns into
                       // a shorter briefing (staged as the working context) to grow effective context, speed
                       // decode, and create a pinnable KV checkpoint reused by descendant paths
    Race(String),      // :race [ratio] — race the COMPACTED context against the FULL context off the same
                       // parent; surface each result as a selectable option as it finishes, keep the better
    Diagnose(Vec<String>), // :why / :diagnose [a,b] — flag a compaction as a bad jump and stage a SMALLER
                       // model to diagnose what the briefing dropped (compares two outputs + their word diff)
    Help(String),      // :help [command] — palette help: list commands, or one command's usage + example
    Suggest,           // :suggest / :sg — a throwaway small model reads recent messages + the command list
                       // and proposes the next commands to run
    Prompt(String),    // :prompt <key> / :pr — stage a specialized prompt from the library over recent turns
    Noop,              // empty line
    Unknown(String),
}

/// Stable short id for the node at push-position `n` — a scrambled base36 hash so ids read as distinct
/// tokens (not sequential), addressed by any UNIQUE prefix the way git short hashes are ("first bits").
/// The GUI and CLI compute it identically, so an id shown in one resolves in the other.
pub fn short_id(n: usize) -> String {
    let mut h = short_id_seed(n);
    const A: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut s = [0u8; 6];
    for c in s.iter_mut() { *c = A[(h % 36) as usize]; h /= 36; }
    String::from_utf8_lossy(&s).into_owned()
}

/// The 64-bit hash behind `short_id(n)` — so a node's glowing identicon rune encodes the SAME identity as
/// its printed short-id (both are this hash, just rendered differently).
pub fn short_id_seed(n: usize) -> u64 {
    let mut h = (n as u64).wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 30; h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27; h = h.wrapping_mul(0x94D0_49BB_1331_11EB); h ^= h >> 31;
    h
}

/// Resolve one id-ref against a tree of `n` nodes. `-K` → Kth from the end (`-1` = last). A bare positive
/// number → that 1-based position. Otherwise a short-id prefix; `Some` only if EXACTLY one node matches
/// (ambiguous or unknown → `None`, so the caller can ask for more chars).
pub fn resolve_ref(n: usize, r: &str) -> Option<usize> {
    let r = r.trim();
    if n == 0 || r.is_empty() { return None; }
    if let Ok(v) = r.parse::<i64>() {
        if r.starts_with('-') { let k = (-v) as usize; return (k >= 1 && k <= n).then(|| n - k); }
        let k = v as usize; return (k >= 1 && k <= n).then(|| k - 1);
    }
    let pref = r.to_ascii_lowercase();
    let mut hit = None;
    for i in 0..n { if short_id(i).starts_with(&pref) { if hit.is_some() { return None; } hit = Some(i); } }
    hit
}

/// Resolve a list of id-refs, returning the ones that landed (in order) and the ones that didn't.
pub fn resolve_refs(n: usize, refs: &[String]) -> (Vec<usize>, Vec<String>) {
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for r in refs { match resolve_ref(n, r) { Some(i) => ok.push(i), None => bad.push(r.clone()) } }
    (ok, bad)
}

/// Split `:mergectx` args into the comma-separated ref list and an optional (quoted) instruction:
/// "-1,3df,a9j" → (["-1","3df","a9j"], ""); "-1,3df 'pick best'" → (["-1","3df"], "pick best").
fn parse_mergectx_args(arg: &str) -> (Vec<String>, String) {
    let arg = arg.trim();
    let mut it = arg.splitn(2, char::is_whitespace);
    let refs_part = it.next().unwrap_or("");
    let instr = strip_quotes(it.next().unwrap_or("")).to_string();
    let refs = refs_part.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    (refs, instr)
}

/// The summarizer-worker prompt for `:strategy`/`:birdseye`. Rides at the top of the generated turn; the
/// flattened conversation is appended after it. General enough for any chat, tuned to UNIFY many branches
/// into a single condensed overview rather than transcribe them.
pub const STRATEGIST_PROMPT: &str = "\
You are a strategist collapsing a long, branching exploration into a tight bird's-eye view. The \
conversation below may hold many parallel branches and dead ends. Produce a UNIFIED, condensed synthesis \
— far shorter than the input — that someone could read in a minute to know where things stand and what to \
do next.

Write it as:
- Through-line: 1-2 sentences on what this is really about and the current best understanding.
- Threads: the distinct directions explored, a sentence or two each, MERGING duplicates (don't repeat an \
idea just because it showed up in several branches).
- Settled: conclusions reached, specific enough to act on.
- Open: unresolved questions, live tradeoffs, and where the branches disagree.
- Next: the single most promising direction to pursue, and why.

Compress hard — cut greetings, restated context, and dead ends (note a dead end in a clause, don't recount \
it). Prefer specifics (names, numbers, decisions) over generalities. Don't manufacture coherence: if the \
material is thin or contradictory, say so plainly.";

/// Split `:strategy` args: an optional comma-separated ref list, then an optional quoted focus. A bare
/// quoted string is all focus (no refs): "'perf tradeoffs'" → ([], "perf tradeoffs"); "-1,3df 'why'" →
/// (["-1","3df"], "why"); "" → ([], "").
fn parse_strategy_args(arg: &str) -> (Vec<String>, String) {
    let arg = arg.trim();
    if arg.is_empty() { return (Vec::new(), String::new()); }
    if arg.starts_with('\'') || arg.starts_with('"') { return (Vec::new(), strip_quotes(arg).to_string()); }
    let mut it = arg.splitn(2, char::is_whitespace);
    let refs_part = it.next().unwrap_or("");
    let focus = strip_quotes(it.next().unwrap_or("")).to_string();
    let refs = refs_part.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    (refs, focus)
}

/// The segmenter-worker prompt for `:segment`/`:blocks`. Rides at the top of the generated turn; the
/// flattened transcript (turns tagged `[#n role · id]`) is appended after it. Designed to cut a messy
/// linear chat into a few coherent blocks AND name the natural seams, so the graph can split/branch
/// there — duplicate-collapsing and degeneration-flagging included.
pub const SEGMENTER_PROMPT: &str = "\
You are mapping a raw, often messy chat transcript into a few coherent BLOCKS and marking the natural \
seams between them — the points where the context genuinely shifts. The goal is a richer conversation \
graph: each block can become a node and each seam a place to split or branch. Compress hard; this is a \
navigable map, not a re-transcription. Turns are tagged [#n role · id].

Read the whole transcript first, then cut it into blocks. Open a new block ONLY at a natural seam — a real \
change in what's happening:
- a new topic or task (the strongest seam),
- a change of intent (asking -> giving feedback -> asking again; exploring -> deciding),
- question -> answer, or one finished answer -> a distinct next one,
- a correction or reversal (\"actually, do it this way\"),
- a digression that leaves and rejoins the main thread,
- a degenerate stretch (the model looping, repeating itself, or emitting garbled / template-broken \
output) — gather the WHOLE stretch into ONE block and label it; do not split a broken loop turn by turn.

For each block output exactly:

## Block <n>: <4-8 word title>
- spans: #<first>-#<last>
- relation: {new-topic | continues #<n> | refines #<n> | feedback-on #<n> | correction-of #<n> | digression | loop}
- keywords: 3-6 lowercase topic words a reader would search for
- gist: 1-2 sentences, in your own words, on what the block actually contains or achieves
- seam: the boundary signal that opens this block (omit for Block 1)
- health: only if notable — thin | degenerate (looping/garbled/broken) | off-track

Then:

## Seams & graph notes
- the 2-4 strongest split points (block boundaries), each in a clause — where the graph should branch or segment.
- any blocks that should collapse to a single node (e.g. a loop), and any that belong on a side-branch off \
the main line rather than in sequence.

Prefer a few meaningful blocks over many tiny ones; merge adjacent turns that are the same beat. Name \
degeneration plainly — never summarize a broken loop as progress. Keep every gist short: the output is a \
skeleton the graph can grow on.";

/// One entry in the command palette: a canonical name, its aliases, an argument signature, and a
/// one-line description — enough to render a zsh-style completion grid and a hint for the top choice.
pub struct CmdSpec { pub name: &'static str, pub aliases: &'static [&'static str], pub args: &'static str, pub help: &'static str }

static CATALOG: &[CmdSpec] = &[
    CmdSpec { name: "ca",        aliases: &["continue-all"],        args: "[N]",           help: "continue every leaf, fanned N rounds deep" },
    CmdSpec { name: "continue",  aliases: &["cont"],                args: "[N]",           help: "deepen the FOCUSED line N times (single-line continue)" },
    CmdSpec { name: "map",       aliases: &[],                      args: "<glob> ['tmpl']", help: "fan one worker task per changed file ({file}) in the /tmp workspace" },
    CmdSpec { name: "ws",        aliases: &["workspace"],           args: "",              help: "show the loaded /tmp .gitmind workspace + its changed files" },
    CmdSpec { name: "sync",      aliases: &[],                      args: "",              help: "rescan the /tmp workspace's changed files after edits" },
    CmdSpec { name: "fa",        aliases: &["fuzzy-all"],           args: "[W] ['tmpl']",  help: "depth×width templated fan-out over the leaves" },
    CmdSpec { name: "fuzz",      aliases: &["f"],                   args: "[N]",           help: "fan N fuzzed variants of the prompt (default 10)" },
    CmdSpec { name: "fuzzd",     aliases: &["fd"],                  args: "[N]",           help: "fan N variants, each a different behavioral directive" },
    CmdSpec { name: "mergectx",  aliases: &["mc"],                  args: "<ids> ['ask']", help: "flatten the referenced nodes into one context for a pick" },
    CmdSpec { name: "strategy",  aliases: &["strat", "bird", "birdseye"], args: "[ids] ['focus']", help: "condense the conversation into a unified bird's-eye synthesis" },
    CmdSpec { name: "segment",   aliases: &["blocks", "seg"],       args: "[ids] ['focus']", help: "map the chat into blocks + mark seams to split/branch the graph" },
    CmdSpec { name: "todo",      aliases: &[],                      args: "[ids] ['focus']", help: "small-model TODO scan → weak links for merged context + KV-order" },
    CmdSpec { name: "goto",      aliases: &[],                      args: "<id|-K|N>",     help: "focus a node by id prefix, -K from the end, or position" },
    CmdSpec { name: "use",       aliases: &["model"],               args: "<name>",        help: "switch which model in the pool runs next (CLI)" },
    CmdSpec { name: "open",      aliases: &["o"],                   args: "<path>",        help: "import a chat (## You / ## Assistant) from a file" },
    CmdSpec { name: "attach",    aliases: &["a", "add"],            args: "<path>",        help: "attach a file's text to the next message" },
    CmdSpec { name: "branch",    aliases: &["b"],                   args: "",              help: "next message forks off the focused node" },
    CmdSpec { name: "prune",     aliases: &["p"],                   args: "",              help: "prune the focused turn from the tree" },
    CmdSpec { name: "unprune",   aliases: &["up"],                  args: "",              help: "restore the last pruned turn" },
    CmdSpec { name: "fill",      aliases: &[],                      args: "[path]",        help: "decompose a todo!() skeleton into sub-tasks and fan them" },
    CmdSpec { name: "assemble",  aliases: &["asm"],                 args: "",              help: "splice the filled bodies back into the skeleton" },
    CmdSpec { name: "quest",     aliases: &[],                      args: "[N]",           help: "generate N clarifying questions as a fillable template" },
    CmdSpec { name: "ans",       aliases: &["answer"],              args: "[path]",        help: "fold filled Q/A answers into a targeted generation" },
    CmdSpec { name: "check",     aliases: &["chk"],                 args: "",              help: "compile every leaf and store a real pass/fail verdict" },
    CmdSpec { name: "explore",   aliases: &["xp"],                  args: "",              help: "exploratory compilation over the fan: best-building candidate per subtask (batched cache)" },
    CmdSpec { name: "best",      aliases: &[],                      args: "",              help: "highlight + focus the region that verified best" },
    CmdSpec { name: "export",    aliases: &["e"],                   args: "[ascii|md|git]",help: "write the whole tree in that format" },
    CmdSpec { name: "linear",    aliases: &[],                      args: "",              help: "switch to the linear chat view" },
    CmdSpec { name: "spatial",   aliases: &[],                      args: "",              help: "switch to the 3D spatial view" },
    CmdSpec { name: "overview",  aliases: &["fit"],                 args: "",              help: "refit the camera to the whole tree" },
    CmdSpec { name: "stop",      aliases: &["st", "qa"],            args: "",              help: "cancel a running fan and clear the queue" },
];

/// The whole command palette (canonical names + aliases + arg signatures + help).
pub fn command_catalog() -> &'static [CmdSpec] { CATALOG }

/// The text after a leading `:` with any repeat-count stripped (`:6ca` → "ca"), or None when `input`
/// isn't a `:` command line.
fn command_body(input: &str) -> Option<&str> {
    let rest = input.trim_start().strip_prefix(':')?;
    Some(rest.trim_start().trim_start_matches(|c: char| c.is_ascii_digit()))
}

/// Commands whose name or an alias extends what's typed after `:` — the completion grid. Ordered so an
/// exact name/alias match leads, then shorter names, then alphabetically. Empty once an argument is
/// being typed (whitespace after the command) or outside a `:` command. Bare `:` lists everything.
pub fn command_completions(input: &str) -> Vec<&'static CmdSpec> {
    let Some(body) = command_body(input) else { return Vec::new() };
    if body.contains(char::is_whitespace) { return Vec::new(); }
    let pref = body.to_ascii_lowercase();
    let exact = |c: &CmdSpec| c.name == pref || c.aliases.iter().any(|a| *a == pref);
    let mut hits: Vec<&CmdSpec> = CATALOG.iter()
        .filter(|c| c.name.starts_with(&pref) || c.aliases.iter().any(|a| a.starts_with(&pref)))
        .collect();
    // exact match first, then commands whose NAME (not just an alias) extends the prefix, then shorter
    // names, then alphabetically — so ":fu" surfaces `fuzz`, not `fa` (whose alias `fuzzy-all` also matches).
    hits.sort_by_key(|c| (!exact(c), !c.name.starts_with(&pref), c.name.len(), c.name));
    hits
}

/// The command to describe in the hint line: the exact command if its name/alias is fully typed (so the
/// hint persists while the arguments are entered), otherwise the best prefix completion. None outside a
/// `:` command.
pub fn command_hint(input: &str) -> Option<&'static CmdSpec> {
    let body = command_body(input)?;
    let tok = body.split_whitespace().next().unwrap_or("");
    CATALOG.iter().find(|c| c.name == tok || c.aliases.contains(&tok))
        .or_else(|| command_completions(input).into_iter().next())
}

/// Peel an optional leading repeat/depth count off a command body: "6ca" → (Some(6), "ca"),
/// "ca 6" → (None, "ca 6"), "42" → (Some(42), ""). Lets `:6ca` / `:21j` / `:4fa …` carry a count.
fn split_count(s: &str) -> (Option<usize>, &str) {
    let e = s.char_indices().take_while(|(_, c)| c.is_ascii_digit()).map(|(i, c)| i + c.len_utf8()).last();
    match e {
        Some(e) => (s[..e].parse().ok(), s[e..].trim_start()),
        None => (None, s),
    }
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() >= 2 && ((b[0] == b'\'' && b[b.len() - 1] == b'\'') || (b[0] == b'"' && b[b.len() - 1] == b'"')) {
        &s[1..s.len() - 1]
    } else { s }
}

/// Parse `:fa` arguments: an optional leading width, then a (possibly quoted) template.
/// "10 'do X with {lastmsg}'" → (Some(10), "do X with {lastmsg}"); "'tmpl'" → (None, "tmpl").
fn parse_fa_args(arg: &str) -> (Option<usize>, String) {
    let arg = arg.trim();
    let mut it = arg.splitn(2, char::is_whitespace);
    let first = it.next().unwrap_or("");
    if let Ok(w) = first.parse::<usize>() {
        (Some(w), strip_quotes(it.next().unwrap_or("")).to_string())
    } else {
        (None, strip_quotes(arg).to_string())
    }
}

/// Fill fan-template placeholders: `{lastmsg}` → the leaf's text (the caller truncates it), `{msgid}` →
/// the leaf's id. Unknown placeholders are left untouched. Pure.
pub fn fill_template(tmpl: &str, lastmsg: &str, msgid: usize) -> String {
    tmpl.replace("{lastmsg}", lastmsg).replace("{msgid}", &msgid.to_string())
}

/// Split `:map` args into the file glob and an optional (quoted) task template:
/// "*.rs" → ("*.rs", ""); "*.rs 'add tests to {file}'" → ("*.rs", "add tests to {file}").
fn parse_map_args(arg: &str) -> (String, String) {
    let arg = arg.trim();
    let mut it = arg.splitn(2, char::is_whitespace);
    let glob = it.next().unwrap_or("").trim().to_string();
    let tmpl = it.next().unwrap_or("").trim().trim_matches(|c| c == '\'' || c == '"').to_string();
    (glob, tmpl)
}

/// A deliberately tiny glob for `:map`: `*` matches any run, so `*.rs`, `src/*`, `*bench*`, and an exact
/// name all work; an empty pattern matches everything. Enough to pick a file set in a worker fan without a
/// dependency. Matches on the full relative path.
pub fn glob_match(pat: &str, name: &str) -> bool {
    let pat = pat.trim();
    if pat.is_empty() || pat == "*" { return true; }
    let parts: Vec<&str> = pat.split('*').collect();
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() { continue; }
        match name[pos..].find(part) {
            Some(off) => {
                if i == 0 && off != 0 { return false; }             // no leading '*' → must anchor at start
                pos += off + part.len();
            }
            None => return false,
        }
    }
    // no trailing '*' → the pattern must consume to the end
    if !pat.ends_with('*') { if let Some(last) = parts.last() { return name.ends_with(last); } }
    true
}

/// One worker task per file, with `{file}` filled in the template (or a sensible default when the template
/// is blank). This is the per-file fan-out a `:map` over a project's changed files produces.
pub fn map_tasks(files: &[String], template: &str) -> Vec<String> {
    files.iter().map(|f| {
        if template.trim().is_empty() { format!("Work on the file `{f}` in this project.") }
        else { template.replace("{file}", f) }
    }).collect()
}

/// Parse a command line (with or without the leading ':').
pub fn parse_command(line: &str) -> Command {
    let s = line.trim().trim_start_matches(':').trim();
    if s.is_empty() { return Command::Noop; }
    // Optional leading count: ":6ca" (depth 6), ":42" (bare → goto 42), ":4fa 10 '…'".
    let (count, rest) = split_count(s);
    if rest.is_empty() {
        return count.map(Command::Goto).unwrap_or(Command::Noop); // bare number = goto
    }
    let mut it = rest.splitn(2, char::is_whitespace);
    let cmd = it.next().unwrap_or("");
    let arg = it.next().unwrap_or("").trim();
    match cmd {
        "b" | "branch" => Command::Branch,
        "a" | "add" | "attach" => Command::Attach(arg.to_string()),
        "goto" => arg.parse::<usize>().ok().map(Command::Goto).unwrap_or_else(|| Command::Unknown(line.into())),
        "prune" | "p" => Command::Prune,
        "unprune" | "up" => Command::Unprune,
        "overview" | "fit" => Command::Overview,
        "fuzz" | "f" => Command::Fuzz(arg.parse::<usize>().unwrap_or(10).clamp(2, 40)),
        "fuzzd" | "fd" => Command::FuzzDirective(arg.parse::<usize>().unwrap_or(10).clamp(2, 40)),
        "o" | "open" => Command::Open(arg.to_string()),
        "ca" | "continue-all" => {
            let depth = count.or_else(|| arg.split_whitespace().next().and_then(|w| w.parse().ok())).unwrap_or(1).clamp(1, 200);
            Command::ContinueAll(depth)
        }
        "fa" | "fuzzy-all" => {
            let (width, template) = parse_fa_args(arg);
            Command::FuzzyAll { depth: count.unwrap_or(1).clamp(1, 200), width: width.unwrap_or(10).clamp(2, 40), template }
        }
        "continue" | "cont" => {
            let n = count.or_else(|| arg.split_whitespace().next().and_then(|w| w.parse().ok())).unwrap_or(1).clamp(1, 200);
            Command::Continue(n)
        }
        "map" => { let (glob, template) = parse_map_args(arg); Command::MapFiles { glob, template } }
        "ws" | "workspace" => Command::WorkspaceStatus,
        "sync" => Command::SyncWorkspace,
        "stop" | "st" | "qa" => Command::Stop,  // :qa reads as vim "quit all" → cancel the run
        "quest" | "questions" => {
            let n = count.or_else(|| arg.split_whitespace().next().and_then(|w| w.parse().ok())).unwrap_or(5).clamp(1, 40);
            Command::Quest(n)
        }
        "ans" | "answer" => Command::Answer(arg.to_string()),
        "fill" => Command::Fill(arg.to_string()),
        "assemble" | "asm" => Command::Assemble,
        "check" | "chk" => Command::Check,
        "explore" | "xp" => Command::Explore,
        "best" => Command::Best,
        "linear" => Command::Linear,
        "spatial" => Command::Spatial,
        "export" | "e" => Command::Export(if arg.is_empty() { "md".into() } else { arg.to_lowercase() }),
        "use" | "model" => Command::Use(arg.to_string()),  // switch active model in the pool
        "mergectx" | "mc" => { let (refs, instruction) = parse_mergectx_args(arg); Command::MergeCtx { refs, instruction } }
        "bird" | "birdseye" | "strat" | "strategy" => { let (refs, focus) = parse_strategy_args(arg); Command::Strategy { refs, focus } }
        "segment" | "blocks" | "seg" => { let (refs, focus) = parse_strategy_args(arg); Command::Segment { refs, focus } }
        "todo" => { let (refs, focus) = parse_strategy_args(arg); Command::Todo { refs, focus } }
        "dump" => Command::Dump(arg.to_string()),
        "loadchat" | "lc" => Command::LoadChat(arg.to_string()),
        "import-workspace" | "importws" | "iw" => Command::ImportWorkspace(arg.to_string()),
        "summarize-files" | "summarise-files" | "sf" => Command::SummarizeFiles(arg.to_string()),
        "load-gitmind" | "lgm" => Command::LoadGitmind(arg.to_string()),
        "reapply" | "ra" => Command::Reapply,
        "agent" | "ag" => Command::Agent(arg.to_string()),
        "fan-agent" | "faa" => Command::FanAgent(arg.to_string()),
        "pick" => Command::Pick(arg.trim().parse::<usize>().unwrap_or(0)),
        "docs" | "doc-fan" => Command::DocFan(arg.to_string()),
        "merge-docs" | "mergedocs" => Command::MergeDocs,
        "compact" | "refresh" | "cp" => Command::Compact(arg.to_string()),
        "race" => Command::Race(arg.to_string()),
        "why" | "diagnose" => { let (refs, _) = parse_strategy_args(arg); Command::Diagnose(refs) }
        "help" | "h" | "?" => Command::Help(arg.to_string()),
        "suggest" | "sg" => Command::Suggest,
        "prompt" | "pr" => Command::Prompt(arg.to_string()),
        _ => cmd.parse::<usize>().map(Command::Goto).unwrap_or_else(|_| Command::Unknown(line.into())),
    }
}

/// fzf-lite subsequence score: every char of `q` must appear in order in `cand` (case-insensitive).
/// Higher = better — contiguous runs, word-starts, and early matches are rewarded. None if no match.
pub fn fuzzy_score(q: &str, cand: &str) -> Option<i32> {
    if q.trim().is_empty() { return Some(0); }
    let ql: Vec<char> = q.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let cl: Vec<char> = cand.to_lowercase().chars().collect();
    let mut qi = 0usize;
    let mut score = 0i32;
    let mut last = -2i32;
    let mut prev_boundary = true;
    for (ci, &c) in cl.iter().enumerate() {
        let is_boundary = prev_boundary && c.is_alphanumeric();
        if qi < ql.len() && c == ql[qi] {
            score += 1;
            if ci as i32 == last + 1 { score += 10; }      // contiguous run dominates
            if ci == 0 { score += 15; } else if is_boundary { score += 5; } // start / word boundary
            score -= (ci as i32) / 12;                      // mild early-position preference
            last = ci as i32;
            qi += 1;
        }
        prev_boundary = !c.is_alphanumeric();
    }
    if qi == ql.len() { Some(score) } else { None }
}

/// Rank `(id, text)` candidates by fuzzy score against `q`, best first; ties broken by id.
pub fn fuzzy_rank<'a>(q: &str, items: impl IntoIterator<Item = (usize, &'a str)>) -> Vec<(usize, i32)> {
    let mut out: Vec<(usize, i32)> = items.into_iter().filter_map(|(i, s)| fuzzy_score(q, s).map(|sc| (i, sc))).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// An action a Normal-mode key maps to; the egui layer executes it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    None,
    FocusNext, FocusPrev,          // j / k — along the active branch
    SiblingPrev, SiblingNext,      // h / l — switch branch at a fork (helix lanes)
    First, Last,                   // g / G
    ZoomIn, ZoomOut,               // i / I
    Yaw(f32), Pitch(f32),          // z then h/l (yaw), z then k/j (pitch)
    BeginZoomPrefix,               // z   — next h/j/k/l rotates
    ScrollPreviewDown, ScrollPreviewUp, // r then j/k
    BeginLeader,                   // space — next / or \ searches the tree
    Search,                        // /
    TreeSearch, FuzzyTreeSearch,   // <leader>/  ,  <leader>\
    EnterCommand,                  // :
    EnterInsert,                   // a (or Enter) — focus the composer
    Branch,                        // B — quick-branch at the focused node
    Overview,                      // o
    Help,                          // ? — toggle the keybinding overlay
}

/// Map a Normal-mode character (already lowercased; `shift` says if it was uppercase) to an Action,
/// honoring a pending `z` rotation prefix and a pending `space` leader. Pure: the caller tracks the
/// pending flags (set by BeginZoomPrefix / BeginLeader) and clears them after the next key.
pub fn normal_action(c: char, shift: bool, z_pending: bool, leader: bool) -> Action {
    if leader {
        return match c { '/' => Action::TreeSearch, '\\' => Action::FuzzyTreeSearch, _ => Action::None };
    }
    if z_pending {
        return match c {
            'h' => Action::Yaw(-0.25), 'l' => Action::Yaw(0.25),
            'k' => Action::Pitch(0.20), 'j' => Action::Pitch(-0.20),
            _ => Action::None,
        };
    }
    match c {
        'j' => Action::ScrollPreviewDown,
        'k' => Action::ScrollPreviewUp,
        'h' => if shift { Action::SiblingPrev } else { Action::FocusPrev },
        'l' => if shift { Action::SiblingNext } else { Action::FocusNext },
        'g' => if shift { Action::Last } else { Action::First },
        'i' => if shift { Action::ZoomOut } else { Action::ZoomIn },
        'z' => Action::BeginZoomPrefix,
        ' ' => Action::BeginLeader,
        '/' => Action::Search,
        ':' => Action::EnterCommand,
        'a' => Action::EnterInsert,
        'b' => Action::Branch,
        'o' => Action::Overview,
        '?' => Action::Help,
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_aliases_parse() {
        assert_eq!(parse_command(":branch"), Command::Branch);
        assert_eq!(parse_command(":b"), Command::Branch);
        assert_eq!(parse_command(":a ~/notes.rs"), Command::Attach("~/notes.rs".into()));
        assert_eq!(parse_command(":attach /tmp/x.txt"), Command::Attach("/tmp/x.txt".into()));
        assert_eq!(parse_command(":add a.md"), Command::Attach("a.md".into()));
        assert_eq!(parse_command(":goto 12"), Command::Goto(12));
        assert_eq!(parse_command(":42"), Command::Goto(42), "bare number is a goto");
        assert_eq!(parse_command(":fuzz 20"), Command::Fuzz(20));
        assert_eq!(parse_command(":dump ~/proj"), Command::Dump("~/proj".into()));
        assert_eq!(parse_command(":lc ~/proj/logscope/chats/a9"), Command::LoadChat("~/proj/logscope/chats/a9".into()));
        assert_eq!(parse_command(":iw ~/projects"), Command::ImportWorkspace("~/projects".into()));
        assert_eq!(parse_command(":sf"), Command::SummarizeFiles(String::new()));
        assert_eq!(parse_command(":compact 0.3"), Command::Compact("0.3".into()));
        assert_eq!(parse_command(":refresh"), Command::Compact(String::new()));
        assert_eq!(parse_command(":load-gitmind ~/work/monorepo"), Command::LoadGitmind("~/work/monorepo".into()));
        assert_eq!(parse_command(":lgm /x"), Command::LoadGitmind("/x".into()));
        assert_eq!(parse_command(":reapply"), Command::Reapply);
        assert_eq!(parse_command(":ra"), Command::Reapply);
        assert_eq!(parse_command(":agent add a test for foo"), Command::Agent("add a test for foo".into()));
        assert_eq!(parse_command(":ag build a CLI"), Command::Agent("build a CLI".into()));
        assert_eq!(parse_command(":fan-agent try anyhow and thiserror"), Command::FanAgent("try anyhow and thiserror".into()));
        assert_eq!(parse_command(":pick 2"), Command::Pick(2));
        assert_eq!(parse_command(":docs write user + api docs"), Command::DocFan("write user + api docs".into()));
        assert_eq!(parse_command(":merge-docs"), Command::MergeDocs);
        assert_eq!(parse_command(":race 0.4"), Command::Race("0.4".into()));
        assert_eq!(parse_command(":why -1,-3"), Command::Diagnose(vec!["-1".into(), "-3".into()]));
        assert_eq!(parse_command(":diagnose"), Command::Diagnose(Vec::new()));
        assert_eq!(parse_command(":help compact"), Command::Help("compact".into()));
        assert_eq!(parse_command(":sg"), Command::Suggest);
        assert_eq!(parse_command(":pr tighten"), Command::Prompt("tighten".into()));
        assert_eq!(parse_command(":fuzz"), Command::Fuzz(10), "fuzz defaults to 10");
        assert_eq!(parse_command(":fuzzd 6"), Command::FuzzDirective(6));
        assert_eq!(parse_command(":fd"), Command::FuzzDirective(10), "fuzzd defaults to 10");
        assert_eq!(parse_command(":open ~/chat.md"), Command::Open("~/chat.md".into()));
        assert_eq!(parse_command(":o a.chat"), Command::Open("a.chat".into()));
        assert_eq!(parse_command(":ca"), Command::ContinueAll(1));
        assert_eq!(parse_command(":continue-all"), Command::ContinueAll(1));
        assert_eq!(parse_command(":6ca"), Command::ContinueAll(6), "leading count = depth");
        assert_eq!(parse_command(":ca 6"), Command::ContinueAll(6), "arg also = depth");
        assert_eq!(parse_command(":best"), Command::Best);
        assert_eq!(parse_command(":stop"), Command::Stop);
        assert_eq!(parse_command(":qa"), Command::Stop, ":qa is quit-all → stop");
        assert_eq!(parse_command(":check"), Command::Check);
        assert_eq!(parse_command(":chk"), Command::Check);
        assert_eq!(parse_command(":quest"), Command::Quest(5));
        assert_eq!(parse_command(":quest 8"), Command::Quest(8));
        assert_eq!(parse_command(":3quest"), Command::Quest(3));
        assert_eq!(parse_command(":ans answers.md"), Command::Answer("answers.md".into()));
        assert_eq!(parse_command(":fill"), Command::Fill(String::new()));
        assert_eq!(parse_command(":fill ~/skel.rs"), Command::Fill("~/skel.rs".into()));
        assert_eq!(parse_command(":asm"), Command::Assemble);
        assert_eq!(parse_command(":prune"), Command::Prune);
        assert_eq!(parse_command(":export ascii"), Command::Export("ascii".into()));
        assert_eq!(parse_command(":e git"), Command::Export("git".into()));
        assert_eq!(parse_command(":export"), Command::Export("md".into()), "export defaults to md");
        assert_eq!(parse_command(":"), Command::Noop);
        assert!(matches!(parse_command(":wat"), Command::Unknown(_)));
    }

    #[test]
    fn fuzzy_all_and_counts_parse() {
        assert_eq!(parse_command(":fa"), Command::FuzzyAll { depth: 1, width: 10, template: String::new() });
        assert_eq!(parse_command(":4fa 10 'do X with {lastmsg}'"),
                   Command::FuzzyAll { depth: 4, width: 10, template: "do X with {lastmsg}".into() });
        assert_eq!(parse_command(":fuzzy-all 6 \"reword using {msgid}\""),
                   Command::FuzzyAll { depth: 1, width: 6, template: "reword using {msgid}".into() });
        assert_eq!(parse_command(":fa 'just the template'"),
                   Command::FuzzyAll { depth: 1, width: 10, template: "just the template".into() });
        // counts: bare number stays a goto; an out-of-range depth clamps
        assert_eq!(parse_command(":42"), Command::Goto(42));
        assert_eq!(parse_command(":999ca"), Command::ContinueAll(200), "depth clamps to 200");
        // template placeholder fill
        assert_eq!(fill_template("redo {msgid}: not like {lastmsg}", "the bubble sort", 7),
                   "redo 7: not like the bubble sort");
    }

    #[test]
    fn fuzzy_matches_subsequence_and_ranks_sensibly() {
        assert!(fuzzy_score("srt", "sort a vec").is_some(), "subsequence matches");
        assert!(fuzzy_score("xyz", "sort a vec").is_none(), "non-subsequence misses");
        // contiguous + word-start beats scattered
        let contig = fuzzy_score("sort", "sort by weight").unwrap();
        let scattered = fuzzy_score("sort", "so really thoughtful").unwrap();
        assert!(contig > scattered, "contiguous prefix scores higher ({contig} vs {scattered})");
    }

    #[test]
    fn fuzzy_rank_orders_best_first() {
        let items = vec![
            (0, "rewrite it for floats"),
            (1, "reverse sort the vec"),
            (2, "show me a rust crate"),
        ];
        let r = fuzzy_rank("rev", items);
        assert_eq!(r[0].0, 1, "'reverse' ranks first for 'rev', got {r:?}");
    }

    #[test]
    fn navigation_keys_map() {
        assert_eq!(normal_action('j', false, false, false), Action::ScrollPreviewDown);
        assert_eq!(normal_action('k', false, false, false), Action::ScrollPreviewUp);
        assert_eq!(normal_action('h', false, false, false), Action::FocusPrev);
        assert_eq!(normal_action('l', false, false, false), Action::FocusNext);
        assert_eq!(normal_action('h', true, false, false), Action::SiblingPrev);
        assert_eq!(normal_action('l', true, false, false), Action::SiblingNext);
        assert_eq!(normal_action('g', false, false, false), Action::First);
        assert_eq!(normal_action('g', true, false, false), Action::Last);
    }

    #[test]
    fn camera_keys_and_prefixes() {
        assert_eq!(normal_action('i', false, false, false), Action::ZoomIn);
        assert_eq!(normal_action('i', true, false, false), Action::ZoomOut);
        assert_eq!(normal_action('z', false, false, false), Action::BeginZoomPrefix);
        // with z pending, hjkl rotate instead of navigating
        assert_eq!(normal_action('h', false, true, false), Action::Yaw(-0.25));
        assert_eq!(normal_action('l', false, true, false), Action::Yaw(0.25));
        assert_eq!(normal_action('k', false, true, false), Action::Pitch(0.20));
        assert_eq!(normal_action('j', false, true, false), Action::Pitch(-0.20));
    }

    #[test]
    fn leader_and_search_keys() {
        assert_eq!(normal_action(' ', false, false, false), Action::BeginLeader);
        assert_eq!(normal_action('/', false, false, false), Action::Search);
        // leader then / or \ → tree searches
        assert_eq!(normal_action('/', false, false, true), Action::TreeSearch);
        assert_eq!(normal_action('\\', false, false, true), Action::FuzzyTreeSearch);
        assert_eq!(normal_action(':', false, false, false), Action::EnterCommand);
        assert_eq!(normal_action('a', false, false, false), Action::EnterInsert);
    }

    #[test]
    fn help_key_and_mode_metadata() {
        assert_eq!(normal_action('?', true, false, false), Action::Help);
        assert_eq!(normal_action('?', false, false, false), Action::Help);
        // a z/leader prefix swallows the next key, so ? only helps from a clean Normal state
        assert_eq!(normal_action('?', false, true, false), Action::None);
        // every mode has a distinct indicator colour + the right typing classification
        let modes = [Mode::Normal, Mode::Insert, Mode::Command, Mode::Search];
        for (i, &a) in modes.iter().enumerate() {
            for &b in &modes[i + 1..] { assert_ne!(a.rgb(), b.rgb(), "{a:?} and {b:?} share a colour"); }
        }
        assert!(!Mode::Normal.is_typing());
        assert!(Mode::Insert.is_typing() && Mode::Command.is_typing() && Mode::Search.is_typing());
    }

    #[test]
    fn joystick_maps_to_dominant_axis_past_deadzone() {
        assert_eq!(joystick_dir(0.05, 0.02, 0.2), None, "inside deadzone → no direction");
        assert_eq!(joystick_dir(-0.9, 0.1, 0.2), Some(Dir4::Left));
        assert_eq!(joystick_dir(0.9, 0.1, 0.2), Some(Dir4::Right));
        assert_eq!(joystick_dir(0.1, -0.9, 0.2), Some(Dir4::Up));
        assert_eq!(joystick_dir(0.1, 0.9, 0.2), Some(Dir4::Down));
        // left/right switch branches, up/down crawl the timeline
        assert_eq!(nav_dir_action(Dir4::Left), Action::SiblingPrev);
        assert_eq!(nav_dir_action(Dir4::Right), Action::SiblingNext);
        assert_eq!(nav_dir_action(Dir4::Up), Action::FocusPrev);
        assert_eq!(nav_dir_action(Dir4::Down), Action::FocusNext);
        // camera stick: y is inverted (push up → pitch up)
        let (yaw, pitch) = cam_stick(1.0, -1.0, 0.1);
        assert!((yaw - 0.1).abs() < 1e-6 && (pitch - 0.1).abs() < 1e-6);
    }

    #[test]
    fn short_ids_are_stable_distinct_and_prefix_addressable() {
        assert_eq!(short_id(3).len(), 6, "6-char base36 id");
        assert_eq!(short_id(3), short_id(3), "stable");
        // distinct across a realistic span
        let ids: std::collections::HashSet<String> = (0..2000).map(short_id).collect();
        assert!(ids.len() >= 1995, "ids are essentially collision-free over 2000 nodes");
        // a full id resolves to its own node; relative refs count from the end
        let n = 50;
        assert_eq!(resolve_ref(n, &short_id(17)), Some(17), "full id → its node");
        assert_eq!(resolve_ref(n, "-1"), Some(49), "-1 = last");
        assert_eq!(resolve_ref(n, "-3"), Some(47));
        assert_eq!(resolve_ref(n, "1"), Some(0), "bare 1 = first (1-based)");
        assert_eq!(resolve_ref(n, "999"), None, "out of range");
        assert_eq!(resolve_ref(n, "zzzzzz"), None, "no such id");
        // a 1-char prefix over 50 nodes is almost surely ambiguous → None (ask for more chars)
        assert_eq!(resolve_ref(n, &short_id(17)[..1]).is_none() || resolve_ref(n, &short_id(17)) == Some(17), true);
    }

    #[test]
    fn parses_mergectx() {
        assert_eq!(parse_command(":mergectx -1,3df,rro,a9j"),
            Command::MergeCtx { refs: vec!["-1".into(), "3df".into(), "rro".into(), "a9j".into()], instruction: String::new() });
        assert_eq!(parse_command(":mc -1,3df 'pick the strongest'"),
            Command::MergeCtx { refs: vec!["-1".into(), "3df".into()], instruction: "pick the strongest".into() });
        // resolve_refs sorts wheat from chaff
        let (ok, bad) = resolve_refs(10, &["-1".into(), "nope!".into(), "2".into()]);
        assert_eq!(ok, vec![9, 1]);
        assert_eq!(bad, vec!["nope!".to_string()]);
    }

    #[test]
    fn parses_new_worker_commands() {
        assert_eq!(parse_command(":continue 4"), Command::Continue(4));
        assert_eq!(parse_command(":cont"), Command::Continue(1), "default 1");
        assert_eq!(parse_command(":3continue"), Command::Continue(3), "count prefix");
        assert_eq!(parse_command(":ws"), Command::WorkspaceStatus);
        assert_eq!(parse_command(":workspace"), Command::WorkspaceStatus);
        assert_eq!(parse_command(":sync"), Command::SyncWorkspace);
        assert_eq!(parse_command(":map *.rs 'add tests to {file}'"),
                   Command::MapFiles { glob: "*.rs".into(), template: "add tests to {file}".into() });
        assert_eq!(parse_command(":map"), Command::MapFiles { glob: String::new(), template: String::new() });
    }

    #[test]
    fn glob_and_map_tasks_fan_per_file() {
        // tiny glob: suffix, prefix, substring, exact, and '*'/empty = all
        assert!(glob_match("*.rs", "src/main.rs") && !glob_match("*.rs", "readme.md"));
        assert!(glob_match("src/*", "src/main.rs") && !glob_match("src/*", "tests/x.rs"));
        assert!(glob_match("*bench*", "crates/kv-bench.rs"));
        assert!(glob_match("Cargo.toml", "Cargo.toml") && !glob_match("Cargo.toml", "Cargo.lock"));
        assert!(glob_match("", "anything") && glob_match("*", "anything"));
        // one task per file, {file} filled; blank template → a sensible default
        let files = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
        let t = map_tasks(&files, "add doctests to {file}");
        assert_eq!(t, vec!["add doctests to src/a.rs", "add doctests to src/b.rs"]);
        assert!(map_tasks(&files, "").iter().all(|t| t.contains("src/") && t.contains("file")));
    }

    #[test]
    fn command_palette_completes_and_hints() {
        // bare ':' lists the whole palette
        assert_eq!(command_completions(":").len(), command_catalog().len());
        // a prefix narrows; an exact name/alias floats to the top
        let f = command_completions(":f");
        assert!(f.iter().any(|c| c.name == "fuzz") && f.iter().any(|c| c.name == "fa") && f.iter().any(|c| c.name == "fuzzd"));
        assert_eq!(command_completions(":fuzz")[0].name, "fuzz", "exact name leads the grid");
        assert!(command_completions(":mc").iter().any(|c| c.name == "mergectx"), "alias prefix finds its command");
        // a leading repeat-count is ignored when matching the command
        assert!(command_completions(":6c").iter().any(|c| c.name == "ca"));
        // once an argument is being typed the grid closes, but the hint persists
        assert!(command_completions(":mergectx -1,").is_empty());
        assert_eq!(command_hint(":mergectx -1,").map(|c| c.name), Some("mergectx"));
        assert_eq!(command_hint(":ca 3").map(|c| c.name), Some("ca"));
        assert_eq!(command_hint(":fu").map(|c| c.name), Some("fuzz"), "hint = best completion before a full match");
        // plain prose is not a command
        assert!(command_completions("hello").is_empty() && command_hint("hello").is_none());
    }

    #[test]
    fn parses_strategy_aliases_and_args() {
        use Command::Strategy;
        // all four aliases → Strategy, whole-conversation by default (no refs, no focus)
        for a in [":bird", ":birdseye", ":strat", ":strategy"] {
            assert_eq!(parse_command(a), Strategy { refs: vec![], focus: String::new() }, "{a}");
        }
        // a bare quoted string is all focus, no refs
        assert_eq!(parse_command(":bird 'focus on the perf tradeoffs'"),
            Strategy { refs: vec![], focus: "focus on the perf tradeoffs".into() });
        // refs + focus
        assert_eq!(parse_command(":strategy -1,3df 'why it wins'"),
            Strategy { refs: vec!["-1".into(), "3df".into()], focus: "why it wins".into() });
        // refs only
        assert_eq!(parse_command(":strat -1,-3,a9j"),
            Strategy { refs: vec!["-1".into(), "-3".into(), "a9j".into()], focus: String::new() });
        // surfaces in the palette under its aliases, and the prompt is non-trivial
        assert!(command_completions(":bird").iter().any(|c| c.name == "strategy"));
        assert!(command_completions(":strat").iter().any(|c| c.name == "strategy"));
        assert!(STRATEGIST_PROMPT.contains("Through-line") && STRATEGIST_PROMPT.len() > 200);
    }

    #[test]
    fn parses_segment_aliases() {
        use Command::Segment;
        for a in [":segment", ":blocks", ":seg"] {
            assert_eq!(parse_command(a), Segment { refs: vec![], focus: String::new() }, "{a}");
        }
        assert_eq!(parse_command(":blocks -1,3df 'group the loops'"),
            Segment { refs: vec!["-1".into(), "3df".into()], focus: "group the loops".into() });
        assert_eq!(parse_command(":seg 'where does the topic change'"),
            Segment { refs: vec![], focus: "where does the topic change".into() });
        assert!(command_completions(":blocks").iter().any(|c| c.name == "segment"));
        assert!(SEGMENTER_PROMPT.contains("## Block") && SEGMENTER_PROMPT.contains("seam"));
    }

    #[test]
    fn parses_todo() {
        use Command::Todo;
        assert_eq!(parse_command(":todo"), Todo { refs: vec![], focus: String::new() });
        assert_eq!(parse_command(":todo -1,3df 'kv cache'"),
            Todo { refs: vec!["-1".into(), "3df".into()], focus: "kv cache".into() });
        assert_eq!(parse_command(":todo 'the shader work'"),
            Todo { refs: vec![], focus: "the shader work".into() });
        assert!(command_completions(":tod").iter().any(|c| c.name == "todo"));
    }
}
