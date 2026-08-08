//! The tick-locked world. System order inside `step()` is part of the
//! determinism contract — never reorder without bumping the data hash story.

use crate::cmd::{Cmd, PlayerCmd};
use crate::fx::{self, Fx, V2};
use crate::map::{GameMap, ResKind, AIR0, SURF_GROUND};
use crate::rng::Pcg;
use echelon_data::{AbilKind, AbilTarget, AbilityDef, BuildStyle, DataPack, DefId, Fnv, Kind, Mover, Op};

const SHIELD_DELAY: i32 = 157; // ~7s
const HARVEST_TICKS: i32 = 40;
const CARRY_ALLOY: i32 = 5;
const CARRY_FLUX: i32 = 4;
const QUEUE_CAP: usize = 6;

#[derive(Clone, Debug, PartialEq)]
pub enum Order {
    Idle,
    Move(V2),
    AttackMove(V2),
    /// stand and fight: fire in place, never chase
    Hold,
    HarvestGo(u32),
    HarvestReturn(u32),
    BuildGo(u32),
    Constructing(u32),
}

#[derive(Clone, Debug)]
pub struct QItem {
    pub def: DefId,
    pub left: i32,
}

#[derive(Clone, Debug)]
pub struct Ent {
    pub alive: bool,
    pub def: DefId,
    pub player: u8,
    pub pos: V2,
    pub hp: Fx,
    pub sh: Fx,
    pub energy: Fx,
    pub cd: i32,
    pub sh_delay: i32,
    pub order: Order,
    pub path: Vec<(i32, i32, u8)>,
    pub path_i: usize,
    pub queue: Vec<QItem>,
    pub spawn_pool: i32,
    pub spawn_t: i32,
    /// uploader -> facility index+1
    pub linked_to: u32,
    pub carry: i32,
    pub carry_flux: bool,
    /// remaining construction ticks (structures); 0 = completed
    pub progress: i32,
    /// site -> builder index+1 (Attend style)
    pub builder: u32,
    pub target: u32,
    /// ground unit currently traversing a bridge deck
    pub on_deck: bool,
    // ---- ability state (all hashed) ----
    pub stun_t: i32,
    pub cloak_t: i32,
    pub spd_pct: i32,
    pub spd_t: i32,
    pub atk_pct: i32,
    pub atk_t: i32,
    pub dtk_pct: i32,
    pub dtk_t: i32,
    pub regen_hps: Fx,
    pub regen_t: i32,
    pub dot_dps: Fx,
    pub dot_t: i32,
    /// timed life in ticks; 0 = permanent
    pub life_t: i32,
    pub abil_cd: [i32; 3],
    /// production rally point in milli-tiles; (0,0) = unset
    pub rally: (i32, i32),
}

/// per-def precomputed runtime numbers (all Fx / ticks)
struct DefRt {
    speed: Fx,   // per tick
    sight: Fx,
    range: Fx,
    splash: Fx,
    hp: Fx,
    sh: Fx,
    energy: Fx,
    attrs: u16,
    build_ticks: i32,
}

pub struct World {
    pub pack: DataPack,
    pub map: GameMap,
    pub ents: Vec<Ent>,
    pub tick: u32,
    /// per player: (alloy, flux)
    pub res: [(i32, i32); 2],
    pub used2: [i32; 2],
    pub cap2: [i32; 2],
    pub races: [usize; 2],
    pub rng: Pcg,
    pub defeated: [bool; 2],
    pub events: Vec<String>,
    rt: Vec<DefRt>,
    life_regen: [Fx; 2], // per tick, by player
    sh_regen: Fx,
    en_regen: Fx,
}

struct Snap {
    alive: bool,
    player: u8,
    pos: V2,
    air: bool,
    structure: bool,
    /// visible to player [0]/[1] (cloak vs detectors)
    vis: [bool; 2],
}

impl World {
    pub fn new(pack: DataPack, map: GameMap, races: [usize; 2], seed: u64) -> World {
        let rt = pack
            .defs
            .iter()
            .map(|(_, d)| DefRt {
                speed: Fx::from_milli(d.speed_milli()).div_int(echelon_data::TICKS_PER_SEC),
                sight: Fx::from_milli(d.sight_milli()),
                range: d
                    .wpn
                    .as_ref()
                    .map(|w| Fx::from_milli(w.range_m))
                    .unwrap_or(fx::ZERO),
                splash: d
                    .wpn
                    .as_ref()
                    .map(|w| Fx::from_milli(w.splash_m))
                    .unwrap_or(fx::ZERO),
                hp: Fx::from_int(d.hp),
                sh: Fx::from_int(d.sh),
                energy: Fx::from_int(d.energy),
                attrs: d.attr_mask(),
                build_ticks: d.build_ticks(),
            })
            .collect();
        let life_regen = [
            Fx::from_milli(pack.races[races[0]].life_regen_m)
                .div_int(echelon_data::TICKS_PER_SEC),
            Fx::from_milli(pack.races[races[1]].life_regen_m)
                .div_int(echelon_data::TICKS_PER_SEC),
        ];
        let mut w = World {
            pack,
            map,
            ents: Vec::new(),
            tick: 0,
            res: [(50, 0), (50, 0)],
            used2: [0, 0],
            cap2: [0, 0],
            races,
            rng: Pcg::new(seed),
            defeated: [false, false],
            events: Vec::new(),
            rt,
            life_regen,
            sh_regen: Fx::from_milli(2000).div_int(echelon_data::TICKS_PER_SEC),
            en_regen: Fx::from_milli(563).div_int(echelon_data::TICKS_PER_SEC),
        };
        w.setup();
        w
    }

    fn worker_def(&self, p: usize) -> DefId {
        let race = self.races[p];
        for (i, (ri, d)) in self.pack.defs.iter().enumerate() {
            if *ri == race && d.kind == Kind::Worker {
                return i as DefId;
            }
        }
        panic!("race {} has no worker", race)
    }
    fn townhall_def(&self, p: usize) -> DefId {
        let race = self.races[p];
        for (i, (ri, d)) in self.pack.defs.iter().enumerate() {
            if *ri == race && d.townhall {
                return i as DefId;
            }
        }
        panic!("race {} has no townhall", race)
    }

    fn setup(&mut self) {
        for p in 0..2usize {
            let (sx, sy) = self.map.starts[p];
            let hall = self.townhall_def(p);
            let hi = self.place_structure(hall, p as u8, sx - 1, sy - 1, true);
            let wd = self.worker_def(p);
            // 12 workers on the alloy line
            let my_nodes: Vec<u32> = self
                .map
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    n.kind == ResKind::Alloy
                        && (n.tx - sx).abs() + (n.ty - sy).abs() < 20
                })
                .map(|(i, _)| i as u32)
                .collect();
            for k in 0..12 {
                let pos = V2::from_tile(sx + (k % 4) - 2, sy + 2 + (k / 4) * (if p == 0 { 1 } else { -1 }));
                let wi = self.spawn_unit(wd, p as u8, pos);
                let node = my_nodes[(k as usize) % my_nodes.len()];
                self.ents[wi as usize].order = Order::HarvestGo(node);
            }
            let _ = hi;
        }
    }

    // ---------- spawning / placement ----------

    fn alloc(&mut self, e: Ent) -> u32 {
        self.ents.push(e);
        (self.ents.len() - 1) as u32
    }

    pub fn spawn_unit(&mut self, def: DefId, player: u8, pos: V2) -> u32 {
        let rt = &self.rt[def as usize];
        let e = Ent {
            alive: true,
            def,
            player,
            pos,
            hp: rt.hp,
            sh: rt.sh,
            energy: rt.energy.div_int(2),
            cd: 0,
            sh_delay: 0,
            order: Order::Idle,
            path: Vec::new(),
            path_i: 0,
            queue: Vec::new(),
            spawn_pool: 0,
            spawn_t: 0,
            linked_to: 0,
            carry: 0,
            carry_flux: false,
            progress: 0,
            builder: 0,
            target: 0,
            on_deck: false,
            stun_t: 0,
            cloak_t: 0,
            spd_pct: 1000,
            spd_t: 0,
            atk_pct: 1000,
            atk_t: 0,
            dtk_pct: 1000,
            dtk_t: 0,
            regen_hps: fx::ZERO,
            regen_t: 0,
            dot_dps: fx::ZERO,
            dot_t: 0,
            life_t: 0,
            abil_cd: [0; 3],
            rally: (0, 0),
        };
        self.used2[player as usize] += self.pack.def(def).supply2;
        self.alloc(e)
    }

    /// place a structure with top-left tile (tx,ty). `completed` skips
    /// construction (initial townhalls).
    pub fn place_structure(
        &mut self,
        def: DefId,
        player: u8,
        tx: i32,
        ty: i32,
        completed: bool,
    ) -> u32 {
        let d = self.pack.def(def).clone();
        let fp = d.fp();
        for x in tx..tx + fp {
            for y in ty..ty + fp {
                let base = self.map.hgt(x, y);
                let top = if base == crate::map::H_VOID {
                    d.depth_eff()
                } else {
                    base.saturating_add(d.depth_eff())
                };
                self.map.set_struct(x, y, top);
            }
        }
        let rt = &self.rt[def as usize];
        let center = V2::new(
            Fx::from_int(tx).add(Fx::from_milli(fp * 500)),
            Fx::from_int(ty).add(Fx::from_milli(fp * 500)),
        );
        let sp = d.spawner.as_ref();
        let e = Ent {
            alive: true,
            def,
            player,
            pos: center,
            hp: rt.hp,
            sh: rt.sh,
            energy: fx::ZERO,
            cd: 0,
            sh_delay: 0,
            order: Order::Idle,
            path: Vec::new(),
            path_i: 0,
            queue: Vec::new(),
            spawn_pool: if completed { sp.map(|s| s.max).unwrap_or(0) } else { 0 },
            spawn_t: sp.map(|s| s.regen_s * echelon_data::TICKS_PER_SEC).unwrap_or(0),
            linked_to: 0,
            carry: 0,
            carry_flux: false,
            progress: if completed { 0 } else { rt.build_ticks },
            builder: 0,
            target: 0,
            on_deck: false,
            stun_t: 0,
            cloak_t: 0,
            spd_pct: 1000,
            spd_t: 0,
            atk_pct: 1000,
            atk_t: 0,
            dtk_pct: 1000,
            dtk_t: 0,
            regen_hps: fx::ZERO,
            regen_t: 0,
            dot_dps: fx::ZERO,
            dot_t: 0,
            life_t: 0,
            abil_cd: [0; 3],
            rally: (0, 0),
        };
        let i = self.alloc(e);
        if completed {
            self.cap2[player as usize] += d.provides2;
        }
        // claim geyser
        if d.gas {
            let (cx, cy) = (tx + fp / 2, ty + fp / 2);
            for n in self.map.nodes.iter_mut() {
                if n.kind == ResKind::Flux && n.tx == cx && n.ty == cy {
                    n.taken_by = i + 1;
                }
            }
        }
        i
    }

    // ---------- command application ----------

    pub fn apply(&mut self, cmds: &[PlayerCmd]) {
        for pc in cmds {
            let p = pc.player;
            match &pc.cmd {
                Cmd::Move { ids, mx, my } => {
                    let to = V2::from_milli(*mx, *my);
                    for &id in ids {
                        self.order_move(id, p, Order::Move(to));
                    }
                }
                Cmd::Attack { ids, mx, my } => {
                    let to = V2::from_milli(*mx, *my);
                    for &id in ids {
                        self.order_move(id, p, Order::AttackMove(to));
                    }
                }
                Cmd::Stop { ids } => {
                    for &id in ids {
                        if self.owned_unit(id, p) {
                            let e = &mut self.ents[id as usize];
                            e.order = Order::Idle;
                            e.path.clear();
                            e.target = 0;
                        }
                    }
                }
                Cmd::Harvest { id, node } => {
                    if self.owned_unit(*id, p)
                        && (*node as usize) < self.map.nodes.len()
                        && self.pack.def(self.ents[*id as usize].def).kind == Kind::Worker
                    {
                        let e = &mut self.ents[*id as usize];
                        e.order = if e.carry > 0 {
                            Order::HarvestReturn(*node)
                        } else {
                            Order::HarvestGo(*node)
                        };
                        e.path.clear();
                    }
                }
                Cmd::Train { structure, unit } => self.try_train(p, *structure, *unit),
                Cmd::Build {
                    worker,
                    unit,
                    tx,
                    ty,
                } => self.try_build(p, *worker, *unit, *tx, *ty),
                Cmd::Hold { ids } => {
                    for &id in ids {
                        if self.owned_unit(id, p)
                            && self.pack.def(self.ents[id as usize].def).kind != Kind::Structure
                        {
                            let e = &mut self.ents[id as usize];
                            e.order = Order::Hold;
                            e.path.clear();
                            e.path_i = 0;
                            e.target = 0;
                        }
                    }
                }
                Cmd::Rally { structure, mx, my } => {
                    if self.owned_unit(*structure, p)
                        && self.pack.def(self.ents[*structure as usize].def).kind
                            == Kind::Structure
                    {
                        self.ents[*structure as usize].rally = (*mx, *my);
                    }
                }
                Cmd::Cast {
                    caster,
                    slot,
                    target,
                    mx,
                    my,
                } => self.try_cast(p, *caster, *slot, *target, *mx, *my),
                Cmd::Link { uploader, facility } => {
                    if self.owned_unit(*uploader, p)
                        && self.owned_unit(*facility, p)
                        && self.pack.def(self.ents[*uploader as usize].def).uploader
                        && !self.pack.def(self.ents[*facility as usize].def).trains.is_empty()
                        && self.ents[*facility as usize].progress == 0
                    {
                        self.ents[*uploader as usize].linked_to = facility + 1;
                    }
                }
            }
        }
    }

    fn owned_unit(&self, id: u32, p: u8) -> bool {
        (id as usize) < self.ents.len()
            && self.ents[id as usize].alive
            && self.ents[id as usize].player == p
    }

    fn order_move(&mut self, id: u32, p: u8, o: Order) {
        if !self.owned_unit(id, p) {
            return;
        }
        let def = self.ents[id as usize].def;
        let d = self.pack.def(def);
        if d.kind == Kind::Structure {
            return;
        }
        let dest = match o {
            Order::Move(v) | Order::AttackMove(v) => v,
            _ => return,
        };
        let from = self.ents[id as usize].pos.tile();
        // idempotent orders: if we already hold a live path to this same
        // destination tile, keep walking it — terminal-phase bots reissue
        // Attack constantly and re-planning 100 units per cadence is the
        // sim's single hottest cost
        {
            let e = &self.ents[id as usize];
            if e.path_i + 2 < e.path.len() {
                if let Some(last) = e.path.last() {
                    if (last.0, last.1) == dest.tile() {
                        return;
                    }
                }
            }
        }
        let path = if d.mover() == Mover::Air {
            self.map.astar_air(from, dest.tile(), d.depth_eff(), 6000)
        } else {
            let fs = if self.ents[id as usize].on_deck { 1 } else { SURF_GROUND };
            self.map.astar(from, fs, dest.tile(), d.depth_eff(), 8000)
        };
        let e = &mut self.ents[id as usize];
        e.order = o;
        e.path = path;
        e.path_i = 0;
        e.target = 0;
    }

    fn count_linked(&self, facility: u32) -> i32 {
        let mut n = 0;
        for e in &self.ents {
            if e.alive
                && e.linked_to == facility + 1
                && e.progress == 0
                && self.pack.def(e.def).uploader
            {
                n += 1;
            }
        }
        n
    }

    fn reqs_met(&self, p: u8, facility: u32, unit: DefId) -> bool {
        for r in &self.pack.def(unit).requires {
            if let Some(nstr) = r.strip_prefix("@uplink") {
                if facility == u32::MAX {
                    return false;
                }
                let need: i32 = nstr.parse().unwrap_or(1);
                if self.count_linked(facility) < need {
                    return false;
                }
            } else {
                let rid = self.pack.id(r);
                let ok = self.ents.iter().any(|e| {
                    e.alive && e.player == p && e.def == rid && e.progress == 0
                });
                if !ok {
                    return false;
                }
            }
        }
        true
    }

    fn try_train(&mut self, p: u8, structure: u32, unit: DefId) {
        if !self.owned_unit(structure, p) {
            return;
        }
        let sdef = self.ents[structure as usize].def;
        let sd = self.pack.def(sdef).clone();
        if self.ents[structure as usize].progress != 0 {
            return;
        }
        if !sd.trains.iter().any(|n| self.pack.id(n) == unit) {
            return;
        }
        if self.ents[structure as usize].queue.len() >= QUEUE_CAP {
            return;
        }
        if !self.reqs_met(p, structure, unit) {
            return;
        }
        let ud = self.pack.def(unit).clone();
        let made = if ud.twin { 2 } else { 1 };
        let pi = p as usize;
        if ud.kind != Kind::Structure
            && self.used2[pi] + ud.supply2 * made > self.cap2[pi]
        {
            return;
        }
        if self.res[pi].0 < ud.cost.0 || self.res[pi].1 < ud.cost.1 {
            return;
        }
        if let Some(stream) = &ud.consumes {
            let has = match &sd.spawner {
                Some(s) => &s.kind == stream && self.ents[structure as usize].spawn_pool > 0,
                None => false,
            };
            if !has {
                return;
            }
            self.ents[structure as usize].spawn_pool -= 1;
        }
        self.res[pi].0 -= ud.cost.0;
        self.res[pi].1 -= ud.cost.1;
        self.used2[pi] += ud.supply2 * made;
        self.ents[structure as usize].queue.push(QItem {
            def: unit,
            left: ud.build_ticks(),
        });
    }

    fn try_build(&mut self, p: u8, worker: u32, unit: DefId, tx: i32, ty: i32) {
        if !self.owned_unit(worker, p) {
            return;
        }
        let wdef = self.ents[worker as usize].def;
        if self.pack.def(wdef).kind != Kind::Worker {
            return;
        }
        if !self.pack.def(wdef).builds.iter().any(|n| self.pack.id(n) == unit) {
            return;
        }
        let ud = self.pack.def(unit).clone();
        let fp = ud.fp();
        let pi = p as usize;
        if self.res[pi].0 < ud.cost.0 || self.res[pi].1 < ud.cost.1 {
            return;
        }
        if !self.reqs_met(p, u32::MAX, unit) {
            return;
        }
        if ud.gas {
            let (cx, cy) = (tx + fp / 2, ty + fp / 2);
            let ok = self
                .map
                .nodes
                .iter()
                .any(|n| n.kind == ResKind::Flux && n.tx == cx && n.ty == cy && n.taken_by == 0);
            if !ok || !self.map.in_bounds(tx, ty) || !self.map.in_bounds(tx + fp - 1, ty + fp - 1)
            {
                return;
            }
        } else if !self.map.area_buildable(tx, ty, fp) {
            return;
        }
        // never build on top of standing units (the builder may — it
        // steps aside / is consumed); enclosure-by-construction is how
        // armies get bricked into pockets
        for (i, e) in self.ents.iter().enumerate() {
            if i as u32 == worker || !e.alive || e.def == u16::MAX {
                continue;
            }
            let d = self.pack.def(e.def);
            if d.kind == Kind::Structure || d.mover() == Mover::Air {
                continue;
            }
            let (ex, ey) = e.pos.tile();
            if ex >= tx && ex < tx + fp && ey >= ty && ey < ty + fp {
                return;
            }
        }
        self.res[pi].0 -= ud.cost.0;
        self.res[pi].1 -= ud.cost.1;
        let site = self.place_structure(unit, p, tx, ty, false);
        let style = self.pack.races[self.races[pi]].build_style;
        match style {
            BuildStyle::Attend => {
                self.ents[site as usize].builder = worker + 1;
                let w = &mut self.ents[worker as usize];
                w.order = Order::BuildGo(site);
                w.path.clear();
                w.carry = 0;
            }
            BuildStyle::Auto => {
                let w = &mut self.ents[worker as usize];
                if w.order == Order::Idle {
                } else {
                    w.order = Order::Idle;
                    w.path.clear();
                }
            }
            BuildStyle::Consume => {
                self.kill(worker);
            }
        }
    }

    fn ability(&self, def: DefId, slot: u8) -> Option<&AbilityDef> {
        let aid = self.pack.def(def).abil.get(slot as usize)?;
        let ix = *self.pack.abil_by_id.get(aid)?;
        Some(&self.pack.abilities[ix as usize])
    }

    fn try_cast(&mut self, p: u8, caster: u32, slot: u8, target: u32, mx: i32, my: i32) {
        if !self.owned_unit(caster, p) || self.ents[caster as usize].stun_t > 0 {
            return;
        }
        let cdef = self.ents[caster as usize].def;
        let a = match self.ability(cdef, slot) {
            Some(a) if a.kind == AbilKind::Active => a.clone(),
            _ => return,
        };
        if self.ents[caster as usize].abil_cd[slot as usize] > 0 {
            return;
        }
        if self.ents[caster as usize].energy < Fx::from_int(a.energy) {
            return;
        }
        let cpos = self.ents[caster as usize].pos;
        let (primary, point) = match a.target {
            AbilTarget::SelfT => (caster + 1, cpos),
            AbilTarget::Point => (0, V2::from_milli(mx, my)),
            AbilTarget::Ally | AbilTarget::Enemy => {
                if (target as usize) >= self.ents.len() || !self.ents[target as usize].alive {
                    return;
                }
                let te = &self.ents[target as usize];
                let ally = te.player == p;
                if (a.target == AbilTarget::Ally) != ally {
                    return;
                }
                (target + 1, te.pos)
            }
        };
        if a.range_m > 0
            && fx::dist2(cpos, point) > fx::range2(Fx::from_milli(a.range_m + 500))
        {
            return;
        }
        let e = &mut self.ents[caster as usize];
        e.energy = e.energy.sub(Fx::from_int(a.energy));
        e.abil_cd[slot as usize] = a.cd_s * echelon_data::TICKS_PER_SEC;
        self.exec_ops(caster as usize, primary, point, &a.ops);
    }

    fn buff_targets(&self, caster: usize, primary: u32, point: V2, r_m: i32, allies: bool) -> Vec<usize> {
        if r_m == 0 {
            let t = if primary > 0 { primary as usize - 1 } else { caster };
            return vec![t];
        }
        let me = self.ents[caster].player;
        let r2 = fx::range2(Fx::from_milli(r_m));
        self.ents
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.alive
                    && e.def != u16::MAX
                    && (e.player == me) == allies
                    && fx::dist2(e.pos, point) <= r2
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn exec_ops(&mut self, caster: usize, primary: u32, point: V2, ops: &[Op]) {
        let me = self.ents[caster].player;
        for op in ops {
            match op {
                Op::Heal(v) => {
                    let t = if primary > 0 { primary as usize - 1 } else { caster };
                    let max = self.rt[self.ents[t].def as usize].hp;
                    let e = &mut self.ents[t];
                    e.hp = e.hp.add(Fx::from_int(*v)).min(max);
                }
                Op::ShieldGive(v) => {
                    let t = if primary > 0 { primary as usize - 1 } else { caster };
                    let max = self.rt[self.ents[t].def as usize].sh;
                    let e = &mut self.ents[t];
                    e.sh = e.sh.add(Fx::from_int(*v)).min(max);
                }
                Op::EnergyGive(v) => {
                    let t = if primary > 0 { primary as usize - 1 } else { caster };
                    let max = self.rt[self.ents[t].def as usize].energy;
                    let e = &mut self.ents[t];
                    e.energy = e.energy.add(Fx::from_int(*v)).min(max);
                }
                Op::SelfDamage(v) => {
                    let e = &mut self.ents[caster];
                    e.hp = e.hp.sub(Fx::from_int(*v));
                    if !e.hp.is_pos() {
                        e.alive = false;
                    }
                }
                Op::KillSelf => self.kill(caster as u32),
                Op::AreaDamage { r_m, dmg, bonus, vs } => {
                    let ts: Vec<usize> = if *r_m == 0 && primary > 0 {
                        vec![primary as usize - 1]
                    } else {
                        self.buff_targets(caster, 0, point, (*r_m).max(1), false)
                    };
                    for t in ts {
                        self.hit(t, Fx::from_int(*dmg), *bonus, *vs);
                    }
                }
                Op::ChainDamage { dmg, jumps, r_m } => {
                    let mut cur = if primary > 0 { primary as usize - 1 } else { continue };
                    let mut hitset = vec![cur];
                    self.hit(cur, Fx::from_int(*dmg), 0, None);
                    for _ in 0..*jumps {
                        let cp = self.ents[cur].pos;
                        let r2 = fx::range2(Fx::from_milli(*r_m));
                        let mut best: (i128, i32) = (i128::MAX, -1);
                        for (j, e) in self.ents.iter().enumerate() {
                            if e.alive
                                && e.def != u16::MAX
                                && e.player != me
                                && !hitset.contains(&j)
                            {
                                let d2 = fx::dist2(cp, e.pos);
                                if d2 <= r2 && d2 < best.0 {
                                    best = (d2, j as i32);
                                }
                            }
                        }
                        if best.1 < 0 {
                            break;
                        }
                        cur = best.1 as usize;
                        hitset.push(cur);
                        self.hit(cur, Fx::from_int(*dmg), 0, None);
                    }
                }
                Op::Dot { dps, dur_s } => {
                    let t = if primary > 0 { primary as usize - 1 } else { continue };
                    self.ents[t].dot_dps = Fx::from_int(*dps);
                    self.ents[t].dot_t = dur_s * echelon_data::TICKS_PER_SEC;
                }
                Op::Stun { r_m, dur_s } => {
                    let dur = dur_s * echelon_data::TICKS_PER_SEC;
                    for t in self.buff_targets(caster, primary, point, *r_m, false) {
                        self.ents[t].stun_t = self.ents[t].stun_t.max(dur);
                    }
                }
                Op::SpeedMul { pct, dur_s, r_m } => {
                    for t in self.buff_targets(caster, primary, point, *r_m, true) {
                        self.ents[t].spd_pct = *pct;
                        self.ents[t].spd_t = dur_s * echelon_data::TICKS_PER_SEC;
                    }
                }
                Op::AtkSpeedMul { pct, dur_s, r_m } => {
                    for t in self.buff_targets(caster, primary, point, *r_m, true) {
                        self.ents[t].atk_pct = *pct;
                        self.ents[t].atk_t = dur_s * echelon_data::TICKS_PER_SEC;
                    }
                }
                Op::DmgTakenMul { pct, dur_s, r_m } => {
                    for t in self.buff_targets(caster, primary, point, *r_m, true) {
                        self.ents[t].dtk_pct = *pct;
                        self.ents[t].dtk_t = dur_s * echelon_data::TICKS_PER_SEC;
                    }
                }
                Op::RegenHp { hps, dur_s } => {
                    let t = if primary > 0 { primary as usize - 1 } else { caster };
                    self.ents[t].regen_hps = Fx::from_int(*hps).div_int(echelon_data::TICKS_PER_SEC);
                    self.ents[t].regen_t = dur_s * echelon_data::TICKS_PER_SEC;
                }
                Op::Cloak { dur_s } => {
                    let t = if primary > 0 { primary as usize - 1 } else { caster };
                    self.ents[t].cloak_t = dur_s * echelon_data::TICKS_PER_SEC;
                }
                Op::Blink { max_m } => {
                    let cp = self.ents[caster].pos;
                    let (np, _) = fx::step_towards(cp, point, Fx::from_milli(*max_m));
                    let (tx, ty) = np.tile();
                    let d = self.pack.def(self.ents[caster].def).clone();
                    let ok = if d.mover() == Mover::Air {
                        self.map.air_passable(tx, ty, d.depth_eff())
                    } else {
                        self.map.passable(tx, ty, 0, d.depth_eff())
                    };
                    if ok {
                        let e = &mut self.ents[caster];
                        e.pos = np;
                        e.path.clear();
                        e.path_i = 0;
                    }
                }
                Op::Summon { unit, count, life_s } => {
                    let ud = self.pack.id(unit);
                    let d = self.pack.def(ud).clone();
                    for _ in 0..*count {
                        let (tx, ty) = self.free_tile_near(point.tile());
                        let idx = if d.kind == Kind::Structure {
                            let fp = d.fp();
                            self.place_structure(ud, me, tx - fp / 2, ty - fp / 2, true)
                        } else {
                            self.spawn_unit(ud, me, V2::from_tile(tx, ty))
                        };
                        self.ents[idx as usize].life_t =
                            life_s * echelon_data::TICKS_PER_SEC;
                    }
                }
            }
        }
    }

    /// autocast pass: heals/repairs/dampeners fire themselves
    fn sys_autocast(&mut self) {
        for i in 0..self.ents.len() {
            if !self.ents[i].alive
                || self.ents[i].def == u16::MAX
                || self.ents[i].stun_t > 0
                || self.ents[i].progress != 0
                || (self.tick.wrapping_add(i as u32)) % 8 != 0
            {
                continue;
            }
            let def = self.ents[i].def;
            let nab = self.pack.def(def).abil.len().min(3);
            for slot in 0..nab {
                let a = match self.ability(def, slot as u8) {
                    Some(a) if a.kind == AbilKind::Auto => a.clone(),
                    _ => continue,
                };
                if self.ents[i].abil_cd[slot] > 0
                    || self.ents[i].energy < Fx::from_int(a.energy)
                {
                    continue;
                }
                let me = self.ents[i].player;
                let cpos = self.ents[i].pos;
                let target: Option<u32> = match a.target {
                    AbilTarget::SelfT => {
                        let max = self.rt[def as usize].hp;
                        let thr = max.mul_int(a.self_hp_below).div_int(100);
                        if a.self_hp_below > 0 && self.ents[i].hp < thr {
                            Some(i as u32 + 1)
                        } else {
                            None
                        }
                    }
                    AbilTarget::Ally => {
                        let r2 = fx::range2(Fx::from_milli(a.range_m));
                        let wants_energy =
                            a.ops.iter().any(|o| matches!(o, Op::EnergyGive(_)));
                        let wants_shield =
                            a.ops.iter().any(|o| matches!(o, Op::ShieldGive(_)));
                        let mut found = None;
                        for (j, e) in self.ents.iter().enumerate() {
                            if j == i || !e.alive || e.def == u16::MAX || e.player != me {
                                continue;
                            }
                            if let Some(attr) = a.ally_attr {
                                if self.rt[e.def as usize].attrs & attr.bit() == 0 {
                                    continue;
                                }
                            }
                            let rt = &self.rt[e.def as usize];
                            let deficit = if wants_energy {
                                rt.energy.is_pos() && e.energy < rt.energy.div_int(2)
                            } else if wants_shield {
                                rt.sh.is_pos() && e.sh < rt.sh
                            } else {
                                e.hp < rt.hp
                            };
                            if deficit && fx::dist2(cpos, e.pos) <= r2 {
                                found = Some(j as u32 + 1);
                                break;
                            }
                        }
                        found
                    }
                    _ => None,
                };
                if let Some(primary) = target {
                    let point = self.ents[(primary - 1) as usize].pos;
                    let e = &mut self.ents[i];
                    e.energy = e.energy.sub(Fx::from_int(a.energy));
                    e.abil_cd[slot] = a.cd_s * echelon_data::TICKS_PER_SEC;
                    self.exec_ops(i, primary, point, &a.ops);
                    break;
                }
            }
        }
    }

    fn kill(&mut self, id: u32) {
        let e = &mut self.ents[id as usize];
        if e.alive {
            e.alive = false;
            e.hp = fx::ZERO;
        }
    }

    // ---------- systems ----------

    pub fn step(&mut self, cmds: &[PlayerCmd]) {
        self.apply(cmds);
        self.sys_production();
        self.sys_spawners();
        self.sys_construction();
        self.sys_autocast();
        self.sys_units();
        self.sys_regen();
        self.sys_cleanup();
        self.tick += 1;
    }

    fn sys_production(&mut self) {
        for i in 0..self.ents.len() {
            if !self.ents[i].alive || self.ents[i].progress != 0 {
                continue;
            }
            let def = self.ents[i].def;
            let d = self.pack.def(def);
            if d.kind != Kind::Structure || self.ents[i].queue.is_empty() {
                continue;
            }
            let slots = if d.parallel > 0 { d.parallel as usize } else { 1 };
            let mut done: Vec<DefId> = Vec::new();
            {
                let q = &mut self.ents[i].queue;
                let n = q.len().min(slots);
                for k in 0..n {
                    q[k].left -= 1;
                }
                while !q.is_empty() && q[0].left <= 0 {
                    done.push(q[0].def);
                    q.remove(0);
                }
            }
            for ud in done {
                let made = if self.pack.def(ud).twin { 2 } else { 1 };
                let around = self.ents[i].pos.tile();
                let player = self.ents[i].player;
                for _ in 0..made {
                    let spot = self.free_tile_near(around);
                    // supply is charged inside spawn_unit — nothing was
                    // pre-charged at enqueue (that phantom "undo" made
                    // every produced unit supply-free)
                    let idx = self.spawn_unit(ud, player, V2::from_tile(spot.0, spot.1));
                    // rally: fresh units walk out
                    let rally = self.ents[i].rally;
                    if rally != (0, 0) {
                        let dest = V2::from_milli(rally.0, rally.1);
                        let pl = self.ents[idx as usize].player;
                        self.order_move(idx, pl, Order::Move(dest));
                    }
                }
            }
        }
    }

    fn free_tile_near(&self, c: (i32, i32)) -> (i32, i32) {
        // prefer tiles with at least two open orthogonal neighbors so
        // spawns never land in a construction pocket; fall back to any
        // legal tile, then to the center itself
        let openness = |x: i32, y: i32| -> i32 {
            [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .filter(|(dx, dy)| self.map.walkable(x + dx, y + dy))
                .count() as i32
        };
        let mut fallback: Option<(i32, i32)> = None;
        for r in 1..10i32 {
            for dx in -r..=r {
                for dy in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let (x, y) = (c.0 + dx, c.1 + dy);
                    if self.map.spawn_ok((c.0, c.1), (x, y)) {
                        if openness(x, y) >= 2 {
                            return (x, y);
                        }
                        if fallback.is_none() {
                            fallback = Some((x, y));
                        }
                    }
                }
            }
        }
        fallback.unwrap_or(c)
    }

    fn sys_spawners(&mut self) {
        for i in 0..self.ents.len() {
            let e = &self.ents[i];
            if !e.alive || e.progress != 0 {
                continue;
            }
            let d = self.pack.def(e.def);
            let sp = match &d.spawner {
                Some(s) => s.clone(),
                None => continue,
            };
            let e = &mut self.ents[i];
            if e.spawn_pool < sp.max {
                e.spawn_t -= 1;
                if e.spawn_t <= 0 {
                    e.spawn_pool += 1;
                    e.spawn_t = sp.regen_s * echelon_data::TICKS_PER_SEC;
                }
            }
        }
    }

    fn sys_construction(&mut self) {
        for i in 0..self.ents.len() {
            if !self.ents[i].alive || self.ents[i].progress == 0 {
                continue;
            }
            let d = self.pack.def(self.ents[i].def);
            if d.kind != Kind::Structure {
                continue;
            }
            let style = self.pack.races[self.races[self.ents[i].player as usize]].build_style;
            let advance = match style {
                BuildStyle::Attend => {
                    let b = self.ents[i].builder;
                    b != 0
                        && self.ents[(b - 1) as usize].alive
                        && self.ents[(b - 1) as usize].order == Order::Constructing(i as u32)
                }
                _ => true,
            };
            if advance {
                self.ents[i].progress -= 1;
                if self.ents[i].progress == 0 {
                    let p = self.ents[i].player as usize;
                    self.cap2[p] += d.provides2;
                    // seed the spawner pool on completion
                    if let Some(s) = &d.spawner {
                        self.ents[i].spawn_pool = 1;
                        self.ents[i].spawn_t = s.regen_s * echelon_data::TICKS_PER_SEC;
                    }
                    // free an attending builder
                    let b = self.ents[i].builder;
                    if b != 0 && self.ents[(b - 1) as usize].alive {
                        let w = &mut self.ents[(b - 1) as usize];
                        if w.order == Order::Constructing(i as u32) {
                            w.order = Order::Idle;
                        }
                    }
                    self.events
                        .push(format!("t{} p{} finished {}", self.tick, p, d.name));
                }
            }
        }
    }

    fn snap(&self) -> Vec<Snap> {
        let mut out: Vec<Snap> = self
            .ents
            .iter()
            .map(|e| {
                if e.def == u16::MAX {
                    return Snap {
                        alive: false,
                        player: 0,
                        pos: e.pos,
                        air: false,
                        structure: false,
                        vis: [true, true],
                    };
                }
                let d = self.pack.def(e.def);
                Snap {
                    alive: e.alive,
                    player: e.player,
                    pos: e.pos,
                    air: d.mover() == Mover::Air,
                    structure: d.kind == Kind::Structure,
                    vis: [true, true],
                }
            })
            .collect();
        // detectors light up cloaked enemies within their sight
        let detectors: Vec<(u8, V2, Fx)> = self
            .ents
            .iter()
            .filter(|e| {
                e.alive
                    && e.def != u16::MAX
                    && e.progress == 0
                    && self.pack.def(e.def).detector
            })
            .map(|e| (e.player, e.pos, self.rt[e.def as usize].sight))
            .collect();
        for (i, e) in self.ents.iter().enumerate() {
            if e.alive && e.def != u16::MAX && e.cloak_t > 0 {
                let enemy = (1 - e.player) as usize;
                let seen = detectors.iter().any(|(dp, dpos, ds)| {
                    *dp != e.player && fx::dist2(*dpos, e.pos) <= fx::range2(*ds)
                });
                out[i].vis[enemy] = seen;
            }
        }
        out
    }

    fn sys_units(&mut self) {
        let snap = self.snap();
        let mut damage: Vec<(u32, Fx, i32, Option<echelon_data::Attr>, Fx, V2)> = Vec::new();
        // (target, dmg, bonus, bonus_vs, splash, at)
        for i in 0..self.ents.len() {
            if !self.ents[i].alive {
                continue;
            }
            let def = self.ents[i].def;
            let d = self.pack.def(def).clone();
            if d.kind == Kind::Structure {
                if self.ents[i].stun_t > 0 {
                    self.ents[i].stun_t -= 1;
                    continue;
                }
                // static defense: acquire & fire in place
                if self.ents[i].progress == 0 {
                    if let Some(w) = &d.wpn {
                        self.structure_fire(i, w, &snap, &mut damage);
                    }
                }
                continue;
            }
            if self.ents[i].cd > 0 {
                self.ents[i].cd -= 1;
            }
            if self.ents[i].stun_t > 0 {
                self.ents[i].stun_t -= 1;
                continue;
            }
            let order = self.ents[i].order.clone();
            match order {
                Order::Idle => {}
                Order::Move(_) => {
                    if self.follow_path(i) {
                        self.ents[i].order = Order::Idle;
                    }
                }
                Order::Hold => {
                    if let Some(w) = d.wpn.clone() {
                        // stand and fight: the structure fire path is
                        // exactly "acquire in range, never chase"
                        self.structure_fire(i, &w, &snap, &mut damage);
                    }
                }
                Order::AttackMove(dest) => {
                    self.attack_move(i, &d, dest, &snap, &mut damage);
                }
                Order::HarvestGo(n) => self.harvest_go(i, n),
                Order::HarvestReturn(n) => self.harvest_return(i, n),
                Order::BuildGo(site) => {
                    let alive_site = (site as usize) < self.ents.len()
                        && self.ents[site as usize].alive;
                    if !alive_site {
                        self.ents[i].order = Order::Idle;
                    } else {
                        let sp = self.ents[site as usize].pos;
                        if fx::dist2(self.ents[i].pos, sp)
                            <= fx::range2(Fx::from_milli(2200))
                        {
                            self.ents[i].order = Order::Constructing(site);
                            self.ents[i].path.clear();
                        } else {
                            self.chase_point(i, sp);
                        }
                    }
                }
                Order::Constructing(site) => {
                    let done = (site as usize) >= self.ents.len()
                        || !self.ents[site as usize].alive
                        || self.ents[site as usize].progress == 0;
                    if done {
                        self.ents[i].order = Order::Idle;
                    }
                }
            }
        }
        self.apply_damage(&snap, damage);
    }

    fn structure_fire(
        &mut self,
        i: usize,
        w: &echelon_data::WeaponDef,
        snap: &[Snap],
        damage: &mut Vec<(u32, Fx, i32, Option<echelon_data::Attr>, Fx, V2)>,
    ) {
        if self.ents[i].cd > 0 {
            self.ents[i].cd -= 1;
            return;
        }
        // stagger acquisition: each unit scans on its phase tick, cutting
        // the O(n^2) hot loop 4x (costs at most 3 ticks of shot latency)
        if (self.tick.wrapping_add(i as u32)) % 4 != 0 {
            return;
        }
        let range = self.rt[self.ents[i].def as usize].range;
        let me = self.ents[i].player;
        let my = self.ents[i].pos;
        let mut best: i64 = i64::MAX;
        let mut ti: i32 = -1;
        for (j, s) in snap.iter().enumerate() {
            if !s.alive || s.player == me || !s.vis[me as usize] {
                continue;
            }
            if s.air && !w.air {
                continue;
            }
            if !s.air && !w.ground {
                continue;
            }
            let d2 = fx::dist2(my, s.pos);
            if d2 <= fx::range2(range) && (d2 as i64) < best {
                best = d2 as i64;
                ti = j as i32;
            }
        }
        if ti >= 0 {
            self.ents[i].cd = w.cd * 1000 / self.ents[i].atk_pct.max(1);
            damage.push((
                ti as u32,
                Fx::from_int(w.dmg),
                w.bonus,
                w.bonus_vs,
                self.rt[self.ents[i].def as usize].splash,
                snap[ti as usize].pos,
            ));
        }
    }

    fn attack_move(
        &mut self,
        i: usize,
        d: &echelon_data::UnitDef,
        dest: V2,
        snap: &[Snap],
        damage: &mut Vec<(u32, Fx, i32, Option<echelon_data::Attr>, Fx, V2)>,
    ) {
        let w = match &d.wpn {
            Some(w) => w.clone(),
            None => {
                if self.follow_path(i) {
                    self.ents[i].order = Order::Idle;
                }
                return;
            }
        };
        let me = self.ents[i].player;
        let my = self.ents[i].pos;
        let sight = self.rt[self.ents[i].def as usize].sight;
        let range = self.rt[self.ents[i].def as usize].range;
        // validate / find target
        let mut t = self.ents[i].target;
        let valid = |j: u32| -> bool {
            let j = j as usize;
            j > 0 && j - 1 < snap.len() && {
                let s = &snap[j - 1];
                s.alive
                    && s.player != me
                    && s.vis[me as usize]
                    && !(s.air && !w.air)
                    && !(!s.air && !w.ground)
                    && fx::dist2(my, s.pos) <= fx::range2(sight.add(Fx::from_int(2)))
            }
        };
        if !valid(t) {
            t = 0;
            // prefer units, then structures
            let mut best: (i32, i64) = (2, i64::MAX);
            for (j, s) in snap.iter().enumerate() {
                if !s.alive || s.player == me || !s.vis[me as usize] {
                    continue;
                }
                if s.air && !w.air {
                    continue;
                }
                if !s.air && !w.ground {
                    continue;
                }
                let d2 = fx::dist2(my, s.pos);
                if d2 > fx::range2(sight) {
                    continue;
                }
                let class = if s.structure { 1 } else { 0 };
                if (class, d2 as i64) < best {
                    best = (class, d2 as i64);
                    t = (j + 1) as u32;
                }
            }
            self.ents[i].target = t;
        }
        if t != 0 {
            let tp = snap[(t - 1) as usize].pos;
            if fx::dist2(my, tp) <= fx::range2(range) {
                if self.ents[i].cd == 0
                    && (self.tick.wrapping_add(i as u32)) % 4 == 0
                {
                    self.ents[i].cd = w.cd * 1000 / self.ents[i].atk_pct.max(1);
                    damage.push((
                        t - 1,
                        Fx::from_int(w.dmg),
                        w.bonus,
                        w.bonus_vs,
                        self.rt[self.ents[i].def as usize].splash,
                        tp,
                    ));
                }
            } else {
                self.chase_point(i, tp);
            }
        } else {
            // no target: keep pushing to the ordered destination
            let arrived = fx::dist2(my, dest) <= fx::range2(Fx::from_int(2));
            if !arrived {
                self.chase_point(i, dest);
            }
        }
    }

    /// path-follow one step; true when the path is exhausted
    fn follow_path(&mut self, i: usize) -> bool {
        let def = self.ents[i].def;
        let mut speed = self.rt[def as usize].speed;
        if self.ents[i].spd_pct != 1000 {
            speed = speed.mul_int(self.ents[i].spd_pct).div_int(1000);
        }
        if self.ents[i].path_i >= self.ents[i].path.len() {
            return true;
        }
        let wp = self.ents[i].path[self.ents[i].path_i];
        let target = V2::from_tile(wp.0, wp.1);
        let (np, reached) = fx::step_towards(self.ents[i].pos, target, speed);
        self.ents[i].pos = np;
        if reached {
            self.ents[i].on_deck = wp.2 == 1;
            self.ents[i].path_i += 1;
        }
        self.ents[i].path_i >= self.ents[i].path.len()
    }

    /// move towards a point, repathing on a deterministic cadence
    fn chase_point(&mut self, i: usize, to: V2) {
        let d = self.pack.def(self.ents[i].def).clone();
        let air = d.mover() == Mover::Air;
        let need = self.ents[i].path_i >= self.ents[i].path.len()
            || (self.tick.wrapping_add(i as u32)) % 24 == 0;
        if need {
            let from = self.ents[i].pos.tile();
            let path = if air {
                self.map.astar_air(from, to.tile(), d.depth_eff(), 4000)
            } else {
                let fs = if self.ents[i].on_deck { 1 } else { SURF_GROUND };
                self.map.astar(from, fs, to.tile(), d.depth_eff(), 1500)
            };
            self.ents[i].path = path;
            self.ents[i].path_i = 0;
        }
        self.follow_path(i);
    }

    fn nearest_hall(&self, p: u8, from: V2) -> Option<u32> {
        let mut best = i128::MAX;
        let mut bi = None;
        for (j, e) in self.ents.iter().enumerate() {
            if e.alive && e.player == p && e.progress == 0 && self.pack.def(e.def).townhall {
                let d2 = fx::dist2(from, e.pos);
                if d2 < best {
                    best = d2;
                    bi = Some(j as u32);
                }
            }
        }
        bi
    }

    fn harvest_go(&mut self, i: usize, n: u32) {
        if (n as usize) >= self.map.nodes.len() {
            self.ents[i].order = Order::Idle;
            return;
        }
        let node = self.map.nodes[n as usize].clone();
        if node.amount <= 0 {
            self.ents[i].order = Order::Idle;
            return;
        }
        let flux = node.kind == ResKind::Flux;
        if flux {
            // needs my extractor on it
            let ok = node.taken_by != 0 && {
                let s = &self.ents[(node.taken_by - 1) as usize];
                s.alive && s.player == self.ents[i].player && s.progress == 0
            };
            if !ok {
                self.ents[i].order = Order::Idle;
                return;
            }
        }
        let np = V2::from_tile(node.tx, node.ty);
        // flux is mined through the extractor shell, so the reach must span
        // the 3x3 footprint (ring tiles sit ~2.0 from the node center)
        let reach = if flux { 2300 } else { 1600 };
        if fx::dist2(self.ents[i].pos, np) <= fx::range2(Fx::from_milli(reach)) {
            // mine in place: progress counts up in `cd` reuse? use carry timer
            self.ents[i].path.clear();
            self.ents[i].spawn_t += 1; // reuse as harvest timer on workers
            if self.ents[i].spawn_t >= HARVEST_TICKS {
                self.ents[i].spawn_t = 0;
                let amt = if flux { CARRY_FLUX } else { CARRY_ALLOY };
                let take = amt.min(self.map.nodes[n as usize].amount);
                self.map.nodes[n as usize].amount -= take;
                self.ents[i].carry = take;
                self.ents[i].carry_flux = flux;
                self.ents[i].order = Order::HarvestReturn(n);
            }
        } else {
            self.chase_point(i, np);
        }
    }

    fn harvest_return(&mut self, i: usize, n: u32) {
        let p = self.ents[i].player;
        let hall = match self.nearest_hall(p, self.ents[i].pos) {
            Some(h) => h,
            None => {
                self.ents[i].order = Order::Idle;
                return;
            }
        };
        let hp = self.ents[hall as usize].pos;
        if fx::dist2(self.ents[i].pos, hp) <= fx::range2(Fx::from_milli(2600)) {
            let c = self.ents[i].carry;
            if self.ents[i].carry_flux {
                self.res[p as usize].1 += c;
            } else {
                self.res[p as usize].0 += c;
            }
            self.ents[i].carry = 0;
            self.ents[i].order = Order::HarvestGo(n);
        } else {
            self.chase_point(i, hp);
        }
    }

    fn apply_damage(
        &mut self,
        snap: &[Snap],
        damage: Vec<(u32, Fx, i32, Option<echelon_data::Attr>, Fx, V2)>,
    ) {
        for (t, base, bonus, bvs, splash, at) in damage {
            self.hit(t as usize, base, bonus, bvs);
            if splash.is_pos() {
                for j in 0..snap.len() {
                    if j == t as usize || !snap[j].alive {
                        continue;
                    }
                    if snap[j].player == snap[t as usize].player
                        && fx::dist2(at, snap[j].pos) <= fx::range2(splash)
                    {
                        self.hit(j, base.div_int(2), bonus / 2, bvs);
                    }
                }
            }
        }
    }

    fn hit(&mut self, t: usize, base: Fx, bonus: i32, bvs: Option<echelon_data::Attr>) {
        if !self.ents[t].alive {
            return;
        }
        let attrs = self.rt[self.ents[t].def as usize].attrs;
        let mut dmg = base;
        if let Some(a) = bvs {
            if attrs & a.bit() != 0 {
                dmg = dmg.add(Fx::from_int(bonus));
            }
        }
        if self.ents[t].dtk_pct != 1000 {
            dmg = dmg.mul_int(self.ents[t].dtk_pct).div_int(1000);
        }
        self.ents[t].sh_delay = SHIELD_DELAY;
        let e = &mut self.ents[t];
        if e.sh.is_pos() {
            if e.sh.0 >= dmg.0 {
                e.sh = e.sh.sub(dmg);
                return;
            } else {
                dmg = dmg.sub(e.sh);
                e.sh = fx::ZERO;
            }
        }
        let armor = Fx::from_int(self.pack.def(e.def).armor);
        let eff = dmg.sub(armor).max(fx::HALF);
        e.hp = e.hp.sub(eff);
        if !e.hp.is_pos() {
            e.alive = false;
        }
    }

    fn sys_regen(&mut self) {
        for i in 0..self.ents.len() {
            if !self.ents[i].alive {
                continue;
            }
            let def = self.ents[i].def;
            let rt_hp = self.rt[def as usize].hp;
            let rt_sh = self.rt[def as usize].sh;
            let rt_en = self.rt[def as usize].energy;
            let p = self.ents[i].player as usize;
            let lr = self.life_regen[p];
            let is_struct = self.pack.def(def).kind == Kind::Structure;
            let e = &mut self.ents[i];
            if lr.is_pos() && !is_struct && e.hp < rt_hp {
                e.hp = e.hp.add(lr).min(rt_hp);
            }
            if rt_sh.is_pos() {
                if e.sh_delay > 0 {
                    e.sh_delay -= 1;
                } else if e.sh < rt_sh {
                    e.sh = e.sh.add(self.sh_regen).min(rt_sh);
                }
            }
            if rt_en.is_pos() && e.energy < rt_en {
                e.energy = e.energy.add(self.en_regen).min(rt_en);
            }
            // ---- ability state upkeep ----
            if e.regen_t > 0 {
                e.regen_t -= 1;
                e.hp = e.hp.add(e.regen_hps).min(rt_hp);
            }
            if e.dot_t > 0 {
                e.dot_t -= 1;
                e.hp = e.hp.sub(e.dot_dps.div_int(echelon_data::TICKS_PER_SEC));
                if !e.hp.is_pos() {
                    e.alive = false;
                }
            }
            if e.spd_t > 0 {
                e.spd_t -= 1;
                if e.spd_t == 0 {
                    e.spd_pct = 1000;
                }
            }
            if e.atk_t > 0 {
                e.atk_t -= 1;
                if e.atk_t == 0 {
                    e.atk_pct = 1000;
                }
            }
            if e.dtk_t > 0 {
                e.dtk_t -= 1;
                if e.dtk_t == 0 {
                    e.dtk_pct = 1000;
                }
            }
            if e.cloak_t > 0 {
                e.cloak_t -= 1;
            }
            for c in e.abil_cd.iter_mut() {
                if *c > 0 {
                    *c -= 1;
                }
            }
            if e.life_t > 0 {
                e.life_t -= 1;
                if e.life_t == 0 {
                    e.alive = false;
                }
            }
        }
    }

    fn sys_cleanup(&mut self) {
        for i in 0..self.ents.len() {
            if self.ents[i].alive || self.ents[i].def == u16::MAX {
                continue;
            }
            // process each corpse exactly once, then tombstone via def MAX
            let def = self.ents[i].def;
            let d = self.pack.def(def).clone();
            let p = self.ents[i].player as usize;
            if d.kind == Kind::Structure {
                let fp = d.fp();
                let (cx, cy) = self.ents[i].pos.tile();
                let tx = cx - fp / 2;
                let ty = cy - fp / 2;
                for x in tx..tx + fp {
                    for y in ty..ty + fp {
                        self.map.set_struct(x, y, 0);
                    }
                }
                if self.ents[i].progress == 0 {
                    self.cap2[p] -= d.provides2;
                }
                for n in self.map.nodes.iter_mut() {
                    if n.taken_by == (i as u32) + 1 {
                        n.taken_by = 0;
                    }
                }
                self.events
                    .push(format!("t{} p{} lost {}", self.tick, p, d.name));
            } else {
                self.used2[p] -= d.supply2;
            }
            // refund queued supply
            let q: Vec<QItem> = std::mem::take(&mut self.ents[i].queue);
            for it in q {
                let ud = self.pack.def(it.def);
                self.used2[p] -= ud.supply2 * if ud.twin { 2 } else { 1 };
            }
            self.ents[i].def = u16::MAX;
        }
        for p in 0..2usize {
            if !self.defeated[p] {
                let has = self.ents.iter().any(|e| {
                    e.alive
                        && e.player as usize == p
                        && e.def != u16::MAX
                        && self.pack.def(e.def).kind == Kind::Structure
                });
                if !has {
                    self.defeated[p] = true;
                    self.events.push(format!("t{} player {} defeated", self.tick, p));
                }
            }
        }
    }

    // ---------- hashing & read helpers ----------

    pub fn state_hash(&self) -> u64 {
        let mut h = Fnv::new();
        h.u32(self.tick);
        h.u64(self.rng.state);
        for p in 0..2 {
            h.i32(self.res[p].0);
            h.i32(self.res[p].1);
            h.i32(self.used2[p]);
            h.i32(self.cap2[p]);
            h.u8(self.defeated[p] as u8);
        }
        for n in &self.map.nodes {
            h.i32(n.amount);
            h.u32(n.taken_by);
        }
        for e in &self.ents {
            h.u8(e.alive as u8);
            if e.def == u16::MAX {
                continue;
            }
            h.u32(e.def as u32);
            h.u8(e.player);
            h.i64(e.pos.x.0);
            h.i64(e.pos.y.0);
            h.i64(e.hp.0);
            h.i64(e.sh.0);
            h.i64(e.energy.0);
            h.i32(e.cd);
            h.i32(e.progress);
            h.i32(e.spawn_pool);
            h.i32(e.carry);
            h.u32(e.linked_to);
            h.u32(e.target);
            h.u8(e.on_deck as u8);
            h.i32(e.stun_t);
            h.i32(e.cloak_t);
            h.i32(e.spd_pct);
            h.i32(e.spd_t);
            h.i32(e.atk_pct);
            h.i32(e.atk_t);
            h.i32(e.dtk_pct);
            h.i32(e.dtk_t);
            h.i64(e.regen_hps.0);
            h.i32(e.regen_t);
            h.i64(e.dot_dps.0);
            h.i32(e.dot_t);
            h.i32(e.life_t);
            for c in e.abil_cd.iter() {
                h.i32(*c);
            }
            h.i32(e.rally.0);
            h.i32(e.rally.1);
            h.u32(e.queue.len() as u32);
            for q in &e.queue {
                h.u32(q.def as u32);
                h.i32(q.left);
            }
            let ot = match &e.order {
                Order::Idle => 0u8,
                Order::Move(_) => 1,
                Order::AttackMove(_) => 2,
                Order::HarvestGo(_) => 3,
                Order::HarvestReturn(_) => 4,
                Order::BuildGo(_) => 5,
                Order::Constructing(_) => 6,
                Order::Hold => 7,
            };
            h.u8(ot);
        }
        h.fin()
    }

    pub fn count_named(&self, p: u8, name: &str, completed_only: bool) -> usize {
        let id = self.pack.id(name);
        self.ents
            .iter()
            .filter(|e| {
                e.alive
                    && e.player == p
                    && e.def == id
                    && (!completed_only || e.progress == 0)
            })
            .count()
    }
    pub fn find_named(&self, p: u8, name: &str, completed_only: bool) -> Option<u32> {
        let id = self.pack.id(name);
        self.ents.iter().enumerate().find_map(|(i, e)| {
            if e.alive && e.player == p && e.def == id && (!completed_only || e.progress == 0)
            {
                Some(i as u32)
            } else {
                None
            }
        })
    }
    pub fn army(&self, p: u8) -> Vec<u32> {
        self.ents
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.alive && e.player == p && e.def != u16::MAX && {
                    let d = self.pack.def(e.def);
                    d.kind == Kind::Unit
                }
            })
            .map(|(i, _)| i as u32)
            .collect()
    }
    pub fn army_supply2(&self, p: u8) -> i32 {
        self.army(p)
            .iter()
            .map(|&i| self.pack.def(self.ents[i as usize].def).supply2)
            .sum()
    }
    pub fn workers(&self, p: u8) -> Vec<u32> {
        self.ents
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.alive
                    && e.player == p
                    && e.def != u16::MAX
                    && self.pack.def(e.def).kind == Kind::Worker
            })
            .map(|(i, _)| i as u32)
            .collect()
    }
    /// max hp of a def (for clients drawing bars)
    pub fn max_hp(&self, def: DefId) -> Fx {
        self.rt[def as usize].hp
    }

    /// sight radius of a live entity (for clients drawing fog)
    pub fn sight(&self, i: usize) -> Fx {
        self.rt[self.ents[i].def as usize].sight
    }

    /// which entities player `p` can currently see: own units always;
    /// enemies when any completed friendly has them in sight, with
    /// cloak honoring the detector rules
    pub fn visible_mask(&self, p: u8) -> Vec<bool> {
        let snap = self.snap();
        let eyes: Vec<(V2, Fx)> = self
            .ents
            .iter()
            .filter(|e| e.alive && e.def != u16::MAX && e.player == p && e.progress == 0)
            .map(|e| (e.pos, self.rt[e.def as usize].sight))
            .collect();
        self.ents
            .iter()
            .enumerate()
            .map(|(i, e)| {
                if !e.alive || e.def == u16::MAX {
                    return false;
                }
                if e.player == p {
                    return true;
                }
                if !snap[i].vis[p as usize] {
                    return false; // cloaked and undetected
                }
                eyes.iter()
                    .any(|(pos, s)| fx::dist2(*pos, e.pos) <= fx::range2(*s))
            })
            .collect()
    }

    /// altitude a flyer is riding at right now (for rendering/targeting).
    /// Crowding fans a stack upward: when same-band flyers share a tile,
    /// later ones bump a band per neighbor below them — capitals rarely
    /// can (their hull hits the ceiling), so they hold their lane while
    /// fighter swarms spread vertically. Derived deterministically from
    /// entity order; never part of the hashed state.
    pub fn fly_level(&self, i: usize) -> u8 {
        let e = &self.ents[i];
        let d = self.pack.def(e.def);
        let (tx, ty) = e.pos.tile();
        let pref = AIR0 + d.fly_layer.clamp(0, 2) as u8;
        let mut crowd = 0u8;
        for (j, o) in self.ents.iter().enumerate() {
            if j >= i {
                break;
            }
            if !o.alive || o.def == u16::MAX {
                continue;
            }
            let od = self.pack.def(o.def);
            if od.mover() != Mover::Air {
                continue;
            }
            if o.pos.tile() == (tx, ty) && AIR0 + od.fly_layer.clamp(0, 2) as u8 == pref {
                crowd += 1;
            }
        }
        self.map
            .air_level(tx, ty, pref.saturating_add(crowd.min(2)), d.depth_eff())
    }

    pub fn alive_units(&self, p: u8) -> usize {
        self.ents
            .iter()
            .filter(|e| e.alive && e.player == p && e.def != u16::MAX)
            .count()
    }
}
