//! `water_sim_core` — the pure heart of the dynamic water sim, extracted from the inline bin loop
//! so it can actually be TESTED and iterated on (operator: "water-vs-water and lava-vs-water and
//! erosion tests might help you iterate, and some cave-filling tests"). This mirrors the inline
//! settle algorithm exactly; the bin should be refactored to call it (a device-verified step) so
//! the two can't drift. Until then this is the canonical reference + the place new rules are proven
//! before wiring.
//!
//! ## Status check (audited): the refactor still has not happened
//!
//! Nothing anywhere references this module — `water_sim_core` has zero callers, so the bin's
//! inline loop is still the one that runs and this is still a copy. "Until then" has outlasted
//! several revisions, which is what that phrase usually does.
//!
//! Two things a reader should know before trusting this file:
//!
//! * It claims to mirror the inline algorithm EXACTLY, and nothing enforces that. There is no
//!   diff, no shared test vector, no guard. If the bin's loop has been tuned since extraction,
//!   this file is a plausible-looking lie — and it is the file someone would naturally read to
//!   understand the water sim, because it is the tested one.
//! * The settle is DEFAULT ON since r637 (see the bin's `water_sim_enabled`), so the inline
//!   copy is live for every player, not an experiment.
//!
//! The safe next step is the one the paragraph above already names: make the bin call this, on
//! a device where the result can be seen. Verifying the mirror by reading is not the same thing
//! and should not be recorded as if it were.
//!
//! Model: a square grid of world COLUMNS. Each column has a `base` (the y of its first water cell =
//! floor+1), a `depth` (how many water cells it holds), an optional `steam` back-pressure, and a
//! `bottomless` flag (floor below the sim window — all deep ocean). The settle moves water between
//! neighbours to equalise the free surface `base+depth+steam`, conserving volume. Bottomless
//! columns are inert (the r441 fix: they neither send nor receive, else they manufacture water).

#[derive(Clone, Debug)]
pub struct ColumnGrid {
    pub side: usize,
    /// y of each column's first water cell (floor+1). Length side*side.
    pub base: Vec<f32>,
    /// water cells held by each column.
    pub depth: Vec<f32>,
    /// optional per-column back-pressure (steam); zeros if unused.
    pub steam: Vec<f32>,
    /// true where the floor is below the sim window (deep ocean) — inert in the settle.
    pub bottomless: Vec<bool>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SettleStats {
    /// total water transferred across all iterations.
    pub moved: f32,
    /// columns that changed depth this call.
    pub changed: usize,
}

impl ColumnGrid {
    pub fn new(side: usize) -> Self {
        let n = side * side;
        Self {
            side,
            base: vec![0.0; n],
            depth: vec![0.0; n],
            steam: vec![0.0; n],
            bottomless: vec![false; n],
        }
    }

    #[inline]
    fn surf(&self, i: usize) -> f32 {
        self.base[i] + self.depth[i] + self.steam[i]
    }

    /// Total water volume currently held (sum of depth over non-bottomless columns).
    pub fn volume(&self) -> f32 {
        self.depth
            .iter()
            .zip(&self.bottomless)
            .filter(|(_, &b)| !b)
            .map(|(&d, _)| d)
            .sum()
    }

    /// Run `iters` settle iterations. `f` is the settle fraction (<0.25 for stability), `max_flow`
    /// caps total water a column may shed across the whole call (per-tick fill cap), `max_depth`
    /// clamps depth to the sim window height. Returns transfer stats. Volume-conserving over
    /// non-bottomless columns.
    pub fn settle(&mut self, iters: usize, f: f32, max_flow: f32, max_depth: f32) -> SettleStats {
        let p = FluidParams { f, max_flow, max_depth, yield_h: 0.0, infinite_sources: true };
        self.settle_with(iters, &p, |_| {})
    }

    /// The settle, with the full parameter set and a hook that sees each
    /// iteration's per-column sends (`[+x, -x, +z, -z]`), which the game folds
    /// into the flow field the water surface shader reads.
    ///
    /// `p.yield_h` is a Bingham-style yield: a column only pushes toward a
    /// neighbour whose surface is more than `yield_h` lower, and then by
    /// `f * (df - yield_h)`. Water uses 0. Lava uses about one block, so it
    /// stands in one-block steps and a one-deep sheet on the flat never creeps.
    ///
    /// `p.infinite_sources`: bottomless columns send and never deplete (the
    /// r680 ocean). When false they are inert (the r441 rule), which is what a
    /// fluid without an infinite reservoir (lava) needs.
    pub fn settle_with(
        &mut self,
        iters: usize,
        p: &FluidParams,
        mut on_step: impl FnMut(&[[f32; 4]]),
    ) -> SettleStats {
        let (f, max_flow, max_depth) = (p.f, p.max_flow, p.max_depth);
        let n = self.side * self.side;
        let side = self.side;
        let before: Vec<f32> = self.depth.clone();
        let mut sent_this_call = vec![0.0f32; n];
        let mut moved = 0.0f32;

        for _ in 0..iters {
            let mut send = vec![[0.0f32; 4]; n]; // +x,-x,+z,-z
            for zz in 0..side {
                for xx in 0..side {
                    let i = xx + zz * side;
                    let d = self.depth[i];
                    if d <= 0.0 {
                        continue;
                    }
                    // r680: a bottomless column is an INFINITE SOURCE — it sends
                    // downhill into lower real neighbours (ocean pours into a cave)
                    // but never depletes (held in the write-back) and never receives.
                    let is_source = self.bottomless[i];
                    if is_source && !p.infinite_sources {
                        continue;
                    }
                    let surf = self.surf(i);
                    let mut want = [0.0f32; 4];
                    let mut wt = 0.0f32;
                    let consider = |dir: usize, j: usize, want: &mut [f32; 4], wt: &mut f32| {
                        if self.bottomless[j] {
                            return;
                        }
                        let df = surf - self.surf(j) - p.yield_h;
                        if df > 0.0 {
                            want[dir] = f * df;
                            *wt += want[dir];
                        }
                    };
                    if xx + 1 < side {
                        consider(0, i + 1, &mut want, &mut wt);
                    }
                    if xx > 0 {
                        consider(1, i - 1, &mut want, &mut wt);
                    }
                    if zz + 1 < side {
                        consider(2, i + side, &mut want, &mut wt);
                    }
                    if zz > 0 {
                        consider(3, i - side, &mut want, &mut wt);
                    }
                    if wt <= 0.0 {
                        continue;
                    }
                    let mut scale = if wt > d && !is_source { d / wt } else { 1.0 };
                    let total = wt * scale;
                    let cap = (max_flow - sent_this_call[i]).max(0.0);
                    if total > cap && total > 0.0 {
                        scale *= cap / total;
                    }
                    sent_this_call[i] += wt * scale;
                    send[i] = [
                        want[0] * scale,
                        want[1] * scale,
                        want[2] * scale,
                        want[3] * scale,
                    ];
                }
            }
            on_step(&send);
            for zz in 0..side {
                for xx in 0..side {
                    let i = xx + zz * side;
                    let out = send[i][0] + send[i][1] + send[i][2] + send[i][3];
                    let mut net = -out;
                    moved += out;
                    if xx + 1 < side {
                        net += send[i + 1][1];
                    }
                    if xx > 0 {
                        net += send[i - 1][0];
                    }
                    if zz + 1 < side {
                        net += send[i + side][3];
                    }
                    if zz > 0 {
                        net += send[i - side][2];
                    }
                    // r680: bottomless sources hold their level (infinite ocean);
                    // only real columns settle.
                    if !self.bottomless[i] {
                        self.depth[i] = (self.depth[i] + net).max(0.0).min(max_depth);
                    }
                }
            }
        }

        let changed = before
            .iter()
            .zip(&self.depth)
            .filter(|(&a, &b)| (a - b).abs() > 1e-4)
            .count();
        SettleStats { moved, changed }
    }
}

/// Settle tuning shared by water and lava. See [`ColumnGrid::settle_with`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FluidParams {
    /// Settle fraction per iteration (< 0.25 for stability).
    pub f: f32,
    /// Most a column may shed across one call (the per-tick fill cap).
    pub max_flow: f32,
    /// Depth clamp (the sim window height).
    pub max_depth: f32,
    /// Surface step a column holds without flowing.
    pub yield_h: f32,
    /// Bottomless columns are infinite sources (true) or inert (false).
    pub infinite_sources: bool,
}

impl FluidParams {
    /// Lava: slow, holds one-block steps, no infinite reservoir.
    pub fn lava(max_depth: f32) -> Self {
        FluidParams { f: 0.08, max_flow: 0.6, max_depth, yield_h: 1.0, infinite_sources: false }
    }
}

impl ColumnGrid {
    /// Whole cells per column for the write-back, conserving volume.
    ///
    /// The game used to write `depth.round()` per column, which is not
    /// volume-preserving: two columns settled to 2.5 and 0.5 wrote 3 + 1 = 4
    /// cells from 3, and 2.4 + 0.4 wrote 2 from 3. Each tick re-gathers the
    /// written cells, so the error compounded: water appeared or vanished at
    /// every half-block.
    ///
    /// Here the real columns share `round(total)` cells by largest remainder:
    /// every column gets `floor(depth)`, and the leftover cells go to the
    /// largest fractional parts. `prev` (the cell counts gathered this tick)
    /// breaks near-ties in favour of the column that already had the cell, so
    /// a level lake whose share is x.5 per column does not flip cells back and
    /// forth every tick. Bottomless columns get `round(depth)` and are not
    /// counted (they are never written).
    pub fn quantize(&self, prev: &[f32]) -> Vec<i32> {
        let n = self.depth.len();
        let mut out = vec![0i32; n];
        let mut total = 0.0f64;
        let mut floors = 0i64;
        let mut cand: Vec<(f32, usize)> = Vec::new();
        for i in 0..n {
            let d = self.depth[i].max(0.0);
            if self.bottomless[i] {
                out[i] = d.round() as i32;
                continue;
            }
            total += d as f64;
            let fl = d.floor();
            out[i] = fl as i32;
            floors += fl as i64;
            let frac = d - fl;
            if frac > 1e-4 {
                let had = prev.get(i).map_or(false, |&p| p >= fl + 1.0 - 1e-3);
                cand.push((frac + if had { 0.25 } else { 0.0 }, i));
            }
        }
        let want = total.round() as i64 - floors;
        if want > 0 {
            cand.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
            for &(_, i) in cand.iter().take(want as usize) {
                out[i] += 1;
            }
        }
        out
    }
}

/// How much of an over-budget write-back to apply, as `(adds, removes)`.
///
/// Applying the first `budget` cell changes in scan order is not volume-safe:
/// a column drained at the start of the scan and the column it drained into
/// at the end get one half of the move each, so water vanishes (or appears)
/// until a later tick catches up. Instead the applied adds and removes keep
/// the tick's intended net change (`adds - removes`: sources, rain, boiling),
/// or as much of it as fits, and spend the rest of the budget in matched
/// pairs.
pub fn split_budget(adds: usize, removes: usize, budget: usize) -> (usize, usize) {
    if adds + removes <= budget {
        return (adds, removes);
    }
    let net = adds as i64 - removes as i64;
    let b = budget as i64;
    if net.abs() >= b {
        return if net > 0 { (budget, 0) } else { (0, budget) };
    }
    let pairs = (b - net.abs()) / 2;
    let (a, r) = if net >= 0 { (pairs + net, pairs) } else { (pairs, pairs - net) };
    (a.min(adds as i64) as usize, r.min(removes as i64) as usize)
}

/// What [`gather`] reads out of one column for one fluid.
#[derive(Clone, Debug, PartialEq)]
pub struct Gathered {
    pub sample: ColumnSample,
    /// Open runs, bottom to top, as `(top, next)`: `top` is a floor-capable
    /// cell whose cell above is open, and `next` is the next floor-capable
    /// cell above it (`i32::MAX` if none in the window). Fluid arriving from
    /// the side can come to rest on `top` if it arrives between the two.
    pub tops: Vec<(i32, i32)>,
}

/// [`gather_column`] for either fluid, plus the landing heights a dry column
/// needs. For `Cell::Water`, lava is a floor (water rests and boils on it).
/// For `Cell::Lava`, water is open space (the lava/water reaction is separate).
pub fn gather(cells: &[Cell], win_lo: i32, fluid: Cell) -> Gathered {
    let floor_like = |c: Cell| match fluid {
        Cell::Lava => c == Cell::Solid,
        _ => c == Cell::Solid || c == Cell::Lava,
    };
    let mut floor: Option<i32> = None;
    let mut last_solid: Option<i32> = None;
    let mut last_solid_lava = false;
    let mut depth = 0.0f32;
    let mut on_lava = false;
    let mut tops = Vec::new();
    for (k, &c) in cells.iter().enumerate() {
        let y = win_lo + k as i32;
        if c == fluid {
            depth += 1.0;
            floor = last_solid;
            on_lava = last_solid_lava;
        } else if floor_like(c) {
            last_solid = Some(y);
            last_solid_lava = c == Cell::Lava;
            if let Some(last) = tops.last_mut() {
                let (_, next): &mut (i32, i32) = last;
                if *next == i32::MAX {
                    *next = y;
                }
            }
            let above_open = cells.get(k + 1).map_or(true, |&a| !floor_like(a));
            if above_open {
                tops.push((y, i32::MAX));
            }
        }
    }
    Gathered { sample: ColumnSample { floor, depth, on_lava }, tops }
}

/// Give dry columns a floor so fluid can spread onto them.
///
/// The gather only knows a column's floor when the column holds fluid (the
/// bed under its topmost fluid cell, r445), so every dry column came back
/// floorless. Floorless means bottomless, and bottomless columns never
/// receive: water could not run into a trench dug from a lake, pour over a
/// ledge onto dry ground, or form a puddle on dry land in the rain.
///
/// A dry column next to a wet one lands the inflow on the highest top that
/// (a) lies below that neighbour's surface, so a cave roof above the water is
/// not mistaken for the floor (the same care r445 took for wet columns), and
/// (b) is open at the height of the neighbour's water, so a lake does not
/// leak through a rock wall into a cave pocket behind it. A dry column with
/// no wet neighbour takes its highest top (where rain lands). Columns with no
/// such top in the window stay floorless.
pub fn resolve_dry_floors(side: usize, floor: &mut [Option<i32>], depth: &[f32], tops: &[Vec<(i32, i32)>]) {
    let n = side * side;
    let before: Vec<Option<i32>> = floor.to_vec();
    for i in 0..n {
        if depth[i] > 0.0 || before[i].is_some() {
            continue;
        }
        let (x, z) = (i % side, i / side);
        let mut any_wet = false;
        let mut best: Option<i32> = None;
        let mut nb = |j: usize| {
            if depth[j] <= 0.0 {
                return;
            }
            let Some(fj) = before[j] else { return };
            any_wet = true;
            let s = fj as f32 + 1.0 + depth[j];
            let lowest_water = fj + 1;
            let fit = tops[i]
                .iter()
                .rev()
                .find(|&&(t, next)| (t as f32 + 1.0) < s && next > lowest_water)
                .map(|&(t, _)| t);
            if let Some(t) = fit {
                best = Some(best.map_or(t, |b| b.max(t)));
            }
        };
        if x + 1 < side { nb(i + 1); }
        if x > 0 { nb(i - 1); }
        if z + 1 < side { nb(i + side); }
        if z > 0 { nb(i - side); }
        floor[i] = if any_wet { best } else { tops[i].last().map(|&(t, _)| t) };
    }
}

/// A column cell, abstracting the bin's BlockId for the gather logic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cell {
    Air,
    Water,
    Solid,
    Lava,
}

/// What a single column gather yields: the floor (the solid the water RESTS ON — the topmost solid
/// BELOW the topmost water cell), the water depth, and whether that floor is lava.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnSample {
    /// y of the bed under the topmost water, or None if no solid lies below the water (bottomless).
    pub floor: Option<i32>,
    pub depth: f32,
    pub on_lava: bool,
}

/// Scan a column from bottom (`cells[0]` at y=`win_lo`) to top. Mirrors the bin gather: the floor is
/// the topmost solid BELOW the topmost water — NOT the topmost solid overall — so a solid ABOVE the
/// water (overhang / ceiling / placed block / terrain lip) does not become the floor (which would
/// make the write-back stack water on top of it: the r445 "rises occasionally" bug).
pub fn gather_column(cells: &[Cell], win_lo: i32) -> ColumnSample {
    let mut floor: Option<i32> = None;
    let mut last_solid: Option<i32> = None;
    let mut last_solid_lava = false;
    let mut depth = 0.0f32;
    let mut on_lava = false;
    for (k, &c) in cells.iter().enumerate() {
        let y = win_lo + k as i32;
        match c {
            Cell::Water => {
                depth += 1.0;
                floor = last_solid;
                on_lava = last_solid_lava;
            }
            Cell::Solid => {
                last_solid = Some(y);
                last_solid_lava = false;
            }
            Cell::Lava => {
                last_solid = Some(y);
                last_solid_lava = true;
            }
            Cell::Air => {}
        }
    }
    ColumnSample { floor, depth, on_lava }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Helper: a flat floor at base=0 everywhere, given depths.
    fn grid(side: usize, depths: &[f32]) -> ColumnGrid {
        let mut g = ColumnGrid::new(side);
        g.depth.copy_from_slice(depths);
        g
    }

    fn max_surf_spread(g: &ColumnGrid) -> f32 {
        let surfs: Vec<f32> = (0..g.depth.len())
            .filter(|&i| g.depth[i] > 0.0)
            .map(|i| g.base[i] + g.depth[i])
            .collect();
        let hi = surfs.iter().cloned().fold(f32::MIN, f32::max);
        let lo = surfs.iter().cloned().fold(f32::MAX, f32::min);
        hi - lo
    }

    #[test]
    fn water_vs_water_levels_out() {
        // two columns, one tall one empty, same floor → should equalise to ~2 each
        let mut g = grid(2, &[4.0, 0.0, 0.0, 0.0]);
        let v0 = g.volume();
        g.settle(40, 0.125, 1e9, 1000.0);
        assert!((g.volume() - v0).abs() < 1e-3, "volume conserved");
        // the two cells along x should converge toward equal surface
        assert!((g.depth[0] - g.depth[1]).abs() < 0.5, "{:?}", g.depth);
    }

    #[test]
    fn volume_is_conserved() {
        let mut g = grid(3, &[9.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let v0 = g.volume();
        g.settle(50, 0.2, 1e9, 1000.0);
        assert!((g.volume() - v0).abs() < 1e-2, "v0={v0} now={}", g.volume());
    }

    #[test]
    fn cave_filling_a_hole_fills_from_neighbours() {
        // center column is a deep hole (low floor), neighbours hold water at a higher surface.
        // Model the hole as a lower base; water should flow IN and raise the hole's depth.
        let mut g = ColumnGrid::new(3);
        for i in 0..9 {
            g.base[i] = 10.0;
            g.depth[i] = 5.0; // surface at 15 everywhere
        }
        // center: floor much lower (a cave), starts empty
        let c = 4;
        g.base[c] = 0.0;
        g.depth[c] = 0.0; // surface at 0 — far below neighbours
        let v0 = g.volume();
        g.settle(80, 0.125, 1e9, 1000.0);
        assert!((g.volume() - v0).abs() < 1e-2, "volume conserved while filling");
        assert!(g.depth[c] > 4.0, "hole filled with water: {:?}", g.depth[c]);
        // surfaces should be much closer than the initial 15-block gap
        assert!(max_surf_spread(&g) < 3.0, "spread {}", max_surf_spread(&g));
    }

    #[test]
    fn bottomless_columns_are_inert_no_water_created() {
        // r441 REGRESSION: a bottomless column next to a tall floored column must NOT manufacture
        // water. Before the fix, the floored column drained into the bottomless one and the
        // bottomless one's "received" water reappeared elsewhere → volume grew.
        let mut g = ColumnGrid::new(2);
        g.base = vec![0.0, 0.0, 0.0, 0.0];
        g.depth = vec![10.0, 0.0, 0.0, 0.0];
        g.bottomless = vec![false, true, false, false]; // col 1 is deep ocean
        let v0 = g.volume(); // counts only non-bottomless
        g.settle(50, 0.125, 1e9, 1000.0);
        // bottomless col never gains depth, and total floored volume can't increase
        assert_eq!(g.depth[1], 0.0, "bottomless stayed empty");
        assert!(g.volume() <= v0 + 1e-3, "no water created: {} <= {}", g.volume(), v0);
    }

    #[test]
    fn bottomless_ocean_pours_into_adjacent_cave_to_sea_level() {
        // 3×3: an infinite ocean column beside an empty deep cave, surrounded by
        // high land. The ocean should pour into the cave until it reaches sea
        // level, then stop — and the ocean itself must never drain.
        let mut g = ColumnGrid::new(3);
        let n = 9;
        for i in 0..n {
            g.base[i] = 20.0; // land well above sea level
            g.depth[i] = 0.0;
            g.bottomless[i] = false;
        }
        // i=0: bottomless ocean, surface at sea level 10.
        g.base[0] = 0.0;
        g.depth[0] = 10.0;
        g.bottomless[0] = true;
        // i=1: an empty cave with its floor deep below sea level.
        g.base[1] = 0.0;
        g.depth[1] = 0.0;
        let ocean_before = g.depth[0];

        for _ in 0..300 {
            g.settle(4, 0.2, 100.0, 100.0);
        }

        let cave_surf = g.base[1] + g.depth[1];
        assert!(cave_surf > 8.0, "cave should fill toward sea level, got surf {}", cave_surf);
        assert!(cave_surf <= 10.5, "cave must not overfill past sea level, got {}", cave_surf);
        assert_eq!(g.depth[0], ocean_before, "bottomless ocean must never drain");
    }

    #[test]
    fn bottomless_ocean_does_not_flood_higher_land() {
        // An ocean at sea level 10 next to land whose floor is ABOVE sea level
        // must not push water uphill.
        let mut g = ColumnGrid::new(3);
        for i in 0..9 {
            g.base[i] = 15.0;
            g.depth[i] = 0.0;
            g.bottomless[i] = false;
        }
        g.base[0] = 0.0;
        g.depth[0] = 10.0;
        g.bottomless[0] = true;
        for _ in 0..100 {
            g.settle(4, 0.2, 100.0, 100.0);
        }
        // land neighbour (i=1) stays dry — its floor (15) is above sea level (10).
        assert_eq!(g.depth[1], 0.0, "ocean must not flood land above sea level");
    }

    #[test]
    fn max_flow_caps_per_call_transfer() {
        // with a tight per-call cap, a single tall column can't dump everything at once
        let mut capped = grid(2, &[20.0, 0.0, 0.0, 0.0]);
        capped.settle(8, 0.125, 1.0, 1000.0); // cap = 1 block shed total
        // the source kept most of its water because shedding was capped
        assert!(capped.depth[0] > 18.0, "capped source kept water: {:?}", capped.depth);

        let mut uncapped = grid(2, &[20.0, 0.0, 0.0, 0.0]);
        uncapped.settle(8, 0.125, 1e9, 1000.0);
        assert!(uncapped.depth[0] < capped.depth[0], "uncapped shed more");
    }

    #[test]
    fn settled_water_stops_moving() {
        // already-flat water shouldn't move (active=false equivalent)
        let mut g = grid(3, &[3.0; 9]);
        let stats = g.settle(8, 0.125, 1e9, 1000.0);
        assert!(stats.moved < 1e-3, "flat water is quiescent: moved={}", stats.moved);
    }

    #[test]
    fn gather_normal_floor_below_water() {
        use Cell::*;
        // y=0 solid floor, water at 1,2, air above
        let s = gather_column(&[Solid, Water, Water, Air], 0);
        assert_eq!(s.floor, Some(0));
        assert_eq!(s.depth, 2.0);
        assert!(!s.on_lava);
    }

    #[test]
    fn gather_overhang_does_not_lift_floor() {
        use Cell::*;
        // r445 REGRESSION: floor at 0, water at 1,2, OVERHANG solid at 3 (ceiling). The floor must
        // stay 0 (the bed under the water) — NOT 3 (the overhang) — so water isn't stacked on top.
        let s = gather_column(&[Solid, Water, Water, Solid, Air], 0);
        assert_eq!(s.floor, Some(0), "overhang must not become the floor");
        assert_eq!(s.depth, 2.0);
    }

    #[test]
    fn gather_bottomless_when_no_solid_below_water() {
        use Cell::*;
        // water all the way down, no solid below it in the window → bottomless (inert in settle)
        let s = gather_column(&[Water, Water, Water], 0);
        assert_eq!(s.floor, None);
        assert_eq!(s.depth, 3.0);
    }

    /// Build a grid the way the game does: gather each column from cells,
    /// resolve dry floors, then map floors to bases and bottomless flags.
    fn from_columns(side: usize, cols: &[Vec<Cell>], fluid: Cell) -> (ColumnGrid, Vec<Option<i32>>) {
        let g: Vec<Gathered> = cols.iter().map(|c| gather(c, 0, fluid)).collect();
        let mut floor: Vec<Option<i32>> = g.iter().map(|x| x.sample.floor).collect();
        let depth: Vec<f32> = g.iter().map(|x| x.sample.depth).collect();
        let tops: Vec<Vec<(i32, i32)>> = g.iter().map(|x| x.tops.clone()).collect();
        resolve_dry_floors(side, &mut floor, &depth, &tops);
        let mut grid = ColumnGrid::new(side);
        for i in 0..side * side {
            grid.depth[i] = depth[i];
            match floor[i] {
                Some(f) => grid.base[i] = f as f32 + 1.0,
                None => {
                    grid.base[i] = -1.0;
                    grid.bottomless[i] = true;
                }
            }
        }
        (grid, floor)
    }

    fn col(solid_to: i32, fluid: Cell, fluid_cells: i32, height: i32) -> Vec<Cell> {
        (0..height)
            .map(|y| if y <= solid_to { Cell::Solid } else if y <= solid_to + fluid_cells { fluid } else { Cell::Air })
            .collect()
    }

    #[test]
    fn water_runs_into_a_trench_dug_beside_a_lake() {
        // 3x3: a lake (floor 4, 4 deep) everywhere except a dry trench at the
        // centre whose floor is at 1. Before resolve_dry_floors the trench was
        // bottomless and stayed dry forever.
        let mut cols = vec![col(4, Cell::Water, 4, 16); 9];
        cols[4] = col(1, Cell::Water, 0, 16);
        let (mut g, floor) = from_columns(3, &cols, Cell::Water);
        assert_eq!(floor[4], Some(1), "the trench has a floor");
        let v0 = g.volume();
        for _ in 0..40 {
            g.settle(8, 0.125, 2.0, 16.0);
        }
        assert!(g.depth[4] > 2.0, "trench filled: {}", g.depth[4]);
        assert!((g.volume() - v0).abs() < 1e-2);
    }

    #[test]
    fn a_dry_column_under_a_roof_takes_the_floor_below_the_water() {
        // Dry column: solid 0..=2, air 3..=6, a roof at 7..=8, air above. Its
        // wet neighbour's surface is at 6. The inflow lands on 2, not on the
        // roof at 8.
        let mut dry = col(2, Cell::Water, 0, 12);
        dry[7] = Cell::Solid;
        dry[8] = Cell::Solid;
        let wet = col(1, Cell::Water, 4, 12); // floor 1, water 2..=5, surface 6
        let wall = col(11, Cell::Water, 0, 12);
        let (_, floor) = from_columns(2, &[wet, dry, wall.clone(), wall], Cell::Water);
        assert_eq!(floor[1], Some(2));
    }

    #[test]
    fn dry_ground_away_from_water_takes_its_top_for_rain() {
        // No wet neighbour anywhere: each column's floor is its highest top,
        // which is where rain lands (apply_rain skips floorless columns, so
        // before this no puddle could start on dry land).
        let mut c = col(3, Cell::Water, 0, 12);
        c[8] = Cell::Solid; // a ledge above: rain lands on it, not below it
        let (_, floor) = from_columns(2, &[c.clone(), c.clone(), c.clone(), c], Cell::Water);
        assert_eq!(floor, vec![Some(8); 4]);
    }

    #[test]
    fn a_lake_does_not_leak_through_rock_into_a_pocket_behind_it() {
        // Dry column: solid 0..=2, a sealed pocket 3..=5, solid 6..=15. Its
        // neighbour holds water on a floor at 8 (water 9..=12). The pocket
        // is below the surface but walled off at the water's height.
        let mut dry: Vec<Cell> = (0..20).map(|y| if y <= 15 { Cell::Solid } else { Cell::Air }).collect();
        for y in 3..=5 {
            dry[y] = Cell::Air;
        }
        let wet = col(8, Cell::Water, 4, 20);
        let wall = col(19, Cell::Water, 0, 20);
        let (_, floor) = from_columns(2, &[wet, dry, wall.clone(), wall], Cell::Water);
        assert_eq!(floor[1], None, "no floor reachable from the lake");
    }

    #[test]
    fn quantize_conserves_whole_cells() {
        // The old per-column round wrote 4 cells from 3 here.
        let mut g = grid(2, &[0.5, 2.5, 0.0, 0.0]);
        let q = g.quantize(&[0.0, 3.0, 0.0, 0.0]);
        assert_eq!(q.iter().sum::<i32>(), 3, "{q:?}");
        // An exact tie: without the tie-break the lower index would win.
        assert_eq!(q[1], 3, "the column that had the cell keeps it: {q:?}");
        g.depth = vec![2.4, 0.4, 0.2, 0.0];
        let q = g.quantize(&[0.0; 4]);
        assert_eq!(q.iter().sum::<i32>(), 3, "{q:?}");
        // A full settle-and-quantize loop never changes the cell count.
        let mut g = grid(3, &[9.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 3.0]);
        let mut prev: Vec<f32> = g.depth.clone();
        for _ in 0..30 {
            g.settle(8, 0.125, 1.0, 100.0);
            let q = g.quantize(&prev);
            assert_eq!(q.iter().sum::<i32>(), 12);
            g.depth = q.iter().map(|&c| c as f32).collect();
            prev = g.depth.clone();
        }
    }

    #[test]
    fn quantize_skips_bottomless() {
        let mut g = grid(2, &[1.5, 10.0, 0.5, 0.0]);
        g.bottomless[1] = true;
        let q = g.quantize(&[0.0; 4]);
        assert_eq!(q[0] + q[2] + q[3], 2);
        assert_eq!(q[1], 10);
    }

    #[test]
    fn split_budget_keeps_the_net_change() {
        assert_eq!(split_budget(3, 2, 10), (3, 2));
        // Pure move over budget: equal halves.
        assert_eq!(split_budget(50, 50, 20), (10, 10));
        // Net inflow of 6 (a source) with a budget of 10: all 6 plus 2 pairs.
        assert_eq!(split_budget(30, 24, 10), (8, 2));
        // Net bigger than the budget: all of it one way.
        assert_eq!(split_budget(40, 0, 10), (10, 0));
        assert_eq!(split_budget(0, 40, 10), (0, 10));
        for (a, r, b) in [(7, 3, 5), (3, 7, 5), (100, 99, 3), (1, 0, 1)] {
            let (x, y) = split_budget(a, r, b);
            assert!(x + y <= b && x <= a && y <= r, "{a} {r} {b} -> {x} {y}");
        }
    }

    #[test]
    fn lava_holds_one_block_steps_and_does_not_creep_on_the_flat() {
        let p = FluidParams::lava(64.0);
        // One-deep sheet beside empty flat ground: stays put.
        let mut sheet = grid(2, &[1.0, 0.0, 0.0, 0.0]);
        sheet.settle_with(50, &p, |_| {});
        assert_eq!(sheet.depth[0], 1.0);
        // A 5-tall pile spreads, but slower than water, and stops with steps.
        let mut lava = grid(3, &[0.0, 0.0, 0.0, 0.0, 5.0, 0.0, 0.0, 0.0, 0.0]);
        let mut water = lava.clone();
        lava.settle_with(4, &p, |_| {});
        water.settle(4, 0.125, 1e9, 64.0); // water's settle fraction, no shed cap
        assert!(lava.depth[4] > water.depth[4], "lava is slower: {} vs {}", lava.depth[4], water.depth[4]);
        for _ in 0..400 {
            lava.settle_with(4, &p, |_| {});
        }
        assert!(lava.depth[4] < 5.0, "the pile spread");
        assert!((lava.volume() - 5.0).abs() < 1e-3, "lava volume conserved");
        for i in 0..9 {
            let (x, z) = (i % 3, i / 3);
            for (j, ok) in [(i + 1, x + 1 < 3), (i + 3, z + 1 < 3)] {
                if ok {
                    assert!((lava.depth[i] - lava.depth[j]).abs() <= 1.0 + 1e-3, "{:?}", lava.depth);
                }
            }
        }
    }

    #[test]
    fn lava_bottomless_is_inert_not_a_source() {
        let p = FluidParams::lava(64.0);
        let mut g = grid(2, &[6.0, 0.0, 0.0, 0.0]);
        g.bottomless[0] = true;
        g.settle_with(50, &p, |_| {});
        assert_eq!(g.depth[1], 0.0, "no lava from a bottomless column");
    }

    #[test]
    fn gather_lava_treats_water_as_open() {
        use Cell::*;
        let g = gather(&[Solid, Lava, Lava, Water, Air], 0, Lava);
        assert_eq!(g.sample.floor, Some(0));
        assert_eq!(g.sample.depth, 2.0);
        let w = gather(&[Solid, Lava, Water, Water, Air], 0, Water);
        assert_eq!(w.sample.floor, Some(1), "water rests on lava");
        assert!(w.sample.on_lava);
        assert_eq!(w.tops, vec![(1, i32::MAX)]);
    }

    #[test]
    fn gather_floor_is_lava_when_bed_is_lava() {
        use Cell::*;
        let s = gather_column(&[Lava, Water, Water, Air], 0);
        assert_eq!(s.floor, Some(0));
        assert!(s.on_lava, "water sitting on lava reports on_lava");
    }
}
