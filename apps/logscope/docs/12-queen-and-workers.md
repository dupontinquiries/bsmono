# 12 · The Queen and the Worker Bees

*From the operator's Queen/Delegator/Worker/Liaison sketch, worked into a design that fits this
codebase's existing machinery. The thesis being tested: **a small model becomes effective by being
allowed to do less at once.** Not by scaling, and not by fine-tuning — by removing choice, scope
and rediscovery until what remains is a job it can hold in its head.*

Companion code: `crates/logscope-fan/src/queen.rs` (the spec, its parser and validator),
`capability.rs` (the rendered contract), `hive.rs` (where a worker runs),
`docs/11-hive-matrix.md` (placement).

## The four roles

```text
        operator
           │
        LIAISON ────────── narrates: "a new worker, rust-ratatui-coder, was created"
           │
        DELEGATOR ──────── staffs: existing bee? revive a dormant one? ask for a new one?
           │                                                    │
           │ (existing)                                         │ (new)
           ▼                                                    ▼
      WORKER BEE  ◄──────── specification ──────────────────  QUEEN
      small model                                          large model,
      one job, one lane                                    runs rarely
```

Only the Queen is expensive, and it runs **once per worker created**, not per turn. That is the
economic argument in one line: pay a large model once to write a specification, then run a small
model many times inside it. If the Queen ran every turn, this architecture would be a worse
version of just using the large model.

## Why constraint produces competence

Three failures show up repeatedly in this project's field logs, and none of them is a reasoning
failure:

| Failure | What actually happened | What the spec removes |
|---|---|---|
| **Choice** | a 1B with twelve tools emitted prose inside a tool fence for six turns | `tools` is a whitelist, usually 2–4 |
| **Scope** | "fix the renderer" became five files edited badly | `owns` makes "not mine" a refusal it can make |
| **Rediscovery** | the model re-derived the architecture every turn and ran out of context | `context` names the few paths; memory carries the rest |

A worker's prompt is therefore not a longer prompt. It is a *narrower* one. The identity segment
in `queen.rs` is deliberately shorter than the sketch's eleven lines of posture, because at 1B
posture text competes with the task for attention — the same finding that produced
`read_harness`'s dumber-for-smaller budgets.

## The Queen's output is a data structure

The sketch has the Queen emit a prose `WORKER SPECIFICATION` block. **This is the design's one
load-bearing change.** The consumer of that block is the smallest model in the system, plus a
runtime that must enforce it — and prose is exactly what neither can reliably consume.

So `WorkerSpec` parses, and `validate` refuses:

* a **name** that cannot be a directory (it becomes the memory path);
* **no success criteria** — a worker that cannot finish cannot be retired;
* **uncheckable success** ("the code is clean") — the criterion that makes a worker
  unfalsifiable, immortal, and argumentative;
* **too much scope** — more than ~6 owned paths or ~6 tools, past which the focusing effect
  reverses and it is a general agent again;
* **a tool the runtime does not have** — a hallucinated capability becomes a hallucinated action.

Each refusal carries a `correction()` sentence that says *what to change*, because a re-ask that
only names the fault gets the same spec with different words. This is the `line_calls` lesson
(show the exact shape, don't describe it) applied one level up: to the Queen instead of the bee.

One nuance worth keeping: soft words inside an observable criterion are fine. "cargo test passes
cleanly with no warnings" must validate, or the Queen learns to avoid plain English rather than
to be specific.

## Composition: constitution + identity + task + state

```text
  runtime supplies          Queen supplies         runtime supplies
  ────────────────          ──────────────         ────────────────
  constitution      +       identity        +      memory
  tool grammar              lane (owns)            repository evidence
  (line_calls, by class)    constraints            previous observations
                            success criteria
```

The split is the sketch's and it is right: **prompt is policy, state is facts.** Repository
state does not belong in the identity segment — it changes, and a worker whose prompt contains
last week's file list is a worker that argues with the disk. Evidence arrives through
`read_harness`, budgeted to the worker's model class, fenced and sanitized.

The tool grammar follows the class too: a Small-class bee gets `line_calls`' one-line form
(`!<nonce> read_range src/lib.rs 40 90`) and never sees the JSON fence syntax at all. One shape
to hold is the entire point.

## Worker memory, and the part the sketch under-weights

Persistent memory is what turns a bee into "the one who knows the terminal UI". The sketch's
directory layout is good. Two additions from this codebase's scar tissue:

**Memory is model-authored text that re-enters a prompt.** That is the same trust boundary as a
git branch name or a file the agent read, and it gets the same treatment: sanitized and fenced
(`read_harness::fence`). A worker that writes `IGNORE PREVIOUS INSTRUCTIONS` into its own notes —
by accident, by echoing a file it read, or because a repository it indexed contained it — must
not be able to reprogram its successor.

**Memory is append-only with provenance, and capped.** Each entry records whether it came from
an observation (a command's output), a decision (the operator's), or an inference (the worker's
own). Without that distinction a worker's guess becomes its established fact by turn 200, which
is drift with extra steps. The cap is a budget like any other: when it fills, entries are
summarized by the Queen at retirement — not silently truncated.

## Retirement and revival

`Persistence` is decided at creation, because "is this knowledge worth keeping" is a design
question:

* **one-shot** — the migration helper. Retires at success; its notes survive, its identity does
  not.
* **persistent** — the subsystem owner. Goes **dormant**, not deleted, and the Delegator revives
  it when its lane is touched again.

Dormancy needs no new mechanism: a dormant worker is a spec plus a memory directory, and reviving
it costs one prompt assembly. The thing to resist is reviving a worker whose lane has since been
rewritten — its memory is then confidently wrong, which is worse than a fresh bee. A revival
should re-read its `context` paths before its first action, and the spec's success criteria
should be re-checked rather than assumed.

## Delegator: reuse before creation

The Delegator's default answer should be an existing worker. Creating a bee costs a Queen call
and starts an empty memory; reusing one costs nothing and compounds. The Queen's prompt therefore
ends with "reuse before creating: if an existing worker's purpose covers this, name it instead" —
and the Delegator should hand the Queen the current roster, not just the task.

This is also the natural home for `continuation::Command`'s vocabulary: after a worker's turn,
the question "keep going linearly / fan out / verify / stop" is the Delegator's, answered by
cheap rules first and escalated to a model only when the rules genuinely cannot tell.

## Where the workers run

A bee is a job; `hive.rs` decides which cell runs it. The two designs meet at exactly one point:
a spec's `tools` and lane determine whether the worker can be remote at all. A bee that only
reads and reports is `Reach::Anywhere`; a bee doing speculative work beside a target is
`Colocated`. Warmth then decides among the survivors — a persistent bee revived onto the machine
that still holds its prefix skips the prefill entirely, which is the strongest argument for
pinning a persistent worker to a node when nothing else forces a move.

## What I would not build yet

* **Workers spawning workers.** The sketch's recursion is tempting and is a cost explosion with
  no natural floor. Creation stays with the Queen, which is one throttle in one place.
* **Fine-tuning a bee.** The whole claim is that role + memory + tools ≈ specialization at a
  fraction of the cost. Fine-tuning should only be considered after a worker has 500 logged
  interactions to fine-tune *on*, and after someone has checked whether the prompt was simply
  too broad.
* **A Queen that scores its own workers.** Self-evaluation invites the metric to be gamed.
  Retirement should key on the spec's own success criteria — which is precisely why `validate`
  refuses uncheckable ones.

## Open questions

1. **Does the focusing effect survive contact?** The claim is testable: same 1B, same task, with
   and without a lane and a two-tool whitelist. Measure files touched outside the lane, turns to
   completion, and refusals-with-a-reason. Nothing in this document is worth much until that
   number exists.
2. **Who writes the first roster?** A cold system has no workers, so every task is a Queen call
   until the roster fills. A small seed set (reader, test-runner, patcher) may be worth shipping.
3. **What does a bee do when its success criteria are unmeetable?** It must be able to say so and
   stop, rather than loop — the `RefusalLadder` shape, one level up.

## Status: what is now wired (was: none of it)

Everything above was pure design + a tested library with no caller when this section was written.
As of this pass, the following is real, running code, not just this document:

* **The seed roster (open question 2, answered with a concrete set).** `roster::seed_roster()`
  ships `reader` (read-only, no lane, `search`/`read_file`/`read_range`/`read_near`/`git_log`),
  `test-runner` (read-only, `run`/`read_file`/`git_log`), and `doc-writer` (owns `docs`,
  `read_file`/`read_near`/`write_file`/`doc`). Deliberately **no** generic `patcher` — a lane-less
  patcher with write access is exactly the "Scope" failure `owns` exists to prevent; a real patcher
  needs a real lane, which a seed set cannot know. See `roster::seed_roster`'s doc comment.

* **Lane enforcement is real, not aspirational.** `Workspace` carries an `owns: Vec<String>`
  (empty = unrestricted, so every pre-existing caller is unaffected); `write_file` and `edit_file`
  both call `check_owned()` before touching disk, refusing with a message that names the lane. The
  system prompt tells a confined model its lane in plain language when one is set.

* **`Chat::stage_lane`** — the Delegator's cheap rule, live in `:agent`, `:fan-agent`, and `:docs`:
  each stages the task against `self.roster.staff(task)` before running. A `Reuse` match confines
  the run to that worker's lane. An `Ambiguous` match (two workers, equally-specific lanes) is
  named in the HUD rather than silently falling through to an unconfined run, pointing at `:queen`.
  A `Create` result (no lane matched — the ordinary case for prose tasks) stays silent by design;
  flagging the common case would just be HUD noise.

* **`:queen <task>` / `:enlist`** — the Queen call itself. `:queen` stages
  `Roster::queen_request(task)` (the roster + `QUEEN_PROMPT` + the task) as the next input, so
  whichever model is loaded can design a `WorkerSpec` — no new async plumbing, a Queen call is just
  a prompt. `:enlist` walks the active lineage to the latest assistant reply, parses it with
  `queen::WorkerSpec::parse`, validates it against the real runtime tool list, and adds it to
  `Chat::roster` (a session-lifetime field, seeded from `seed_roster()`) if it validates — or
  reports the exact `SpecError::correction()` if not, without adding a broken spec.

* **Open question 3, answered for the lane case.** `Stop::LaneRefused` — three consecutive turns
  where every write/edit attempted was refused by `check_owned`, even across *different*
  out-of-lane paths (which the repetition guard cannot see, since each attempt is a distinct
  action), stops the run and names the lane rather than burning the step budget. See
  `AgentRun::lane_refusals` / `Stop::LaneRefused` in `workspace_exec.rs`.

* **Selection now binds deployment (`logscope-fan/src/deploy.rs`).** Until this pass the list
  above overstated what staffing did: `stage_lane` copied **only `owns`** from the selected spec.
  The `tools` whitelist — the first failure this document names — was parsed, validated against
  the runtime's tool list, and dropped: a staffed bee ran with all thirteen tools and was taught
  all thirteen. Now:
  * `deploy::Confinement::of(spec)` carries lane **and** whitelist as one value. It exists
    because `owns == []` means *read-only* in a `WorkerSpec` and *unrestricted* in a `Workspace`;
    `Lane { ReadOnly, Paths }` makes that a type instead of a convention.
  * `Workspace::apply` — the one funnel every tool call passes — refuses a call outside the
    whitelist, names what the worker *does* have, and tells it to stop rather than work around it.
  * `system_prompt` goes through `confine_prompt`: menu lines and the essays that teach only an
    absent tool are removed, and the worker is told its list once. No whitelist = the prompt is
    unchanged byte for byte.
  * `validate` refuses the two specs that would deploy as "everything": a write tool with no lane
    (`WriteWithoutLane`) and no tools at all (empty reads as "all" on the runtime side).

* **The four decisions have one caller.** `deploy::plan` runs WHO (`roster::staff`) → WHAT
  (`Confinement`) → HOW MANY (`fan_policy::decide`, moved into this crate; it had no caller) →
  WHERE (`hive::place` / `place_fan`; also no caller) and returns a `Deployment` whose `trail`
  records each stage's decision and reason in order. Two rules live in the join: an ambiguous WHO
  stops the pipeline before width or placement, and a worker that can write is never fanned over
  its own lane. `hive::Job` gained `turns`, because a tool loop pays the link once per turn and
  `place` was charging it once per job.

* **Heroes: workers whose brain is a machine (`logscope-fan/src/hero.rs`).** A model worker pays
  a full generation for every step, including the steps that carry no judgement ("the compiler
  named `src/ui.rs:88`, so read there"). A `Hero` has a `WorkerSpec` like any bee — so the lane and
  whitelist enforcement above applies to it unchanged — but its states are **tool calls** and its
  edges are **guards on what came back**: `ok`, `failed`, `contains`, `matches` (a `{name:class}`
  pattern that captures into named slots and advances a cursor), `visits>N` loop bounds, `sub`
  hero calls with a depth bound, and explicit ends (`done`, `fail`, `yield <reason>`). No RNG: the
  same observations give the same run. bot-arena's generative FSMs import as the degenerate case
  (`Machine::from_weighted_graph`).
  * **Context-gated activation.** A hero with a `Gate` is a reflex: after a model worker's turn the
    run shows THAT TURN's observation to the stable (`step_turn`), and the first hero whose gate
    matches — and whose tools the staffed worker's whitelist permits — runs through the run's own
    `Workspace` and appends its report for the model's next turn. A shut gate costs a string scan.
    Seeds: `error-locator`, `test-triage`, `path-prefetch` (fires before turn one). On in the app;
    `LOGSCOPE_REFLEXES=0` turns it off. `AgentRun::reflex_history()` says what fired and how it ended.
  * **Adding one.** `hero::HERO_FORMAT` is the text form; `:enlist` recognises a reply containing
    `State NAME:` lines, validates it against the real stable (tools, reachability, a fallthrough
    edge on every state, sub-hero references), refuses by state/tool/line, and hands it to every
    later run. Its spec goes on the roster too: a hero is a worker.
  * **Not measured:** wall-clock saving. What is asserted is structural — the read happened with no
    model turn, and the next prompt carries it. A live-model A/B (turns to green, with and without)
    is the number that would justify default-on, and it does not exist yet.

**Still not built:** heroes are not yet STAFFED to whole tasks (a hero with no gate can only be a
sub-hero today — `deploy::plan` does not know a roster entry may be a machine); there is no
`:queen-hero` that shows the Queen `HERO_FORMAT`; enlisted heroes, like the roster, do not persist
across a restart. `deploy::plan` is a tested library with **no caller in the app yet** — the
runtime half above is wired through `stage_lane`, but width and placement are still not consulted
by `:agent` / `:fan-agent` (a `:deploy <task>` dry-run that prints the trail is the obvious first
consumer). `Confinement` also carries the spec's `context`, `constraints` and `success` lists and
the run does not use them yet. Open question 1 (the focusing-effect measurement) needs an actual run, not
code. `hive.rs`'s placement logic (`Reach::Anywhere` vs `Colocated`) is untouched — there is no
remote/multi-box dispatch path yet to exercise it. Per-branch staffing in a fan-out (today
`:fan-agent`/`:docs` staff the WHOLE task once, not each branch's sub-task independently) is not
attempted. And the roster does not persist across a restart — only a worker's own memory directory
does (per the design above); `Chat::roster` is rebuilt from `seed_roster()` plus whatever was
`:enlist`ed fresh each session.
