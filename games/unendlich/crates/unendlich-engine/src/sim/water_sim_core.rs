//! `water_sim_core` — the pure heart of the dynamic water sim, extracted from the inline bin loop
//! so it can actually be TESTED and iterated on (operator: "water-vs-water and lava-vs-water and
//! erosion tests might help you iterate, and some cave-filling tests"). This mirrors the inline
//! settle algorithm exactly; the bin should be refactored to call it (a device-verified step) so
//! the two can't drift. Until then this is the canonical reference + the place new rules are proven
//! before wiring.
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
                    let surf = self.surf(i);
                    let mut want = [0.0f32; 4];
                    let mut wt = 0.0f32;
                    let consider = |dir: usize, j: usize, want: &mut [f32; 4], wt: &mut f32| {
                        if self.bottomless[j] {
                            return;
                        }
                        let df = surf - self.surf(j);
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

    #[test]
    fn gather_floor_is_lava_when_bed_is_lava() {
        use Cell::*;
        let s = gather_column(&[Lava, Water, Water, Air], 0);
        assert_eq!(s.floor, Some(0));
        assert!(s.on_lava, "water sitting on lava reports on_lava");
    }
}
