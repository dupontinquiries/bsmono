# Curve references, clip transform, and the key pipeline

Two design threads, written down because both were worked out in conversation
and neither is finished. The first has code behind it; the second has none yet
and is here so the decisions are made before any is written.

---

## 1. Curve references and dummy chaining

**Status:** `studio-app/src/curve_refs.rs`, 16 tests. No schema — `Refs` and
`Dummy` live nowhere in a `Project` yet, deliberately.

### What is built

A curve may read another curve, scaled. That makes evaluation a graph, and a
graph can close on itself.

Cycles are **accepted and reported**, never refused. The order people work in
is: build the clip, get the motion right by eye, discover afterwards that the
relationship drawn is circular. Refusing the edit at that moment throws away
the part they got right.

The repair is an explicit intermediary — a **dummy** — holding sampled values
instead of a live reference. `A → B → A` becomes `A → B → D`. The loop is gone,
the shape is kept, and the dummy is a visible object rather than an invisible
rule the evaluator applies.

`promote_to_dummy` returns a **plan**, not an application, so a palette can
preview "this cuts b → a and freezes 9 points" before anything changes.

Three decisions worth not relitigating:

- The dummy holds what the chain **evaluated to**, not a copy of the referenced
  curve. The artist dialled in a result; one link of that chain is a different
  thing.
- It cuts the edge that **closes** the loop — the reference most recently
  drawn, which is what a person recognises as "the thing I just did".
- It is **frozen**, and `Promotion.frozen` says so. A sampled dummy no longer
  follows what it was reading. Right trade — a live cycle has no defined value
  at all — but a UI must warn rather than present a free fix.

### Chaining, and the hazard it adds

Freeze a dummy **from another dummy** and its points embed that one's state at
freeze time. Re-promote upstream and every downstream dummy silently describes
a shape that exists nowhere. The numbers stay plausible, the curve still draws,
and the only symptom is that a change upstream does not arrive.

`Dummy.generation` bumps on re-promotion; downstream records the generation it
sampled; `stale_against` compares them and returns **(dummy, source) pairs** —
the stale thing is the relationship, and naming only the node leaves the person
hunting.

### What a rope UI needs, and has

- `depths` lays nodes out by **longest** path from a root. Shortest-path would
  let a node fed by both a root and a deep chain sit next to the root, with its
  rope running backwards across the whole graph — the exact tangle the UI
  exists to avoid. Longest path makes every rope run left to right, asserted
  over every edge.
- `depths` survives a **cyclic** graph, because the layout is what the user is
  looking at when they decide to promote. A layout that hangs on unrepaired
  input is useless at the moment it is needed.

### Open

- Where `Refs` lives in the schema, and whether a reference is a field on
  `Curve` or a separate map. Deferred until the design has been used.
- `AutoTarget` reaches gain and effect params only. There is no target meaning
  "the mix below this track", which is what would make the adjustment-layer
  idea real.
- Crossfade as an object spanning two clips, rather than `Op::SetClipFade` on
  one. A fade against silence belongs to its clip; a crossfade is a
  relationship and drifts when either side moves.

---

## 2. Clip transform, letterboxing, and keying

**Status:** nothing built. The constraint below decides most of it.

### The constraint that changes every answer

`studio-video` **has no frame buffer**. Its whole stance, from its own module
docs: "No frame is decoded here — the cut list is pure timeline arithmetic, the
command is a string, and ffmpeg does the rest."

So a transform is not a function applied to pixels. It is **text appended to an
ffmpeg `filter_complex`**, and every design decision follows from that:

- Anything expressible as a filter is nearly free.
- Anything needing to read a pixel — a colour-picker eyedropper for the key,
  auto-detecting letterbox bars in a source — is a different kind of work and
  needs a decode path that does not exist.
- The transform must be **declarative and serialisable**, because it has to
  survive into a string. That is a constraint but also a gift: it forces the
  model to stay data.

### Letterboxing already exists, hardcoded

`edit.rs:127` builds one fit string for every cut:

```
scale={w}:{h}:force_original_aspect_ratio=decrease,
pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,
setsar=1,fps={fps},format=yuv420p
```

That is a complete letterbox policy: **fit inside, centre, pad with black**. It
is applied to every cut with no way to choose otherwise.

So letterboxing is not a feature to add. It is a **policy to expose**. The three
a person wants are:

| policy | filter shape | when |
|---|---|---|
| `Fit` (current) | `decrease` + `pad` | mixed sources, nothing cropped |
| `Fill` | `increase` + `crop` | full frame, edges lost |
| `Stretch` | plain `scale` | never the default, occasionally correct |

**Do not bake bars as an effect.** Letterboxing is a *consequence* of the fit
policy and the frame size. Baking black pixels into a clip means changing the
project's aspect later cannot undo them, and a source that was already
letterboxed gets bars inside bars. Keep it computed.

### What makes a transform usable rather than technically present

These are the details that decide it, and they are all cheap if chosen up front:

**Normalised coordinates, not pixels.** A transform in pixels breaks the moment
the project resolution changes or a source of a different size is used. `0.5`
means half the frame at any resolution; `960` means half of exactly one.

**A settable anchor, defaulting to centre.** This is the single detail that
makes scaling feel right or wrong. Scaling about the top-left sends a clip
diving off-screen; scaling about the centre is what every editor expects. But
the anchor must be *settable*, because "scale from this corner" is how a person
does a Ken Burns move. ffmpeg has no anchor concept, so it is resolved into the
crop/pad offsets — which is exactly the kind of thing the model should do once
rather than each caller doing differently.

**A fixed, documented order of operations.** Scale → rotate → translate, and
say so. Every other order produces different results from the same numbers, and
an editor who cannot predict the result stops using the feature. The order is
not a matter of taste once written down; the harm is in leaving it implicit.

**Transform is a clip property, not a chain slot.** If it is an effect in
`Clip.effects`, its position relative to a blur or a key changes the result,
and users will put it in the wrong place and file a bug. Fixed position in the
pipeline, always. This matches how `Fades` should have worked and is the same
lesson.

**Interaction with fit must be defined.** A clip transform composes *after* the
fit policy has already normalised the source to the frame. Otherwise "scale 1.0"
means something different for a 4:3 and a 16:9 source, and the number stops
meaning anything.

### Greenscreen: the pipeline question

`colorkey` and `chromakey` are both ffmpeg filters, so the keying itself is
cheap. The expensive parts are structural:

**A key produces alpha, and the concat pipeline has none.** The current graph
concatenates cuts into one opaque picture and `format=yuv420p` discards alpha
at the end of every fit string. Keying is meaningless without something
underneath to see through to, so the real work is **overlay**, not the key:
video tracks currently stack by "the top one is the picture", which is a
switch, not a composite.

That is the honest scope. Keying is a small filter on top of a compositing
change that does not exist yet.

**Key before transform.** A key sampled after scaling keys against interpolated
pixels — edges that were one colour become a gradient, and the tolerance that
worked at source resolution spills or leaves fringes. Key at native resolution,
then transform the result.

**Spill suppression is a separate control.** Every usable keyer has it, because
a subject lit by a green wall carries green on their edges even after the
background is keyed out. Without it, keying looks amateur and people blame the
key tolerance, which is the wrong knob.

**No eyedropper without a decode path.** The natural UI is "click the green" —
and that needs a decoded frame, which this crate explicitly does not do. Either
a key colour is typed, or a decode path arrives first. Worth deciding
deliberately rather than discovering halfway through.

### Suggested order

1. **Fit policy** as a project setting — `Fit`/`Fill`/`Stretch`. Smallest real
   win, exposes a hardcoded decision, no new concepts.
2. **Clip transform** — normalised, anchored, fixed order, fixed pipeline
   position. Composes after fit.
3. **Overlay compositing** — the change that makes video tracks composite
   rather than switch. The prerequisite for anything alpha.
4. **Key + spill** — small once (3) exists.

Attempting (4) before (3) produces a key with nothing to reveal, which is the
kind of feature that ships and then gets reported as broken.

---

## TODO list

Written as tasks rather than prose so someone can pick one up cold. Each says
what is already there, because in every case so far the surprise was how much
was.

### T1 — Fit policy as a setting *(small, no new concepts)*
`edit.rs:127` hardcodes fit-inside-and-pad. Make it `Fit`/`Fill`/`Stretch` on
the project. `Fill` is `force_original_aspect_ratio=increase` plus a centred
`crop`; `Stretch` is a plain `scale`. Keep the bars computed — never baked.
**Test it by the filter string**, the way `render_args` is already tested; no
frame decode needed.

### T2 — Clip transform *(medium; the details are the work)*
Normalised coords, settable anchor defaulting to centre, fixed order
scale→rotate→translate, a clip **property** not an entry in `Clip.effects`,
composed *after* T1's fit. Sparse map on `Project` like `fades` — there are 221
`Clip` literals and widening the struct is the rewrite that corrupted
studio-app once already.

### T3 — Overlay compositing *(large; the real blocker)*
Video tracks currently switch, not composite: *"the FIRST video track is on
top, the top one is the picture"* (`edit.rs:13`). Becomes an `overlay` chain,
and `format=yuv420p` has to stop ending every fit string. **Everything alpha
depends on this** and nothing else does.

### T4 — Chroma key + spill *(small once T3 exists)*
`chromakey` filter, keyed at **native resolution before** T2's transform, or
the tolerance fights interpolated edges. Spill suppression is its own control —
without it people blame the tolerance, which is the wrong knob. No eyedropper
without a decode path; type the colour or decide to build decoding.

### T5 — `Refs` in the schema *(blocked on nothing but a decision)*
`curve_refs` is tested and stores nowhere. Decide: field on `Curve`, or a
`Project`-level map like `fades`. The map is probably right for the same
reasons, but this should follow a first real use.

### T6 — `AutoTarget::MixBelow { track }` *(the adjustment layer)*
`Gate` already has `Window` and `Group`, which scope *when*. There is no target
meaning "everything below this track", so a modifier layer can scope time but
not subject. This is the missing half of the adjustment-layer idea and it needs
T3 to mean anything for picture.

### T7 — Crossfade as an object *(supersedes part of `Op::SetClipFade`)*
A fade against silence belongs to its clip and should stay. A crossfade is a
relationship between two clips and drifts when either moves. Keep the per-clip
op; add a crossfade that references both clip ids and owns the overlap.

### T8 — Rope UI *(needs a surface, not a model)*
`depths` and `stale_against` are ready. Needs its own tab or popup. Draw the
stale **rope**, not the stale node — `stale_against` returns pairs for exactly
that reason.
