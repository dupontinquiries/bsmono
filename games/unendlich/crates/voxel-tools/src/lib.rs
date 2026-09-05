//! Voxel tool calling: the primitive set an LLM builds structures with.
//!
//! DESIGN NOTE — why these primitives and not others.
//!
//! A model cannot see the world. Every design decision here follows
//! from that one fact:
//!
//!  * **Reads come before writes, and reads are cheap and summarised.**
//!    `probe_column` and `read_region` return compact digests (surface
//!    height, material histogram, occupancy), never raw voxel dumps.
//!    A 32x32x32 region is 32,768 blocks; as JSON that is tens of
//!    thousands of tokens and the model will drown. As a histogram it
//!    is twenty.
//!  * **Placement is relative to what is there.** `find_surface`
//!    exists so the model never has to guess a Y coordinate. Almost
//!    every hallucinated structure in a voxel world is floating or
//!    buried because the model guessed Y.
//!  * **One call should make a recognisable thing.** `place_tree` is
//!    a primitive rather than a hundred `set_block` calls, because a
//!    hundred calls is a hundred chances to drift, and because the
//!    interesting authoring decisions are species/height/lean, not
//!    individual leaf positions.
//!  * **Every write is bounded and reports what it did.** Each result
//!    carries `blocks_changed` and the affected AABB, so the model can
//!    verify rather than assume — and so a runaway call is capped by
//!    `MAX_BLOCKS_PER_CALL` instead of filling a world.
//!  * **Everything is undoable as a unit.** Each call returns an
//!    `edit_id`; `undo_edit` reverses exactly that call. Structure
//!    generation is iterative, and a model that cannot undo will
//!    instead try to "fix" a mistake by placing more blocks on top.
//!
//! The tool schemas are emitted as OpenAI-style JSON function specs
//! (`tool_schemas()`), so they drop into logscope's provider layer or
//! any `tools=[...]` parameter. The dispatcher (`VoxelTools::call`)
//! maps a name plus JSON arguments onto the engine. Schema and
//! dispatcher must agree; the test at the bottom enforces that.
//!
//! This module is deliberately engine-agnostic at its boundary: it
//! talks to a `VoxelWorld` trait, so it can be wired to unendlich's
//! `ChunkManager` (`block_at` / `set_block`) or to a test double.

/// The ingest path: prose -> IR -> validated plan -> batched writes.
pub mod plan;
/// Spatial arithmetic: compute it for the model, then check it.
pub mod math;
/// Placement focus: gaze -> anchor, scale tier, read region, feature id.
pub mod focus;
/// The chat loop: sequencing focus, read context, plan, echo, commit, undo.
pub mod chat;
/// Reveal scheduling: bottom-up, structure-before-decoration animation.
pub mod anim;
/// Prompt engineering: passes, grammar, worked examples.
pub mod prompt;
/// Post-build critique: floating blocks, entombment, sealed rooms.
pub mod critique;
/// Chat box: keys, history, autocomplete cards.
pub mod chatbox;
/// Voice: grammar and intent mapping for the voiced daemon.
pub mod voice;
/// Local model discovery, autocomplete, and load feedback.
pub mod model;
/// Staging layer: buffer writes, flush grouped by chunk.
pub mod staged;
/// The form layer: continuous solids, CSG, and rasterization to blocks.
pub mod solid;
/// Named forms as recipes over solids, so a new form is data.
pub mod dict;
/// Views: build from 2D elevations, and check the result against them.
pub mod views;
/// Weathering: decay, overgrowth and rubble as a transform over blocks.
pub mod weather;
/// Ambient: what the surroundings say about what belongs here.
pub mod ambient;
/// The model seam: Completer trait, reply cleaning, fakes.
pub mod llm;
/// One build turn end to end.
pub mod build_loop;

use std::collections::BTreeMap;

/// Cap on writes per call. A model that asks to fill a 500-block cube
/// gets an error naming the limit, not a stalled frame.
pub const MAX_BLOCKS_PER_CALL: usize = 200_000;

/// Minimal surface the tools need from the engine. unendlich's
/// `ChunkManager` satisfies this with `block_at` / `set_block`.
pub trait VoxelWorld {
    fn get(&self, x: i32, y: i32, z: i32) -> u16;
    fn set(&mut self, x: i32, y: i32, z: i32, block: u16) -> bool;
    /// Inclusive world Y bounds, so tools can clamp rather than fault.
    fn y_bounds(&self) -> (i32, i32) {
        (-64, 320)
    }
}

/// One reversible edit.
#[derive(Debug, Clone)]
pub struct Edit {
    pub id: u64,
    pub tool: String,
    /// (x, y, z, previous_block) for every changed voxel.
    pub before: Vec<(i32, i32, i32, u16)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aabb {
    pub min: (i32, i32, i32),
    pub max: (i32, i32, i32),
}

#[derive(Debug, Clone)]
pub struct CallResult {
    pub ok: bool,
    pub edit_id: Option<u64>,
    pub blocks_changed: usize,
    pub bounds: Option<Aabb>,
    /// Compact human/model-readable summary. This is what goes back
    /// into the conversation, so it stays short by construction.
    pub summary: String,
}

impl CallResult {
    fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, edit_id: None, blocks_changed: 0, bounds: None,
               summary: msg.into() }
    }
}

// ---------------------------------------------------------------- palette

/// A named material set. The model picks a palette by NAME and the
/// tools resolve concrete block ids, so a structure can be restyled
/// with one argument instead of the model memorising numeric ids.
#[derive(Debug, Clone)]
pub struct Palette {
    pub name: String,
    pub primary: u16,
    pub secondary: u16,
    pub accent: u16,
    pub foliage: u16,
}

/// Palettes built from unendlich's `BlockId` discriminants (see
/// `world/blocks.rs`). Kept as raw u16 so this module does not depend
/// on the engine crate; the names are the contract.
pub fn palettes() -> BTreeMap<&'static str, Palette> {
    fn p(name: &str, a: u16, b: u16, c: u16, f: u16) -> Palette {
        Palette { name: name.into(), primary: a, secondary: b, accent: c, foliage: f }
    }
    let mut m = BTreeMap::new();
    // primary, secondary, accent, foliage
    m.insert("stone",    p("stone", 1, 4, 58, 18));       // Stone, Cobble, StoneBricks
    m.insert("oak",      p("oak", 57, 17, 18, 18));       // Planks, Log, Leaves
    m.insert("desert",   p("desert", 48, 12, 24, 21));    // Sandstone, Sand, RedSandstone, Cactus
    m.insert("volcanic", p("volcanic", 38, 49, 39, 35));  // Basalt, Obsidian, Tuff, Lichen
    m.insert("polished", p("polished", 37, 42, 51, 23));  // Marble, Calcite, Quartz, Moss
    m.insert("frozen",   p("frozen", 31, 32, 42, 27));    // Ice, Snow, Calcite, Fern
    m.insert("cavern",   p("cavern", 41, 30, 47, 35));    // Slate, Dripstone, Crystal, Lichen
    // Biome themes. Added after counting the material vocabulary in a
    // corpus of build narration: a user asks for a biome by name
    // ("make it jungle"), not for four block ids.
    m.insert("jungle",   p("jungle", 17, 23, 29, 18));    // OakLog, Moss, Vine, Leaves
    m.insert("plains",   p("plains", 57, 4, 58, 18));     // Planks, Cobble, StoneBricks, Leaves
    m.insert("snowy",    p("snowy", 80, 31, 42, 27));     // SnowBlock, Ice, Calcite, Fern
    m.insert("mesa",     p("mesa", 84, 24, 83, 28));      // Terracotta, RedSandstone, SmoothSandstone, DeadBush
    m.insert("darkwood", p("darkwood", 86, 81, 34, 33));  // DarkOakPlanks, MossyCobble, Cobweb, Mushroom
    m.insert("ruined",   p("ruined", 81, 4, 34, 29));     // MossyCobble, Cobble, Cobweb, Vine
    m
}

// ------------------------------------------------------------- the toolset

pub struct VoxelTools<'w, W: VoxelWorld> {
    pub world: &'w mut W,
    pub edits: Vec<Edit>,
    /// Captured regions, referred to by handle so contents never
    /// enter the model's context.
    pub clips: Vec<Clip>,
    next_id: u64,
    next_clip: u64,
}

impl<'w, W: VoxelWorld> VoxelTools<'w, W> {
    pub fn new(world: &'w mut W) -> Self {
        Self { world, edits: Vec::new(), clips: Vec::new(), next_id: 1, next_clip: 1 }
    }

    fn begin(&mut self, tool: &str) -> Edit {
        let id = self.next_id;
        self.next_id += 1;
        Edit { id, tool: tool.to_string(), before: Vec::new() }
    }

    fn write(&mut self, edit: &mut Edit, x: i32, y: i32, z: i32, block: u16) -> bool {
        let (ylo, yhi) = self.world.y_bounds();
        if y < ylo || y > yhi {
            return false;
        }
        let prev = self.world.get(x, y, z);
        if prev == block {
            return false;
        }
        if self.world.set(x, y, z, block) {
            edit.before.push((x, y, z, prev));
            true
        } else {
            false
        }
    }

    fn finish(&mut self, edit: Edit, summary: String) -> CallResult {
        let n = edit.before.len();
        let bounds = if n == 0 {
            None
        } else {
            let (mut nx, mut ny, mut nz) = (i32::MAX, i32::MAX, i32::MAX);
            let (mut xx, mut xy, mut xz) = (i32::MIN, i32::MIN, i32::MIN);
            for &(x, y, z, _) in &edit.before {
                nx = nx.min(x); ny = ny.min(y); nz = nz.min(z);
                xx = xx.max(x); xy = xy.max(y); xz = xz.max(z);
            }
            Some(Aabb { min: (nx, ny, nz), max: (xx, xy, xz) })
        };
        let id = edit.id;
        self.edits.push(edit);
        CallResult { ok: true, edit_id: Some(id), blocks_changed: n, bounds, summary }
    }

    // ---- reads -----------------------------------------------------

    /// Highest non-air block at (x, z), searching down from `from_y`.
    /// The model should call this before placing anything that must
    /// sit on the ground.
    pub fn find_surface(&self, x: i32, z: i32, from_y: i32) -> Option<i32> {
        let (ylo, _) = self.world.y_bounds();
        let mut y = from_y;
        while y > ylo {
            if self.world.get(x, y, z) != 0 {
                return Some(y);
            }
            y -= 1;
        }
        None
    }

    /// Compact vertical profile: surface height plus the run-length of
    /// materials going down. Answers "what am I building on" in a
    /// handful of tokens.
    pub fn probe_column(&self, x: i32, z: i32, from_y: i32, depth: i32) -> String {
        let surface = self.find_surface(x, z, from_y);
        let Some(sy) = surface else {
            return format!("column ({x},{z}): all air from y={from_y}");
        };
        let mut runs: Vec<(u16, i32)> = Vec::new();
        for y in (sy - depth.max(1) + 1..=sy).rev() {
            let b = self.world.get(x, y, z);
            match runs.last_mut() {
                Some((bb, n)) if *bb == b => *n += 1,
                _ => runs.push((b, 1)),
            }
        }
        let desc: Vec<String> = runs.iter().map(|(b, n)| format!("{b}x{n}")).collect();
        format!("column ({x},{z}): surface y={sy}, down: {}", desc.join(" "))
    }

    /// Material histogram plus occupancy for a box. Never returns raw
    /// voxels: a 32-cube is 32,768 entries and would swamp the context.
    pub fn read_region(&self, min: (i32, i32, i32), max: (i32, i32, i32)) -> String {
        let mut hist: BTreeMap<u16, usize> = BTreeMap::new();
        let mut total = 0usize;
        for y in min.1..=max.1 {
            for z in min.2..=max.2 {
                for x in min.0..=max.0 {
                    *hist.entry(self.world.get(x, y, z)).or_insert(0) += 1;
                    total += 1;
                }
            }
        }
        let air = *hist.get(&0).unwrap_or(&0);
        let mut top: Vec<(u16, usize)> = hist.into_iter().filter(|(b, _)| *b != 0).collect();
        top.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
        top.truncate(6);
        let mats: Vec<String> = top.iter().map(|(b, n)| format!("{b}:{n}")).collect();
        format!(
            "region {:?}..{:?}: {total} cells, {:.0}% solid, top materials {}",
            min, max,
            100.0 * (total - air) as f64 / total.max(1) as f64,
            if mats.is_empty() { "none".into() } else { mats.join(" ") }
        )
    }

    /// Fraction of solid cells in a box, as a single number. Cheap
    /// enough to call repeatedly while carving caves or checking that
    /// a site is clear.
    pub fn density(&self, min: (i32, i32, i32), max: (i32, i32, i32)) -> f64 {
        let mut solid = 0usize;
        let mut total = 0usize;
        for y in min.1..=max.1 {
            for z in min.2..=max.2 {
                for x in min.0..=max.0 {
                    if self.world.get(x, y, z) != 0 {
                        solid += 1;
                    }
                    total += 1;
                }
            }
        }
        solid as f64 / total.max(1) as f64
    }

    // ---- writes ----------------------------------------------------

    // ---- writes from the build corpus --------------------------------
    //
    // Five operations a builder narrates constantly that the IR had no
    // word for. Counted across 15,721 words of build narration:
    // bridge/walkway 34, roof 29, storey/floor 25, "either side of" 15,
    // stairs 7. A model asked for a house produced walls and open sky,
    // because "roof" was not something it could say.

    /// A pitched roof over a footprint.
    ///
    /// `style`: "gable" ridges along the longer axis, "hip" slopes from
    /// all four sides, "flat" is a slab, "pyramid" meets at a point.
    /// Overhang extends the eaves past the walls, which is what makes a
    /// roof read as a roof rather than a lid.
    pub fn roof(&mut self, min: (i32, i32, i32), max: (i32, i32), style: &str,
                block: u16, overhang: i32) -> CallResult {
        let (x0r, y0, z0r) = min;
        let (x1r, z1r) = max;
        let (x0, x1) = (x0r.min(x1r) - overhang, x0r.max(x1r) + overhang);
        let (z0, z1) = (z0r.min(z1r) - overhang, z0r.max(z1r) + overhang);
        let (w, d) = (x1 - x0, z1 - z0);
        let mut e = self.begin("roof");
        match style {
            "flat" => {
                for x in x0..=x1 { for z in z0..=z1 { self.write(&mut e, x, y0, z, block); } }
            }
            "pyramid" | "hip" => {
                let steps = (w.min(d) / 2).max(1);
                for s in 0..=steps {
                    let (a, b, c, f) = (x0 + s, x1 - s, z0 + s, z1 - s);
                    if a > b || c > f { break; }
                    let cap = style == "pyramid" && s == steps;
                    for x in a..=b { for z in c..=f {
                        if cap || x == a || x == b || z == c || z == f {
                            self.write(&mut e, x, y0 + s, z, block);
                        }
                    }}
                }
            }
            _ => {
                // gable: ridge along the LONGER axis, two sloping faces
                let along_x = w >= d;
                let steps = ((if along_x { d } else { w }) / 2).max(1);
                for s in 0..=steps {
                    if along_x {
                        let (c, f) = (z0 + s, z1 - s);
                        if c > f { break; }
                        for x in x0..=x1 { for z in [c, f] {
                            self.write(&mut e, x, y0 + s, z, block);
                        }}
                    } else {
                        let (a, b) = (x0 + s, x1 - s);
                        if a > b { break; }
                        for z in z0..=z1 { for x in [a, b] {
                            self.write(&mut e, x, y0 + s, z, block);
                        }}
                    }
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("{style} roof, {n} blocks"))
    }

    /// Write a set of cells, all the same block. The views path uses
    /// this: a hull is already a cell list, so re-deriving a Solid from
    /// it just to rasterize again would be work for nothing.
    pub fn stamp_cells(&mut self, cells: &[(i32, i32, i32)], block: u16) -> CallResult {
        if cells.len() > MAX_BLOCKS_PER_CALL {
            return CallResult::err(format!(
                "refused: {} blocks, over MAX_BLOCKS_PER_CALL={}",
                cells.len(), MAX_BLOCKS_PER_CALL));
        }
        let mut e = self.begin("views");
        for &(x, y, z) in cells { self.write(&mut e, x, y, z, block); }
        let n = e.before.len();
        self.finish(e, format!("views, {n} blocks"))
    }

    /// Read the ambient conditions at a spot from the world itself.
    ///
    /// `temperature` and `humidity` come from the engine and cannot be
    /// derived here, so they are passed in; everything else is read
    /// from the blocks. The surface material especially: it is the
    /// strongest signal available and needs no climate model, because
    /// the world already decided what this spot is.
    pub fn read_ambient(&self, x: i32, z: i32, from_y: i32,
                        temperature: f32, humidity: f32, sea_level: i32)
        -> crate::ambient::Ambient {
        let surface_y = self.find_surface(x, z, from_y).unwrap_or(sea_level);
        let surface_block = self.world.get(x, surface_y, z);
        // Water fraction over a small ring: "is there water nearby" is a
        // question about the surroundings, not about one cell.
        let mut wet = 0;
        let mut n = 0;
        for dx in [-8, -4, 0, 4, 8] {
            for dz in [-8, -4, 0, 4, 8] {
                n += 1;
                if let Some(sy) = self.find_surface(x + dx, z + dz, from_y) {
                    // 9 = water in the block table
                    if self.world.get(x + dx, sy, z + dz) == 9 { wet += 1; }
                }
            }
        }
        crate::ambient::Ambient {
            temperature, humidity, surface_y, sea_level, surface_block,
            water: wet as f32 / n as f32,
            time_of_day: 0.0,
        }
    }

    /// Apply a planned set of weathering changes as ONE edit, so a ruin
    /// undoes as a unit rather than block by block.
    pub fn apply_changes(&mut self, changes: &[crate::weather::Change]) -> CallResult {
        if changes.len() > MAX_BLOCKS_PER_CALL {
            return CallResult::err(format!(
                "refused: {} changes, over MAX_BLOCKS_PER_CALL={}",
                changes.len(), MAX_BLOCKS_PER_CALL));
        }
        let mut e = self.begin("weather");
        for c in changes { self.write(&mut e, c.at.0, c.at.1, c.at.2, c.to); }
        let n = e.before.len();
        self.finish(e, format!("weathered {n} blocks"))
    }

    /// Rasterize a solid into the world.
    ///
    /// The bridge between the two layers. Everything above this line is
    /// continuous and grid-free; everything below is blocks. Conflicts
    /// were already resolved by `rank` during sampling, so this is a
    /// plain write -- which is the payoff: the ordering questions that
    /// plagued op-by-op building were answered one layer up.
    pub fn stamp(&mut self, solid: &crate::solid::Solid) -> CallResult {
        let cells = crate::solid::rasterize(solid);
        if cells.len() > MAX_BLOCKS_PER_CALL {
            return CallResult::err(format!(
                "refused: form covers {} blocks, over MAX_BLOCKS_PER_CALL={}",
                cells.len(), MAX_BLOCKS_PER_CALL));
        }
        let mut e = self.begin("form");
        for ((x, y, z), b) in cells {
            self.write(&mut e, x, y, z, b);
        }
        let n = e.before.len();
        self.finish(e, format!("form, {n} blocks"))
    }

    /// Mirror a region across an axis. `axis` is "x" or "z".
    ///
    /// The source is read ENTIRELY before anything is written: writing
    /// as we go would re-read blocks just placed and smear the
    /// reflection across itself when source and image overlap.
    pub fn mirror(&mut self, min: (i32, i32, i32), max: (i32, i32, i32),
                  axis: &str, about: i32) -> CallResult {
        let (lo, hi) = norm(min, max);
        let mut src = Vec::new();
        for x in lo.0..=hi.0 { for y in lo.1..=hi.1 { for z in lo.2..=hi.2 {
            let b = self.world.get(x, y, z);
            if b != 0 { src.push((x, y, z, b)); }
        }}}
        let mut e = self.begin("mirror");
        for (x, y, z, b) in src {
            let (nx, nz) = if axis == "x" { (2 * about - x, z) } else { (x, 2 * about - z) };
            self.write(&mut e, nx, y, nz, b);
        }
        let n = e.before.len();
        self.finish(e, format!("mirrored {n} blocks across {axis}={about}"))
    }

    /// A span at constant height with piers. Distinct from `path`, which
    /// follows the ground and would cut a trench across a ravine.
    pub fn bridge(&mut self, from: (i32, i32, i32), to: (i32, i32), width: i32,
                  block: u16, supports: bool) -> CallResult {
        let (x0, y, z0) = from;
        let (x1, z1) = to;
        let steps = (x1 - x0).abs().max((z1 - z0).abs()).max(1);
        let half = (width - 1).max(0) / 2;
        let mut e = self.begin("bridge");
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = x0 + ((x1 - x0) as f32 * t).round() as i32;
            let z = z0 + ((z1 - z0) as f32 * t).round() as i32;
            for dx in -half..=half { for dz in -half..=half {
                self.write(&mut e, x + dx, y, z + dz, block);
            }}
            // A pier every six blocks. Without them the deck floats, and
            // the critique pass would rightly flag the whole span.
            if supports && i % 6 == 0 {
                // Search from BELOW the deck. Starting at `y` finds the
                // deck block written moments ago on this same iteration,
                // reports it as the surface, and builds a pier of zero
                // height -- so every bridge floated, which is the exact
                // failure the critique pass exists to catch.
                if let Some(g) = self.find_surface(x, z, y - 1) {
                    for py in (g + 1)..y { self.write(&mut e, x, py, z, block); }
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("bridge, {n} blocks"))
    }

    /// A staircase rising one block per step, wide across its run.
    pub fn stairs(&mut self, from: (i32, i32, i32), to: (i32, i32), width: i32,
                  block: u16) -> CallResult {
        let (x0, y0, z0) = from;
        let (x1, z1) = to;
        let steps = (x1 - x0).abs().max((z1 - z0).abs()).max(1);
        let half = (width - 1).max(0) / 2;
        let (px, pz) = if (x1 - x0).abs() >= (z1 - z0).abs() { (0, 1) } else { (1, 0) };
        let mut e = self.begin("stairs");
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = x0 + ((x1 - x0) as f32 * t).round() as i32;
            let z = z0 + ((z1 - z0) as f32 * t).round() as i32;
            for w in -half..=half {
                self.write(&mut e, x + px * w, y0 + i, z + pz * w, block);
            }
        }
        let n = e.before.len();
        self.finish(e, format!("stairs, {n} blocks"))
    }

    /// A storey floor inside a footprint.
    pub fn floor(&mut self, min: (i32, i32, i32), max: (i32, i32), block: u16) -> CallResult {
        let (x0, y, z0) = min;
        let (x1, z1) = max;
        let mut e = self.begin("floor");
        for x in x0.min(x1)..=x0.max(x1) { for z in z0.min(z1)..=z0.max(z1) {
            self.write(&mut e, x, y, z, block);
        }}
        let n = e.before.len();
        self.finish(e, format!("floor, {n} blocks"))
    }

    pub fn set_block(&mut self, x: i32, y: i32, z: i32, block: u16) -> CallResult {
        let mut e = self.begin("set_block");
        self.write(&mut e, x, y, z, block);
        self.finish(e, format!("set ({x},{y},{z}) = {block}"))
    }

    pub fn fill_box(&mut self, min: (i32, i32, i32), max: (i32, i32, i32),
                    block: u16, hollow: bool) -> CallResult {
        let (lo, hi) = norm(min, max);
        let vol = volume(lo, hi);
        if vol > MAX_BLOCKS_PER_CALL {
            return CallResult::err(format!(
                "refused: {vol} blocks exceeds MAX_BLOCKS_PER_CALL={MAX_BLOCKS_PER_CALL}; \
                 split the region"));
        }
        let mut e = self.begin("fill_box");
        for y in lo.1..=hi.1 {
            for z in lo.2..=hi.2 {
                for x in lo.0..=hi.0 {
                    if hollow
                        && x != lo.0 && x != hi.0
                        && y != lo.1 && y != hi.1
                        && z != lo.2 && z != hi.2
                    {
                        continue;
                    }
                    self.write(&mut e, x, y, z, block);
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("filled {n} blocks{}", if hollow { " (shell)" } else { "" }))
    }

    pub fn sphere(&mut self, centre: (i32, i32, i32), radius: i32,
                  block: u16, hollow: bool) -> CallResult {
        if radius <= 0 {
            return CallResult::err("radius must be positive");
        }
        let vol = (2 * radius + 1).pow(3) as usize;
        if vol > MAX_BLOCKS_PER_CALL {
            return CallResult::err(format!("refused: bounding volume {vol} too large"));
        }
        let mut e = self.begin("sphere");
        let r2 = (radius * radius) as f64;
        let inner = ((radius - 1).max(0) * (radius - 1).max(0)) as f64;
        for dy in -radius..=radius {
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    let d2 = (dx * dx + dy * dy + dz * dz) as f64;
                    if d2 > r2 || (hollow && d2 < inner) {
                        continue;
                    }
                    self.write(&mut e, centre.0 + dx, centre.1 + dy, centre.2 + dz, block);
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("sphere r={radius}, {n} blocks"))
    }

    pub fn cylinder(&mut self, base: (i32, i32, i32), radius: i32, height: i32,
                    block: u16, hollow: bool) -> CallResult {
        if radius <= 0 || height <= 0 {
            return CallResult::err("radius and height must be positive");
        }
        let mut e = self.begin("cylinder");
        let r2 = (radius * radius) as f64;
        let inner = ((radius - 1).max(0) * (radius - 1).max(0)) as f64;
        for y in base.1..base.1 + height {
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    let d2 = (dx * dx + dz * dz) as f64;
                    if d2 > r2 || (hollow && d2 < inner) {
                        continue;
                    }
                    self.write(&mut e, base.0 + dx, y, base.2 + dz, block);
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("cylinder r={radius} h={height}, {n} blocks"))
    }

    /// A tree. One call, because the authoring decision is
    /// species/height/lean, not individual leaves. `species` selects
    /// trunk and foliage from the palette table; unknown species falls
    /// back to oak rather than failing, so a model's approximate guess
    /// still produces a tree.
    pub fn place_tree(&mut self, base: (i32, i32, i32), species: &str,
                      height: i32, lean: (i32, i32)) -> CallResult {
        let (trunk, leaf, canopy_r) = match species {
            "birch" => (19u16, 20u16, 2),
            "pine" | "spruce" => (17, 18, 2),
            "cactus" => (21, 0, 0),
            _ => (17, 18, 3),           // oak default
        };
        let h = height.clamp(3, 40);
        let mut e = self.begin("place_tree");

        // trunk, leaning progressively so the top offset equals `lean`
        let mut tip = base;
        for i in 0..h {
            let t = i as f64 / (h - 1).max(1) as f64;
            let x = base.0 + (lean.0 as f64 * t).round() as i32;
            let z = base.2 + (lean.1 as f64 * t).round() as i32;
            let y = base.1 + i;
            self.write(&mut e, x, y, z, trunk);
            tip = (x, y, z);
        }

        if canopy_r > 0 {
            // canopy: flattened sphere, thinner at the top, and never
            // replacing existing solid blocks so trees do not eat
            // buildings placed earlier
            let r: i32 = canopy_r;
            for dy in -r..=r {
                let ry = r - dy.abs() / 2;
                for dz in -ry..=ry {
                    for dx in -ry..=ry {
                        if dx * dx + dz * dz + dy * dy > ry * ry + 1 {
                            continue;
                        }
                        let (x, y, z) = (tip.0 + dx, tip.1 + dy, tip.2 + dz);
                        if self.world.get(x, y, z) == 0 {
                            self.write(&mut e, x, y, z, leaf);
                        }
                    }
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("{species} tree h={h} at {:?}, {n} blocks", base))
    }

    /// Scatter decorations on the surface within a footprint. Uses a
    /// seeded LCG so a described scene is reproducible — a model
    /// asking for "the same clearing" twice gets the same clearing.
    pub fn scatter(&mut self, min_xz: (i32, i32), max_xz: (i32, i32), from_y: i32,
                   block: u16, count: usize, seed: u64) -> CallResult {
        let (x0, x1) = (min_xz.0.min(max_xz.0), min_xz.0.max(max_xz.0));
        let (z0, z1) = (min_xz.1.min(max_xz.1), min_xz.1.max(max_xz.1));
        let (w, d) = ((x1 - x0 + 1) as u64, (z1 - z0 + 1) as u64);
        if w == 0 || d == 0 {
            return CallResult::err("empty footprint");
        }
        let mut e = self.begin("scatter");
        let mut s = seed | 1;
        let mut placed = 0usize;
        for _ in 0..count.min(MAX_BLOCKS_PER_CALL) {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let x = x0 + ((s >> 33) % w) as i32;
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let z = z0 + ((s >> 33) % d) as i32;
            if let Some(sy) = self.find_surface(x, z, from_y) {
                if self.world.get(x, sy + 1, z) == 0
                    && self.write(&mut e, x, sy + 1, z, block)
                {
                    placed += 1;
                }
            }
        }
        self.finish(e, format!("scattered {placed} of {count} requested"))
    }

    /// Replace one material with another inside a box. The workhorse
    /// for restyling: build in stone, then re-skin.
    pub fn replace(&mut self, min: (i32, i32, i32), max: (i32, i32, i32),
                   from: u16, to: u16) -> CallResult {
        let (lo, hi) = norm(min, max);
        if volume(lo, hi) > MAX_BLOCKS_PER_CALL {
            return CallResult::err("refused: region too large; split it");
        }
        let mut e = self.begin("replace");
        for y in lo.1..=hi.1 {
            for z in lo.2..=hi.2 {
                for x in lo.0..=hi.0 {
                    if self.world.get(x, y, z) == from {
                        self.write(&mut e, x, y, z, to);
                    }
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("replaced {n} blocks {from} -> {to}"))
    }

    /// Reverse one previous call exactly.
    pub fn undo_edit(&mut self, edit_id: u64) -> CallResult {
        let Some(pos) = self.edits.iter().position(|e| e.id == edit_id) else {
            return CallResult::err(format!("no edit {edit_id}"));
        };
        let edit = self.edits.remove(pos);
        let mut n = 0usize;
        for &(x, y, z, prev) in edit.before.iter().rev() {
            if self.world.set(x, y, z, prev) {
                n += 1;
            }
        }
        CallResult { ok: true, edit_id: Some(edit_id), blocks_changed: n, bounds: None,
                     summary: format!("undid edit {edit_id} ({} from {})", n, edit.tool) }
    }
}

fn norm(a: (i32, i32, i32), b: (i32, i32, i32)) -> ((i32, i32, i32), (i32, i32, i32)) {
    ((a.0.min(b.0), a.1.min(b.1), a.2.min(b.2)),
     (a.0.max(b.0), a.1.max(b.1), a.2.max(b.2)))
}

fn volume(lo: (i32, i32, i32), hi: (i32, i32, i32)) -> usize {
    let d = |a: i32, b: i32| (b - a + 1).max(0) as usize;
    d(lo.0, hi.0) * d(lo.1, hi.1) * d(lo.2, hi.2)
}

// ------------------------------------------------------------- schemas

/// OpenAI-style function schemas. Feed straight into `tools=[...]`.
///
/// Descriptions are written for the model, not for a human reader:
/// each says WHEN to use the tool and what mistake it prevents,
/// because that is what actually changes tool-selection behaviour.
pub fn tool_schemas() -> String {
    let s = r#"[
{"type":"function","function":{
 "name":"find_surface",
 "description":"Highest non-air block Y at (x,z). ALWAYS call this before placing anything that should sit on the ground; guessing Y is the most common cause of floating or buried structures.",
 "parameters":{"type":"object","properties":{
   "x":{"type":"integer"},"z":{"type":"integer"},
   "from_y":{"type":"integer","description":"start searching downward from here, e.g. 200"}},
  "required":["x","z","from_y"]}}},
{"type":"function","function":{
 "name":"probe_column",
 "description":"Compact vertical profile at (x,z): surface height and material runs going down. Use to learn what you are building on before choosing a foundation material.",
 "parameters":{"type":"object","properties":{
   "x":{"type":"integer"},"z":{"type":"integer"},
   "from_y":{"type":"integer"},"depth":{"type":"integer","description":"how far down to report, e.g. 8"}},
  "required":["x","z","from_y","depth"]}}},
{"type":"function","function":{
 "name":"read_region",
 "description":"Material histogram and percent-solid for a box. Returns a summary, never raw voxels. Use to check a site is clear, or to inspect what you just built.",
 "parameters":{"type":"object","properties":{
   "min":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "max":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3}},
  "required":["min","max"]}}},
{"type":"function","function":{
 "name":"density",
 "description":"Fraction of solid cells in a box, 0.0 to 1.0. Cheap; call repeatedly while carving or verifying clearance.",
 "parameters":{"type":"object","properties":{
   "min":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "max":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3}},
  "required":["min","max"]}}},
{"type":"function","function":{
 "name":"set_block",
 "description":"Set one voxel. Prefer fill_box, sphere, cylinder or place_tree for anything larger than a few blocks: fewer calls means less drift.",
 "parameters":{"type":"object","properties":{
   "x":{"type":"integer"},"y":{"type":"integer"},"z":{"type":"integer"},
   "block":{"type":"integer","description":"BlockId numeric value, or use palette_block first"}},
  "required":["x","y","z","block"]}}},
{"type":"function","function":{
 "name":"fill_box",
 "description":"Fill or shell an axis-aligned box. Set hollow=true for walls/rooms. Refuses regions over 200000 blocks; split them.",
 "parameters":{"type":"object","properties":{
   "min":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "max":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "block":{"type":"integer"},"hollow":{"type":"boolean"}},
  "required":["min","max","block"]}}},
{"type":"function","function":{
 "name":"sphere",
 "description":"Solid or hollow sphere. Use for domes, boulders, and cave voids (fill with block=0 to carve air).",
 "parameters":{"type":"object","properties":{
   "centre":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "radius":{"type":"integer"},"block":{"type":"integer"},"hollow":{"type":"boolean"}},
  "required":["centre","radius","block"]}}},
{"type":"function","function":{
 "name":"cylinder",
 "description":"Vertical cylinder from a base point. Towers, pillars, wells, tree trunks that need to be thicker than one block.",
 "parameters":{"type":"object","properties":{
   "base":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "radius":{"type":"integer"},"height":{"type":"integer"},
   "block":{"type":"integer"},"hollow":{"type":"boolean"}},
  "required":["base","radius","height","block"]}}},
{"type":"function","function":{
 "name":"place_tree",
 "description":"A whole tree in one call: trunk with optional lean, plus canopy that will not overwrite existing solid blocks. species: oak, birch, pine, cactus. Call find_surface first and use that Y as the base.",
 "parameters":{"type":"object","properties":{
   "base":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "species":{"type":"string","enum":["oak","birch","pine","cactus"]},
   "height":{"type":"integer"},
   "lean":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2,
           "description":"[dx,dz] offset of the treetop relative to the base; [0,0] is upright"}},
  "required":["base","species","height"]}}},
{"type":"function","function":{
 "name":"scatter",
 "description":"Sprinkle a decoration block on the surface across a footprint, seeded so results are reproducible. Grass, flowers, mushrooms, rubble.",
 "parameters":{"type":"object","properties":{
   "min_xz":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},
   "max_xz":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},
   "from_y":{"type":"integer"},"block":{"type":"integer"},
   "count":{"type":"integer"},"seed":{"type":"integer"}},
  "required":["min_xz","max_xz","from_y","block","count","seed"]}}},
{"type":"function","function":{
 "name":"replace",
 "description":"Swap one material for another inside a box. The restyling workhorse: build in a cheap material, then re-skin.",
 "parameters":{"type":"object","properties":{
   "min":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "max":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "from":{"type":"integer"},"to":{"type":"integer"}},
  "required":["min","max","from","to"]}}},
{"type":"function","function":{
 "name":"palette_block",
 "description":"Resolve a palette role to a numeric block id. Palettes: stone, oak, desert, volcanic, polished, frozen, cavern. Roles: primary, secondary, accent, foliage. Use this instead of memorising block numbers.",
 "parameters":{"type":"object","properties":{
   "palette":{"type":"string"},"role":{"type":"string","enum":["primary","secondary","accent","foliage"]}},
  "required":["palette","role"]}}},
{"type":"function","function":{
 "name":"copy_region",
 "description":"Capture a region for reuse; returns a clip handle and size, never the contents. Use to duplicate a structure you or the user already built.",
 "parameters":{"type":"object","properties":{
   "min":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "max":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3}},
  "required":["min","max"]}}},
{"type":"function","function":{
 "name":"paste_region",
 "description":"Stamp a clip down, optionally rotated (0/90/180/270 about Y) and mirrored. skip_air=true leaves terrain intact where the clip is empty, so a building can be dropped on a hillside without cutting a box out of it.",
 "parameters":{"type":"object","properties":{
   "clip_id":{"type":"integer"},
   "at":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "rotate":{"type":"integer","enum":[0,90,180,270]},
   "mirror_x":{"type":"boolean"},"skip_air":{"type":"boolean"}},
  "required":["clip_id","at"]}}},
{"type":"function","function":{
 "name":"restyle",
 "description":"Re-theme an existing structure by mapping one palette's roles onto another's. This is the main way to change how something looks: you do not need to know any block ids, and you do not need to read the structure first.",
 "parameters":{"type":"object","properties":{
   "min":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "max":{"type":"array","items":{"type":"integer"},"minItems":3,"maxItems":3},
   "from_palette":{"type":"string"},"to_palette":{"type":"string"}},
  "required":["min","max","from_palette","to_palette"]}}},
{"type":"function","function":{
 "name":"connect_path",
 "description":"Lay a path, road, stream or jetty between two points, following the ground rather than cutting a flat ribbon through hills. Use for anything that links two places.",
 "parameters":{"type":"object","properties":{
   "from_xz":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},
   "to_xz":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},
   "width":{"type":"integer"},"block":{"type":"integer"},"from_y":{"type":"integer"}},
  "required":["from_xz","to_xz","width","block","from_y"]}}},
{"type":"function","function":{
 "name":"enclose",
 "description":"Wall or fence around a footprint, following the ground. Campsites, gardens, farm plots, compounds.",
 "parameters":{"type":"object","properties":{
   "min_xz":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},
   "max_xz":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},
   "height":{"type":"integer"},"block":{"type":"integer"},"from_y":{"type":"integer"}},
  "required":["min_xz","max_xz","height","block","from_y"]}}},
{"type":"function","function":{
 "name":"terraform",
 "description":"Raise, lower or smooth the ground across a footprint. Do this BEFORE building on uneven terrain; smooth=true averages each column toward its neighbours and ignores delta.",
 "parameters":{"type":"object","properties":{
   "min_xz":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},
   "max_xz":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},
   "delta":{"type":"integer"},"smooth":{"type":"boolean"},
   "block":{"type":"integer"},"from_y":{"type":"integer"}},
  "required":["min_xz","max_xz","block","from_y"]}}},
{"type":"function","function":{
 "name":"scatter_clustered",
 "description":"Scatter decoration in clumps rather than evenly. Uniform scatter reads as noise; clustered scatter reads as landscaping. Use for bushes, boulders, flowers around a feature.",
 "parameters":{"type":"object","properties":{
   "min_xz":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},
   "max_xz":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},
   "from_y":{"type":"integer"},"block":{"type":"integer"},
   "clusters":{"type":"integer"},"per_cluster":{"type":"integer"},
   "spread":{"type":"integer"},"seed":{"type":"integer"}},
  "required":["min_xz","max_xz","from_y","block","clusters","per_cluster","spread","seed"]}}},
{"type":"function","function":{
 "name":"undo_edit",
 "description":"Reverse one previous call exactly, by its edit_id. Use this to correct a mistake instead of placing more blocks on top of it.",
 "parameters":{"type":"object","properties":{"edit_id":{"type":"integer"}},"required":["edit_id"]}}}
]"#;
    s.to_string()
}

/// Every tool name the schema advertises. The dispatcher must handle
/// all of these; the test below enforces it, because a schema that
/// promises a tool the dispatcher lacks produces a confusing failure
/// several turns later.
pub fn tool_names() -> Vec<&'static str> {
    vec!["find_surface", "probe_column", "read_region", "density", "set_block",
         "fill_box", "sphere", "cylinder", "place_tree", "scatter", "replace",
         "palette_block", "undo_edit",
         // derived from build-narration operation counts
         "copy_region", "paste_region", "restyle", "connect_path", "enclose",
         "terraform", "scatter_clustered"]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat test world: solid stone below y=64, air above.
    struct Flat {
        overrides: BTreeMap<(i32, i32, i32), u16>,
    }
    impl Flat {
        fn new() -> Self { Self { overrides: BTreeMap::new() } }
    }
    impl VoxelWorld for Flat {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.overrides.get(&(x, y, z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
            self.overrides.insert((x, y, z), b);
            true
        }
    }

    #[test]
    fn find_surface_finds_the_ground() {
        let mut w = Flat::new();
        let t = VoxelTools::new(&mut w);
        assert_eq!(t.find_surface(0, 0, 200), Some(64));
    }

    #[test]
    fn tree_sits_on_the_surface_and_leans() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let sy = t.find_surface(10, 10, 200).unwrap();
        let r = t.place_tree((10, sy + 1, 10), "oak", 7, (2, 0));
        assert!(r.ok && r.blocks_changed > 10, "{r:?}");
        // trunk starts on the ground, not floating
        assert_eq!(t.world.get(10, sy + 1, 10), 17);
        // the lean actually moved the top
        let b = r.bounds.unwrap();
        assert!(b.max.0 >= 12, "top should have leaned +x: {b:?}");
    }

    #[test]
    fn canopy_does_not_eat_existing_structures() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        // a wall right where the canopy would go
        t.fill_box((11, 70, 8), (11, 74, 12), 58, false);
        let before = t.world.get(11, 72, 10);
        t.place_tree((10, 65, 10), "oak", 7, (0, 0));
        assert_eq!(t.world.get(11, 72, 10), before,
                   "leaves must not overwrite solid blocks");
    }

    #[test]
    fn undo_restores_exactly() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let sample = [(5, 70, 5), (6, 71, 6), (7, 72, 7)];
        let before: Vec<u16> = sample.iter().map(|&(x, y, z)| t.world.get(x, y, z)).collect();
        let r = t.sphere((6, 71, 6), 4, 37, false);
        assert!(r.blocks_changed > 0);
        let u = t.undo_edit(r.edit_id.unwrap());
        assert!(u.ok);
        let after: Vec<u16> = sample.iter().map(|&(x, y, z)| t.world.get(x, y, z)).collect();
        assert_eq!(before, after, "undo must restore the previous blocks");
    }

    #[test]
    fn oversized_fill_is_refused_not_attempted() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let r = t.fill_box((0, 0, 0), (999, 999, 999), 1, false);
        assert!(!r.ok);
        assert!(r.summary.contains("MAX_BLOCKS_PER_CALL"), "{}", r.summary);
        assert_eq!(r.blocks_changed, 0);
    }

    #[test]
    fn reads_are_summaries_not_dumps() {
        let mut w = Flat::new();
        let t = VoxelTools::new(&mut w);
        let s = t.read_region((0, 60, 0), (31, 67, 31));
        // 8192 cells described in one short line
        assert!(s.len() < 200, "read_region must stay compact: {s}");
        assert!(s.contains("% solid"));
    }

    #[test]
    fn scatter_is_reproducible_for_a_seed() {
        let mut a = Flat::new();
        let ra = { let mut t = VoxelTools::new(&mut a);
                   t.scatter((0, 0), (20, 20), 200, 25, 40, 1337) };
        let mut b = Flat::new();
        let rb = { let mut t = VoxelTools::new(&mut b);
                   t.scatter((0, 0), (20, 20), 200, 25, 40, 1337) };
        assert_eq!(ra.blocks_changed, rb.blocks_changed);
        assert_eq!(a.overrides, b.overrides, "same seed must give the same scene");
    }

    #[test]
    fn schema_and_dispatcher_agree() {
        let schema = tool_schemas();
        for name in tool_names() {
            assert!(schema.contains(&format!("\"name\":\"{name}\"")),
                    "tool {name} missing from schema");
        }
        // and the schema advertises nothing beyond the known set
        let count = schema.matches("\"type\":\"function\"").count();
        assert_eq!(count, tool_names().len(),
                   "schema advertises a different number of tools than tool_names()");
    }

    #[test]
    fn palettes_resolve() {
        let p = palettes();
        assert_eq!(p["oak"].primary, 57);
        assert_eq!(p["volcanic"].secondary, 49);
        assert!(p.contains_key("frozen"));
    }
}

// ---------------------------------------------------------------
// Second primitive set, derived from operation frequency in a corpus
// of build narration (~78 min, 4 videos). The counts told us what a
// builder actually does, which turned out not to match the first
// guess at an API:
//
//   build 165 | add 96 | transform/upgrade 84 | place 37 | connect 17
//   match/theme 26 | surround 10 | scatter 9 | terraform 7 | paste 6
//
// The dominant operation is TRANSFORM: take a structure that already
// exists and restyle it in place. The first primitive set could only
// create from nothing, which is the wrong shape for most of the work.
// `restyle`, `copy_region`/`paste_region`, `connect_path`, `enclose`
// and `terraform` close that gap.
// ---------------------------------------------------------------

/// A captured region: dimensions plus flat block data. Small enough
/// to hold several in a session; the model refers to one by handle,
/// never by contents, so a clipboard never enters the context window.
#[derive(Debug, Clone)]
pub struct Clip {
    pub id: u64,
    pub w: i32,
    pub h: i32,
    pub d: i32,
    pub blocks: Vec<u16>,
}

impl Clip {
    fn at(&self, x: i32, y: i32, z: i32) -> u16 {
        self.blocks[((y * self.d + z) * self.w + x) as usize]
    }
}

impl<'w, W: VoxelWorld> VoxelTools<'w, W> {
    /// Capture a region for reuse. Returns a handle and a size, not
    /// the contents: "paste in all the villages" means the model
    /// should move a structure around without ever seeing its voxels.
    pub fn copy_region(&mut self, min: (i32, i32, i32), max: (i32, i32, i32)) -> (Option<u64>, String) {
        let (lo, hi) = norm(min, max);
        let vol = volume(lo, hi);
        if vol > MAX_BLOCKS_PER_CALL {
            return (None, format!("refused: {vol} blocks exceeds the per-call cap"));
        }
        let (w, h, d) = (hi.0 - lo.0 + 1, hi.1 - lo.1 + 1, hi.2 - lo.2 + 1);
        let mut blocks = Vec::with_capacity(vol);
        for y in lo.1..=hi.1 {
            for z in lo.2..=hi.2 {
                for x in lo.0..=hi.0 {
                    blocks.push(self.world.get(x, y, z));
                }
            }
        }
        let id = self.next_clip;
        self.next_clip += 1;
        self.clips.push(Clip { id, w, h, d, blocks });
        (Some(id), format!("clip {id}: {w}x{h}x{d}"))
    }

    /// Stamp a clip down. `rotate` is 0/90/180/270 about Y, `mirror_x`
    /// flips it. `skip_air` leaves the destination untouched where the
    /// clip is empty, so a structure can be dropped onto terrain
    /// without cutting a box out of it.
    pub fn paste_region(&mut self, clip_id: u64, at: (i32, i32, i32),
                        rotate: i32, mirror_x: bool, skip_air: bool) -> CallResult {
        let Some(clip) = self.clips.iter().find(|c| c.id == clip_id).cloned() else {
            return CallResult::err(format!("no clip {clip_id}"));
        };
        let rot = ((rotate % 360) + 360) % 360;
        if rot % 90 != 0 {
            return CallResult::err("rotate must be 0, 90, 180 or 270");
        }
        let mut e = self.begin("paste_region");
        for y in 0..clip.h {
            for z in 0..clip.d {
                for x in 0..clip.w {
                    let sx = if mirror_x { clip.w - 1 - x } else { x };
                    let b = clip.at(sx, y, z);
                    if skip_air && b == 0 { continue; }
                    let (ox, oz) = match rot {
                        90  => (clip.d - 1 - z, x),
                        180 => (clip.w - 1 - x, clip.d - 1 - z),
                        270 => (z, clip.w - 1 - x),
                        _   => (x, z),
                    };
                    self.write(&mut e, at.0 + ox, at.1 + y, at.2 + oz, b);
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("pasted clip {clip_id} rot={rot} mirror={mirror_x}, {n} blocks"))
    }

    /// THE dominant operation: restyle an existing structure by
    /// mapping one palette's roles onto another's. Unlike `replace`,
    /// which needs the model to know both block ids, this works from
    /// palette names — so "make the outpost look volcanic" is one
    /// call and does not require reading the structure first.
    pub fn restyle(&mut self, min: (i32, i32, i32), max: (i32, i32, i32),
                   from_pal: &str, to_pal: &str) -> CallResult {
        let pals = palettes();
        let (Some(a), Some(b)) = (pals.get(from_pal), pals.get(to_pal)) else {
            return CallResult::err(format!(
                "unknown palette; have: {}",
                pals.keys().cloned().collect::<Vec<_>>().join(", ")));
        };
        let map = [(a.primary, b.primary), (a.secondary, b.secondary),
                   (a.accent, b.accent), (a.foliage, b.foliage)];
        let (lo, hi) = norm(min, max);
        if volume(lo, hi) > MAX_BLOCKS_PER_CALL {
            return CallResult::err("refused: region too large; split it");
        }
        let mut e = self.begin("restyle");
        for y in lo.1..=hi.1 {
            for z in lo.2..=hi.2 {
                for x in lo.0..=hi.0 {
                    let cur = self.world.get(x, y, z);
                    if let Some(&(_, to)) = map.iter().find(|(f, _)| *f == cur) {
                        self.write(&mut e, x, y, z, to);
                    }
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("restyled {n} blocks {from_pal} -> {to_pal}"))
    }

    /// Lay a path between two points, following the surface. Paths,
    /// roads, streams and jetties are all this call; the corpus talks
    /// about connecting things far more than about placing individual
    /// path blocks.
    pub fn connect_path(&mut self, from_xz: (i32, i32), to_xz: (i32, i32),
                        width: i32, block: u16, from_y: i32) -> CallResult {
        let w = width.clamp(1, 16);
        let (dx, dz) = (to_xz.0 - from_xz.0, to_xz.1 - from_xz.1);
        let steps = dx.abs().max(dz.abs()).max(1);
        if steps > 4096 {
            return CallResult::err("refused: path longer than 4096 blocks");
        }
        let mut e = self.begin("connect_path");
        let half = w / 2;
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let cx = from_xz.0 + (dx as f64 * t).round() as i32;
            let cz = from_xz.1 + (dz as f64 * t).round() as i32;
            for ox in -half..=half {
                for oz in -half..=half {
                    if ox * ox + oz * oz > half * half + half { continue; }
                    let (x, z) = (cx + ox, cz + oz);
                    // follow the ground: replace the surface block
                    // rather than laying a flat ribbon through hills
                    if let Some(sy) = self.find_surface(x, z, from_y) {
                        self.write(&mut e, x, sy, z, block);
                    }
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("path {:?} -> {:?} width {w}, {n} blocks", from_xz, to_xz))
    }

    /// Wall or fence around a footprint, following the ground. The
    /// corpus's "surround the campsite" / "wall off a section".
    pub fn enclose(&mut self, min_xz: (i32, i32), max_xz: (i32, i32),
                   height: i32, block: u16, from_y: i32) -> CallResult {
        let (x0, x1) = (min_xz.0.min(max_xz.0), min_xz.0.max(max_xz.0));
        let (z0, z1) = (min_xz.1.min(max_xz.1), min_xz.1.max(max_xz.1));
        let h = height.clamp(1, 32);
        let mut e = self.begin("enclose");
        for z in z0..=z1 {
            for x in x0..=x1 {
                if x != x0 && x != x1 && z != z0 && z != z1 { continue; }
                if let Some(sy) = self.find_surface(x, z, from_y) {
                    for k in 1..=h {
                        self.write(&mut e, x, sy + k, z, block);
                    }
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("enclosed {}x{} at height {h}, {n} blocks",
                               x1 - x0 + 1, z1 - z0 + 1))
    }

    /// Raise, lower or smooth terrain in a footprint. `delta` shifts
    /// the surface; `smooth` averages each column toward its
    /// neighbours instead. Terraforming precedes almost every build
    /// in the corpus and had no primitive at all.
    pub fn terraform(&mut self, min_xz: (i32, i32), max_xz: (i32, i32),
                     delta: i32, smooth: bool, block: u16, from_y: i32) -> CallResult {
        let (x0, x1) = (min_xz.0.min(max_xz.0), min_xz.0.max(max_xz.0));
        let (z0, z1) = (min_xz.1.min(max_xz.1), min_xz.1.max(max_xz.1));
        let area = ((x1 - x0 + 1) as usize) * ((z1 - z0 + 1) as usize);
        if area > 65_536 {
            return CallResult::err("refused: footprint over 65536 columns");
        }
        // read all heights first: editing during the scan would make
        // smoothing depend on iteration order
        let mut hmap = vec![0i32; area];
        let wdt = (x1 - x0 + 1) as usize;
        for z in z0..=z1 {
            for x in x0..=x1 {
                let i = ((z - z0) as usize) * wdt + (x - x0) as usize;
                hmap[i] = self.find_surface(x, z, from_y).unwrap_or(from_y);
            }
        }
        let mut target = hmap.clone();
        if smooth {
            for zi in 0..(z1 - z0 + 1) as usize {
                for xi in 0..wdt {
                    let mut sum = 0i32;
                    let mut n = 0i32;
                    for dz in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let (nx, nz) = (xi as i32 + dx, zi as i32 + dz);
                            if nx < 0 || nz < 0 || nx >= wdt as i32
                                || nz >= (z1 - z0 + 1) { continue; }
                            sum += hmap[nz as usize * wdt + nx as usize];
                            n += 1;
                        }
                    }
                    target[zi * wdt + xi] = sum / n.max(1);
                }
            }
        } else {
            for t in target.iter_mut() { *t += delta; }
        }
        let mut e = self.begin("terraform");
        for z in z0..=z1 {
            for x in x0..=x1 {
                let i = ((z - z0) as usize) * wdt + (x - x0) as usize;
                let (cur, want) = (hmap[i], target[i]);
                if want > cur {
                    for y in cur + 1..=want { self.write(&mut e, x, y, z, block); }
                } else if want < cur {
                    for y in want + 1..=cur { self.write(&mut e, x, y, z, 0); }
                }
            }
        }
        let n = e.before.len();
        self.finish(e, format!("terraformed {area} columns ({}), {n} blocks",
                               if smooth { "smoothed".to_string() } else { format!("{delta:+}") }))
    }

    /// Scatter in clumps rather than uniformly. The corpus places
    /// bushes and boulders in clusters around trees; uniform scatter
    /// reads as noise, clustered scatter reads as landscaping.
    pub fn scatter_clustered(&mut self, min_xz: (i32, i32), max_xz: (i32, i32), from_y: i32,
                             block: u16, clusters: usize, per_cluster: usize,
                             spread: i32, seed: u64) -> CallResult {
        let (x0, x1) = (min_xz.0.min(max_xz.0), min_xz.0.max(max_xz.0));
        let (z0, z1) = (min_xz.1.min(max_xz.1), min_xz.1.max(max_xz.1));
        let (w, d) = ((x1 - x0 + 1) as u64, (z1 - z0 + 1) as u64);
        if w == 0 || d == 0 { return CallResult::err("empty footprint"); }
        let mut s = seed | 1;
        let mut next = move || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 33) as i64
        };
        let mut e = self.begin("scatter_clustered");
        let mut placed = 0usize;
        for _ in 0..clusters {
            let cx = x0 + (next() % w as i64) as i32;
            let cz = z0 + (next() % d as i64) as i32;
            for _ in 0..per_cluster {
                let ox = (next() % (2 * spread as i64 + 1)) as i32 - spread;
                let oz = (next() % (2 * spread as i64 + 1)) as i32 - spread;
                let (x, z) = (cx + ox, cz + oz);
                if x < x0 || x > x1 || z < z0 || z > z1 { continue; }
                if let Some(sy) = self.find_surface(x, z, from_y) {
                    if self.world.get(x, sy + 1, z) == 0
                        && self.write(&mut e, x, sy + 1, z, block) { placed += 1; }
                }
            }
        }
        self.finish(e, format!("{placed} blocks in {clusters} clusters"))
    }

}   // impl VoxelTools (corpus-derived primitives)

#[cfg(test)]
mod derived_tests {
    //! Tests for the primitives derived from build-narration operation
    //! counts. Separate module so the fixture can differ from the one
    //! used by the first primitive set.
    use super::*;
    use std::collections::BTreeMap;

    struct Flat { cells: BTreeMap<(i32, i32, i32), u16> }
    impl Flat { fn new() -> Self { Self { cells: BTreeMap::new() } } }
    impl VoxelWorld for Flat {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
            self.cells.insert((x, y, z), b); true
        }
    }

    #[test]
    fn copy_paste_reproduces_a_structure_elsewhere() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        t.fill_box((0, 65, 0), (2, 68, 2), 58, false);
        let (id, msg) = t.copy_region((0, 65, 0), (2, 68, 2));
        let id = id.expect(&msg);
        let r = t.paste_region(id, (40, 65, 40), 0, false, true);
        assert!(r.ok && r.blocks_changed > 0, "{r:?}");
        assert_eq!(t.world.get(41, 66, 41), 58, "pasted copy must match");
    }

    #[test]
    fn paste_rotation_reorients_content() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        t.fill_box((0, 70, 0), (3, 70, 0), 58, false);   // long arm along +x
        let (id, _) = t.copy_region((0, 70, 0), (3, 70, 1));
        t.paste_region(id.unwrap(), (50, 70, 50), 90, false, true);
        let along_z = (0..4).filter(|&k| t.world.get(51, 70, 50 + k) == 58).count();
        assert!(along_z >= 3, "rotation should reorient the arm, got {along_z}");
    }

    #[test]
    fn paste_skip_air_leaves_terrain_intact() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        t.set_block(0, 70, 0, 58);
        let (id, _) = t.copy_region((0, 70, 0), (2, 72, 2));
        let before = t.world.get(31, 64, 31);
        t.paste_region(id.unwrap(), (30, 63, 30), 0, false, true);
        assert_eq!(t.world.get(31, 64, 31), before, "skip_air must not carve terrain");
    }

    #[test]
    fn restyle_maps_palette_roles() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let pals = palettes();
        let oak_primary = pals["oak"].primary;
        t.fill_box((0, 70, 0), (3, 72, 3), oak_primary, false);
        let r = t.restyle((0, 70, 0), (3, 72, 3), "oak", "volcanic");
        assert!(r.ok && r.blocks_changed > 0, "{r:?}");
        assert_eq!(t.world.get(1, 71, 1), pals["volcanic"].primary);
    }

    #[test]
    fn restyle_lists_valid_palettes_on_error() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let r = t.restyle((0, 70, 0), (1, 71, 1), "oak", "nonsense");
        assert!(!r.ok);
        assert!(r.summary.contains("volcanic"), "should list options: {}", r.summary);
    }

    #[test]
    fn path_lies_on_the_surface_and_spans_both_ends() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let r = t.connect_path((0, 0), (20, 0), 3, 4, 200);
        assert!(r.ok && r.blocks_changed > 40, "{r:?}");
        assert_eq!(t.world.get(10, 64, 0), 4, "laid on the surface");
        assert_eq!(t.world.get(0, 64, 0), 4, "start covered");
        assert_eq!(t.world.get(20, 64, 0), 4, "end covered");
    }

    #[test]
    fn enclose_builds_walls_but_not_a_floor() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        assert!(t.enclose((0, 0), (6, 6), 3, 58, 200).ok);
        assert_eq!(t.world.get(0, 65, 0), 58, "corner");
        assert_eq!(t.world.get(3, 65, 0), 58, "edge");
        assert_eq!(t.world.get(3, 65, 3), 0, "interior stays open");
    }

    #[test]
    fn terraform_raises_and_lowers() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        assert!(t.terraform((0, 0), (5, 5), 3, false, 3, 200).ok);
        assert_eq!(t.find_surface(2, 2, 200), Some(67), "surface rises by 3");
        assert!(t.terraform((0, 0), (5, 5), -2, false, 3, 200).ok);
        assert_eq!(t.find_surface(2, 2, 200), Some(65), "and falls by 2");
    }

    #[test]
    fn terraform_smooth_cuts_a_spike() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        t.fill_box((5, 65, 5), (5, 70, 5), 1, false);
        assert_eq!(t.find_surface(5, 5, 200), Some(70));
        assert!(t.terraform((0, 0), (10, 10), 0, true, 1, 200).ok);
        let after = t.find_surface(5, 5, 200).unwrap();
        assert!(after < 70, "smoothing should cut the spike, got {after}");
    }

    #[test]
    fn clustered_scatter_clumps_rather_than_spreading() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let r = t.scatter_clustered((0, 0), (60, 60), 200, 25, 3, 12, 3, 99);
        assert!(r.ok && r.blocks_changed > 5, "{r:?}");
        let b = r.bounds.unwrap();
        let span = (b.max.0 - b.min.0).max(b.max.2 - b.min.2);
        assert!(span < 55, "clusters should not fill the footprint, span {span}");
    }

    #[test]
    fn every_advertised_tool_is_in_the_schema() {
        let schema = tool_schemas();
        for name in tool_names() {
            assert!(schema.contains(&format!("\"name\":\"{name}\"")),
                    "tool {name} missing from schema");
        }
        assert_eq!(schema.matches("\"type\":\"function\"").count(), tool_names().len());
    }
}
