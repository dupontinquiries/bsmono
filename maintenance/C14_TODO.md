# C14_TODO — open threads from the c14 session

Same convention as `C9`–`C13`: every item says how far it actually got. None of
those files are superseded.

Session: `3f1e9e5a..45f7bf26`, 38 commits. blackstar-rs at **1546 green** under
`--no-default-features`. New crates: `shared/opus-kit`,
`games/shared/isosurface`, `games/shared/linebank`.

Three themes ran end to end: **the licensed-vocabulary cleanup finished**,
**smooth terrain became buildable**, and **a voice pipeline exists that has
never touched a microphone but has run on real files**.

---

## 0. THINGS THAT COST THIS SESSION TIME, AND WILL COST YOURS

Read this section before running anything. Every line is something that
presented as a different problem than it was.

**The default feature set does not build in a container.** `cpal` needs
`libasound2-dev` and there is no `sudo`. Use `--no-default-features`, which the
manifest already documents as the silent-binary path. Symptom without it: a
link error that looks like a code error.

**`cargo test` without `--bins` fails before running anything.**
`examples/silhouette_audit` references `crate::anim` and `crate::hardpoints`,
which do not resolve. Three errors, nothing to do with your change, and they
mask the entire suite. Use `cargo test --no-default-features --bins`.

**`--arch emit-c` needs the binary rebuilt FIRST.** Running it against a stale
binary writes a header that looks updated and is not — the file's mtime moves
and its contents do not. Chain it: `cargo run ... -- --arch emit-c && cargo
test ...`. The header check is the only thing that catches this.

**Disk fills at ~5 GB per build tree and there are several.** The container hit
100% twice. The second time presented as `rustc` exiting 101 with "could not
compile" and no error above it, which reads exactly like a code failure. Check
`df` before believing a mysterious compile error. `games/unendlich/target`
reached 5.4 GB, `blackstar-rs/target` 3.2 GB.

**Anything under `apps/studio/crates/*/render/` or blackstar's `render/` is not
compiled headlessly.** `render` is behind `--features render-core`.
`cargo check --lib` passes and tells you nothing; the tests simply are not in
the binary, reported as "N filtered out, 0 passed", which reads like a filter
typo. A module written there is unverified BY CONSTRUCTION. `smooth_vertex.rs`
lost a revision to this before being moved to `src/world/`.

**Backticks in a `git commit -m` message get command-substituted** and silently
delete words from the body. Use `-F` with a file for anything long.

**Test builds are 5–9 minutes.** Run them detached (`setsid nohup ... &`) and
poll, or the harness times out mid-link and you learn nothing.

**The toolchain**: `rustup.sh` wants `static.rust-lang.org`, which is not in the
allowlist, but the zip carries the full offline bundle —
`PREFIX=/home/claude/.rust sh INSTALL.sh` works. `export
PATH=/home/claude/.rust/bin:$PATH`.

---

## 1. THE VOCABULARY CLEANUP — CLOSED, 30 FLOORS REMAIN

~680 licensed marks down to 30, none of which are in-fiction vocabulary. The
live enforcement is **`blackstar-rs/src/vocab.rs`**, a census that asserts an
exact count per term so a removal FAILS and prompts you to lower the number,
and an addition fails too.

`docs/RENAME_MAP.md` is now a record rather than a plan. `docs/films/
CANON_DECISIONS.md` has the full cast under "The cast, named".

### The names, and which are load-bearing

| was | is |
|---|---|
| Autobots / Decepticons | **the Enduring** / **the Sundered** |
| Optimus Prime | **Keelwright** |
| Rodimus / Hot Rod | **Halyard** / **Jib** (Jib becomes Halyard — the name change IS the inheritance) |
| Megatron | **Cleave** |
| Jhiaxus | **Assay** |
| Unicron | **the Unending** |
| the Matrix | **the Charge** |
| Prime (title) | dropped |
| Cybertronian | **Machine** (`Body::Machine`, `machine.rs`) |
| Cybertron | **the Foundry** in the archive; **home** to the Enduring |
| energon | **valence** |

Three pairs stop working if one is taken and the other left:
Enduring/Unending, Keelwright/Halyard, Jib/Halyard.

### Five ways a vocabulary sweep bites, all of which happened

1. **It leaves `src/`.** Boss ids in `assets/raids/bosses.json`, catalogue ids
   in `tools/meta/library.json`, thirteen docs including `STORY_BIBLE.md`
   (whose claim block is asserted against `archive::CLAIMS`).
   `raidcon`'s roster-matches-json test was twice the only thing standing
   between a rename and silent data drift.
2. **It corrupts code that reasons ABOUT the names.** `saga.rs` has two guards
   whose needles are the licensed words. A sweep rewrote them **five separate
   times**, turning a licensed-mark detector into a detector of the replacement
   names — and the symptom was always a count going UP, reading like a
   reintroduction. Now guarded by `vocab::no_sweep_has_eaten_a_guard`, and the
   files are listed in `vocab::DO_NOT_SWEEP` (`saga.rs`, `grandeur.rs`,
   `vocab.rs`).
3. **It falsifies citations.** `grandeur.rs` is a table of measurements from
   real reference material where the LABEL identifies the source. A sweep
   renamed "Optimus Prime" to this project's character, turning a published
   28 ft production figure into a false attribution. Only the ungrammatical
   half of the same sweep ("a the Foundry spire") made anyone look.
4. **Writing about a mark creates an occurrence of it.** Guard needles, policy
   notes, this file. A census cannot tell a use from a mention, so every
   explanation raises a floor. Expected, not alarming.
5. **`\b` does not work.** Underscores are word characters, so `\benergon\b`
   misses `energon_value` and `at_energon`. The first pass left 30 occurrences
   and looked like it had worked.

### The lesson that generalises

**A guard only guards what it enumerates.** The needles were eaten five times
before a test was added. Then the same failure appeared one level up: the
census is case-sensitive by design and nobody enumerated SCREAMING_CASE, which
is exactly where display strings and public statics live — `patrol.rs` had
`name: "CYBERTRON"` on a destination **the player reads**, `survival.rs`
returned `"CYBERTRONIAN"`, `saga.rs` exported `RODIMUS_PRIME_ARC`. Nine in
`src/`, thirteen in the screenplays, all reported as zero.

### The 30 that remain and cannot go lower

Citations in `grandeur.rs` / `compact.rs` / `xform.rs` (~15), guard needles
(6), asset filenames naming a real game (3), policy notes (~6). Each is
documented in `CENSUS` with its reason.

---

## 2. THE WORLDHOST SEAM — three modules across, physics is a decision

`games/shared/worldhost` existed and was used at **three call sites** while
thirteen modules still took `&World`. It was declared more than done.

Now: `footing.rs` and `camera.rs` are through the seam, `src/seam.rs` holds the
list plus a test that reads each module's source via `include_str!` and fails
if `&World` gets back in. **Verified by injecting a regression and watching it
fire** — a guard that has never failed is unproven.

Three methods were added, each because porting needed a fact the trait could
not express: `gravity_dir`, `ground_plane` (because `ground_h` returns `f32`
and so can only say HOW HIGH, never WHETHER), and `platform_vel`. All default
to the flat-world answer.

`worldhost::conformance` is the contract as executable functions, public so
other crates' tests can call it. It runs against blackstar's tile `World`
(MEADOW and KUIPER), the unendlich adapter, and the built-in hosts.

**`worldhost-unendlich` was written against the engine source and had never
compiled.** It does now (`--features engine`). Rewritten into `VoxelSource` (3
questions) plus a pure `Host`, so the geometry tests in milliseconds and the
`ChunkManager` impl is nine lines. Two bugs fixed in the rewrite:

* `ChunkManager::block_at` returns `Air` for a NON-RESIDENT chunk, so a
  clearance built on it called the streaming frontier free space and a body
  walked off the world. `Cell` is three-state now and `Unloaded` defaults to
  `Solid`.
* `ground_h` must return `y + 1`, the top FACE. Returning the index buries the
  body half a block.

**OPEN — physics.** `worldhost`'s header says physics is deliberately outside,
and the numbers agree: 15 in-trait accesses against 75 outside, mostly
`bodies`(18), `normal`(9), `shapes`(8), `launchers`(6). It does not ask "what
is the world like here", it asks "give me your solids". Behind a trait that
costs a virtual call per contact in the inner loop or a shape vocabulary both
engines share. **Do not start it by adding one more method to the trait.**

---

## 3. SMOOTH TERRAIN — headless half complete, render half untouchable here

`games/unendlich/docs/TERRAIN_RESOLUTION.md` is the decision record. Two of its
claims were WRONG and are corrected there:

* Fields are **not** per-cell. `FIELD_RESOLUTION = 8`, so a field grid is 8³ =
  512 samples per 32³ chunk — 2 KB, not 128 KB. The "fields dominate storage"
  argument against a smaller block was not real.
* **The density field already exists.** `WorldModel::density(x,y,z,t) -> f64`
  is the source of truth for solidity and `cpu_generator` builds the block grid
  by THRESHOLDING it at 0.0. The block array is a one-bit quantisation of the
  terrain; the precision a smooth mesher needs was computed and discarded.
  Generated terrain needs **no new storage at all**.
  `density_from_surface`'s own comment says it is tuned to be monotonic through
  the surface — which is exactly what linear interpolation between two samples
  needs. The generator was written to be isosurfaced; nobody had done it.

Built: `games/shared/isosurface` (surface nets, gradient normals),
`world/smooth.rs` (the bridge + materials), `world/smooth_plan.rs` (LOD
planning), `world/smooth_vertex.rs` (12-byte GPU format),
`world/smooth_chunk.rs` (one call, plan → uploadable).

**LOD stitching is solved**, and `TERRAIN_RESOLUTION.md` was wrong that it
would be the bulk of the work. Boundary cells facing a coarser neighbour are
meshed on the NEIGHBOUR'S cell grid (`Seams`) — 1.46 cells of disagreement down
to 0.14 — and the structural remainder closes completely when the fine chunk
meshes one coarse footprint PAST the plane, at which point it owns the
neighbour's cell and emits the bridging quads. Every coarse vertex matches to
within 1e-3.

**A failed approach is recorded in the header so it is not retried**:
quantising the boundary vertex POSITION made the seam WORSE (3.86 → 5.27),
because surface nets encodes the surface location *in* the vertex position, so
rounding destroys the answer.

**`PackedVertex` cannot carry this path** — no normal, and `pack_pos` clamps to
`[0,32]` so every LOD-overlap vertex would silently collapse onto the chunk
boundary. The seam work undone at upload, by a clamp, with no error anywhere.

**Blocked here, not blocked in general**: the Vulkan pipeline binding, the
octahedral-normal shader, and the smooth/greedy choice in `meshing_pool` are
all inside the feature-gated tree.

### 0.5 m blocks

Still open, and now cheaper than it looked. The real blocker is not memory: the
engine has **no block-to-metre constant** and **eight independent gravity
constants in three unit systems** — the player falls at 28 blocks/s² while
loose entities fall at 9.8 m/s², and the two documented block lengths differ by
3x. `crates/unendlich-engine/src/scale.rs` names the scale once and pins the
current state in characterisation tests. It changes nothing: retuning gravity
is a design decision that wants a playtest.

---

## 4. THE VOICE PIPELINE — complete except encode

Everything that does not need libopus is built, tested and composed. It has run
on the real screenplays.

```
cargo run -- --vo                    # the bank, its faults, its size
cargo run -- --vo --write DIR        # a script per actor, ready for a booth
```

Against the real tree: 56 written lines, 16 barks, 5 speakers, 130 takes,
~322 s in the booth, **1.00 MB of Opus against 31 MB as WAV**.

Crates: `games/shared/linebank` (ids, screenplay parsing, script, slicing,
alignment, audit, budget, the whole import in `session::import`),
`shared/opus-kit` (container arithmetic), plus `wav` and `envelope` added to
`shared/audio-kit`.

### Design decisions worth not relitigating

* **Variety is a selection problem, not a content problem.** `Bag` draws
  without replacement and refuses to put the take you just heard first after a
  reshuffle. Five takes through a bag beat a dozen through `rand()`.
* **Units are phrases, not words.** Splicing word-level takes fails on PROSODY,
  not on the joins. `Carrier` models carrier-and-slot and exists mainly to make
  the cost visible: two slots of 4 and 6 is 24 reads, not 10.
* **Ids derive from CONTENT** — speaker plus opening four words. Position-
  derived ids renumber on insertion and ids are permanent. Content-derived ids
  are stable under reordering and change only when the opening words change,
  which is exactly when a re-record is due.
* **Alignment is by order and mismatches are loud.** No slate heuristic: half
  the bank is blips and a blip is SHORTER than the slate announcing it.
* **Encode last.** Record lossless, slice, verify, then encode. Masters are
  never vendored.

### Bugs found in existing code

* `apps/voiceterm/src/acoustic_index.rs::load_wav_16k_mono` advances by
  `p += size` with **no RIFF pad byte**. One odd-sized chunk — exactly what a
  metadata-writing recorder produces — and every later chunk id reads a byte
  off, `data` is never found, and the file SILENTLY does not load.
* `pre_skip` is handled in `beatspace` and `studio-media` independently;
  `studio-media` also handles the **end-granule trim**, which is the one that
  gets missed. On a loop it is a tick every bar; on a bank of 300 ms blips it
  is a soft edge on every one.

### Open

* **Opus ENCODE.** Nothing in the tree encodes Opus; bo-suite's clips came from
  an external tool. Needs libopus behind a feature.
* A pack container saves 22% on blips alone but only 5.1% across a realistic
  bank, and costs a bespoke demuxer.
  `assets::the_pack_is_not_yet_worth_a_bespoke_demuxer` asserts the 10% line so
  the decision reopens by failing.

---

## 5. STORY — decisions are in `docs/films/CANON_DECISIONS.md`

That file is the record and says "settled unless marked OPEN". Built this
session from it: `link.rs` (the Watch's tether and the fare home),
`superform.rs` (super bodies DERIVED, not authored), `fragment.rs` (Fragments
of the Lattice), and the Lattice as **boss 20** (`the_anvil`, SYNC, arena
`unending`).

Two design failures worth knowing:

* **The obvious super-body transformation flattens the roster.** "Taller and
  broader, breadth faster than height" scored **-0.98** on recognisability —
  super Ice read as Fire — because BREADTH IS THE AXIS THE ROSTER IS SPREAD
  ALONG and `profile` is height-normalised. The fix separates uniform growth
  (`height_m` alone, which costs no identity) from amplified nature (each knob
  pushed further FROM the ice reference). A fragment should make a body more
  itself.
* **Four tests hardcoded the boss count and only one should have.**
  `raidcon`'s is the right home for the literal; the others now derive from
  `BOSSES.len()`, so a roster change is acknowledged in exactly one place.

Still OPEN in canon: the ledger (specificity loss — must cost DETAIL, never
power), the thermal gradient, a fourth `Medium` for a finding that is GIVEN
rather than found.

---

## 6. MODULES WITH NO CALLER

Honest gaps, not forgotten ones.

* **`suitability.rs`** — environment-driven vehicle choice, fully tested, no
  consumer, because no vehicle-selection surface exists. Wiring it now would
  mean inventing the caller.
* `linebank::verify` is called only by tests; the real caller needs
  `voiceprint-kit` wired to actual audio.

`link.rs` DID get a caller: `finale.rs`'s Photon climax now refuses off the
tether, which is the one place in six films the ceiling is reached.

---

## 7. THE HABIT THAT FOUND THE MOST BUGS

Nearly every real defect this session came from **a test written to assert a
design claim rather than a threshold**.

* Comparing gradient normals against face-averaged ones (to justify the
  choice) found that the triangle winding was inverted — the world would have
  rendered inside out, and watertightness, seam closure and vertex placement
  are all winding-agnostic, so nine passing tests said nothing.
* Asserting the `Bag` sequence differs between seeds found that `seed | 1`
  collapses ADJACENT seeds, so callers seeded from an incrementing id all
  heard the same sequence.
* Running the screenplay parser on the REAL files found seven speakers for five
  actors, because `**THE MORPHER (V.O.)**` is the same person. The fixture was
  correct and complete and missed it, because it was written from the same
  mental model as the parser.

The corollary: **when a test fails at a number close to the assertion, find out
why before loosening it.** The normals test failing at 0.9903 against 0.995 was
discretisation error — but loosening it to 0.98 would have hidden the winding
bug. The envelope test failing at 500 vs 502 was integer-hop drift worth
documenting, not smoothing over.
