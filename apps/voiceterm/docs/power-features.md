# Power features

What it takes for web2qml's terminal to feel like a place where
real work gets done — not just a friendly learning surface for
newcomers, but a thinking environment a power user actually
chooses to spend time in.

Status: **ACTIVE ROADMAP**. This is the doc the next several
terminal-side ships will reference. It is the counterpoint to
[bystanders.md](bystanders.md), which is shelved; power features
are the priority right now.

For the existing terminal infrastructure this builds on, see
[terminal.md](terminal.md).


## Why this exists

Web2qml's voice-first identity is real and differentiating, but
voice without power undersells the terminal. Power users won't
switch to a worse-but-talkier terminal. The voice and the power
have to coexist or the app stays a curiosity.

The honest read of where the terminal stands today:

The **foundations are correct**. Real PTY via portable_pty, real
ANSI parser, real cell grid with wide-char support, alt-screen
detection, mouse reporting passthrough, bracketed paste,
scrollback with absolute-line anchoring, block tracking, smart-
cd undo, smart token recognition for paths, AI-assisted block
summaries. None of this is fake or hand-waved.

The **power-user surfaces are sparse**. No scrollback search.
No splits or multi-pane. No session persistence. No
notifications on long-running command completion. No first-
class git integration. Find is rudimentary (in-page only, no
result navigation, no terminal-scrollback or filesystem
modes). Keybindings are limited and not extensible.

The gap isn't in the bones — it's in the *surfaces a power
user reaches for reflexively* and finds missing. This doc
catalogs those gaps and proposes how to close them.


## The strategic framing

Three principles shape what we build and how:

### 1. Thinking surface, not workshop

Web2qml isn't trying to replace konsole, alacritty, kitty, or
wezterm. Those are workshops — places where the user already
knows what they're doing and wants the tool to get out of the
way.

Web2qml is a **thinking surface**. The voice channel makes
narration native. The block visual separates commands from
output for review. The AI summaries augment recall. The smart
tokens make output interactive. Together these turn the
terminal into a place where the user *figures things out* —
exploring a new codebase, working through a build error,
learning an unfamiliar tool, demonstrating something to a
colleague.

A power user comes here when they want to *think with the
terminal*. They go back to their workshop terminal when they
want to *execute*.

### 2. Teaching surface

The features that look like "training wheels" from a solo
power-user perspective become **pedagogical affordances** when
a mentor is teaching a mentee in the same session.

- Voice narration → the teacher explains what they're doing
- Block visual → the learner can follow command-vs-output
- AI summary → augments the teacher's explanation
- Smart tokens → the learner can click into things being
  mentioned
- Multi-speaker awareness (bystanders, deferred) → the system
  knows who's teaching and who's learning

The right way to think about audience isn't "power user vs
learner." It's **the pair**. Mentor + mentee, both in the
room. The friendly affordances aren't *for* the learner — they
let the teacher *teach better*.

This positions web2qml uniquely. A power user who would never
use it solo might still use it when teaching someone, OR when
recording a screencast, OR when pair-programming, OR when
demonstrating to a colleague. The user said it directly: "even
if I want to use konsole, this is how I can share what I do
with others."

### 3. Hand-off friendly, export over lock-in

Everything web2qml lets the user do should leave **usable
artifacts in their actual filesystem and shell history**, so
they can keep working in their preferred tools after the
session.

- Commands executed via voice are appended to the actual
  `~/.bash_history` / `~/.zsh_history`
- Saved command outputs go to predictable local paths
- Knowledge extracted via browser-side AI summaries go to local
  markdown files
- Workspace state is mirrored as files the user can read /
  symlink / version-control / sync

The user goes back to their workshop terminal and finds web2qml
left them *equipped*, not *dependent*. This is the inverse of
the typical SaaS lock-in pattern. The morning-coffee scenario
the user described — talking to the computer for an hour and
walking away with a head start in their normal toolchain — is
the canonical use case.


## The gap audit

Catalog of what's missing relative to power-user expectations.
Roughly ordered by impact vs effort.

### High-impact, small-to-medium effort

#### Scrollback search

Power users hit `Ctrl+F` or `/` reflexively to find earlier
output. Web2qml today passes `Ctrl+F` through to the PTY (as
byte 0x06) on the terminal tab, which is correct for shells
and TUIs that bind it — but means there's no native scrollback
search. Find that command output from 20 minutes ago: stuck.

What it needs:
- A universal find bar (see "Find as a verb" below) that on
  the terminal tab searches the scrollback by default
- Result navigation (next/prev, n/N like vim)
- Result highlighting in the rendered scrollback
- Case-sensitivity + regex toggles
- Voice command: "find errors" / "find the cargo output"

The scrollback already lives in `pty_term.rs::scrollback: Vec<Vec<Cell>>`,
so the data is there. The work is parser + UI.

#### Long-running command completion notifications

The block tracker already knows when a block ends — that's the
existing infrastructure for AI summaries. Surfacing a toast (and
optional sound) when a block of duration > N seconds finishes
*while the user is on another tab* is a small, high-impact ship.

Pattern: user runs `cargo build`, switches to the browser tab to
read docs, build finishes, toast pings them. They don't have to
keep checking back.

Tunable: which durations qualify (default 10s), notification
style (toast / system notification / sound), and per-command
overrides (always notify for `cargo test`, never for `ls`).

#### Git first-class integration

Currently just hardcoded palette entries (`git status`, `git add
.`, `git commit -m "update"`, `git log --oneline -10`, `git
diff`). That's it.

Easy wins:

- **Auto-detect git repo at cwd**, surface state in a small
  badge near the prompt or in a status area. Branch name,
  dirty/clean, ahead/behind. Updates on every block-end. Voice:
  "what branch am I on" → reads the badge state aloud.
- **Smart git command parsing.** "Show me the diff" → `git
  diff`. "What changed today" → `git log --since=midnight
  --oneline`. "Switch to main" → `git checkout main`. "Undo my
  last commit" → `git reset --soft HEAD~1` with a confirmation
  toast.
- **Git output integration with block tracker.** `git status`
  output should highlight files clickably (the smart-token
  code already exists for paths; needs to fire on git output).
  `git log` output gets clickable commit hashes. `git diff`
  output gets syntax-aware coloring via the summary path.
- **Branch-aware shell history surfacing.** Last time the user
  was on this branch, what did they run? Surface as palette
  suggestions on branch switch. This is the *standout* feature
  — nobody really does it and it's incredibly powerful for
  context-switching between branches/projects.
- **Pre-commit hook awareness.** "Commit this" → checks if hooks
  would block, runs them, shows results, then commits or
  surfaces failures.

Voice + git is a natural pair because git commands are verbose
("git reset --soft HEAD~1 -- src/foo.rs") and that's exactly
what voice can compress into "undo my last commit, just the
foo file."

#### Find as a universal verb

Today's find: in-page only on browser, no result navigation, no
regex, no terminal mode, no filesystem mode. The user called it
"very ugly and very basic."

The right principle: **find is one verb that adapts to what
you're looking at.**

- Browser tab → in-page find (current behavior, but with proper
  result navigation, highlighting, jumps)
- Terminal tab → scrollback search (see above)
- Filesystem mode → built-in ripgrep wrapper, results as
  clickable smart tokens
- "Find this in code AND in my notes" → cross-surface search

The user's phrase "sharing with greps somehow" hints at a
deeper idea: filesystem search results should be **first-class
results that integrate with the terminal**. The user finds a
match, taps to open in their editor (`vim file:line`), or
pipes the results into another command. The find surface
becomes a command-input source, not just a viewer.

Voice integration: "find foo" opens the bar with "foo". "Find
errors in the last build" → terminal scrollback search,
filtered to the last block. "Find every TODO in this repo" →
ripgrep mode on the cwd.

#### Keybinding audit + the missing essentials

The user named: arrow keys, multi-tab, minimize/maximize, "plus
others I probably don't know." Most of these probably work but
have edge cases. The audit:

**Terminal byte-passthrough (should all work, audit anyway):**
- Arrow keys, PgUp/Dn, Home/End — for shell readline and TUIs
- Ctrl+arrow — word navigation in readline
- Ctrl+C/D/Z/U/W/L/R — terminal control bytes
- Alt+F/B — word jump in readline
- Tab — completion in shell

**Web2qml-level keybindings (should exist, audit gaps):**
- Ctrl+Tab / Ctrl+Shift+Tab — switch tab forward/back
- Ctrl+1..9 — jump to tab N
- Ctrl+T — new tab
- Ctrl+W — close tab
- Ctrl+Shift+T — reopen closed tab
- F11 — toggle fullscreen
- Ctrl+= / Ctrl+- / Ctrl+0 — font size up/down/reset
- Window controls: minimize / maximize (Qt window decoration
  defaults, but make sure they actually work)

The shift here isn't only about adding chords — it's about
**making the chord surface visible**. A power user new to the
app should be able to discover the bindings without reading
docs: a `?` shortcut that overlays all current shortcuts on the
screen, ordered by category.

### High-impact, large effort

#### Sessions and workspaces

This is the foundational ship for the morning-coffee scenario.
The user spends an hour talking + browsing + running commands
in a directory. What comes out?

The user pushed back on my framing here, and they're right:
*"power users try to capture full state so they can resume the
thinking they had."* Block-tracker artifacts persisted to disk
are useful, but they're a *thin* state — the commands and
outputs and summaries. The user's *mental state* — what they
were exploring, what hypotheses they had, where they got stuck
— is what they actually want to resume.

The challenge: state-as-context is inherently fuzzy. AI
summaries help (each block has a summary that captures
"what we just did"). The voice transcripts of the session
help (they capture what the user said *about* what they were
doing). Together they're a richer-than-shell-history record
of the thinking.

The user also raised a really good question — *"what do
browsers do as well since we have a lot of state and we might
want to save playgrounds and card states"* — which expands the
scope. Workspace persistence isn't just terminal; it's
browser tabs, card canvas state, niftyplot drawings, voice
intent cache, paragraph selections, reading positions.

A simple-modeled, efficient first cut:

- **Workspace = directory association**. Opening web2qml in
  `~/Code/foo` activates "foo's workspace." All state changes
  during this session get tagged with that workspace.
- **Persisted records** (JSON-lines for grep-ability, SQLite for
  query if it gets big — start with JSONL):
  - Terminal blocks (commands + summaries, NOT full output —
    too heavy; offer "expand to load full output from
    timestamped file")
  - Browser tab history within this session
  - Card canvas snapshots
  - Voice transcripts (with speaker attribution)
  - User-saved files / outputs ("save this output" voice
    command)
  - Cwd + git branch at each meaningful moment
- **Storage**: `~/.config/web2qml/workspaces/<cwd-hash>/<timestamp>.jsonl`
  for the session log, with full outputs in a sibling
  `outputs/<block-id>.txt`.
- **Portability layer**: a `<timestamp>.zsh-functions` /
  `<timestamp>.bash-functions` file alongside the session log,
  capturing any aliases or one-liners the user developed in
  the session. The user `source`s this in their normal shell
  later. The shell history append goes to actual `~/.bash_history`
  for `Ctrl+R` retrieval in konsole.
- **On reopening**: scan workspaces matching the cwd, show
  "you have 12 prior sessions here, last touched 3 days ago"
  as a small affordance. One tap loads the most recent
  session's blocks into the scrollback area (display-only, the
  PTY is fresh). Voice replay: "show me what I did yesterday
  here."

The user's "what do browsers do" question is the right
prompt. Browsers persist:
- Tab state (open URLs, scroll positions)
- History (URLs visited, when)
- Cookies / local storage (per-site state)
- Form data (per-form recall)
- Bookmarks (user-curated)

The browser model maps cleanly:
- Tabs ↔ web2qml tabs
- History ↔ block tracker + voice transcripts
- Per-site state ↔ per-workspace state
- Form data ↔ partial command typing recovery (already exists
  for the command bar; could extend)
- Bookmarks ↔ user-saved command palette additions

The web2qml difference: terminal state is *fundamentally
process-bound* (a live shell can't be serialized), so we can't
do what a browser does for tab restore. What we *can* do is
reload the **memory** of the session — the artifacts, the
narrative, the warm cache of "what was I thinking" — so the
user resumes the thinking even though the live process is
fresh.

#### Multiplexer integration (tmux / zellij)

Rather than rolling our own splits/sessions/multi-pane, lean on
the existing terminal multiplexer ecosystem. Tmux is the
canonical choice (most installed, most stable); zellij is the
modern alternative with better defaults.

The user articulated the right framing: *"this is what I would
have wanted when starting tmux — how can I use this answered
without leaving my screen... playing with the features right
away to build intuition."*

That's the design target. Tmux is famously underused because
its discovery surface is bad — keys are arcane, the prefix is
weird, the documentation assumes you already know. Web2qml
becomes the **discovery surface** for tmux: voice does the
chord, the UI shows what just happened, the user builds
intuition by trying things without first reading docs.

Implementation directions:

- **First-class detection.** When `term_foreground_command()`
  returns "tmux" or "zellij," the block tracker disables
  (its prompt heuristics don't apply inside a multiplexer);
  the UI shows a small badge indicating "tmux: session foo,
  pane 2 of 3." Existing alt-screen detection logic mostly
  applies.
- **Auto-attach by cwd.** Optional setting (off by default,
  prompt to enable on first tmux detection): when opening
  web2qml in `~/Code/foo`, auto-`tmux new-session -A -s foo`.
  Session named after the cwd's basename, attached if it
  exists, created if not. Closing web2qml leaves the session
  alive on the system; reopening reattaches. Persistence
  across restarts comes for free.
- **Voice → tmux commands.** "Split horizontally" → `tmux
  split-pane -h`. "New pane below" → `tmux split-pane -v`.
  "Swap panes" → `tmux swap-pane`. "Send to other pane" →
  `tmux send-keys -t :.+ '<cmd>' Enter`. The voice layer
  becomes the *teacher* for tmux's command vocabulary.
- **On-screen affordance.** A small tmux-aware indicator
  showing pane layout and the active pane, with hover labels
  for each pane's foreground process. Click a pane label to
  switch focus.
- **Discoverability.** A "what tmux commands do you have"
  voice query → shows the small chord palette of common tmux
  ops with their tmux key equivalents, so the user learns
  konsole-applicable shortcuts while using voice.

The hand-off principle applies cleanly: the user can attach to
the same tmux session from konsole. The voice did the work,
the session persists, the user is free.

Zellij vs tmux: I lean tmux for the first ship because it's
more widespread (every distro, every dev box, every server
the user might ssh to). Zellij could be a secondary supported
multiplexer once tmux integration is solid. Don't build a
zellij-only path — build the abstraction, then ship tmux
support first, zellij later.

#### Dotfiles / environment awareness

The user said: *"I see the value in [this one], don't understand
much about [chord palette]."* Worth being more concrete about
what dotfiles awareness gets them.

The terminal currently spawns the user's actual shell with the
user's actual env, so dotfiles already work transparently
(aliases, functions, PS1, direnv hooks). What's *missing* is
surfacing.

Concretely:

- **Alias awareness.** Parse `alias` output once per shell
  spawn. When the user types or voices a command that has a
  defined alias, suggest the alias: "you have `ll` aliased to
  `ls -la` — use it?" Small UX nicety, makes the user's
  customizations *visible*.
- **direnv awareness.** When direnv activates a new env in a
  directory, show what changed (the env vars added) as a
  toast or status badge. The user gets confirmation their
  per-project setup is active without having to inspect.
- **Tool-version awareness.** asdf, mise, rbenv, pyenv, nvm —
  when a version manager switches versions, surface it. "Now
  using node 18.x (was 20.x)."
- **PS1 features the user is missing.** If the user's PS1
  doesn't show git branch but they're in a git repo, web2qml
  shows the branch in its own status area (see git integration
  above). Belt-and-suspenders so the user always sees the
  state.

The framing: web2qml respects whatever the user has configured
and *supplements* where their config is silent, without
overriding their choices. The terminal stays the user's terminal.

### Medium-impact, varying effort

#### Customizable keybinding chord palette

The user said they don't understand this one well. Let me make
it concrete.

Vim has `:map`, Emacs has `define-key`, tmux has `bind-key`,
every serious editor has user-defined keyboard shortcuts. The
power-user pattern: "I run `cargo check` 30 times a day; let
me bind it to F4." Or: "I always do `git status` then `git
diff`; let me make `Ctrl+G S` run both."

In web2qml today this would be done via voice: "run cargo
check." That works, but:

- Open offices, recording sessions, accessibility contexts make
  voice impractical
- Some users just prefer keyboards
- A power user wants their muscle memory to *transfer* between
  sessions and machines

The shape of the feature:

- A config file or settings UI listing chords and the actions
  they fire. Actions can be: shell commands, voice intents,
  internal app actions (open palette, toggle find, etc.).
- A "record this" affordance: user fires the action they want,
  then assigns it a chord.
- Visual chord-palette overlay (Ctrl+? or similar) that shows
  all defined chords + categories.
- Per-workspace chords (so a Rust project has different chords
  than a Python project).

Lower priority than sessions/git/find because voice already
covers the personalization need for most users. But for the
specific power-user audience that *won't* use voice, this is
table stakes.

#### Notifications when long-running things finish

Covered in high-impact section above. Small, can ship anytime.

### Medium-impact, medium effort

#### SSH speedups and remote-aware workflows

The user described: "if I ssh onto a device I can leave commands
and reference tables there, but that might get dangerous too."

The danger is real. Writing files to a remote system via voice
intent is a surface that needs care:

- Permission to write — needs to be opt-in per remote host
- Path safety — voice shouldn't be able to write to arbitrary
  paths on the remote
- Audit trail — every voice-driven write should be logged
  with timestamp and the originating utterance

That said, the *useful* version is real:

- "Leave a note on this server about the deploy steps" → writes
  a `.txt` to `~/web2qml-notes/` on the remote
- "Save this command output for later" → writes to the remote
  filesystem so the user has it when they ssh in next time
  manually
- "What did I do on this server last time" → reads back the
  remote `~/web2qml-notes/` directory or similar

Detection: parse `SSH_CLIENT` / `SSH_TTY` env in the spawned
shell to know we're on a remote; flag it in the workspace
metadata. Smart tokens for paths qualify with `local` vs
`remote:hostname`.

This is downstream of the workspace ship. Build the foundation
first.

#### Block history surface

The block tracker already records every command. What's missing
is a visual surface for browsing it.

Sketch:

- A side drawer ("blocks") that lists recent blocks as
  collapsible cards
- Each card shows: command, exit code, duration, summary
- Click a card to expand the output inline
- Filter by: this session, this directory, this branch, exit
  status, time range, command pattern
- Voice: "show me the last block that failed" → drawer opens,
  scrolled to that block

This is medium effort but the data structure exists.
Mostly UI work.

### Lower priority

#### Git pre-commit hook integration
Covered above; nice but not essential first cut.

#### Tool-version surfacing
Covered above; nice but not essential first cut.

#### Plugin / extension model
Powerful long-term but a serious architectural ask. Defer
until the basics are solid.


## Architectural decisions needed before code

Some things in this doc need design passes before
implementation, not just "ship it":

### Workspace data model

What goes into the workspace JSONL files, what goes into
sibling files, what gets indexed for fast query, what gets
periodically pruned? Schema design matters because we'll be
stuck with it long-term.

Open questions:
- Where does the size budget land (5 MB per workspace? Soft cap
  with pruning?)
- How does git-branch context get woven in (per-branch
  sub-workspace, or branch-tagged within one workspace?)
- How do voice transcripts cross-reference blocks (block_id
  field on each utterance? Time-window matching?)
- What's the read API for "show me my last session" — single
  file load + parse, or a tiny index?

### Multiplexer strategy

Tmux vs zellij vs both vs auto-detect. Picking tmux first
seems right but the abstraction layer matters: don't make the
QML side ship tmux-specific code. Build a `Multiplexer` trait
in Rust that tmux and zellij both implement, expose generic
operations.

Open questions:
- What's the minimal protocol — pane list, current pane,
  send-keys, split, swap, close?
- Detection: command name (`tmux`/`zellij`) or env vars
  (`TMUX`/`ZELLIJ`)?
- Auto-attach: prompt user once per workspace, or have a
  global setting, or always-on?

### Session restore semantics

What "restore my session" actually means. Three escalating
levels:

1. **Display-only**: show the user the artifacts (blocks,
   transcripts, files) from the prior session. PTY is fresh.
   Low risk, high value, easy to build.
2. **Replay**: re-run a curated subset of the previous session's
   commands. Risky (idempotency, side effects), needs explicit
   user approval per command, but high value when it works.
3. **Detached PTY persistence**: actually keep the PTY alive
   across app restarts. Requires either a daemon process or
   handing off to tmux (which is what tmux is for, so we'd
   just use it).

I lean #1 + #3 (via tmux). #2 is interesting but a
separate ship.


## Shipping plan

Roughly:

### Phase 1: small, visible wins (next 3-5 ships)

In a sequence that compounds:

1. **Scrollback search** — universal find bar with terminal
   scrollback mode, result navigation, highlighting. The
   foundation for "find as a verb."
2. **Long-running command notifications** — toast / sound on
   block-end when duration exceeds threshold. The smallest
   ship that makes the terminal feel *aware*.
3. **Keybinding audit + ? overlay** — fill the obvious gaps
   (Ctrl+T new tab, Ctrl+W close tab, Ctrl+Tab switch, font
   resize, etc.). Add a `?` overlay that shows all current
   chords. Visible discovery.
4. **Git auto-detect + status badge** — minimal git awareness:
   detect repo, show branch + dirty state. No commands yet,
   just visibility. The seed for everything else git.
5. **Filesystem find mode** — extend the find bar with a
   ripgrep-backed mode for cwd. Results as smart tokens.

### Phase 2: workspace foundation (medium ships)

6. **Workspace persistence — block tracker side.** JSONL log of
   blocks + summaries per cwd, sibling files for full outputs.
   Display-only "show prior session" affordance. No restore yet.
7. **Workspace persistence — voice / browser side.** Add voice
   transcripts and browser-history entries to the workspace
   record. Voice "show me what I read about" queries.
8. **Hand-off layer.** Append voice-executed commands to actual
   shell history. Write saved outputs to predictable cwd-relative
   paths. The morning-coffee scenario starts working.

### Phase 3: multiplexer + git deepening (larger ships)

9. **Tmux first-class detection.** When tmux is foreground,
   block tracker pauses, UI shows tmux-aware status. No
   commands yet.
10. **Tmux voice commands.** "Split", "swap", "send to other
    pane." Voice teaches tmux without the user having to read
    docs.
11. **Auto-attach by cwd.** Opt-in feature: open in `~/Code/foo`
    → auto-`tmux new-session -A -s foo`. Persistence-by-tmux.
12. **Git deep integration.** Smart command parsing, output
    integration, branch-aware history.

### Phase 4: ambitious surfaces (future)

13. **Block history drawer.** Visual browse of past blocks
    with filters.
14. **Customizable chord palette.** User-defined keyboard
    shortcuts, per-workspace.
15. **SSH-aware workspace.** Detect remote shells, route
    "save to my computer" appropriately.
16. **Cross-surface find** — code AND notes AND scrollback in
    one search.

Don't take this sequence as committed — it's a sketch. Each
phase is its own design conversation when we get there. The
purpose of laying it out is to show the *shape* of where the
work is heading, not to commit to a delivery order.


## Notes on what's not in scope

A few things explicitly NOT in this doc:

- **Bystanders / vote primitive / audio attribution.** Shelved.
  See [bystanders.md](bystanders.md). Power features outrank
  bystanders for now.
- **Adaptive desktop hover handles for selection.** Separate
  ship in the selection-UX arc, not a power feature.
- **The in-page reading effect (replacing the teleprompter).**
  Separate arc. Tracked in the changelog deferred list.
- **Plugin / extension model.** Long-term consideration; the
  basics aren't solid enough to plan extension points yet.
- **Anything that requires a daemon process or system service.**
  Web2qml stays a single-process app. Multiplexing via tmux is
  fine because tmux is already a service the user runs; we
  don't add infrastructure.


## Framings to preserve

The load-bearing ideas from the conversations that produced
this doc:

- **"Even if I want to use konsole, this is how I can share
  what I do with others."** The teaching-surface positioning.
  Web2qml as the mentor's tool, not just the learner's tool.
- **"Sip the coffee and talk to the computer to 10x an hour
  of morning relaxation while getting ahead on knowledge for
  a new project."** The hand-off-friendly principle. The
  artifacts the user leaves with matter as much as the
  in-session experience.
- **"How can I use this answered without leaving my screen...
  playing with the features right away to build intuition."**
  The discovery principle. Especially applies to tmux:
  web2qml as the *learning surface* for tools that
  power users want to learn but can't be bothered to read
  about.
- **"Power users try to capture full state so they can resume
  the thinking they had."** State-as-thinking, not state-as-
  process. The voice transcripts and AI summaries matter
  because they capture *what the user was figuring out*, not
  just *what commands ran*.
- **"Is there anything we are not taking an easy win on?"**
  The audit framing. This doc is the answer. Most of what's
  in here is "easy wins we haven't taken yet" plus a few
  bigger architectural moves.

If a future implementation choice would compromise one of
these, that's a strong signal to choose differently.
