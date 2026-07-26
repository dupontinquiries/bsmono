# assetkit — Cybertron asset library manager

egui front-end for the model pipeline, built on `shared/uikit/crates/uikit-egui`
so it inherits the carved-material look and the palette bridge.

    Library   scan a tree, score every mesh, sort and filter by form class
    Viewer    wireframe preview with a lying-down check
    Export    run the pipeline steps, with output streamed into the panel
    GAN       retrieval baseline + conditional-upscaler training

## Build

    # adjust the uikit paths in Cargo.toml to your monorepo checkout, then
    cargo run --release

Needs **rustc >= 1.76** (egui 0.29's `ecolor`/`emath` require it).

## What is verified and what is not

`src/mesh.rs` — IQM + OBJ loading, and the whole scoring stack (compactness,
coherence, connected components, openness, form class) — **compiles and its
algorithms were validated against known models**:

    model       compactness  coherence  members   verdict
    scorponok         95.7       0.789      384    bony (the reference)
    grimlock          30.4         ---       52    shell
    jazz              29.2         ---       42    shell

`src/main.rs` — the UI — is **not compiled**. It was written in an environment with
rustc 1.75 and no route to upgrade, so every widget call is correct-by-inspection
against `shared/uikit` rather than proven. Expect first-build fixes; the logic it
drives is all in `mesh.rs` or in the existing python tools.

## The score is a filter, not a verdict

`bony = compactness × coherence³ × ln(members)`

Compactness (area / volume^(2/3)) is the discriminator for skeletal vs hollow, but
**it rewards surface area and noise has plenty**. Measured against an adversarial
mesh — a slab of jittered boxes with no structure at all:

    scorponok (real)          96
    clean grid of boxes      112     <- already beats it
    jittered boxes           881     <- 9x

The `coherence³` term (mean |dot| of edge-sharing face normals) is the corrective:
real plating is locally smooth and meets at clean edges, so the clean box grid
drops from 112 to 4.1. It is a **partial** fix — coarsely jittered boxes keep
planar faces and so keep usable coherence, and on the real library several smooth
terrain meshes top the list on coherence alone with almost no members.

Use it to choose candidates to look at. Never as evidence a model is good.

## Why exports shell out to python

The pipeline tools are tested and in use. A second Rust implementation would be a
second thing to keep correct, and the measurement core that genuinely benefits from
Rust (library-wide sweeps) is already ported here in `mesh.rs`.

## assetkit-cli — the verified half

The CLI has no egui dependency, so it builds anywhere and **is compiled and
cross-checked against the python it replaces**:

    assetkit-cli score   <dir>  [--top N] [--csv f]     library_score.py
    assetkit-cli view    <file> [--views front,side]    ascii_view.py
    assetkit-cli harvest <dir>  [--out DIR]             part_harvest.py
    assetkit-cli morph   <file> [--out DIR]             voxel_morph.py
    assetkit-cli info    <file>

    rustc --edition 2021 -O src/cli.rs -o assetkit-cli

Cross-checks performed:

  * `info` on imported_jazz_part1.iqm → 398 verts, 480 tris, 2 components,
    compactness 12.5 — identical to the python on the same file.
  * `score` output matches library_score.rs row for row on 674 models.
  * IQM writer round-trips: 106 triangles written, 106 read back.
  * 60 morph clusters written by the Rust all load in the python reader.

`view` is strictly better than the python original, which cannot open IQM at all
(it routes through process_models.load_geometry, which handles only glTF/OBJ).

## Full port status

    module      replaces                     verified against python
    mesh.rs     part of process_models       identical verts/tris/components
    gltf.rs     _load_gltf_geometry          identical on 4 models (see below)
    json.rs     (python's json)              hand-written, no dependencies
    iqm.rs      texture_export's writer      round-trips; python reads its output
    harvest.rs  part_harvest / limb_split    identical component counts
    ascii.rs    ascii_view                   handles IQM, which python cannot
    pairs.rs    shape_pairs                  437 pairs vs python's 436

glTF cross-check — the Rust loader parses accessors, byte strides, all index
component types, GLB containers, primitive modes and the node hierarchy, with a
hand-written JSON reader and no crates:

    model                      rust                python
    fall_of_cybertron_jazz     8846 v / 10660 t    8846 v / 10660 t
    fall_of_cybertron_grimlock 12512 v / 13768 t   12512 v / 13768 t
    scorponok                  63551 v / 87663 t   63551 v / 87663 t
    ravage                     7088 v / 10004 t    7088 v / 10004 t

### Known difference: decimation strength

`pairs` produces a smaller detail gap than the python (+3% depth relief vs +14%).
The Rust uses vertex-clustering decimation, iterated, which reduces 2-4x per pass
and cannot hit an arbitrary target exactly; python uses a quadric decimator that
lands on it. The pairs are still correctly ordered — blob less detailed than
detail — but the gap is narrower, which makes the learning task easier and less
useful. **For GAN training data, prefer the python `shape_pairs.py` until this is
replaced with a proper edge-collapse decimator.**

### Still in python, deliberately

Texture conversion (PNG/JPEG codecs) — needs image codecs, and it is I/O bound
rather than compute bound, so Rust buys nothing.

## raids

    assetkit-cli raids [--raid ID] [--phases N] [--seed N]

Ports gen_raids.py. Cross-checked against it: same 19 ids, **identical mechanic
assignments and premise lines** (phase map choices differ — different RNG, both
valid). The Rust output builds: `cmake --build` clean, 62/62 with the generated
header.

It also patches `maps.json` in place to add `kuiper` and `unicron`, which the old
hand-written `raid_plan.h` referenced but which were never defined — two of three
phases were loading an undefined theme. That required a JSON *writer* in `json.rs`,
because clobbering a hand-edited content file with a lossy round-trip would be
worse than not writing it at all.

The GUI's **Raids** page wraps the same code: generate, browse the 19, inspect the
phases, and write the json + header. The premise line sits on screen next to the
mechanic it produced, so the derivation is visible rather than buried in a file.

## train — the CPU falsifier

    assetkit-cli train pairs/pairs.f32 [--epochs N] [--adv] [--limit N]

A dependency-free CPU conv net (residual generator + paired patch critic, Adam,
BCE-with-logits matching kBM3) trained on downsampled pairs. **It is not a training
rig — it is a falsifier.** A GAN on a GPU always produces something and the loss
always falls, so it cannot answer "is this task learnable". This can, in seconds.

The only baseline that matters is IDENTITY — returning the blob unchanged. Beating
MEAN is worthless because copying the input beats the mean trivially.

Current result on both pair sets:

    rust pairs   (+3% relief gap)   vs IDENTITY  -0.1%
    python pairs (+14% relief gap)  vs IDENTITY  +0.4%

**The net has not learned anything.** It reproduces its input, which is exactly what
a residual generator does when the target is nearly the input. That is a data
problem, not an architecture problem: the blob is not degraded enough for there to
be a mapping to learn. Widen the gap before spending GPU time.

## Two-stage design, and what the falsifier says about it

The proposal: one GAN/DIS pair predicts asset **position / scale / orientation**,
a second takes **exploded parts vs big empty box equivalents** and fits the actual
geometry. `pairs.rs` supports both halves:

    placement()          9 numbers per part (position, scale, aspect) for stage 1
    pairs --degrade box  box -> part pairs for stage 2

**Stage 2's data is dramatically better than blob->detail.** Measured on the same
437 components:

    degradation   mean |blob-detail|   depth relief gap
    decimate                   0.15               +3%
    box                        0.58             +150%

A box shares only the part's extent, so "what fills a volume of this shape" is a
real question. Decimation produced blobs so close to the target that a residual net
just copied its input.

**But the falsifier does not pass it yet:**

    IDENTITY  0.626    return the box unchanged
    MEAN      0.399    return the training mean
    NET       0.408    trained net    (+34% vs identity, -1.3% vs mean)

The net beats identity by a third and still loses narrowly to the mean. It has found
the output *distribution* — "emit something part-shaped" — without conditioning on
its input. Swept lr 0.001–0.004 and 12–24 filters; the gap is stable, and lr 0.004
diverges.

Which baseline is hard depends on the degradation, and getting that wrong flatters
the model in both directions:

    decimated blobs -> IDENTITY is strong, MEAN is trivial
    box blobs       -> IDENTITY is trivial, MEAN is the real bar

The verdict now requires beating **both**. Stage 2 is a much better-posed problem
than what came before, and the remaining gap looks like model capacity rather than
data — which is exactly the case where GPU time is worth spending, on an
architecture with a downsampling bottleneck rather than three same-size convs.

## GAN page — the two-stage flow

    Stage 1 — placement   assetkit-cli place <donors> --out placement
                          9 numbers per part (position, scale, aspect)
                          436 parts exported; descriptors verified to VARY, so
                          there is something to predict rather than a constant

    Stage 2 — fitting     assetkit-cli pairs <donors> --degrade box --out pairs
                          assetkit-cli train pairs/pairs.f32

The degradation toggle is the most consequential control on the page, so the page
states the measured difference rather than leaving it to be discovered:

    decimate   relief gap   +3%   a residual net just copies its input
    box        relief gap +150%   a real question with a real answer

Three buttons in escalating cost — **CPU check**, **retrieval baseline**, **GPU
train** — because the cheap questions should be asked first. The CPU check is a
falsifier: a GPU run always produces something and its loss always falls, so it
cannot tell you whether a task is learnable.

Verdict colouring in the log is meaningful: red for HAS NOT LEARNED, amber for
PARTIAL, green for LEARNS. Standing result on box pairs is **PARTIAL** — +30% vs
identity, −4.7% vs mean — the net has found the output distribution without
conditioning on its input.

## Fill page — the approach that works

`Fill` is the primary generative path, so it gets its own page rather than being
buried under GAN.

    target      pick a model, or "use Library selection"
    catalog     harvested parts (a Harvest button builds one from the Library root)
    region      whole | chest | legs | head
    keep shell  additive vs replacement
    passes/pop/gens

Three things the page states rather than leaves the user to discover, because each
was measured and each is easy to get wrong:

  * **harvested parts beat generated primitives** — +13% fit, 5.6x the structural
    components, coherence 0.81 vs 0.57
  * **turning off "keep shell" REPLACES the model**, and at IoU ~0.38 that is a
    rougher silhouette, not a better one. Warned in amber.
  * **7 passes measured worse than 6** (0.366 vs 0.385) — the search space outgrows
    the budget. Warned at the point of choosing.

Log colouring: green when it beats random, amber when a pass fails to improve or a
region turns out not to be hollow, red for errors.

## rigscore — humanoid riggability, before rigging

    assetkit-cli rigscore <file|dir> [--min-tris N]

Scores how well a model supports human-like motion, from geometry alone:

    SEPARABILITY  do components partition into limbs, or is it one fused blob
    SYMMETRY      worst left/right pair, by mass and mirrored position
    PROPORTION    limb spans against anthropometric fractions of stature
                  (head .130, shoulder .259, thigh .245, shin .246, leg .530)
    ARTICULATION  is there a waist -- a narrowing between segments to pivot about

Symmetry carries the highest weight (0.35) because it is what actually breaks a
walk cycle, and it is the **worst** limb pair rather than the average.

**Validated against known outcomes** from the rig work:

    model        sep   sym  prop  artic  TOTAL  verdict    what actually happened
    jazz        0.73  0.99  0.65   0.08   0.71  RIGGABLE   rigged, joint gap 0.084
    grimlock    0.96  1.00  0.72   0.00   0.78  RIGGABLE   rigged, gap 0.45
    zetaprime   1.00  0.49  0.56   0.00   0.58  MARGINAL   FAILED, arms 6818 v 100
    soldier     0.74  0.00  0.46   0.52   0.39  MARGINAL   FAILED, fell back to static

The averaged version scored Zeta Prime **0.59 symmetry / RIGGABLE** — a false
positive — because its legs were fine and the mean hid its broken arms. Taking the
worst pair drops it to 0.49 and MARGINAL. That correction came from checking the
score against outcomes already measured, not from reasoning about it.

## hub — the ship as a homebase

    assetkit-cli hub [--out content/hub.json] [--header engine_src/exengine/hub_plan.h]

Eight rooms laid along the ship's spine — bridge forward, hangar and airlock aft:

    bridge         navigation console: choose where the ship goes next
    corridor_fore  spine, viewports both sides
    upgrade_bay    chassis rigs and part racks
    market         trade deck
    quarters       where the crew bring you problems
    corridor_aft   colder back here
    airlock        cycle through and walk the hull
    hangar         drop bay, launches and landings

Ten console destinations, each declaring **how you travel**: drive 3, fly 3, land 4.
Travel is a mode rather than a loading screen, because the engine already has a
landing sequence (`g_landing_active`, `g_landing_ring`, `g_land_impact`) and flight
pitch/roll — arrival should use them.

**Three validations, each for a bug that is invisible until someone walks into it:**

  * every room **reachable from the bridge** — an upgrade bay nobody can reach is
    worse than no upgrade bay
  * every exit **mutual** — a door that works one way looks perfectly reasonable in
    a table
  * every destination map **exists in maps.json** — a raid phase pointing at an
    undefined map was a real bug that shipped

It refuses to write if any check fails. The generated `hub_plan.h` compiles and
reads correctly from C (verified with a standalone harness).

### Fitting the hub to a hull

    assetkit-cli hub --ship engine_src/data/ship.iqm [--shrink 0.72]

Rooms keep their **order along the spine and their graph**, but positions and sizes
come from the hull — so a stubby lander and a long cruiser both get a bridge forward
and a hangar aft from one layout. The long axis is detected, not assumed, and each
slice's cross-section is measured at the 15th/85th percentile so a single antenna
does not define a room.

Two more checks, both for faults invisible until someone walks the corridor:

  * **no room overlaps** — two rooms sharing space is a hub you can fall between
  * **all rooms inside the hull** — a room poking through the skin shows the outside
    from inside, which reads as a hole rather than a window

Verified on two very different hulls (the ship, and Grimlock as a stand-in for a
differently-proportioned one): both fit 8 rooms with no overlaps and nothing outside.

**A bug worth recording**: the first version floored room size at `0.6`, written for
metres. Meshes are normalised to −1..1, so on a 2-unit hull *every* half-extent
clamped to 0.6 — all eight rooms became the same oversized box and overlapped each
other. The floor is now a fraction of the hull. Absolute constants and normalised
coordinates do not mix.
