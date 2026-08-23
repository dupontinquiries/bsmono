# SALVAGE — what to take out of `games/blackstar` before it goes

The C engine is being retired: no raytracing, and it ran poorly on mobile
against the wgpu build. This is the survey of what is in there that
`blackstar-rs` does not have, so the decision to drop it is made with the
inventory in hand rather than from the line counts.

Two things are worth saying before the list.

**Most of the C tree is already duplicated.** Its biggest modules — `menu`,
`mapgen`, `vehicle`, `enemy`, `encounter`, `weapon`, `element`, `campaign` —
all have Rust counterparts, usually under different names (`enemy.c` is
`skirmish.rs`, `element.c` folded into `weapons.rs`). Name collisions
understate the overlap rather than the reverse. Nothing in this document is
about those.

**The renderer is not worth an argument.** `render/` is 19k lines and most of
it is vendored `stb_image`, `stb_vorbis`, `stb_truetype` and `glad`. The
hand-written part is `renderer.c` at 597 lines plus `vk_backend.c` at 354.
That is not a body of work the Rust side needs.

---

## TAKE — architecture that does not exist in Rust at all

**STATUS: PORTED** — `blackstar-rs/src/brain.rs`, driven by `--brain`.

### 1. The LLM brain seam (`combat_json.c`, `commander_ai.c`) — ~440 lines

This is the most valuable thing in the C tree and it is not a feature, it is a
seam.

`combat_json` defines a JSON boundary in both directions. **Snapshot**
(game → brain) serialises the combat state: the player, allies, troops with
their owned-space zones and modes, plus engagement signals — aggression,
recent damage, kills, pressure. **Commands** (brain → game) is a parsed
directive set: `flock_to`, `flock_spread`, `hold`, `focus`, `retreat`,
`ally_follow`.

`commander_ai` then makes the load-bearing choice, in its own words: a
commander FSM emits *the same* `ex_cj_command_t` an LLM would. **A
deterministic commander and an LLM commander are interchangeable behind one
interface, and the FSM layer is a strict subset of what the LLM layer can
express.**

The Rust side has nothing like this. `content::COMMANDERS` is five archetypes
with four dials — tuning parameters consumed by a one-dimensional resolver.
It describes how a squad behaves; it does not let anything *issue an order*.

Worth taking on its own merits, and doubly so given how much of this session
went into `skirmish`, `perception` and `approach`: those produce exactly the
state a snapshot would carry, and consume exactly the orders a command would
give.

**STATUS: PORTED** — `blackstar-rs/src/hive.rs`, driven by `--brain hive`.

### 2. `hive_ai.c` — 178 lines

A swarm engine for the Corrupted, and explicitly *different in kind* from
commander doctrine: where a commander issues open top-down orders, the hive is
patient and indirect — below critical mass the infected disperse and creep,
massing quietly instead of charging.

`corruption.rs` exists on the Rust side and models what corruption *is*. It
does not model how a corrupted swarm *decides*. This is the missing half, and
the design note — that the enemy which does not fight like an army should not
share the army's decision engine — is the part worth keeping even if the code
is rewritten.

**STATUS: THE METHOD IS PORTED** — `blackstar-rs/src/tuner.rs`, `--tune search`.
The Markov FSM itself is NOT, and deliberately: this tree has no per-enemy
behaviour layer, so porting the artifact would mean building a system with no
consumer and then a tuner for it. The loop — headless sim, fitness function,
mutation search — was the transferable part, and it is pointed at the dials
that already exist.

### 3. `behavior.c` + `tools/tune_fsm.py` — a methodology, not a module

Enemies run a Markov chain over behaviours; each decision tick samples the next
behaviour from a transition matrix chosen by (context, archetype). The matrices
are **tuned offline by evolutionary search over a headless combat simulation**
and generated into `behavior_gen.h`.

The 45-line runtime is the least interesting part. The valuable thing is the
loop: a headless sim, a search, and generated tables. That is the same shape as
this project's other generated content, and `blackstar-rs` already has the
headless sim half (`skirmish`, `campsim`) with nothing searching over it.

---

## TAKE — content and mechanics from the film scripts

These are written material with no Rust equivalent, and they came out of the
scripts rather than out of the code.

**STATUS: `spearcon` and `warp_wave` are ported** — `blackstar-rs/src/hazard.rs`,
driven by `--hazard`. Both were the same idea (a thing arriving at a place at a
time that nobody can call off), so the clock is shared and the two mechanics are
thin over it. The C `ex_spearcon_update` had a real defect that did not survive
the port: it incremented its returned detonation count even when the caller's
buffer was full, so reading `n` entries out of a shorter array read past the
end. Returning a `Vec` makes that unrepresentable rather than fixed, and there
is a test pinning that all sixteen are accounted for.

`lore.c` is still to do.

| module | lines | what it is |
|---|---|---|
| `spearcon.c` | 58 | Cryotek's lieutenants' sticky spears — plant, fuse burns, delayed detonation. "Ticking-bomb tension that forces everyone to keep moving." |
| `warp_wave.c` | 31 | The Ice-Clone quantum leap's *consequence*. The jump is silent and undetectable; the only evidence is a gravity wave arriving **seconds later** at the landing point. The jumper knows it is coming, cannot stop or shape it, and it does not care who is standing there. |
| `lore.c` | 299 | 54 codex entries, original writing on the six-film arc. Includes the Archivist material, which **predates** the Rust story bible and is therefore source rather than derivative. |

`warp_wave` is the one to port carefully. A mechanic whose entire content is a
consequence you cannot aim, arriving after the fact, is unusual and it is
exactly the Watch-bearer's register.

---

## TAKE — tooling, and this is the big one

The C tree carries **18,328 lines of Python across 98 files**. `blackstar-rs`
has 7,099 across 22. The gap is not evenly spread: `blackstar-rs/tools/pipeline`
has five scripts and the C tree has the whole asset chain.

Not mirrored on the Rust side, largest first:

```
gen_world          1820      gen_props           932
solve_placements    651      limb_split          554
arena_forge         486      pack_bots           479
gen_spine           419      render_parity       291
analyze_models      284      gen_texture         275
```

`render_parity.py` deserves a specific mention: a parity harness is the one
tool that makes a renderer port checkable, and it is in the tree being retired.

**But the dependency is weaker than it looks, and I checked before saying so.**
The two trees do not share an asset format. `blackstar-rs/assets/` holds
**glTF/GLB** (21 `.glb`, 5 `.gltf`) and `model.rs` loads directories of those
directly; the C tree builds **`.iqm`** into `engine_src/data/` with a generated
`imported_models.h`. The Rust game does not consume the C pipeline's output at
all, so retiring it strands nothing.

What it *does* do is leave a copy of the old pipeline lying in
`blackstar-rs/tools/pipeline/install_models.py` whose docstring still describes
writing `engine_src/data/imported_*.iqm` and tables "included by game.c". That
is a C-tree tool sitting in the Rust tree, and when the C tree goes it becomes
a script that documents a build nobody can run. **Delete it with the C code, or
port it to the glTF path deliberately — do not leave it.**

The tools genuinely worth porting are therefore the ones that AUTHOR content
rather than install it: `gen_world`, `gen_props`, `solve_placements`,
`limb_split`, `gen_spine`, `arena_forge`. Those operate on geometry and
placement, upstream of whichever container format wins.

---

## LEAVE

- **The renderer.** `vk_backend`, `renderer.c`, `glad`, the `stb` vendoring.
  This is the reason for the decision, not a casualty of it.
- **`msdf.c`** (1,120 lines, text rendering) — real work, but wgpu has its own
  answer and this is not it.
- **`net_transport` / `netcode`** (365 lines) — `blackstar-rs` vendors its own
  `netcode`.
- **Everything with a Rust counterpart**, per the note at the top.

---

## KEEP THE HABITS

Not code, and probably the most durable thing here. The C tree's test culture
is where this project's discipline comes from: `content_validate` as a build
step, generated headers verified against a hand-written table, Python lints
registered as ctest targets, `test_tuning.py` checking that a data file and a
baked header agree.

Several of this session's guards in `blackstar-rs` are copies of that pattern.
When the C tree goes, the pattern should not go with it — the Rust side should
finish absorbing it rather than reinventing it later.

---

## ORDER OF WORK

1. **Audit the tools, first and quickly.** Not because assets are at risk —
   they are not, the formats are disjoint — but because
   `blackstar-rs/tools/pipeline/` already contains at least one stale C-tree
   script, and that class of thing rots quietly. Decide per tool: port to
   glTF, or delete alongside the C code.
2. **The film-script mechanics next** — `warp_wave`, `spearcon`, `lore`. Small,
   self-contained, and the writing is the value.
3. **The brain seam after that.** It is the most interesting item and the least
   urgent, because it is architecture for a feature that does not exist yet on
   either side.
4. **`hive_ai` and the FSM tuning loop last**, as a pair. Both want the Rust
   headless sim to be the thing they run against, and that sim is still growing.

## WHAT I GOT WRONG WHILE WRITING THIS

My first draft made "establish that the Rust tree can build its own assets"
step one, on the strength of an earlier observation that `blackstar-rs` reads
models and never seemed to build them. Checking it, the premise was wrong: the
formats are disjoint, the Rust tree reads glTF straight out of `assets/`, and
nothing it needs comes through the C `.iqm` chain. The real risk was the
opposite of the one I named — not a missing pipeline, but a stale one already
copied across.

## GAME.C — SURVEYED, AND IT IS A DESIGN CORPUS

The caveat below was right and the answer changed what "taking it" means.

`game.c` opens with about two hundred lines of design commentary: FORTY-TWO
named systems, argued for, most sourced to the screenplay. Underneath, the
implementations are inline against the C renderer — `AIDEN'S SKATEBOARD` is
two globals and a note. There is almost nothing to translate.

So it is harvested rather than ported:
`blackstar-rs/docs/INHERITED_DESIGN.md` carries all forty-two with their own
prose. The design survives deletion in a way renderer-coupled C does not.

Some were never built. `PALE VECTOR` has a paragraph and close to no code. I
tried to classify implemented-vs-not by grepping identifiers and the heuristic
was too crude to trust, so nothing is labelled — a confident wrong label is
worse than none.

The ones with the strongest claim on the Rust tree: `AIDEN'S SKATEBOARD` (a
human-scale vehicle mode, for a campaign that needs human traversal),
`PALE VECTOR` (a non-combat apparition who warps out if you press him — the
archaeology layer's register, apparently unbuilt), `DISGUISE PROTOCOL` (a prop
hunt falling out of the transform verb), `AMALGAM OF THE GATE` (a gestalt
fused from failed frames, now that `combine.rs` can build one), and
`LOW-G LOCOMOTION`.

## UNCERTAINTY

I read the headers and doc comments of every C module and the full source of
almost none — including `game.c`, whose design block I have now read in full
and whose 12,000 lines of implementation I have not. A system implemented
without a banner comment would still be invisible to this survey.
