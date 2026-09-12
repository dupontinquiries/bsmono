# Voxel workflow: narration, screen capture, and the tool layer

## Start here: `REMAINING_WORK.md`

Nine steps from this crate to typing in a world and watching a tower
rise. Roughly a day, and step 3 (the `VoxelWorld` impl on
`ChunkManager`, whose `set_block` takes `&self`) carries all the risk.
Everything in this crate is tested; nothing in it is compiled against
unendlich.

## Models — `src/model.rs`

`~/models` is already the monorepo convention
(`logscope::llm_backend::default_models_dir`). This mirrors that
contract with no dependencies, and adds what a chat surface needs:
the **quant tag** parsed from the filename, and a **load state
machine** so the box says `loading ...` then
`model loaded: qwen2.5-7b-instruct-q4_k_m  ctx 16384  in 3.2s`
instead of freezing silently.

```bash
cargo run -- models              # your real ~/models
cargo run -- models /some/dir    # or anywhere
```

- **Autocomplete prefers prefix, then substring**, because people type
  the size (`7b`) not the vendor prefix. `/model qw` + Tab completes.
- **Ambiguity is an error that lists candidates.** Silently picking the
  first match is how you load a 40 GB model by accident. An exact name
  always wins over a partial one.
- Shards (`-00002-of-00005`) are skipped so a split model appears once.
- `likely_fits(vram_gb)` is a crude labelled guess, there to warn before
  a load, not to be precise.

## Quickstart — how to actually run this

```bash
# A. the tool layer (no game needed, no network, no deps)
cd voxel
cargo run -- demo --ascii      # builds a scene, prints a cross-section
cargo run -- schemas           # the JSON tool schemas for tools=[...]
cargo run -- palettes          # palette -> block id table
cargo test                     # 9 tests

# B. the video pipeline
cd ../pipeline/ocrscan && cargo build --release && cd ../../voxel
./run_video.sh /path/to/part1.mkv palette_example.tsv
```

`run_video.sh` does proxy -> motion plan -> dense capture of the
cinematic sections, and writes `<name>.plan.tsv`, `<name>.motion.tsv`
and `<name>.blocks.tsv` next to your video. Verified on a 25 s slice
of part1.mkv: it found the PAN section and pulled 77 block-change
events out of it.

If it reports no PAN sections, that is a real answer about the footage
about half the time -- see the calibration section in
MOTION_PLANNING.md before changing thresholds.


Three pieces, each usable on its own.

1. **Transcribe your verbal description** — same pipeline as before.
2. **Capture block modifications from the screen** — `ocrscan blocks`,
   a new mode that finds *changes*, not text.
3. **Give an LLM primitives to build with** — `voxel_tools.rs`.

---

## 1. Transcribe the narration

```bash
nix-shell            # from the pipeline directory

# one file, no chapter marks:
./transcribe.sh ~/videos/voxel-design transcripts distil-large-v3

# or directly, if you want to watch it:
whisper-ctranslate2 \
    --model distil-large-v3 \
    --language en \
    --output_dir transcripts \
    --output_format json \
    --vad_filter True \
    --compute_type auto \
    --initial_prompt "$(cat glossary_prompt.txt)" \
    ~/videos/voxel-design/session.mkv
```

Then the same cleanup chain, which matters more here than you might
expect — a design narration is dense with invented proper nouns
(biome names, block names, your own coinages) and those are exactly
what whisper mangles:

```bash
python3 mkglossary.py ~/monorepo/games/unendlich   # BlockId names etc.
python3 correct.py transcripts/*.json --out-dir corrected --report corrections.tsv
python3 suspects.py corrected/*.json --min-count 2 --out suspects.tsv
$EDITOR subs.tsv          # add real garbles you find; check context first
python3 clean_transcripts.py corrected --out-dir cleaned
```

`mkglossary.py` pointed at the unendlich crate picks up `BlockId`
variants, so `Dripstone`, `RedSandstone` and `Tuff` will be spelled
right instead of becoming "drip stone" and "red sand stone".

For an hour of narration the digest is worth generating too — but for
design intent specifically, read it. An hour is ~8k words, twenty
minutes of reading, and the payoff is a written spec of how you build
things, which is what the tool layer should be shaped around.

---

## 2. Capture block modifications: `ocrscan blocks`

Text OCR is the wrong instrument for a voxel editor. What matters is
**what changed where**, which is a frame-differencing problem needing
no model at all.

```bash
# make a palette file from your own block colours, one line each:
#   name<TAB>#rrggbb
printf 'stone\t#808084\noak_planks\t#ba9660\ngrass\t#4a603a\n' > palette.tsv

ocrscan blocks --video session.mkv \
    --interval 1 \
    --palette palette.tsv \
    --grid-x 32 --grid-y 18 \
    --threshold 18 --min-cells 1 \
    --out blocks.tsv
```

Output is a timeline of edit events:

```
t=2.0   160x120 at (280,320)  cells=12  #7f7f81 -> stone
t=5.0   160x160 at (800,280)  cells=15  #b8945f -> oak_planks
t=7.0   160x120 at (280,320)  cells=12  #282f30 -> ?grass
```

Read that third row: a region reverted to terrain colour, so it is a
**removal**, and the `?` prefix means the colour was not close to any
palette entry — the tool flags uncertainty rather than guessing.

How it works, and its limits:

- Frames are downsampled to a coarse grid (default 32x18) so camera
  drift and dithering do not register as edits. Raise `--grid-x/y`
  for finer localisation, at the cost of noise sensitivity.
- Changed cells are grouped into connected components; each becomes
  one event with a bounding box and cell count.
- The event colour is taken from the **top half of cells by delta**,
  not the mean of the whole region. This matters: averaging the whole
  component mixes the new block with terrain still visible at its
  edges, and on the test frame that named oak planks "stone" because
  the blend landed midway between two palette entries.
- **Non-goals, on purpose:** no 3D reconstruction, no per-voxel
  identity. Screen pixels cannot give those honestly.

Pair it with the transcript by timestamp and you have both halves:
the narration says what you *meant*, the timeline says what actually
appeared, and disagreements are the interesting part.

---

## 3. The tool layer: `voxel/voxel_tools.rs`

Thirteen tools an LLM can build structures with, built against
unendlich's real types — `BlockId` discriminants from
`world/blocks.rs`, and a `VoxelWorld` trait that `ChunkManager`
satisfies with its existing `block_at` / `set_block`.

**Reads** — `find_surface`, `probe_column`, `read_region`, `density`
**Writes** — `set_block`, `fill_box`, `sphere`, `cylinder`,
`place_tree`, `scatter`, `replace`
**Support** — `palette_block`, `undo_edit`

`tool_schemas()` emits OpenAI-style JSON, so it drops straight into
`tools=[...]` or logscope's provider layer.

### Why these, specifically

Every choice follows from one fact: **the model cannot see the
world.**

- **Reads return summaries, never voxels.** A 32-cube is 32,768
  cells; as JSON that is tens of thousands of tokens. `read_region`
  answers in one line: cell count, percent solid, top six materials.
- **`find_surface` exists so Y is never guessed.** Nearly every
  hallucinated voxel structure is floating or buried because the model
  invented a Y coordinate. The schema description says to call it
  first, in those words.
- **One call makes a recognisable thing.** `place_tree` beats a
  hundred `set_block` calls, because a hundred calls is a hundred
  chances to drift, and the real authoring decisions are
  species/height/lean — not leaf positions.
- **Writes are bounded and self-reporting.** Each returns
  `blocks_changed` plus an AABB, and anything over 200,000 blocks is
  *refused with the limit named* rather than attempted.
- **Everything is undoable as a unit.** `undo_edit` reverses exactly
  one call. Without it a model "fixes" mistakes by piling more blocks
  on top.
- **Palettes are named, not numeric.** `palette_block("volcanic",
  "primary")` beats memorising that Basalt is 38, and lets a structure
  be restyled by changing one argument.

### Behaviours worth knowing

- `place_tree` canopies **never overwrite existing solid blocks**, so
  trees do not eat buildings placed earlier. There is a test for it.
- `scatter` is seeded, so "the same clearing" twice is the same
  clearing.
- `sphere` with `block=0` carves air — that is how you dig caves.

### Wiring it to unendlich

```rust
impl VoxelWorld for ChunkManager {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { self.block_at(x, y, z) as u16 }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
        self.set_block(x, y, z, BlockId::from_u16(b))
    }
}
```

Note `ChunkManager::set_block` takes `&self` in the current tree, so
either adapt the signature or wrap in whatever interior mutability it
already uses — check before assuming.

### Status

9 tests pass standalone (`rustc --test voxel_tools.rs`): surface
finding, tree placement and lean, canopy non-destruction, exact undo,
oversized-fill refusal, read compactness, scatter reproducibility,
palette resolution, and a schema/dispatcher agreement check so the
schema can never advertise a tool the dispatcher lacks. **Not
compiled against unendlich** — the trait impl above is the seam and
needs your engine's real signatures.

---

## Suggested order

1. Transcribe and *read* the narration. It is the spec.
2. Run `ocrscan blocks` over the same video. Now you have intent and
   action on one timeline.
3. Where the narration says "and then I decorate the edges" and the
   timeline shows fifty small scattered events, that is a primitive
   you are missing. Add it to `voxel_tools.rs`.

That third step is the actual method: the corpus tells you which
primitives to build, instead of you guessing the API from first
principles.
