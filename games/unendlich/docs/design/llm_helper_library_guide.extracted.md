
Rust Helper Library Design
Guidance for a Large Language Model — v2
Grep-First Scanning · Compact Metrics Scratchpad · Reuse Ratios · Performance Tracking · 1000-File Session Protocol · Checkpoint Format
v1→v2: Added §3b Grep-First Scanning (exact shell commands per pattern, run once across all files), §7b Compact Metrics Scratchpad (numeric-only tracking table, reuse ratio, performance gains), §10 1000-File Session Protocol (batching, priority sort, checkpoint blob). All additions are designed to minimise token usage — grep instead of read, numbers instead of prose, checkpoint instead of re-scan.
This document instructs a large language model (Opus 4.8 / Fable or equivalent) on how to refactor a 1000-file Rust game-engine codebase into shared helper libraries whose design priorities are: pure functions, maximum code reuse, fewer total lines, and measurable performance improvements. The example code is a seed sample — the real job is the full codebase, processed across many sessions.


  🦀  Codebase Context — What This Project Is

1.  Codebase Context
The codebase is a voxel game engine written in Rust, organised as a Cargo workspace. The workspace crate of interest is `unendlich-engine`. It is a mixed crate — pure data/logic modules coexist with thin egui UI shells and simulation code. The example files provided are:

File
What it contains
creature_design.rs
Procedural creature blueprints — coloured box sketches. Seed-derived dragon skin genetics (scale bumpiness, pattern, accent colour). Box roles (Body / Neck / Head / Wing / etc.). No physics, no ECS.
creature.rs
Multi-AABB physics bodies. Part enum (Jaw, Foot, Wing, Eye…), Capabilities booleans, PartState health boxes, GradedCapabilities continuous health fractions, Anatomy builder.
creature_viewer.rs
Pure orthographic projection of a blueprint into a unit square. ViewAxis (Side/Front/Top), ProjectedBox, stats readout, ViewerState. No egui dependency.
creature_viewer_ui.rs
Thin egui shell over creature_viewer. ComboBox pickers, allocate_painter, rect_filled. Feature-gated on egui_debug. All logic delegated to the pure layer.
shadow_cascade.rs
PSSM shadow-split distances. Pure math: uniform vs logarithmic blend, cascade lookup. Fully unit-tested.
shadow_length.rs
Shadow length factor from sun elevation. One function, pure, panic-free.
inventory.rs
ItemStack, SlotGrid, Hotbar, Inventory. Pure data model — no I/O or rendering. Stack merging, splitting, two-pass add, total/remove/is_full_for.
inventory_screen.rs
InventoryScreen state machine. Open/close/toggle, Tab cycling, cursor-held-stack, left_click / right_click interactions. Pure, deterministic, unit-tested.

The real codebase you will be asked to work with is much larger — genetics, ECS, world generation, rendering, AI simulation, bending physics, audio. The files above are the seed sample that makes the patterns visible. The same patterns will recur throughout the full codebase. Your first job with any new file is to identify which known pattern it exhibits, not to understand every line.


  🎯  Goals — What the Refactoring Should Achieve

2.  Goals
Two primary goals drive the library design. They are ordered: correctness first, then both goals simultaneously.

Goal 1 — Functional programming style. Pure functions (no mutation of reference arguments, deterministic output for identical inputs, no hidden state). Functions compose. Data flows through transforms, not out-parameters. Iterators preferred over index loops when the intent is clearer.
Goal 2 — Reduced total lines of code through function reuse. When the same three-line calculation appears in five places, it becomes one named function and five call sites. The goal is not golf — names and doc-comments should be kept. The goal is that changing the algorithm in one place changes it everywhere.

A third goal follows from the first two: discoverability. A new function should be easy to find. Helper modules should have narrow, honest names. A module called `bbox` has bounding-box utilities. A module called `hash_util` has hash functions. Do not create a catch-all `util.rs` with unrelated items.

2.1  Non-Goals
Do not change public API signatures unless the refactoring requires it. Functions that are tested should remain callable with the same arguments — the new helper may be used internally, but the outer function stays.
Do not convert imperative code that is already clear to iterators for the sake of style. Convert it when the duplicate logic becomes a reusable function.
Do not add dependencies. All helpers should use only the Rust standard library and types already present in the codebase. No new Cargo dependencies.
Do not break determinism. Every function in the helper libraries must be deterministic given its inputs. No random state, no time, no I/O.


  🔍  Scanning Strategy — Working Across Files Larger Than Context

3.  Scanning Strategy — Larger Than Context
You will be given more code than fits in one context window. Do not try to read everything before acting. Use the following staged approach:

3.1  Stage 1 — Catalogue Before Reading
Before reading any file in detail, get the file list and sizes. Ask the user (or use available tools) to produce:

  Shell — cataloguing before reading
# File list with line counts for the engine crate:
find crates/unendlich-engine/src -name "*.rs" | xargs wc -l | sort -rn | head -40

# For each file you are about to read, check what it imports:
grep -n "^use " src/target_file.rs | head -30

The import list tells you immediately which other modules this file depends on, which is often enough to classify the file's role without reading it. A file that imports only `std` and local `world::blocks` is almost certainly a pure data module. A file that imports `egui` is a thin UI shell.

3.2  Stage 2 — Pattern Recognition Pass
Read each file looking for the six pattern categories listed in §4. Do not try to understand the whole file — scan for signatures. Specifically:

Any inline f32/f64 arithmetic that operates on [f32; 3] or [f32; 2] arrays → bbox or vecmath candidate.
Any function whose body is only a wrapping_add / wrapping_mul sequence on u64 → hash_util candidate.
Any function that folds over an iterator of Option<T> or Part-filtered slices → iter_util candidate.
Any pair of functions that differ only in whether they compute a min or a max (or a sum vs product) → a single higher-order function with a closure argument.
Any match arm that is duplicated across files (e.g., the ViewAxis::axes() pattern appearing again in a different projector) → shared enum or projection utility.
Any method that modifies a &mut Vec<Option<T>> in two passes → inventory_ops candidate.

Log every candidate with: file name, line number, pattern category, and a one-line description. Do not refactor yet — complete the scan of all files first so you can see the full extent of each pattern.

3.3  Stage 3 — Cluster by Pattern, Design Library, Then Rewrite
Once the scan is complete, group candidates by pattern category. For each group: design the helper function, write it once in the appropriate helper module, then go back and replace each occurrence. Run the tests after each replacement, not at the end.

Order matters:  The lowest-level utilities (hash, vec math) must exist before higher-level utilities (bbox, projection) that call them. Design bottom-up, refactor top-down.


  ⚡  Grep-First Scanning — One Command Per Pattern, Across All Files

3b.  Grep-First Scanning — Token-Efficient Discovery
The codebase has over 1000 files. Reading each one to find patterns is prohibitively expensive in tokens. Instead: run one grep per pattern across the entire source tree at the start of every session. The grep output is small (file paths + line numbers); the file content is large. Read a file only after grep confirms it contains a target pattern. This is the most important efficiency rule in the document.

Token cost comparison:  grep -rl across 1000 files: ~80 tokens of output. Reading 1000 files at 80 lines average: ~80,000 tokens. Grep-first costs 0.1% as much for the discovery phase.

3b.1  Canonical Grep Commands — One Per Pattern
Run these at session start. Save the output as the GREP_RESULTS block in your scratchpad (§7b). Do not re-run them mid-session — the results are stable unless new files are added.

  Bash — run all at session start, save output to scratchpad GREP_RESULTS block
# ── Pattern 4.1: splitmix64 hash (any file containing the magic constants)
grep -rl "0x9E37_79B9_7F4A_7C15\|0xBF58_476D_1CE4_E5B9" src/

# ── Pattern 4.2: AABB inline bounds loop (the characteristic lo/hi fold)
grep -rln "f32::MIN\|f32::MAX" src/ | xargs grep -l "lo\.min\|hi\.max" 2>/dev/null

# ── Pattern 4.3: inline 3D normalise (squared-length then sqrt then divide)
grep -rn "\[0\] \* .*\[0\] + " src/ | grep -v "#\[test\]\|//!" | cut -d: -f1 | sort -u

# ── Pattern 4.3b: inline per-channel lerp (the accent-blend pattern)
grep -rn "\[0\] - base\[0\]\) \* t\|base\[k\].*accent\[k\]" src/ | cut -d: -f1 | sort -u

# ── Pattern 4.4: typed-part fraction fold (filter by Part kind, map fraction)
grep -rn "\.fraction()" src/ | grep -v "#\[test\]" | cut -d: -f1 | sort -u

# ── Pattern 4.5: modular ring advance (rem_euclid or manual % n)
grep -rln "rem_euclid\|% ALL\.len\|% n as i32\|% len as i32" src/

# ── Pattern 4.6: two-pass slot iteration (the "Pass 1/Pass 2" or quota loop)
grep -rln "Pass 1\|Pass 2\|// top up\|remaining.*== 0.*break\|iter_mut.*slot" src/

# ── BONUS: find every inline clone() in iterator chains (allocations to cut)
grep -rn "\.clone()\." src/ | grep -v "#\[test\]\|//!" | wc -l

# ── BONUS: find every Vec::new() in a loop body (avoidable allocations)
grep -rn "Vec::new\|vec!\[\]" src/ | grep -v "#\[test\]\|struct\|pub " | wc -l


3b.2  Priority Sort — Which Files to Process First
After running the grep commands, sort the discovered files by total pattern hits. Files with the most hits across all patterns have the highest refactoring yield. Process them first.

  Bash — priority sort: most hits first = highest yield per token spent
# Count total pattern hits per file (combine all grep outputs, count lines per file):
cat grep_p41.txt grep_p42.txt grep_p43.txt grep_p44.txt grep_p45.txt grep_p46.txt \
  | sort | uniq -c | sort -rn | head -30

# Output looks like:
#  12 src/creature_design.rs
#   9 src/creature_viewer.rs
#   7 src/genetics.rs
#   6 src/world/chunk.rs
# ... (process in this order)

# The top-30 files will account for roughly 70-80% of the refactoring yield.
# Process the rest in bulk (they each have 1-2 hits; fix mechanically).


3b.3  Single-File Confirmation Before Reading
When you are about to read a specific file, first run a targeted grep to confirm which lines have the pattern and what the surrounding context is. This often tells you everything you need to make the replacement without reading the whole file.

  Bash — targeted single-file grep before reading: read lines not files
# Before reading genetics.rs — confirm pattern 4.1 and get context:
grep -n "wrapping_add\|wrapping_mul\|>> 40\|>> 30\|>> 27\|>> 31" src/genetics.rs

# Output (3 lines) tells you: function name, line number, whether it's identical
# to mix64, and whether there's a unit-map variant. You now know exactly what
# to replace without reading the other 400 lines of the file.

# Before reading creature_viewer.rs for pattern 4.2:
grep -n "f32::MAX\|f32::MIN\|lo\.min\|hi\.max\|\.offset\[" src/creature_viewer.rs

# This shows you all the loop variables and axis indices at once.
# Compare the line numbers — if they cluster (lines 590-600 and 625-638)
# you have two separate instances. If they are spread across the file,
# you may have a helper already partially in place.


3b.4  The "Read Second" Rule
Only read a file's full content when the grep output is ambiguous — when you cannot determine the replacement from the matching lines alone. Ambiguous situations: the surrounding type is unclear, the replacement needs to know which trait the struct implements, or the pattern spans a method boundary that grep can't show. Even then, request only the relevant range (line numbers from grep ± 20 lines), not the whole file.

Situation
Token-efficient action
grep shows exact match; replacement is mechanical
Replace without reading the file. Trust the grep. Verify by running tests.
grep shows match; unclear which struct owns the method
grep -n "^impl\|^pub struct" file.rs | head -20 — get the type hierarchy without reading logic.
Pattern spans multiple lines; grep misses it
grep -A 3 -B 1 "pattern_start" file.rs — get 5 lines of context, not the whole file.
Truly need to understand the file
Read only: the module doc comment (head -30), the pub fn signatures (grep "^pub fn"), and the relevant method body. Not the tests, not the helper fns.


  🗂  Pattern Catalogue — Six Categories Found in the Seed Code

4.  Pattern Catalogue — Instances Found in Seed Files
The following patterns were identified in the eight seed files. The same patterns will appear throughout the larger codebase with the same structural signature. Use this catalogue as your recognition vocabulary.

4.1  Splitmix64 Hash — Duplicated Identically in Two Files
  Pattern — two identically-structured functions with different names
  creature_design.rs: fn skin_mix(seed: u64) -> u64   (lines ~80-85)
  creature_viewer.rs: fn hash_unit(x: u64) -> f32     (lines ~550-555)

  skin_mix does the splitmix64 bit-mixing.
  hash_unit does the same mixing then maps to [0,1) via >> 40 / 2^24.
  They share every magic constant. They are the same function.

  skin_unit in creature_design.rs wraps skin_mix → [0,1) the same way as hash_unit.
  So skin_unit ≡ hash_unit — just with "skin_" prefix and called from a different file.

This is the highest-confidence duplicate in the seed: character-for-character identical logic, different names, different files. Every subsequent file that needs deterministic per-index hashing will write its own copy unless you extract it.

  Rust — extracted hash_util::mix64 and mix64_unit
// Target signature for hash_util module:

/// Splitmix64: bijective u64 → u64 bit-mixer. Deterministic, avalanche-correct.
/// Use as the foundation for any per-seed deterministic value generation.
#[inline]
pub const fn mix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Map a u64 hash to [0, 1). The top 24 bits after mix give uniform floats.
#[inline]
pub fn mix64_unit(x: u64) -> f32 {
    (mix64(x) >> 40) as f32 / (1u64 << 24) as f32
}

// creature_design.rs after refactor:
fn skin_unit(seed: u64) -> f32 { crate::hash_util::mix64_unit(seed) }

// creature_viewer.rs after refactor:
fn hash_unit(x: u64) -> f32 { crate::hash_util::mix64_unit(x) }
// (or inline the call directly — the named wrapper is optional)


4.2  AABB Bounds — Five Separate Inline Loops Over Box Slices
  Pattern — axis-aligned bounding box computed inline each time
  creature_design.rs::standing_height()  — iterates boxes, min/max on Y
  creature_design.rs::eye_height()       — iterates boxes, min on Y only
  creature_design.rs::resting_gaze()     — iterates boxes, min/max on X
  creature_viewer.rs::project()          — iterates boxes, min/max on U and V axes
  creature_viewer.rs::stats()            — iterates boxes, min/max on X, Y, Z all three

  All five loops have identical structure:
      let mut lo = f32::MAX;
      let mut hi = f32::MIN;
      for b in &bp.boxes {
          lo = lo.min(b.offset[i] - b.half[i]);
          hi = hi.max(b.offset[i] + b.half[i]);
      }
  Only the axis index `i` and the source slice differ.

  Rust — extracted bbox::axis_bounds and aabb3
// Target — bbox module, works for any iterator of (center, half) pairs:

/// Axis-aligned bounding interval for one axis across a set of boxes.
/// Returns (min_edge, max_edge). Returns (0.0, 0.0) on empty input.
pub fn axis_bounds(boxes: impl Iterator<Item = (f32, f32)>) -> (f32, f32) {
    boxes.fold((f32::MAX, f32::MIN), |(lo, hi), (center, half)| {
        (lo.min(center - half), hi.max(center + half))
    })
}

/// Full 3-axis AABB from an iterator of BlueprintBox-like items.
/// Returns ((xmin,xmax), (ymin,ymax), (zmin,zmax)).
pub fn aabb3(boxes: impl Iterator<Item = [f32; 6]> + Clone)
    -> ((f32, f32), (f32, f32), (f32, f32))
{
    // Each item: [cx, cy, cz, hx, hy, hz]
    let x = axis_bounds(boxes.clone().map(|b| (b[0], b[3])));
    let y = axis_bounds(boxes.clone().map(|b| (b[1], b[4])));
    let z = axis_bounds(boxes       .map(|b| (b[2], b[5])));
    (x, y, z)
}

// standing_height after refactor:
pub fn standing_height(&self) -> f32 {
    let (lo, hi) = bbox::axis_bounds(
        self.boxes.iter().map(|b| (b.offset[1], b.half[1]))
    );
    if hi >= lo { hi - lo } else { 0.0 }
}


4.3  3D Vector Math — Inline Normalise, Lerp, Dot
  Pattern — [f32; 3] arithmetic written inline at each use site
  creature_design.rs::gaze_forward():
      let len = (d[0]*d[0] + d[1]*d[1] + d[2]*d[2]).sqrt();
      [d[0]/len, d[1]/len, d[2]/len]

  creature_viewer.rs::pattern_modulate(): inline lerp for each channel
      base[k] + (accent[k] - base[k]) * t
  (also used as a closure `mix` inside the same function)

  creature_viewer.rs::project(): bumpiness shading inline multiply per channel
      *c = (*c * shade).clamp(0.0, 1.0)

  These will recur everywhere that handles positions, colours, or directions.
  The pattern is always: array of 2 or 3 f32, element-wise operation.

  Rust — extracted vecmath::norm3, lerp3, scale_clamp01
// vecmath module — no-dependency, const-fn-friendly where possible:

pub type V3 = [f32; 3];

#[inline] pub fn len3(v: V3) -> f32 { (v[0]*v[0] + v[1]*v[1] + v[2]*v[2]).sqrt() }

#[inline] pub fn norm3(v: V3) -> V3 {
    let l = len3(v);
    if l > 1e-6 { [v[0]/l, v[1]/l, v[2]/l] } else { [1.0, 0.0, 0.0] }
}

/// Element-wise linear interpolation. t is NOT clamped — callers clamp if needed.
#[inline] pub fn lerp3(a: V3, b: V3, t: f32) -> V3 {
    [a[0] + (b[0]-a[0])*t, a[1] + (b[1]-a[1])*t, a[2] + (b[2]-a[2])*t]
}

/// Scale every element and clamp to [0,1]. Used for colour shading.
#[inline] pub fn scale_clamp01(v: V3, s: f32) -> V3 {
    [v[0]*s, v[1]*s, v[2]*s].map(|x| x.clamp(0.0, 1.0))
}

/// Average of a non-empty iterator of V3s. Panics on empty.
pub fn mean3(iter: impl Iterator<Item = V3>) -> V3 {
    let (sum, n) = iter.fold(([0.0f32;3], 0u32),
        |(s, n), v| ([s[0]+v[0], s[1]+v[1], s[2]+v[2]], n+1));
    assert!(n > 0);
    [sum[0]/n as f32, sum[1]/n as f32, sum[2]/n as f32]
}

// gaze_forward after refactor:
pub fn gaze_forward(&self) -> V3 {
    let eye = self.eye_point();
    let snout = self.boxes.iter().find(|b| b.role == BoxRole::Snout)
        .map(|b| b.offset);
    let d = snout.map_or([1.0, 0.0, 0.0], |s|
        [s[0]-eye[0], s[1]-eye[1], s[2]-eye[2]]);
    vecmath::norm3(d)
}


4.4  Fractional-Average Over a Typed Part Set
  Pattern — the same filter-fold structure repeated for mean and min
  Anatomy::mean_fraction(kind: Part) -> Option<f32>
      for ps in self.parts.iter().filter(|p| p.part == kind) { sum += fraction; n += 1 }
      if n == 0 { None } else { Some(sum / n as f32) }

  Anatomy::min_fraction(kind: Part) -> Option<f32>
      self.parts.iter().filter(|p| p.part == kind)
          .map(|p| p.fraction()).fold(None, |acc, f| Some(acc.map_or(f, |a| a.min(f))))

  Both functions: filter by kind, map to f32, reduce with a different combiner.
  The pattern will repeat for max_fraction, total_fraction, any_destroyed, etc.
  A single higher-order function eliminates all of them.

  Rust — fractions_of iterator replaces mean_fraction and min_fraction
// In anatomy.rs (or a shared iter_util module):

/// Reduce fractions of all parts of `kind` with `fold_fn`.
/// Returns None if no such part exists on this anatomy.
fn fold_fraction<F>(&self, kind: Part, init: f32, fold_fn: F) -> Option<f32>
where F: Fn(f32, f32) -> f32
{
    let mut result = None;
    for ps in self.parts.iter().filter(|p| p.part == kind) {
        let f = ps.fraction();
        result = Some(result.map_or(f, |acc| fold_fn(acc, f)));
    }
    // Adjust for mean: sum was accumulated, need to divide by count.
    // Use the count version instead for mean (see mean_fraction below).
    result
}

// Or more elegantly with iterators:
fn fractions_of(&self, kind: Part) -> impl Iterator<Item = f32> + '_ {
    self.parts.iter().filter(move |p| p.part == kind).map(|p| p.fraction())
}

fn min_fraction(&self, kind: Part) -> Option<f32> {
    self.fractions_of(kind).reduce(f32::min)
}

fn mean_fraction(&self, kind: Part) -> Option<f32> {
    let (sum, n) = self.fractions_of(kind).fold((0.0, 0u32), |(s, n), f| (s+f, n+1));
    if n == 0 { None } else { Some(sum / n as f32) }
}

// Future callers get max_fraction for free:
fn max_fraction(&self, kind: Part) -> Option<f32> {
    self.fractions_of(kind).reduce(f32::max)
}


4.5  Modular Index Cycling — Tab and Hotbar
  Pattern — (index + delta).rem_euclid(len) appears in multiple types
  inventory_screen.rs::Tab::next()   → (index + 1)   % ALL.len()
  inventory_screen.rs::Tab::prev()   → (index + len - 1) % len
  inventory.rs::Hotbar::scroll()     → (selected + delta).rem_euclid(n)

  All three are the same "wrap an integer delta around a ring of N items" operation.
  When the pattern appears on menus, tab bars, hotbars, and skill rings,
  it should be one function, not three.

  Rust — ring_advance eliminates all modular-index cycling variants
/// Advance `index` by `delta` steps, wrapping around `len`. Works for
/// positive and negative deltas. Panics if `len == 0`.
#[inline]
pub fn ring_advance(index: usize, delta: i32, len: usize) -> usize {
    ((index as i32 + delta).rem_euclid(len as i32)) as usize
}

// Tab::next after refactor:
pub fn next(self) -> Tab { Tab::ALL[ring_advance(self.index(), 1, Tab::ALL.len())] }

// Hotbar::scroll after refactor:
pub fn scroll(&mut self, delta: i32) {
    self.selected = ring_advance(self.selected, delta, self.grid.len());
}


4.6  Two-Pass Mutable Slot Scan — SlotGrid::add and remove
  Pattern — two passes over &mut Vec<Option<T>> with a quota
  SlotGrid::add():
    Pass 1: iterate all slots, merge into matching stacks until count hits 0.
    Pass 2: iterate all slots again, place into empty slots until count hits 0.
    Return leftover.

  SlotGrid::remove():
    Single pass: iterate slots, drain matching items until remaining hits 0.
    Structurally the same loop, different predicate and accumulator direction.

  When crafting, loot, and auto-sort arrive, this pattern multiplies.
  A functional iterator approach processes each slot exactly once and
  composes without the raw loop.

  Rust — drain_slots replaces both passes in add and the loop in remove
// A functional helper that processes slots with a per-slot function.
// The fold accumulates a "remaining quota"; the slot fn returns how much it consumed.

/// Apply `slot_fn` to each slot in order until `quota` reaches zero.
/// `slot_fn(slot, quota) -> consumed`. Mutates `slots` in place via the slot fn.
pub fn drain_slots<T, F>(slots: &mut [Option<T>], quota: u16, mut slot_fn: F) -> u16
where F: FnMut(&mut Option<T>, u16) -> u16
{
    let mut remaining = quota;
    for slot in slots.iter_mut() {
        if remaining == 0 { break; }
        remaining -= slot_fn(slot, remaining);
    }
    remaining  // leftover not consumed
}

// remove() after refactor — single-pass, reads naturally:
pub fn remove(&mut self, item: BlockId, n: u16) -> u16 {
    let taken = drain_slots(&mut self.slots, n, |slot, remaining| {
        match slot {
            Some(s) if s.item == item => {
                let take = remaining.min(s.count);
                s.count -= take;
                if s.is_empty() { *slot = None; }
                take
            }
            _ => 0,
        }
    });
    n - taken  // actually removed
}



  📦  Helper Library Specifications — Where Each Utility Lives

5.  Helper Library Specifications
Each helper module is a small, focused .rs file within the engine crate (or extracted to a sub-crate if it becomes large). Place helper modules in `crates/unendlich-engine/src/util/` to keep them separate from the domain code. Expose them via `pub use util::bbox;`, etc., from `lib.rs`.

Module
File
Contains — full API surface
hash_util
util/hash_util.rs
mix64(u64)->u64, mix64_unit(u64)->f32. Optionally: mix64_salt(u64, u64)->u64 for salted seeds (replaces the XOR-with-constant pattern in dragon_skin_genes). No other items.
vecmath
util/vecmath.rs
V3 = [f32;3], V2 = [f32;2]. len2, len3, norm2, norm3, lerp2, lerp3, dot3, scale2, scale3, scale_clamp01, add3, sub3, mean3. All #[inline], all pure.
bbox
util/bbox.rs
Uses vecmath. axis_bounds(impl Iterator<Item=(f32,f32)>)->(f32,f32). aabb3 for triple-axis bounds. span(lo,hi)->f32. center(lo,hi)->f32. All pure, all take iterators so callers do not need to materialise slices.
ring
util/ring.rs
ring_advance(index, delta, len)->usize. Possibly ring_clamp (non-wrapping variant for sliders). Nothing else — keep this one small.
iter_util
util/iter_util.rs
drain_slots for Vec<Option<T>> with quota. option_mean, option_min, option_max (reduce wrappers that return None on empty). fold_nonzero (fold but short-circuit when accumulator hits zero). Do NOT put unrelated items here.

5.1  Module Dependency Order
Modules must be written in this order — each depends only on what is below it:

  Build order — write these first, before touching domain code
  1. hash_util  — no dependencies at all. Write first.
  2. vecmath    — no dependencies (pure arithmetic). Write second.
  3. bbox       — depends on vecmath for V3. Write third.
  4. ring       — no dependencies. Write any time.
  5. iter_util  — no dependencies (generic over T). Write any time.

  Domain refactors (creature_design, creature, creature_viewer, inventory,
  inventory_screen) happen AFTER all helpers are written and their own
  unit tests pass.

  Do not inline a helper call into domain code before the helper module
  compiles cleanly on its own.


5.2  Testing Policy for Helper Modules
Every function in every helper module must have at least one `#[test]`. The tests for helpers should be harder than the tests for the domain code — helpers are called everywhere, so they need to cover edge cases (empty iterators, zero lengths, NaN inputs, identity cases). Steal test cases from the existing domain tests where they apply.

  Rust — helper module tests must cover zero and NaN cases
// Example tests that must pass for vecmath::norm3:
#[test] fn norm3_unit_vec_unchanged() {
    let v = [1.0_f32, 0.0, 0.0];
    let n = norm3(v);
    assert!((len3(n) - 1.0).abs() < 1e-6);
    assert!((n[0] - 1.0).abs() < 1e-6);
}
#[test] fn norm3_zero_returns_fallback() {
    let n = norm3([0.0, 0.0, 0.0]);
    assert!((len3(n) - 1.0).abs() < 1e-6, "fallback must be a unit vec");
}
#[test] fn norm3_nan_does_not_propagate() {
    let n = norm3([f32::NAN, 0.0, 0.0]);
    assert!(n.iter().all(|x| x.is_finite()), "no NaN in output");
}



  ⚙  Workflow — How to Process Each New File You Are Given

6.  Workflow — Processing Each New File
When a file from the full codebase is added to context, follow these steps in order. Do not skip ahead to writing replacement code.

  Step-by-step file processing protocol
  STEP 1 — CLASSIFY:
    Read the module-level doc comment and the `use` imports.
    Label the file as: pure-math | pure-data | pure-logic | egui-shell | rendering | AI/sim.
    Pure modules are refactor targets. Non-pure modules may only need helper calls added.

  STEP 2 — SCAN FOR PATTERNS:
    Read the file looking for Pattern 4.1 through 4.6 (hash, bbox, vecmath,
    fraction-fold, ring-advance, slot-drain).
    Add each candidate to your running log: (file, line, pattern_id, function_name).

  STEP 3 — CHECK IF HELPER EXISTS:
    For each candidate: does the target helper function already exist?
    If yes: replace the inline code with a call to the helper.
    If no: write the helper first (in the appropriate util/ module),
           add its tests, confirm it compiles, then replace.

  STEP 4 — LOOK FOR NEW PATTERNS:
    After handling known patterns, look for NEW duplication not in the catalogue.
    If the same three-line calculation appears twice in THIS file, it is a
    candidate for a private helper function within the file.
    If it appears in any other file you have seen, it belongs in util/.
    Add the new pattern to the catalogue.

  STEP 5 — VERIFY:
    Confirm that all existing tests in the file still pass.
    Confirm the file compiles (or ask the user to verify if you cannot run cargo).
    Report: lines before / lines after / helpers added / helpers reused.


6.1  The Running Log Format
Keep a running log across all files processed. Format each entry consistently so it can be reviewed at the end of the session:

  Log entry format
  FILE: creature_design.rs
  PATTERN: 4.1 (hash duplicate)
  LOCATION: fn skin_mix (line 82), fn skin_unit (line 87)
  ACTION: replaced with hash_util::mix64, hash_util::mix64_unit
  LINES_SAVED: 7 (removed 2 functions, added 2 single-line calls)
  STATUS: done / pending / blocked

  FILE: creature_viewer.rs
  PATTERN: 4.1 (hash duplicate)
  LOCATION: fn hash_unit (line 552)
  ACTION: replaced with hash_util::mix64_unit
  LINES_SAVED: 4
  STATUS: done

  FILE: creature_viewer.rs
  PATTERN: 4.2 (AABB bounds)
  LOCATION: project() lines 590-600, stats() lines 625-638
  ACTION: replaced with bbox::axis_bounds and bbox::aabb3
  LINES_SAVED: 18
  STATUS: done



  📄  Output Format — What to Produce

7.  Output Format
For each session (a set of files processed together), produce the following, in this order:

1. New helper module source files (util/hash_util.rs, util/vecmath.rs, etc.), complete and compilable, with all tests.
2. Modified domain files, complete. Do not produce diffs — produce the full file. The user will replace the original.
3. The running log, consolidated, sorted by pattern ID.
4. A summary table: total lines before, total lines after, net reduction, helper functions added, call sites created.

If a helper module already exists from a previous session, produce only the additions to it (new functions), not the full file again — but flag which lines to insert and where.

7.1  Summary Table Format
  End-of-session summary table
  ┌──────────────────────────────────────────────────────────────────────┐
  │  REFACTORING SESSION SUMMARY                                         │
  ├──────────────────────────────┬──────────────────────────────────────┤
  │  Files processed             │  8 (seed set)                        │
  │  Lines before                │  1601                                │
  │  Lines after                 │  ????  (fill in after refactor)      │
  │  Net reduction               │  ????                                │
  │  Helper functions created    │  mix64, mix64_unit, axis_bounds,     │
  │                              │  aabb3, norm3, lerp3, scale_clamp01, │
  │                              │  mean3, ring_advance, drain_slots,   │
  │                              │  fractions_of                        │
  │  Call sites created          │  ????                                │
  │  Patterns still pending      │  none / list any blocked items       │
  └──────────────────────────────┴──────────────────────────────────────┘



  📊  Metrics Scratchpad — Compact Numeric Tracking

7b.  Metrics Scratchpad — What to Track and How
Prose log entries cost tokens and become unreadable at scale. Replace them with a compact numeric scratchpad that updates in place. The scratchpad has three parts: the helper registry (counts per function), the file tally (one row per processed file), and the session totals. Emit the full scratchpad at the end of each session as the checkpoint for the next session.

7b.1  The Scratchpad Format
  Compact scratchpad — maintain this across all turns in a session
  ── HELPERS ─────────────────────────────  (add a row when a helper is created)
  fn             | mod        | call_sites | p_id
  mix64          | hash_util  | 0          | 4.1
  mix64_unit     | hash_util  | 0          | 4.1
  axis_bounds    | bbox       | 0          | 4.2
  aabb3          | bbox       | 0          | 4.2
  norm3          | vecmath    | 0          | 4.3
  lerp3          | vecmath    | 0          | 4.3
  scale_clamp01  | vecmath    | 0          | 4.3
  mean3          | vecmath    | 0          | 4.3
  fractions_of   | anatomy    | 0          | 4.4
  ring_advance   | ring       | 0          | 4.5
  drain_slots    | iter_util  | 0          | 4.6

  ── FILE TALLY ──────────────────────────  (one row per processed file)
  file                    | p_ids    | loc- | helpers_called
  creature_design.rs      | 4.1,4.2  |  22  | mix64_unit(2) axis_bounds(1)
  creature_viewer.rs      | 4.1,4.2  |  25  | mix64_unit(1) axis_bounds(2) aabb3(1)
  creature.rs             | 4.4      |   9  | fractions_of(2)
  inventory.rs            | 4.5,4.6  |  11  | ring_advance(1) drain_slots(2)
  inventory_screen.rs     | 4.5      |   5  | ring_advance(2)

  ── SESSION TOTALS ──────────────────────
  files_processed:  5 / 1000
  loc_removed:      72
  call_sites_added: 12
  reuse_ratio:      12 / 11 = 1.09  (call_sites / helpers; target > 3.0 at 100 files)
  perf_gains:       2× iter eliminated, 1× alloc removed, 0 const_fn promotions
  new_patterns:     0

  ── GREP_RESULTS ────────────────────────  (from §3b.1 at session start)
  P4.1: creature_design.rs creature_viewer.rs genetics.rs dragon.rs [+12 more]
  P4.2: creature_design.rs creature_viewer.rs world/chunk.rs [+8 more]
  P4.3: creature_design.rs creature_viewer.rs physics.rs [+5 more]
  P4.4: creature.rs anatomy.rs ai/behaviour.rs [+3 more]
  P4.5: inventory.rs inventory_screen.rs ui/tabs.rs hotbar.rs [+6 more]
  P4.6: inventory.rs crafting.rs loot.rs [+4 more]


Reuse ratio target:  At 5 files: ratio of ~1.0 is normal (helpers just created). At 50 files: target > 3.0 (each helper called 3+ times on average). At 200 files: target > 6.0. A ratio below 2.0 after 50 files means you are creating helpers that are not being reused — stop and check if the pattern was real.

7b.2  Performance Metrics — Beyond Line Count
Line count is the primary metric but not the only one. Four performance-relevant categories should be tracked separately because they represent measurable runtime improvements, not just code size:

Metric
Label in scratchpad
What it means and how to count it
Double-iteration eliminated
iter×
A two-pass mutable scan replaced by a single-pass drain_slots call. Each elimination means the slot vector is traversed once instead of twice. Count: +1 per two-pass→one-pass replacement.
Heap allocation eliminated
alloc
A Vec::collect() or vec![] inside a hot path replaced by an iterator chain or a stack-allocated fixed array. Count: +1 per removed allocation in code that is called per-tick or per-entity.
const fn promotion
const
A function that previously could not be called at compile time is now `const fn` (the hash functions are the primary example — seed-derived lookups can fold to compile-time constants). Count: +1 per helper that gains `const fn`.
Clone eliminated
clone
A .clone() in an iterator chain replaced by a borrowed iterator or a Copy type. Count: +1 per removed clone() in non-test code.

Performance metrics go in the perf_gains line of the scratchpad as a compact list: `2× iter× 1 alloc 3 const 0 clone`. They do not need prose — the count is enough. When a session ends, the total across all sessions gives a clear picture of runtime improvement alongside code reduction.

7b.3  The Reuse Ratio — The Compound Efficiency Number
The reuse ratio is `total call_sites / total helper functions`. It answers the question: is the refactoring actually achieving reuse, or is it just moving code around? A helper called only once is a net loss (added abstraction, no reduction). A helper called ten times is a significant gain.

Reuse ratio range
What it means
Action
< 1.5 (early)
Helpers just created, not yet spread. Normal at < 10 files.
Continue. Do not evaluate yet.
1.5 – 3.0
Helpers being adopted but slowly. Some patterns may not be as widespread as expected.
Check GREP_RESULTS — confirm the patterns are present in files not yet processed.
3.0 – 8.0
Target range at 50+ files. Each helper is saving real lines.
Continue at current pace.
> 8.0
A few helpers are being used everywhere. Pattern was very widespread.
These high-ratio helpers should have extra tests. They are the most load-bearing code in the util layer.
Drops between sessions
New helpers were added but are not being called yet.
Check which helpers have 0 call_sites. Either process the files that need them, or reconsider whether they should exist.


  🚫  Anti-Patterns — What Not to Do

8.  Anti-Patterns to Avoid
8.1  Do Not Create a "util.rs" God Module
A single `util.rs` with unrelated functions is not a helper library — it is a junk drawer. Every item in a helper module must be cohesively related. If you find yourself adding a hash function and a ring-advance function to the same file, stop and separate them.

8.2  Do Not Extract a Function That Is Only Called Once
A private helper called exactly once is only justified when the name makes the calling code dramatically clearer. If the inline version is already clear, leave it. Extract when: (a) the same code appears twice, OR (b) the extracted function has a stable name that appears in documentation/comments already.

8.3  Do Not Change Types to Make Reuse Easier
If the helper works most naturally on `Iterator<Item = (f32, f32)>` but the call site uses `&[BlueprintBox]`, add the mapping at the call site rather than changing the helper to accept `&[BlueprintBox]`. The helper stays generic; the call site does the adaptation. This is the correct functional composition direction.

8.4  Do Not Remove Tests When Refactoring
If a function that had tests is replaced by a helper call, the test moves to the helper module. Do not delete the test — relocate it. If the test was testing the domain behaviour (not the math), keep it in the domain file and verify it still passes after the replacement.

8.5  Do Not Use Traits Where Functions Are Sufficient
For small helper libraries, free functions are better than traits. Traits are appropriate when multiple types need to implement the same behaviour. If the helper only ever operates on `[f32; 3]`, it does not need a `Vector3` trait — it is just a function that takes `[f32; 3]`. Traits add complexity without benefit when there is only one implementor.

8.6  Do Not Ignore NaN and Division-by-Zero
The existing code already handles edge cases explicitly (the `if len > 1e-6` guard in `gaze_forward`, the `if n == 0` guard in `mean_fraction`). When extracting these into helpers, carry the guards along. A helper that panics on empty input or produces NaN is worse than the original inline code. Every helper function must specify its behaviour on pathological inputs in its doc comment.


9.  Quick Reference — Pattern → Target Function
When you see this in new code
Pattern ID
Target function to call or create
wrapping_add / wrapping_mul chain on u64 with the same three magic constants
4.1 — hash
hash_util::mix64(u64) -> u64
mix64 result >> 40 divided by 2^24 to get f32
4.1 — hash
hash_util::mix64_unit(u64) -> f32
lo = f32::MAX; hi = f32::MIN; for b { lo.min(..) hi.max(..) }
4.2 — bbox
bbox::axis_bounds(iter of (center, half)) -> (lo, hi)
Full 3-axis min/max loop over boxes for dimensions
4.2 — bbox
bbox::aabb3(iter of [cx,cy,cz,hx,hy,hz])
sqrt(x²+y²+z²) followed by divide-each-element
4.3 — vecmath
vecmath::norm3(V3) -> V3
a[k] + (b[k] - a[k]) * t for each channel k
4.3 — vecmath
vecmath::lerp3(a, b, t) -> V3
*c = (*c * shade).clamp(0.0, 1.0) per element
4.3 — vecmath
vecmath::scale_clamp01(V3, f32) -> V3
.filter(|p| p.part == kind).map(|p| p.fraction()).fold(None, …)
4.4 — fractions
anatomy.fractions_of(kind) -> impl Iterator<Item=f32>
(index + 1) % len or (index + len - 1) % len
4.5 — ring
ring::ring_advance(index, delta, len) -> usize
for slot in slots.iter_mut() { if remaining == 0 break; … }
4.6 — slot drain
iter_util::drain_slots(&mut slots, quota, slot_fn) -> u16

  🔁  1000-File Session Protocol — Batching, Checkpoints, and Continuity

10.  1000-File Session Protocol
One session cannot process 1000 files. Each session processes a batch, emits a compact checkpoint at the end, and the next session ingests that checkpoint to continue. The checkpoint is the only shared state. It must be pasteable in under 80 lines.

10.1  Session Sizing
Session type
Files processed
When to use
Bootstrap
0 — setup only
Session 1: run all greps (§3b.1), build priority list, create all util/ helpers with tests. Emit initial checkpoint. Process no domain files yet.
Normal
15–25 files
Standard batch. Ingest checkpoint, process next N files from priority queue, update scratchpad, emit checkpoint.
Bulk
40–60 files (1–2 hits)
For files with only mechanical replacements. Grep-confirm each line, replace without reading the file, batch 40+ at once.
Audit
0 — metrics only
Every ~100 files: check reuse_ratio, check for ZERO_CS_HELPERS, verify no test regressions.

10.2  Checkpoint Format
  Checkpoint — compact state blob; ~30 lines; everything the next session needs
# CHECKPOINT — paste at the start of the next session as-is
# session:4  processed:68/1000

HELPERS:  # fn | mod | call_sites | pattern | const
  mix64         hash_util  cs:47 p:4.1 const:y
  mix64_unit    hash_util  cs:39 p:4.1 const:y
  axis_bounds   bbox       cs:28 p:4.2 const:n
  aabb3         bbox       cs:11 p:4.2 const:n
  norm3         vecmath    cs:19 p:4.3 const:n
  lerp3         vecmath    cs:24 p:4.3 const:n
  scale_clamp01 vecmath    cs:8  p:4.3 const:n
  mean3         vecmath    cs:6  p:4.3 const:n
  fractions_of  anatomy    cs:14 p:4.4 const:n
  ring_advance  ring       cs:31 p:4.5 const:n
  drain_slots   iter_util  cs:9  p:4.6 const:n
# totals: helpers:11  cs:236  reuse_ratio:21.5

TOTALS: loc-:1847  iter×:12  alloc:7  const_fn:2  clone:9

QUEUE:  # next files, sorted by hit count desc
  genetics.rs:9  dragon.rs:8  world/chunk.rs:7  ai/behaviour.rs:6
  physics.rs:5  rendering/mesh.rs:5  [+932 remaining at 1-3 hits]

NEW_PATTERNS:
  P4.7: min([f32;3],[f32;3]) inline — genetics.rs:240 dragon.rs:118
         → target: vecmath::min3(V3,V3)->V3  (needs grep to count total)

BLOCKED: rendering/shader.rs  # reason: wgpu import; verify pure sections first
ZERO_CS: none


10.3  Session Start with a Checkpoint
When a session opens with a checkpoint: parse QUEUE (first 20–25 entries are this session's work list), parse NEW_PATTERNS (grep for any new ones before reading files), parse ZERO_CS (if a helper has been zero for two sessions, evaluate removing it). Do NOT re-run the full grep suite — the QUEUE already has sorted results. Only re-grep for newly discovered patterns.

10.4  Progress Milestones
Files done
Milestone check
Target metrics
10
All helpers created
All 11 helpers exist with tests. No ZERO_CS. reuse_ratio > 1.0.
50
Reuse established
reuse_ratio > 3.0. At least 3 helpers with cs > 10. iter× > 5.
100
Bulk phase
Remaining files mostly 1–3 hits. Switch to bulk sessions. loc_removed > 2× util/ line count.
300
New pattern audit
Any NEW_PATTERN with > 15 remaining instances → create helper now. reuse_ratio > 6.0. loc_removed > 8% of original codebase.
1000
Final summary
Final summary table (§7). Total loc_removed, perf totals, final reuse_ratio, BLOCKED files list.

10.5  When Not to Create a Helper
< 3 grep instances in the entire codebase: private function within the file only. Not in util/.
3–5 instances: create util/ helper, mark low-reuse. If no new call_sites after 100 files, inline it back.
6+ instances: always create the util/ helper. The name earns its existence.


11.  Final Deliverables
At completion (1000 files processed, or all pattern instances exhausted):

The util/ helper modules — complete, tested, stable. These become the project's shared vocabulary for all future code.
All processed source files with helpers substituted — complete files, not diffs.
The final checkpoint as the summary document: reuse_ratio, total loc_removed, all perf totals, BLOCKED list with reasons.
A one-paragraph addendum listing any new patterns discovered during the run, in the §4 format, for inclusion in the next version of this guidance document.


End of LLM guidance document v2. Supply this document as system context before providing codebase files. Session 1 is the bootstrap (grep + helpers only, no domain files). Every subsequent session: ingest checkpoint → process 15–25 files from QUEUE → update scratchpad → emit checkpoint. The reuse_ratio in the final checkpoint is the single most important indicator of whether the refactoring succeeded.

