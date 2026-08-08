//! Layered tile map. Vertical space is 9 quantized levels: ground terrain
//! occupies L0..L5, air flies A0..A2 (levels 6..8). A tile owns up to two
//! walkable *surfaces*: its terrain top, and optionally a bridge deck at a
//! higher level — units path over the deck or under it when their vertical
//! depth clears (unit_top <= deck level). Ramps connect consecutive levels.
//! Structures fill [terrain .. terrain+depth), which can poke into low air.

use crate::fx::V2;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub const GROUND_LEVELS: u8 = 6;
pub const AIR0: u8 = 6; // first air level
pub const AIR_LAYERS: u8 = 3;
pub const TOP_LEVEL: u8 = 8; // highest occupiable level (AIR0 + AIR_LAYERS - 1)
pub const H_VOID: u8 = 255; // bottomless chasm: air crosses, ground never

pub const SURF_GROUND: u8 = 0;
pub const SURF_DECK: u8 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResKind {
    Alloy,
    Flux,
}

#[derive(Clone, Debug)]
pub struct ResNode {
    pub tx: i32,
    pub ty: i32,
    pub kind: ResKind,
    pub amount: i32,
    pub taken_by: u32,
}

pub struct GameMap {
    pub w: i32,
    pub h: i32,
    terrain: Vec<u8>,   // 0..5 or H_VOID
    ramp_lo: Vec<u8>,   // 255 = not a ramp; else connects lo <-> lo+1
    bridge: Vec<u8>,    // 0 = none; else deck level (2..=5)
    build: Vec<bool>,   // static buildability (terrain surface only)
    struct_top: Vec<u8>, // 0 = free; else occupied, top level of structure
    pub nodes: Vec<ResNode>,
    pub starts: [(i32, i32); 2],
}

impl GameMap {
    fn idx(&self, x: i32, y: i32) -> usize {
        (y * self.w + x) as usize
    }
    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w && y < self.h
    }
    pub fn hgt(&self, x: i32, y: i32) -> u8 {
        if self.in_bounds(x, y) {
            self.terrain[self.idx(x, y)]
        } else {
            H_VOID
        }
    }
    pub fn ramp(&self, x: i32, y: i32) -> Option<u8> {
        if self.in_bounds(x, y) {
            let r = self.ramp_lo[self.idx(x, y)];
            if r != 255 {
                return Some(r);
            }
        }
        None
    }
    pub fn bridge(&self, x: i32, y: i32) -> u8 {
        if self.in_bounds(x, y) {
            self.bridge[self.idx(x, y)]
        } else {
            0
        }
    }
    pub fn struct_top(&self, x: i32, y: i32) -> u8 {
        if self.in_bounds(x, y) {
            self.struct_top[self.idx(x, y)]
        } else {
            0
        }
    }
    pub fn set_struct(&mut self, x: i32, y: i32, top: u8) {
        if self.in_bounds(x, y) {
            let i = self.idx(x, y);
            self.struct_top[i] = top;
        }
    }

    /// the walkable level of a surface on this tile, if that surface exists
    pub fn surf_level(&self, x: i32, y: i32, surf: u8) -> Option<u8> {
        if !self.in_bounds(x, y) {
            return None;
        }
        match surf {
            SURF_DECK => {
                let b = self.bridge(x, y);
                if b > 0 {
                    Some(b)
                } else {
                    None
                }
            }
            _ => {
                let t = self.hgt(x, y);
                if t == H_VOID {
                    None
                } else {
                    Some(t)
                }
            }
        }
    }

    /// can a ground unit of vertical `depth` stand on this surface?
    pub fn passable(&self, x: i32, y: i32, surf: u8, depth: u8) -> bool {
        let Some(lvl) = self.surf_level(x, y, surf) else {
            return false;
        };
        if surf == SURF_GROUND {
            if self.struct_top(x, y) > 0 {
                return false;
            }
            let b = self.bridge(x, y);
            if b > 0 && lvl + depth > b {
                return false; // doesn't fit under the deck
            }
        }
        true
    }

    /// legacy convenience: depth-1 ground passability
    pub fn walkable(&self, x: i32, y: i32) -> bool {
        self.passable(x, y, SURF_GROUND, 1)
    }
    pub fn buildable(&self, x: i32, y: i32) -> bool {
        self.in_bounds(x, y)
            && self.build[self.idx(x, y)]
            && self.struct_top(x, y) == 0
            && self.hgt(x, y) != H_VOID
            && self.bridge(x, y) == 0
    }
    pub fn area_buildable(&self, tx: i32, ty: i32, size: i32) -> bool {
        let base = self.hgt(tx, ty);
        for x in tx..tx + size {
            for y in ty..ty + size {
                if !self.buildable(x, y) || self.hgt(x, y) != base || self.ramp(x, y).is_some() {
                    return false;
                }
            }
        }
        true
    }

    /// may a ground unit step between two surfaces? equal levels join;
    /// a one-level difference needs a ramp on either endpoint.
    pub fn step_ok(&self, a: (i32, i32), sa: u8, b: (i32, i32), sb: u8, depth: u8) -> bool {
        if !self.passable(b.0, b.1, sb, depth) {
            return false;
        }
        let (Some(la), Some(lb)) = (self.surf_level(a.0, a.1, sa), self.surf_level(b.0, b.1, sb))
        else {
            return false;
        };
        if la == lb {
            return true;
        }
        let d = if la > lb { la - lb } else { lb - la };
        d == 1
            && (self.ramp(a.0, a.1).is_some() || self.ramp(b.0, b.1).is_some())
            && sa == SURF_GROUND
            && sb == SURF_GROUND
    }

    /// spawn adjacency: same rules as one step
    pub fn spawn_ok(&self, from: (i32, i32), to: (i32, i32)) -> bool {
        self.step_ok(from, SURF_GROUND, to, SURF_GROUND, 1)
    }

    // ---------------- construction ----------------

    pub fn standard() -> GameMap {
        let (w, h) = (64, 64);
        let n = (w * h) as usize;
        let mut terrain = vec![0u8; n];
        let mut build = vec![true; n];
        for x in 0..w {
            for y in 0..h {
                let border = x == 0 || y == 0 || x == w - 1 || y == h - 1;
                let band = (28..=35).contains(&y)
                    && (10..=53).contains(&x)
                    && !(20..=25).contains(&x)
                    && !(38..=43).contains(&x);
                if border || band {
                    terrain[(y * w + x) as usize] = H_VOID;
                    build[(y * w + x) as usize] = false;
                }
            }
        }
        let starts = [(10, 10), (53, 53)];
        let mut nodes = Vec::new();
        for (si, &(sx, sy)) in starts.iter().enumerate() {
            let dir = if si == 0 { -1 } else { 1 };
            for k in 0..6i32 {
                nodes.push(ResNode {
                    tx: sx + dir * 4 + (k % 3) * dir,
                    ty: sy + dir * 3 + (k / 3) * dir * 2 - dir * 1,
                    kind: ResKind::Alloy,
                    amount: 20000,
                    taken_by: 0,
                });
            }
            nodes.push(ResNode {
                tx: sx - dir * 2,
                ty: sy + dir * 5,
                kind: ResKind::Flux,
                amount: 20000,
                taken_by: 0,
            });
        }
        let mut m = GameMap {
            w,
            h,
            terrain,
            ramp_lo: vec![255; n],
            bridge: vec![0; n],
            build,
            struct_top: vec![0; n],
            nodes,
            starts,
        };
        let coords: Vec<(i32, i32)> = m.nodes.iter().map(|nd| (nd.tx, nd.ty)).collect();
        for (x, y) in coords {
            let i = m.idx(x, y);
            m.build[i] = false;
        }
        m
    }

    /// MapDef v2: `height` digits 0-5 / '#'; optional `ramps` layer
    /// ('.' or digit k = ramp k<->k+1, tile terrain must equal k);
    /// optional `bridges` layer ('.' or deck level 2-5).
    pub fn from_def(def: &echelon_data::MapDef) -> Result<GameMap, String> {
        let h = def.height.len() as i32;
        if h == 0 {
            return Err("empty height layer".into());
        }
        let w = def.height[0].chars().count() as i32;
        let n = (w * h) as usize;
        let dims = |layer: &Vec<String>, name: &str| -> Result<(), String> {
            if layer.is_empty() {
                return Ok(());
            }
            if layer.len() as i32 != h {
                return Err(format!("{} row count mismatch", name));
            }
            for (y, r) in layer.iter().enumerate() {
                if r.chars().count() as i32 != w {
                    return Err(format!("{} row {} width mismatch", name, y));
                }
            }
            Ok(())
        };
        dims(&def.things, "things")?;
        dims(&def.ramps, "ramps")?;
        dims(&def.bridges, "bridges")?;
        if def.things.len() as i32 != h {
            return Err("things layer required".into());
        }

        let mut terrain = vec![H_VOID; n];
        let mut build = vec![false; n];
        for (y, row) in def.height.iter().enumerate() {
            if row.chars().count() as i32 != w {
                return Err(format!("height row {} width mismatch", y));
            }
            for (x, c) in row.chars().enumerate() {
                let i = y * w as usize + x;
                match c {
                    '0'..='5' => {
                        terrain[i] = c as u8 - b'0';
                        build[i] = true;
                    }
                    '#' => {}
                    _ => return Err(format!("bad height char {:?} at {},{}", c, x, y)),
                }
            }
        }
        let mut ramp_lo = vec![255u8; n];
        for (y, row) in def.ramps.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                let i = y * w as usize + x;
                match c {
                    '.' | ' ' => {}
                    '0'..='4' => {
                        let k = c as u8 - b'0';
                        if terrain[i] != k {
                            return Err(format!(
                                "ramp {} on terrain {} at {},{}",
                                k, terrain[i], x, y
                            ));
                        }
                        ramp_lo[i] = k;
                        build[i] = false;
                    }
                    _ => return Err(format!("bad ramp char {:?} at {},{}", c, x, y)),
                }
            }
        }
        let mut bridge = vec![0u8; n];
        for (y, row) in def.bridges.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                let i = y * w as usize + x;
                match c {
                    '.' | ' ' => {}
                    '2'..='5' => {
                        let b = c as u8 - b'0';
                        if terrain[i] != H_VOID && terrain[i] >= b {
                            return Err(format!("bridge {} not above terrain at {},{}", b, x, y));
                        }
                        bridge[i] = b;
                        build[i] = false; // no building on or under decks
                    }
                    _ => return Err(format!("bad bridge char {:?} at {},{}", c, x, y)),
                }
            }
        }
        let mut nodes = Vec::new();
        let mut starts: Vec<(i32, i32)> = Vec::new();
        for (y, row) in def.things.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                let i = y * w as usize + x;
                let (tx, ty) = (x as i32, y as i32);
                match c {
                    '.' | ' ' => {}
                    'a' | 'f' => {
                        nodes.push(ResNode {
                            tx,
                            ty,
                            kind: if c == 'a' { ResKind::Alloy } else { ResKind::Flux },
                            amount: if c == 'a' {
                                def.alloy_amount
                            } else {
                                def.flux_amount
                            },
                            taken_by: 0,
                        });
                        build[i] = false;
                    }
                    '1' => starts.insert(0, (tx, ty)),
                    '2' => starts.push((tx, ty)),
                    _ => return Err(format!("bad things char {:?} at {},{}", c, x, y)),
                }
            }
        }
        if starts.len() != 2 {
            return Err(format!("need exactly 2 starts, found {}", starts.len()));
        }
        Ok(GameMap {
            w,
            h,
            terrain,
            ramp_lo,
            bridge,
            build,
            struct_top: vec![0; n],
            nodes,
            starts: [starts[0], starts[1]],
        })
    }

    // ---------------- pathing ----------------

    /// ground A* over (tile, surface) nodes for a unit of vertical `depth`.
    /// Returns waypoints (x, y, surf). Empty = unreachable (after a
    /// closest-expanded fallback).
    pub fn astar(
        &self,
        from: (i32, i32),
        from_surf: u8,
        to: (i32, i32),
        depth: u8,
        limit: usize,
    ) -> Vec<(i32, i32, u8)> {
        let n2 = (self.w * self.h) as usize * 2;
        let nid = |x: i32, y: i32, s: u8| (self.idx(x, y) * 2 + s as usize);
        let goal = to;
        let mut best = vec![i32::MAX; n2];
        let mut came: Vec<i32> = vec![-1; n2];
        let mut heap: BinaryHeap<Reverse<(i32, u32, i32, i32, u8)>> = BinaryHeap::new();
        let hf = |x: i32, y: i32| (x - goal.0).abs() + (y - goal.1).abs();
        let mut seq: u32 = 0;
        let start_surf = if self.passable(from.0, from.1, from_surf, depth) {
            from_surf
        } else if self.passable(from.0, from.1, SURF_GROUND, depth) {
            SURF_GROUND
        } else {
            from_surf
        };
        best[nid(from.0, from.1, start_surf)] = 0;
        heap.push(Reverse((hf(from.0, from.1), seq, from.0, from.1, start_surf)));
        let mut expanded = 0usize;
        let mut found: Option<(i32, i32, u8)> = None;
        while let Some(Reverse((_, _, x, y, s))) = heap.pop() {
            if (x, y) == goal {
                found = Some((x, y, s));
                break;
            }
            expanded += 1;
            if expanded > limit {
                break;
            }
            let g = best[nid(x, y, s)];
            const D: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
            for (dx, dy) in D {
                let (nx, ny) = (x + dx, y + dy);
                if !self.in_bounds(nx, ny) {
                    continue;
                }
                for ns in [SURF_GROUND, SURF_DECK] {
                    let ok = if (nx, ny) == goal && ns == SURF_GROUND {
                        // goal exception: may enter an occupied goal tile
                        // (attacking a structure) if the levels connect
                        self.surf_level(nx, ny, ns).map_or(false, |lb| {
                            self.surf_level(x, y, s).map_or(false, |la| {
                                la == lb
                                    || ((la as i32 - lb as i32).abs() == 1
                                        && (self.ramp(x, y).is_some()
                                            || self.ramp(nx, ny).is_some()))
                            })
                        })
                    } else {
                        self.step_ok((x, y), s, (nx, ny), ns, depth)
                    };
                    if !ok {
                        continue;
                    }
                    let ni = nid(nx, ny, ns);
                    if g + 1 < best[ni] {
                        best[ni] = g + 1;
                        came[ni] = nid(x, y, s) as i32;
                        seq += 1;
                        heap.push(Reverse((g + 1 + hf(nx, ny), seq, nx, ny, ns)));
                    }
                }
            }
        }
        let target = match found {
            Some(t) => t,
            None => {
                let mut bi = -1i32;
                let mut bh = i32::MAX;
                for i in 0..n2 {
                    if best[i] != i32::MAX {
                        let t = i / 2;
                        let (x, y) = ((t as i32) % self.w, (t as i32) / self.w);
                        let hh = hf(x, y);
                        if hh < bh {
                            bh = hh;
                            bi = i as i32;
                        }
                    }
                }
                if bi < 0 {
                    return Vec::new();
                }
                let t = bi as usize / 2;
                let s = (bi as usize % 2) as u8;
                let (gx, gy) = ((t as i32) % self.w, (t as i32) / self.w);
                if (gx, gy) == from {
                    return Vec::new();
                }
                (gx, gy, s)
            }
        };
        let mut path = vec![target];
        let mut cur = nid(target.0, target.1, target.2);
        while came[cur] >= 0 {
            cur = came[cur] as usize;
            let t = cur / 2;
            let s = (cur % 2) as u8;
            let pt = ((t as i32) % self.w, (t as i32) / self.w);
            if pt == from {
                break;
            }
            path.push((pt.0, pt.1, s));
        }
        path.reverse();
        path
    }

    /// the flight floor over a tile: a flyer's hull base can never sit
    /// below open air (AIR0) nor inside a structure poking into the sky.
    pub fn air_floor(&self, x: i32, y: i32) -> u8 {
        AIR0.max(self.struct_top(x, y))
    }
    /// can a flyer of vertical `depth` occupy any altitude over this tile?
    /// (its base must fit between the floor and the ceiling)
    pub fn air_passable(&self, x: i32, y: i32, depth: u8) -> bool {
        self.in_bounds(x, y) && self.air_floor(x, y) + depth <= TOP_LEVEL + 1
    }
    /// the altitude a flyer actually rides at over this tile: its
    /// preferred band, pushed up by whatever it must clear, capped so the
    /// hull top stays within the sky.
    pub fn air_level(&self, x: i32, y: i32, pref: u8, depth: u8) -> u8 {
        let cap = (TOP_LEVEL + 1).saturating_sub(depth);
        self.air_floor(x, y).max(pref).min(cap)
    }

    /// air pathing as pseudo-ground: the flyer terrain-follows the
    /// structure skyline, climbing bands freely; only tiles whose floor
    /// leaves no room for the hull (tall structure vs deep ship) block.
    pub fn astar_air(
        &self,
        from: (i32, i32),
        to: (i32, i32),
        depth: u8,
        limit: usize,
    ) -> Vec<(i32, i32, u8)> {
        let clear = |x: i32, y: i32| self.air_passable(x, y, depth);
        // fast path: straight line if every tile on the segment is clear
        let mut ok = true;
        let steps = (to.0 - from.0).abs().max((to.1 - from.1).abs()).max(1);
        for i in 0..=steps {
            let x = from.0 + (to.0 - from.0) * i / steps;
            let y = from.1 + (to.1 - from.1) * i / steps;
            if !clear(x, y) {
                ok = false;
                break;
            }
        }
        if ok {
            return vec![(to.0, to.1, SURF_GROUND)];
        }
        let n = (self.w * self.h) as usize;
        let mut best = vec![i32::MAX; n];
        let mut came: Vec<i32> = vec![-1; n];
        let mut heap: BinaryHeap<Reverse<(i32, u32, i32, i32)>> = BinaryHeap::new();
        let hf = |x: i32, y: i32| (x - to.0).abs() + (y - to.1).abs();
        let mut seq = 0u32;
        best[self.idx(from.0, from.1)] = 0;
        heap.push(Reverse((hf(from.0, from.1), 0, from.0, from.1)));
        let mut expanded = 0usize;
        let mut reached = false;
        while let Some(Reverse((_, _, x, y))) = heap.pop() {
            if (x, y) == to {
                reached = true;
                break;
            }
            expanded += 1;
            if expanded > limit {
                break;
            }
            let g = best[self.idx(x, y)];
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x + dx, y + dy);
                let pass = clear(nx, ny) || (nx, ny) == to;
                if !pass || !self.in_bounds(nx, ny) {
                    continue;
                }
                let ni = self.idx(nx, ny);
                if g + 1 < best[ni] {
                    best[ni] = g + 1;
                    came[ni] = self.idx(x, y) as i32;
                    seq += 1;
                    heap.push(Reverse((g + 1 + hf(nx, ny), seq, nx, ny)));
                }
            }
        }
        if !reached {
            return vec![(to.0, to.1, SURF_GROUND)];
        }
        let mut path = vec![(to.0, to.1, SURF_GROUND)];
        let mut cur = self.idx(to.0, to.1);
        while came[cur] >= 0 {
            cur = came[cur] as usize;
            let pt = ((cur as i32) % self.w, (cur as i32) / self.w);
            if pt == from {
                break;
            }
            path.push((pt.0, pt.1, SURF_GROUND));
        }
        path.reverse();
        path
    }

    pub fn find_build_spot(&self, center: (i32, i32), size: i32) -> Option<(i32, i32)> {
        for r in 2..14i32 {
            for dx in -r..=r {
                for dy in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let (tx, ty) = (center.0 + dx, center.1 + dy);
                    if self.area_buildable(tx, ty, size) {
                        return Some((tx, ty));
                    }
                }
            }
        }
        None
    }

    pub fn node_pos(&self, i: u32) -> V2 {
        let n = &self.nodes[i as usize];
        V2::from_tile(n.tx, n.ty)
    }
}
