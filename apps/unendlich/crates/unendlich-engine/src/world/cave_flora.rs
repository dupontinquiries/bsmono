//! Cave flora — the assembler that fuses a [`StemGraph`] and a [`LumenField`]
//! into one living plant.
//!
//! A stem grows its node graph (moisture/temperature-gated, see [`crate::world::stem`]);
//! a lumen pearl is dropped on every node, charged by how much sky reaches that
//! cell; and the stem's edges become the light-chain links. The result: the
//! plant grows toward open space, the sky-touching parts charge by day, and at
//! night light pulses along the stem from the exposed sections inward — and
//! simply stops where the cave seals over, because buried pearls never charge.
//!
//! Still `BlockId`-agnostic: [`StemGraph::cells`] is the set of stem blocks to
//! place, each pearl's `emission` is the light the renderer reads. Binding those
//! to real blocks + feeding emission into lighting is the engine-side hookup.

use crate::world::lumen::{LumenField, Pearl};
use crate::world::stem::{Cell, StemGraph};

/// Vertical clearance probed for sky exposure (matches the mesher's sky probe).
const SKY_PROBE: i32 = 16;

/// Fraction of daylight reaching `cell`, by probing straight up: the run of
/// clear cells before the first solid block, over [`SKY_PROBE`]. An open column
/// is `1.0`; a cell directly under rock is `0.0`; a thin ceiling is in between.
pub fn sky_exposure<O>(cell: Cell, occupied: &O) -> f32
where
    O: Fn(Cell) -> bool,
{
    let mut clear = 0;
    for dy in 1..=SKY_PROBE {
        if occupied((cell.0, cell.1 + dy, cell.2)) {
            break;
        }
        clear += 1;
    }
    clear as f32 / SKY_PROBE as f32
}

/// A grown plant: its stem skeleton and the lumen field that lights it.
#[derive(Clone, Debug)]
pub struct CaveFlora {
    pub stem: StemGraph,
    pub lumen: LumenField,
}

impl CaveFlora {
    /// Build the lumen layer for an already-grown `stem`, charging each pearl by
    /// its `exposure`. Pearls share the stem's node order, so the stem's edges
    /// index straight into them.
    pub fn from_stem<E>(stem: StemGraph, exposure: &E) -> Self
    where
        E: Fn(Cell) -> f32,
    {
        let pearls: Vec<Pearl> = stem.nodes.iter().map(|n| Pearl::new(n.cell, exposure(n.cell))).collect();
        let lumen = LumenField::new(pearls, &stem.edges());
        Self { stem, lumen }
    }

    /// Grow a fresh plant from `root` (initial direction `dir`, reproducible by
    /// `seed`) for `steps`, then build its lumen layer. The field closures
    /// sample moisture/temperature/occupancy for growth and sky exposure for
    /// charging — typically `exposure` is [`sky_exposure`] bound to the world.
    #[allow(clippy::too_many_arguments)]
    pub fn grow<M, T, O, E>(
        root: Cell,
        dir: (i8, i8, i8),
        seed: u64,
        steps: u32,
        moisture: &M,
        temperature: &T,
        occupied: &O,
        exposure: &E,
    ) -> Self
    where
        M: Fn(Cell) -> f32,
        T: Fn(Cell) -> f32,
        O: Fn(Cell) -> bool,
        E: Fn(Cell) -> f32,
    {
        let mut stem = StemGraph::new(root, dir, seed);
        for _ in 0..steps {
            stem.grow_step(moisture, temperature, occupied);
        }
        Self::from_stem(stem, exposure)
    }

    /// Advance the light by one tick at the given `daylight` (0 night .. 1 noon).
    pub fn tick(&mut self, daylight: f32) {
        self.lumen.tick(daylight);
    }

    /// The stem blocks to place in the world.
    pub fn cells(&self) -> Vec<Cell> {
        self.stem.cells()
    }

    /// Brightest pearl right now (debug HUD / tests).
    pub fn peak_emission(&self) -> f32 {
        self.lumen.peak_emission()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::stem::StemNode;

    fn k(v: f32) -> impl Fn(Cell) -> f32 {
        move |_| v
    }
    fn open(_: Cell) -> bool {
        false
    }

    /// A hand-built 4-node vertical stem with known linear edges.
    fn linear_stem() -> StemGraph {
        let nodes = (0..4)
            .map(|i| StemNode {
                cell: (0, i, 0),
                parent: if i == 0 { -1 } else { (i - 1) as i32 },
                dir: (0, 1, 0),
                age: 0,
                energy: 0,
                active: false,
            })
            .collect();
        StemGraph { nodes, seed: 0, step: 0 }
    }

    /// Run a full day of charging then a night, returning which pearls fired.
    fn day_then_night(f: &mut CaveFlora) -> std::collections::HashSet<usize> {
        for _ in 0..120 {
            f.tick(1.0);
        }
        let mut fired = std::collections::HashSet::new();
        for _ in 0..60 {
            f.tick(0.0);
            for (i, p) in f.lumen.pearls.iter().enumerate() {
                if p.emission >= 1.0 {
                    fired.insert(i);
                }
            }
        }
        fired
    }

    #[test]
    fn probe_arithmetic() {
        assert_eq!(sky_exposure((0, 0, 0), &open), 1.0);
        assert_eq!(sky_exposure((0, 0, 0), &|c: Cell| c.1 == 1), 0.0);
        let half = sky_exposure((0, 0, 0), &|c: Cell| c.1 == 9); // first blocker at dy=9 → 8 clear
        assert!((half - 0.5).abs() < 1e-6, "thin ceiling gives partial exposure: {half}");
    }

    #[test]
    fn assembler_wires_stem_to_lumen() {
        let f = CaveFlora::grow((0, 0, 0), (0, 1, 0), 1, 12, &k(0.6), &k(0.5), &open, &k(1.0));
        assert_eq!(f.lumen.pearls.len(), f.stem.nodes.len(), "one pearl per node");
        assert_eq!(f.stem.edges().len(), f.stem.nodes.len() - 1, "links match stem edges");
    }

    #[test]
    fn exposed_stem_lights_fully() {
        let mut f = CaveFlora::from_stem(linear_stem(), &k(1.0));
        assert!(f.lumen.pearls.iter().all(|p| p.exposure == 1.0));
        let fired = day_then_night(&mut f);
        assert_eq!(fired.len(), 4, "a fully-exposed stem pulses end to end: {fired:?}");
    }

    #[test]
    fn exposure_gates_the_chain() {
        // Only the root touches sky; the rest are buried.
        let exposure = |c: Cell| if c == (0, 0, 0) { 1.0 } else { 0.0 };
        let mut f = CaveFlora::from_stem(linear_stem(), &exposure);
        let fired = day_then_night(&mut f);
        assert_eq!(fired, std::collections::HashSet::from([0]), "wave stops at uncharged buried pearls: {fired:?}");
    }
}
