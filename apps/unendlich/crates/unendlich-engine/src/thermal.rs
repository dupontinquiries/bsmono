//! Thermal dynamic-material field — the second cullable field after wetness, and
//! the one that drives the most interactions: heat-glow lighting, melting/burning
//! transitions, and thermal-shock shatter. Same shape as [`crate::wetness`]: a
//! sparse field (dry/ambient world is free), a deterministic snapshot-based step
//! (sources inject, heat spreads scaled by the neighbour's conductivity, then
//! everything cools), and a `Material` combine. The transitions it implies
//! (block -> air on shatter, ice -> water on melt, water+lava -> stone) are octree
//! edits the world layer applies from the events this module produces.
//!
//! Heat here is "degrees above ambient", `0..=255`. Lava cells are re-injected to
//! [`MOLTEN`] each step; cooling pulls every other cell back toward ambient, so
//! the field self-prunes and stays sparse.

use crate::material::Material;
use crate::world::blocks::BlockId;
use crate::world::chunk::ChunkCoord;
use std::collections::HashMap;

/// Heat at the environment baseline (no glow, no reaction).
pub const AMBIENT: u8 = 0;
/// Lava-hot — the source temperature.
pub const MOLTEN: u8 = 255;
/// Heat at/above which a surface starts to glow (feeds emissive lighting).
pub const GLOW_THRESHOLD: u8 = 120;

/// Cell coordinate in chunk-independent block space (matches wetness).
pub type Cell = (i32, i32, i32);

const NEIGHBORS: [(i32, i32, i32); 6] =
    [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)];

/// What a block does when pushed past its thermal limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Inert — never fails thermally (stone-cold metals shrug, lava is already molten).
    None,
    /// Brittle fracture (glass, obsidian, crystal): block -> air + shards.
    Shatter,
    /// Stone cracking: weakens / spalls under a steep gradient only.
    Crack,
    /// Combustion (wood, foliage): block -> fire/ash, resisted by wetness.
    Burn,
    /// Phase change (ice -> water, metal -> molten).
    Melt,
}

/// Per-block thermal behaviour.
#[derive(Debug, Clone, Copy)]
pub struct ThermalResponse {
    /// How readily heat passes *into* this block, `0..=255`. Insulators (foliage,
    /// glass) are low; metals and ice conduct; air carries a moderate amount.
    pub conductivity: u8,
    /// Absolute heat at/above which `failure` triggers. `0` = never by heat alone.
    pub reaction_temp: u8,
    /// Spatial heat gradient (or quench rate) that triggers `failure` even below
    /// `reaction_temp` — thermal shock. `255` = immune (ductile / not brittle).
    pub shock_gradient: u8,
    /// The characteristic failure mode.
    pub failure: Failure,
}

#[derive(Clone, Copy)]
enum Class {
    Lava,
    Ice,
    Glassy,
    Metal,
    Organic,
    Mineral,
    Inert,
}

fn class(b: BlockId) -> Class {
    use BlockId::*;
    match b {
        Lava => Class::Lava,
        Ice | Snow | SnowBlock => Class::Ice,
        Glass | Obsidian | Amber | Crystal | Quartz => Class::Glassy,
        Air | Water | Cobweb => Class::Inert,
        IronBlock | GoldBlock => Class::Metal,
        Grass | OakLog | OakLeaves | BirchLog | BirchLeaves | Cactus | Moss | Flower
        | TallGrass | Fern | DeadBush | Vine | Mushroom | Lichen | OakPlanks | Bookshelf
        | Pumpkin | DarkOakPlanks | Carrot | Potato | Cabbage | Sprout => Class::Organic,
        Stone | Dirt | Cobblestone | Bedrock | Sand | Gravel | GoldOre | IronOre | CoalOre
        | Clay | RedSandstone | Dripstone | Limestone | Marble | Basalt | Tuff | Granite
        | Slate | Calcite | Andesite | Schist | Bone | Sandstone | CopperOre | DiamondOre
        | StoneBricks | Bricks | CoalBlock | MossyCobblestone | PolishedStone | PolishedBasalt | ChiseledStoneBricks | SmoothSandstone | Terracotta => Class::Mineral,
    }
}

/// Thermal response for a block. Exhaustive via [`class`] — a new block must be
/// classified, the compiler enforces it.
pub fn thermal_response(b: BlockId) -> ThermalResponse {
    let (conductivity, reaction_temp, shock_gradient, failure) = match class(b) {
        Class::Lava => (255, 0, 255, Failure::None),
        Class::Ice => (200, 35, 60, Failure::Melt), // melts easily; brittle to shock
        Class::Glassy => (40, 230, 70, Failure::Shatter), // insulating but very brittle
        Class::Metal => (255, 235, 255, Failure::Melt), // conducts hard, ductile (no shock)
        Class::Organic => (50, 150, 255, Failure::Burn), // insulating; burns; doesn't shatter
        Class::Mineral => (130, 250, 190, Failure::Crack), // cracks only on a steep gradient
        Class::Inert => (80, 0, 255, Failure::None),
    };
    ThermalResponse { conductivity, reaction_temp, shock_gradient, failure }
}

/// Tunables for [`HeatField::step`] and [`thermal_shock`].
#[derive(Debug, Clone, Copy)]
pub struct ThermalParams {
    /// Flat heat lost crossing one cell boundary before conductivity scaling.
    pub spread_falloff: u8,
    /// Heat lost per step by every non-source cell (cooling toward ambient).
    pub cooling: u8,
    /// Drop in heat between ticks (dT/dt) that counts as a quench for shock.
    pub quench_rate: u8,
}

impl Default for ThermalParams {
    fn default() -> Self {
        Self { spread_falloff: 20, cooling: 8, quench_rate: 120 }
    }
}

/// Sparse heat field (heat above ambient). Like [`crate::wetness::WetnessField`]:
/// only non-ambient cells are stored, so a cold world costs nothing.
#[derive(Debug, Default, Clone)]
pub struct HeatField {
    cells: HashMap<Cell, u8>,
}

impl HeatField {
    pub fn new() -> Self {
        Self { cells: HashMap::new() }
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Heat at a cell, `AMBIENT` if not stored.
    pub fn get(&self, c: Cell) -> u8 {
        self.cells.get(&c).copied().unwrap_or(AMBIENT)
    }

    pub fn set(&mut self, c: Cell, h: u8) {
        if h == AMBIENT {
            self.cells.remove(&c);
        } else {
            self.cells.insert(c, h);
        }
    }

    pub fn active_cells(&self) -> impl Iterator<Item = (&Cell, &u8)> {
        self.cells.iter()
    }

    /// Cull cells outside the active region (matches wetness).
    pub fn retain_within(&mut self, min: Cell, max: Cell) {
        self.cells.retain(|&(x, y, z), _| {
            x >= min.0 && x <= max.0 && y >= min.1 && y <= max.1 && z >= min.2 && z <= max.2
        });
    }

    /// Advance one tick. `sources` (lava-touching cells) are re-injected to
    /// [`MOLTEN`]; heat spreads to neighbours scaled by the *neighbour's*
    /// conductivity; then every non-source cell cools. Deterministic: spread reads
    /// a snapshot and writes the max, so the result is order-independent.
    pub fn step(
        &mut self,
        params: &ThermalParams,
        sources: &[Cell],
        block_at: impl Fn(Cell) -> BlockId,
    ) {
        let mut snap = self.cells.clone();
        for &s in sources {
            snap.insert(s, MOLTEN);
        }

        let mut next = snap.clone();
        for (&cell, &h) in &snap {
            let offer = h.saturating_sub(params.spread_falloff);
            if offer == 0 {
                continue;
            }
            for d in NEIGHBORS {
                let n = (cell.0 + d.0, cell.1 + d.1, cell.2 + d.2);
                let cond = thermal_response(block_at(n)).conductivity as u16;
                let give = ((offer as u16 * cond) / 255) as u8;
                if give == 0 {
                    continue;
                }
                let slot = next.entry(n).or_insert(0);
                *slot = (*slot).max(give);
            }
        }

        if params.cooling > 0 {
            let src: std::collections::HashSet<Cell> = sources.iter().copied().collect();
            for (cell, h) in next.iter_mut() {
                if !src.contains(cell) {
                    *h = h.saturating_sub(params.cooling);
                }
            }
        }

        next.retain(|_, &mut h| h > AMBIENT);
        self.cells = next;
    }
}

/// Apply heat to a block's static material: a hot surface glows. Emission ramps
/// from dim red toward orange/yellow as heat climbs past [`GLOW_THRESHOLD`]
/// (blackbody-ish: red first, then green, blue last), feeding the colored light
/// flood-fill so molten/hot blocks light their surroundings. Roughness drops a
/// little when near-molten (surfaces slick over). Below the threshold the
/// material is returned unchanged.
pub fn apply_heat(base: Material, heat: u8) -> Material {
    if heat < GLOW_THRESHOLD {
        return base;
    }
    let span = (255 - GLOW_THRESHOLD) as f32;
    let t = ((heat - GLOW_THRESHOLD) as f32 / span).clamp(0.0, 1.0); // 0..1
    // Blackbody-ish ramp in 0..=15 light units.
    let r = (4.0 + t * 11.0).round().min(15.0) as u8; // 4..15
    let g = (t * t * 11.0).round().min(15.0) as u8; // 0..~11, lags red
    let b = (t * t * t * 5.0).round().min(15.0) as u8; // 0..5, lags most
    let mut m = base;
    m.emission = [r.max(base.emission[0]), g.max(base.emission[1]), b.max(base.emission[2])];
    m.roughness = (base.roughness * (1.0 - 0.3 * t)).clamp(0.0, 1.0);
    m
}

/// A block that has failed thermally — the world layer turns this into an octree
/// edit (e.g. `Shatter`/`Burn`/`Melt` -> the cell becomes air/water/fire + drops).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShockEvent {
    pub cell: Cell,
    pub failure: Failure,
}

/// Find blocks that fail this tick from heat, gradient, or quench. A cell fails
/// when its block has a non-`None` failure and either: absolute heat >=
/// `reaction_temp`; or it's brittle (`shock_gradient < 255`) and the steepest
/// heat difference to a neighbour, or the drop since `prev`, crosses the
/// threshold. Pure and deterministic; events are sorted for stable output.
pub fn thermal_shock(
    field: &HeatField,
    prev: Option<&HeatField>,
    block_at: impl Fn(Cell) -> BlockId,
    params: &ThermalParams,
) -> Vec<ShockEvent> {
    let mut out = Vec::new();
    for (&cell, &h) in field.active_cells() {
        let resp = thermal_response(block_at(cell));
        if resp.failure == Failure::None {
            continue;
        }
        let hot = resp.reaction_temp > 0 && h >= resp.reaction_temp;
        let brittle = resp.shock_gradient < 255;
        let mut shocked = false;
        if brittle {
            // Steepest gradient to any neighbour.
            let mut max_grad = 0u8;
            for d in NEIGHBORS {
                let n = (cell.0 + d.0, cell.1 + d.1, cell.2 + d.2);
                let grad = h.abs_diff(field.get(n));
                max_grad = max_grad.max(grad);
            }
            if max_grad >= resp.shock_gradient {
                shocked = true;
            }
            // Quench: a large drop since last tick stresses brittle blocks.
            if let Some(p) = prev {
                if p.get(cell).saturating_sub(h) >= params.quench_rate {
                    shocked = true;
                }
            }
        }
        if hot || shocked {
            out.push(ShockEvent { cell, failure: resp.failure });
        }
    }
    out.sort_by_key(|e| e.cell);
    out
}

/// What a failed cell turns into, or `None` for no block change. Brittle and
/// burning blocks are removed (air); melting is a phase change (ice -> water,
/// metal -> molten lava); cracking spalls a mineral one step toward rubble.
pub fn failure_transition(block: BlockId, failure: Failure) -> Option<BlockId> {
    use BlockId::*;
    match failure {
        Failure::None => None,
        Failure::Shatter | Failure::Burn => Some(Air),
        Failure::Melt => Some(match block {
            Ice | Snow | SnowBlock => Water,
            IronBlock | GoldBlock => Lava,
            _ => Water,
        }),
        Failure::Crack => Some(crack_degrade(block)),
    }
}

/// One step of mineral spalling under thermal cracking.
fn crack_degrade(block: BlockId) -> BlockId {
    use BlockId::*;
    match block {
        Stone => Cobblestone,
        Cobblestone => Gravel,
        StoneBricks | Bricks => Cobblestone,
        Sandstone | RedSandstone => Sand,
        _ => Gravel,
    }
}

/// A single block change produced by thermal failure — the world layer applies
/// it via `OctreeArray::set_block`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEdit {
    pub cell: Cell,
    pub new_block: BlockId,
}

/// The result of resolving a tick's shock events: the block edits to apply and
/// the chunks that must be re-meshed afterward.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ThermalOutcome {
    pub edits: Vec<BlockEdit>,
    /// Chunks needing a remesh, deduped and sorted. Includes neighbour chunks
    /// when an edited cell sits on a chunk boundary (cross-chunk face culling
    /// means the neighbour's mesh changes too).
    pub dirty_chunks: Vec<ChunkCoord>,
}

/// Close the thermal loop: turn shock events into block edits + the remesh set.
/// Pure — reads the current block per cell, computes its transition, and
/// collects the dirtied chunks. Events whose transition is a no-op (inert, or
/// the cell is already the target block) produce nothing.
pub fn resolve_thermal_failures(
    events: &[ShockEvent],
    block_at: impl Fn(Cell) -> BlockId,
) -> ThermalOutcome {
    let mut edits = Vec::new();
    let mut dirty = std::collections::BTreeSet::<(i32, i32, i32)>::new();
    for ev in events {
        let cur = block_at(ev.cell);
        let Some(new_block) = failure_transition(cur, ev.failure) else { continue };
        if new_block == cur {
            continue;
        }
        edits.push(BlockEdit { cell: ev.cell, new_block });
        // This cell's chunk, plus any neighbour chunk it borders.
        let here = ChunkCoord::from_block_pos(ev.cell.0, ev.cell.1, ev.cell.2);
        dirty.insert((here.x, here.y, here.z));
        for d in NEIGHBORS {
            let n = (ev.cell.0 + d.0, ev.cell.1 + d.1, ev.cell.2 + d.2);
            let c = ChunkCoord::from_block_pos(n.0, n.1, n.2);
            dirty.insert((c.x, c.y, c.z));
        }
    }
    let dirty_chunks = dirty.into_iter().map(|(x, y, z)| ChunkCoord { x, y, z }).collect();
    ThermalOutcome { edits, dirty_chunks }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::material_for;

    fn solid(_c: Cell) -> BlockId {
        BlockId::Stone
    }

    #[test]
    fn shatter_and_burn_clear_the_block_and_dirty_its_chunk() {
        let ev = [ShockEvent { cell: (5, 5, 5), failure: Failure::Shatter }];
        let out = resolve_thermal_failures(&ev, |_| BlockId::Glass);
        assert_eq!(out.edits, vec![BlockEdit { cell: (5, 5, 5), new_block: BlockId::Air }]);
        assert!(out.dirty_chunks.contains(&ChunkCoord { x: 0, y: 0, z: 0 }));
        // Interior cell: only its own chunk is dirty.
        assert_eq!(out.dirty_chunks.len(), 1);

        let burn = resolve_thermal_failures(
            &[ShockEvent { cell: (5, 5, 5), failure: Failure::Burn }],
            |_| BlockId::OakLog,
        );
        assert_eq!(burn.edits[0].new_block, BlockId::Air);
    }

    #[test]
    fn melt_turns_ice_to_water_and_metal_to_lava() {
        let ice = resolve_thermal_failures(
            &[ShockEvent { cell: (0, 0, 0), failure: Failure::Melt }],
            |_| BlockId::Ice,
        );
        assert_eq!(ice.edits[0].new_block, BlockId::Water);
        let metal = resolve_thermal_failures(
            &[ShockEvent { cell: (0, 0, 0), failure: Failure::Melt }],
            |_| BlockId::IronBlock,
        );
        assert_eq!(metal.edits[0].new_block, BlockId::Lava);
    }

    #[test]
    fn crack_spalls_stone_one_step_toward_rubble() {
        let s = resolve_thermal_failures(
            &[ShockEvent { cell: (0, 0, 0), failure: Failure::Crack }],
            |_| BlockId::Stone,
        );
        assert_eq!(s.edits[0].new_block, BlockId::Cobblestone);
        let c = resolve_thermal_failures(
            &[ShockEvent { cell: (0, 0, 0), failure: Failure::Crack }],
            |_| BlockId::Cobblestone,
        );
        assert_eq!(c.edits[0].new_block, BlockId::Gravel);
    }

    #[test]
    fn boundary_edit_dirties_the_neighbour_chunk() {
        // x=31 is the last cell of chunk 0; its +X neighbour is in chunk 1.
        let out = resolve_thermal_failures(
            &[ShockEvent { cell: (31, 5, 5), failure: Failure::Shatter }],
            |_| BlockId::Glass,
        );
        assert!(out.dirty_chunks.contains(&ChunkCoord { x: 0, y: 0, z: 0 }));
        assert!(out.dirty_chunks.contains(&ChunkCoord { x: 1, y: 0, z: 0 }));
    }

    #[test]
    fn inert_or_noop_events_produce_no_edits() {
        // Failure::None never edits.
        let none = resolve_thermal_failures(&[ShockEvent { cell: (0, 0, 0), failure: Failure::None }], solid);
        assert!(none.edits.is_empty() && none.dirty_chunks.is_empty());
        // Shatter on already-air is a no-op (target == current).
        let air = resolve_thermal_failures(
            &[ShockEvent { cell: (0, 0, 0), failure: Failure::Shatter }],
            |_| BlockId::Air,
        );
        assert!(air.edits.is_empty());
    }

    #[test]
    fn full_loop_heat_to_shock_to_edit() {
        // A hot glass cell beside cold neighbours -> shock detects Shatter ->
        // resolve turns it into air + dirties the chunk. The whole heat->remesh
        // chain in one pure pass.
        let p = ThermalParams::default();
        let mut f = HeatField::new();
        f.set((8, 8, 8), 220);
        let events = thermal_shock(&f, None, |_| BlockId::Glass, &p);
        assert!(!events.is_empty(), "steep gradient should shock the glass");
        let out = resolve_thermal_failures(&events, |_| BlockId::Glass);
        assert_eq!(out.edits[0].new_block, BlockId::Air);
        assert!(out.dirty_chunks.contains(&ChunkCoord { x: 0, y: 0, z: 0 }));
    }


    #[test]
    fn heat_spreads_from_a_source_then_cools_away() {
        let mut f = HeatField::new();
        let p = ThermalParams::default();
        let src = [(0, 0, 0)];
        // A few steps with a source: neighbours should warm up.
        for _ in 0..3 {
            f.step(&p, &src, solid);
        }
        assert_eq!(f.get((0, 0, 0)), MOLTEN, "source stays molten");
        assert!(f.get((1, 0, 0)) > 0, "neighbour warmed, got {}", f.get((1, 0, 0)));
        assert!(f.get((2, 0, 0)) < f.get((1, 0, 0)), "heat falls off with distance");

        // Remove the source: the field must cool back to ambient and self-prune.
        for _ in 0..40 {
            f.step(&p, &[], solid);
        }
        assert!(f.is_empty(), "field cooled to ambient and pruned, len {}", f.len());
    }

    #[test]
    fn step_is_order_independent() {
        let p = ThermalParams::default();
        let mut a = HeatField::new();
        let mut b = HeatField::new();
        for c in [(0, 0, 0), (5, 1, 0), (2, 2, 2)] {
            a.set(c, 200);
            b.set(c, 200);
        }
        a.step(&p, &[(0, 0, 0)], solid);
        b.step(&p, &[(0, 0, 0)], solid);
        // Same inputs -> identical field regardless of internal map order.
        let mut av: Vec<_> = a.active_cells().map(|(c, h)| (*c, *h)).collect();
        let mut bv: Vec<_> = b.active_cells().map(|(c, h)| (*c, *h)).collect();
        av.sort();
        bv.sort();
        assert_eq!(av, bv);
    }

    #[test]
    fn conductivity_carries_heat_further_through_metal_than_foliage() {
        let p = ThermalParams::default();
        let metal = |_c: Cell| BlockId::IronBlock; // conductivity 255
        let leaves = |_c: Cell| BlockId::OakLeaves; // conductivity 50
        let mut fm = HeatField::new();
        let mut fl = HeatField::new();
        fm.set((0, 0, 0), 250);
        fl.set((0, 0, 0), 250);
        fm.step(&p, &[(0, 0, 0)], metal);
        fl.step(&p, &[(0, 0, 0)], leaves);
        assert!(
            fm.get((1, 0, 0)) > fl.get((1, 0, 0)),
            "metal {} should out-conduct foliage {}",
            fm.get((1, 0, 0)),
            fl.get((1, 0, 0))
        );
    }

    #[test]
    fn apply_heat_glows_hotter_brighter_and_redshifts_low() {
        let base = material_for(BlockId::Stone);
        // Cold: untouched.
        assert_eq!(apply_heat(base, 50).emission, base.emission);
        // Warm glow: red leads.
        let warm = apply_heat(base, 150).emission;
        assert!(warm[0] > warm[1] && warm[0] > 0, "warm glow is red-led, got {warm:?}");
        // Molten: brighter overall, green has caught up toward orange/yellow.
        let hot = apply_heat(base, 255).emission;
        assert!(hot[0] >= warm[0] && hot[1] > warm[1], "hotter is brighter, {hot:?} vs {warm:?}");
    }

    #[test]
    fn thermal_shock_fails_each_material_in_character() {
        let p = ThermalParams::default();
        // A steep gradient: target cell hot, neighbour cold.
        let mut f = HeatField::new();
        f.set((0, 0, 0), 200);
        // neighbours implicitly ambient (0) -> gradient 200.

        let glass = thermal_shock(&f, None, |_| BlockId::Glass, &p);
        assert_eq!(glass.len(), 1);
        assert_eq!(glass[0].failure, Failure::Shatter, "glass shatters on a steep gradient");

        // Metal is ductile -> no shock failure at the same gradient (and 200 < melt 235).
        let metal = thermal_shock(&f, None, |_| BlockId::IronBlock, &p);
        assert!(metal.is_empty(), "ductile metal survives thermal shock, got {metal:?}");

        // Stone needs a steeper gradient than 200 to crack (threshold 190 < 200 -> cracks here).
        let stone = thermal_shock(&f, None, |_| BlockId::Stone, &p);
        assert_eq!(stone[0].failure, Failure::Crack);

        // Organic past its reaction temp burns (absolute heat, no gradient needed).
        let mut hot = HeatField::new();
        hot.set((0, 0, 0), 200); // > 150 burn temp; neighbours ambient
        let wood = thermal_shock(&hot, None, |_| BlockId::OakLog, &p);
        assert_eq!(wood[0].failure, Failure::Burn, "hot wood burns");
    }

    #[test]
    fn quench_triggers_shock_on_brittle_blocks() {
        let p = ThermalParams::default();
        // Previous tick hot, now suddenly cold (quenched) but neighbours also cold
        // so the spatial gradient alone wouldn't fire.
        let mut prev = HeatField::new();
        prev.set((0, 0, 0), 200);
        let mut now = HeatField::new();
        now.set((0, 0, 0), 60); // dropped 140 >= quench_rate 120
        let ev = thermal_shock(&now, Some(&prev), |_| BlockId::Obsidian, &p);
        assert_eq!(ev[0].failure, Failure::Shatter, "quenched obsidian shatters");
    }
}
