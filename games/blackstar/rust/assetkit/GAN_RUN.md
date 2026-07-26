# Running the two-stage GAN pipeline on the real library

All paths below assume:

    LIB=/home/kesslerd/Downloads/tmp-claude/monorepo/20260720/blackstar/models

Build the CLI first (any rustc, no deps):

    cd rust/assetkit && rustc --edition 2021 -O src/cli.rs -o assetkit-cli

## 0. Pick donors — do not point it at the whole library

The library is tens of GB and every mesh is loaded to score it. Rank first, then
feed the winners in. `score` reads .gltf/.glb/.obj/.iqm directly, so no python
pipeline run is needed:

    ./assetkit-cli score $LIB/round-2-extras/bots --top 40 --csv bots.csv

Take the top of the `bot-bony` class. Then:

    mkdir -p donors
    cp -r $LIB/scorponok_trnasformers_human_alliance \
          $LIB/soundwave_robot_transformers_human_alliance \
          $LIB/megatron_tf1 \
          $LIB/brawl_robot_transformers_human_alliance \
          $LIB/laserbeak_transformers_human_alliance \
          donors/

## 1. Stage 1 — placement

    ./assetkit-cli place donors --out placement

Prints the position and scale ranges and says whether the descriptors vary. If they
come back constant, stage 1 has nothing to learn and something is wrong upstream.

## 2. Stage 2 — build box→part pairs

    ./assetkit-cli pairs donors --out pairs --degrade box --min-tris 50

Watch the relief gap it reports. **`box` should give roughly +150%.** If you run it
with `--degrade decimate` instead it reports about +3%, and the task becomes
unlearnable — that difference is the whole reason for the two-stage split.

## 3. The falsifier, with pictures

    ./assetkit-cli train pairs/pairs.f32 --epochs 8 --limit 400 --show 3

Prints per-epoch train/held-out L1, the two baselines, a verdict, and **ASCII
checkpoints** — input box, target part, net output, side by side, on held-out
examples.

Read the pictures, not just the loss. Current standing result:

    INPUT (box)      TARGET (part)          NET OUTPUT
    @@@@@@@@@@@@     %#####*---+*#%%%###*   +##******##*****#***
    @@@@@@@@@@@@     ######*---+*#%%%###*   *****##*###**#******
    @@@@@@                #%%*    #%%*      *****######**###

The targets differ structurally between examples; the net emits the same mushy
texture for both. That is what "found the output distribution without conditioning
on the input" looks like — and no loss value would have told you.

    vs IDENTITY: +34%     vs MEAN: +1%
    PARTIAL

## 4. When it passes

The verdict requires beating BOTH baselines by >5%. Which one is hard depends on
the degradation — with box inputs, IDENTITY is trivially bad, so MEAN is the real
bar. Only then is the GPU run in `model_cond.rs` worth starting.

The likely fix is capacity: three same-size convs have no bottleneck, so there is
nowhere to encode *which* box this is. `model_cond.rs`'s U-Net encodes down to
8x16 with skips, which is exactly the missing piece.

## Context conditioning — `--context`

    ./assetkit-cli pairs donors --out pairs --degrade box --context
    ./assetkit-cli train pairs/triples.f32 --epochs 10 --show 2

A lone bounding box is **ambiguous**: many different parts fit a box of the same
proportions, so a net given only the box cannot do better than emit the average
part — which is exactly what it did. `--context` writes a third plane carrying the
part's position, scale and aspect *within its model*, so a box at the shoulder and
an identical box at the ankle are no longer the same question. That plane is
stage 1's output feeding stage 2.

Measured, on the same 436 components:

    box only                      +34.2% vs identity   -1.5% vs mean
    box + context (blended)       +25.7%               -1.8%   ← worse
    box + context (real channel)  +34.8%               -0.6%   ← best

Two things worth taking from that. The conditioning **does** help, and it moves the
hard baseline (mean) in the right direction. But blending the context into the box
plane made things *worse* than not using it at all — the two planes mean different
things and averaging them destroys both. It has to be a real second channel.

It is still PARTIAL. The remaining gap looks like architecture rather than data:
three same-size convolutions have no bottleneck, so there is nowhere for "which box
is this" to be encoded even when the information is present in the input.
`model_cond.rs`'s U-Net encodes to 8x16 with skips, which is the missing piece.

## `fill` — genetic search, and why it works where the net did not

    ./assetkit-cli fill target.gltf --catalog solidcat/meshes \
        --pop 90 --gens 70 --passes 4 --out evolved

The conditional net kept finding the output *distribution* without conditioning on
its input, and every fix that added information helped only marginally. Search
sidesteps the problem entirely:

  * no training data — **the target is the fitness function**
  * it optimises the metric we care about rather than a proxy loss
  * every candidate is real catalog geometry, so it cannot produce a smear
  * no GPU

**Multi-pass strides.** Pass 1 fits a few large boxes over the whole region; each
later pass halves the box scale and doubles the gene count. Coarse structure settles
before detail, which is why the output reads as built rather than scattered:

     pass   genes    scale      start       best
        1       6     0.55     0.0259     0.1297
        2      12     0.28     0.0318     0.1980
        3      24     0.18     0.0364     0.2487
        4      48     0.14     0.0556     0.2856

**Hunting random walk.** Mutation step size decays with generation — large jumps
explore, small ones refine. A fixed step either never finds the basin or never
settles in it.

**Linker.** After evolution, adjacent parts are pulled a quarter of the way into
contact, so the assembly reads as connected structure rather than a swarm. That was
the single most-reported fault of the earlier placement work.

**The bar is random, not zero.** Every run scores the best of 40 random assemblies
and reports the margin. If evolution cannot beat random, the fitness function is not
guiding anything and the search is theatre:

    best of 40 RANDOM assemblies: -0.0216
    evolution beats random by +1424% -- the search is working

Fitness is IoU against the target occupancy grid minus a spill penalty. Overlap
alone would be gamed by one enormous part; the spill term is what forces a real fit.

### Tuning what actually matters

Two measured effects, both worth knowing before spending time on the wrong knob:

**Seeding helps the START, not the finish.** Genes are seeded onto cells the target
occupies rather than scattered through its bounding box — Jazz fills only 880 of
4096 cells, so uniform seeding starts 79% of every gene in empty space. That lifted
pass-4 starting fitness from 0.056 to 0.187, but final fitness barely moved
(0.286 → 0.294). The search finds the basin either way; seeding just gets there
sooner.

**Gene count is the real ceiling** — until it isn't:

    passes  genes   best fitness
       4       48        0.2836
       5       96        0.3342
       6      192        0.3852
       7      384        0.3661   ← worse

More parts fit better, up to a point. Past it the search space grows faster than the
budget to explore it. The tool keeps the best pass rather than the last, and says so
when a pass fails to improve.

A pass-6 fill of Jazz scores as real geometry:

    19800 verts, 6600 tris, 24 structural components
    compactness 116.4   open edges 0%   class: bot-bony

Watertight, structured, and classified bony by the same scorer used to rank the
library — which is what the whole detail effort was trying to reach.

## Regions, and a measurement that closes an old question

    ./assetkit-cli fill target.gltf --catalog cat/ --region chest [--interior]

`--region chest|legs|head|y0,y1,lateral` fills part of a model and keeps the shell,
so the result is "Jazz with a built chest" rather than a chest floating in space.
The lateral clip uses the model's **detected mirror axis** — assuming X gave Jazz a
chest region 0.14 wide by 2.00 deep, because Jazz mirrors about Z. That is the same
trap that produced the "weird arm plates" and the invisible ribs.

`--interior` targets the volume a shell *encloses*, found by flood-filling from the
grid boundary — whatever the flood cannot reach is inside. Filling a surface with
solid parts is near-impossible (anything thick enough to register spills through the
skin); filling a cavity is what "structure inside a hollow torso" actually means.

**It also settles a question this project kept circling.** Enclosed cells, whole
model, 16³ grid:

    jazz        3 of 880 shell cells
    grimlock   36 of 1056
    scorponok  10 of 713
    ravage      0 of 668

**These models have essentially no interior.** They are layered panels, not shells
around voids — which is why every earlier attempt to gut a chest and show ribs
produced nothing visible. The tool now detects this and says so, instead of
optimising against a target of two cells.

So the useful application of `fill` is **whole-model or large-region**, where it
works well (Jazz: IoU 0.385, 24 structural components, compactness 116, classified
bot-bony). Cavity-filling is not available on this library, and no amount of tuning
will make it so.

## Which catalog: harvested parts beat generated primitives

Same target, same budget, two parts bins:

    catalog                    IoU     tris  comps    compact   coher   open
    solid primitives        0.3342     3888     17      687.0   0.565     0%
    harvested real parts    0.3770    29869     96       55.2   0.814     2%

Harvested wins on everything that matters: **+13% fit**, **5.6x the structural
components** (96 against 17 — Scorponok, the reference, has 479), and much better
**coherence** (0.814 vs 0.565), meaning smoother, more organised surfaces rather
than a heap of boxes.

The primitives' compactness of **687** is the scoring metric being gamed, exactly as
predicted: disconnected boxes maximise surface-area-per-volume without being
structured at all. Coherence is what exposes it. That is the second time this
project has caught the bony score rewarding noise, and the reason the Library page
labels it "a filter, not a verdict".

Build a harvested catalog from your best models:

    ./assetkit-cli harvest donors/ --out hcat --min-tris 120
    ./assetkit-cli fill target.gltf --catalog hcat/meshes --passes 5

Cost is triangles — 30k against 4k — but that is the right trade for a showcase
model, and `--passes` controls it directly.

## Additive vs replacement — use `--keep-shell`

At IoU ~0.38 the evolved geometry is a **rough reconstruction**, not a better model.
Rendering the two side by side makes that plain: the replacement occupies roughly
the right space but its silhouette is visibly coarser than the original. Replacing a
good model with it is a downgrade.

`--keep-shell` makes the fill **additive** instead — the original model plus evolved
structure:

    model                          tris   comps  compact   coher
    original jazz                 10660      42     29.2   0.753
    evolved (replacement)         27848      96     55.2   0.814
    additive (shell+evolved)      38508     138     35.3   0.792

**Triples the structural members (42 → 138) while keeping the silhouette intact**,
because the shell is still there. That is the combination the whole detail effort
was after: more structure, same character.

It is the default for `--region` and opt-in for whole models:

    ./assetkit-cli fill target.gltf --catalog hcat/meshes --keep-shell --passes 5
    ./assetkit-cli fill target.gltf --catalog hcat/meshes --region chest
