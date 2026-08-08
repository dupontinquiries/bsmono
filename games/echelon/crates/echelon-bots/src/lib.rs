//! Scripted macro AIs, one plan per race. Deterministic pure functions of
//! the sim state: same World in, same commands out — which is exactly what
//! lockstep needs from an in-process opponent.

use echelon_data::{AbilKind, AbilTarget, Kind, Op};
use echelon_sim::map::ResKind;
use echelon_sim::world::Order;
use echelon_sim::{Cmd, PlayerCmd, World};

pub const CADENCE: u32 = 22; // bots think ~once per second

/// A parameterized policy: the scripted bot with every constant exposed.
/// `coach()` reproduces the hand-tuned bot exactly; evolution explores
/// the space around it.
#[derive(Clone, Debug, PartialEq)]
pub struct Genome {
    pub worker_base: i32,
    pub worker_per_hall: i32,
    pub worker_cap: i32,
    pub gas_workers: i32,
    pub expand_min_workers: i32,
    pub expand_bank: i32,
    pub max_halls: i32,
    pub factory_cap: i32,
    pub factory_bank: i32,
    pub attack_base: i32,
    pub wave_step: i32,
    pub wave_cap: i32,
    pub frenzy_t: i32,
    pub terminal_t: i32,
    /// per-race weights over each trainable unit pool (race_index * 12
    /// + roster order); one genome, three lethal doctrines
    pub comp_w: [i32; 36],
    /// cast when energy >= this % of the ability cost
    pub cast_pct: i32,
}

impl Genome {
    pub fn coach() -> Genome {
        Genome {
            worker_base: 12,
            worker_per_hall: 8,
            worker_cap: 26,
            gas_workers: 3,
            expand_min_workers: 13,
            expand_bank: 100,
            max_halls: 3,
            factory_cap: 3,
            factory_bank: 450,
            attack_base: 55,
            wave_step: 25,
            wave_cap: 90,
            frenzy_t: 17_000,
            terminal_t: 20_000,
            comp_w: {
                let mut w = [0i32; 36];
                for r in 0..3 {
                    for k in 0..4 {
                        w[r * 12 + k] = 3;
                    }
                }
                w
            },
            cast_pct: 100,
        }
    }
    /// exploiter archetype: hit early, hit constantly
    pub fn aggro() -> Genome {
        let mut g = Genome::coach();
        g.attack_base = 16;
        g.wave_step = 8;
        g.wave_cap = 46;
        g.frenzy_t = 3_500;
        g.terminal_t = 5_500;
        g.worker_base = 10;
        g.worker_cap = 16;
        g.expand_min_workers = 24; // never expands: one-base pressure
        g.factory_bank = 150;
        for r in 0..3 {
            for k in 0..12 {
                g.comp_w[r * 12 + k] = if k < 2 { 5 } else { 0 };
            }
        }
        g
    }

    pub fn clamp(&mut self) {
        self.worker_base = self.worker_base.clamp(6, 20);
        self.worker_per_hall = self.worker_per_hall.clamp(0, 12);
        self.worker_cap = self.worker_cap.clamp(10, 28);
        self.gas_workers = self.gas_workers.clamp(0, 5);
        self.expand_min_workers = self.expand_min_workers.clamp(8, 24);
        self.expand_bank = self.expand_bank.clamp(0, 800);
        self.max_halls = self.max_halls.clamp(1, 4);
        self.factory_cap = self.factory_cap.clamp(1, 4);
        self.factory_bank = self.factory_bank.clamp(100, 1200);
        self.attack_base = self.attack_base.clamp(10, 80);
        self.wave_step = self.wave_step.clamp(0, 50);
        self.wave_cap = self.wave_cap.clamp(self.attack_base, 110);
        self.frenzy_t = self.frenzy_t.clamp(3_000, 11_000);
        self.terminal_t = self.terminal_t.clamp(self.frenzy_t, 12_500);
        for w in self.comp_w.iter_mut() {
            *w = (*w).clamp(0, 9);
        }
        for r in 0..3 {
            if self.comp_w[r * 12..r * 12 + 12].iter().all(|&w| w == 0) {
                self.comp_w[r * 12] = 1;
            }
        }
        self.cast_pct = self.cast_pct.clamp(60, 300);
    }
    pub fn to_vec(&self) -> Vec<i32> {
        let mut v = vec![
            self.worker_base, self.worker_per_hall, self.worker_cap, self.gas_workers,
            self.expand_min_workers, self.expand_bank, self.max_halls, self.factory_cap,
            self.factory_bank, self.attack_base, self.wave_step, self.wave_cap,
            self.frenzy_t, self.terminal_t,
        ];
        v.extend_from_slice(&self.comp_w);
        v.push(self.cast_pct);
        v
    }
    pub fn from_vec(v: &[i32]) -> Genome {
        let mut g = Genome::coach();
        g.worker_base = v[0]; g.worker_per_hall = v[1]; g.worker_cap = v[2];
        g.gas_workers = v[3]; g.expand_min_workers = v[4]; g.expand_bank = v[5];
        g.max_halls = v[6]; g.factory_cap = v[7]; g.factory_bank = v[8];
        g.attack_base = v[9]; g.wave_step = v[10]; g.wave_cap = v[11];
        g.frenzy_t = v[12]; g.terminal_t = v[13];
        if v.len() == 27 {
            // migrate v1 genomes: tile the single doctrine across races
            for r in 0..3 {
                for i in 0..12 {
                    g.comp_w[r * 12 + i] = v[14 + i];
                }
            }
            g.cast_pct = v[26];
        } else {
            for i in 0..36 {
                g.comp_w[i] = v[14 + i];
            }
            g.cast_pct = v[50];
        }
        g.clamp();
        g
    }
    /// the race's trainable, non-worker unit pool in roster order
    pub fn pool(w: &World, p: u8) -> Vec<String> {
        let ri = w.races[p as usize];
        w.pack
            .defs
            .iter()
            .filter(|(r, d)| {
                *r == ri
                    && d.kind == Kind::Unit
                    && w.pack
                        .defs
                        .iter()
                        .any(|(r2, s)| *r2 == ri && s.trains.iter().any(|t| t == &d.name))
            })
            .map(|(_, d)| d.name.clone())
            .take(12)
            .collect()
    }
}

pub struct Plan {
    pub worker: &'static str,
    pub supply: &'static str,
    pub order: &'static [(&'static str, usize)],
    pub comp: &'static [&'static str],
    pub attack_s2: i32,
    /// primary production structure to mass once the opener is done
    pub factory: &'static str,
    pub max_factories: usize,
}

pub fn plan(race: &str) -> Plan {
    match race {
        "Genetron" => Plan {
            worker: "ACR",
            supply: "Node",
            order: &[
                ("Manufacturer", 12),
                ("Uploader", 13),
                ("FilteringPlant", 13),
                ("OutfittingStation", 14),
                ("Fabricator", 14),
            ],
            comp: &["Spitfire", "Blitzer", "Spitfire", "Fixer", "Equalizer"],
            attack_s2: 60,
            factory: "Manufacturer",
            max_factories: 3,
        },
        "Xayid" => Plan {
            worker: "Scavenger",
            supply: "FeedingPool",
            order: &[
                ("XayidDen", 12),
                ("Siphoner", 13),
                ("MutagenChamber", 13),
                ("BilePit", 14),
            ],
            comp: &["Xayidling", "Mendling", "Roamer", "Scorpalisk"],
            attack_s2: 50,
            factory: "XayidDen",
            max_factories: 3,
        },
        "Keiron" => Plan {
            worker: "Converter",
            supply: "Citadel",
            order: &[("Formulator", 12), ("Edifice", 12), ("Sanctum", 14)],
            comp: &["Volt", "Pariah", "Meridian", "Pulsar"],
            attack_s2: 50,
            factory: "Citadel",
            max_factories: 3,
        },
        _ => panic!("unknown race {}", race),
    }
}

#[derive(Default)]
pub struct Bot {
    pub order_issued: Vec<u32>,
    pub next_attack: u32,
    /// escalates the attack threshold so later pushes hit harder
    pub wave: u32,
    pub expo_at: u32,
}

pub fn milli_of_tile(t: (i32, i32)) -> (i32, i32) {
    (t.0 * 1000 + 500, t.1 * 1000 + 500)
}

pub fn bot_step(w: &World, p: u8, plan: &Plan, st: &mut Bot, out: &mut Vec<PlayerCmd>) {
    bot_step_g(w, p, plan, &Genome::coach(), st, out)
}

pub fn bot_step_g(
    w: &World,
    p: u8,
    plan: &Plan,
    g: &Genome,
    st: &mut Bot,
    out: &mut Vec<PlayerCmd>,
) {
    if st.order_issued.is_empty() {
        st.order_issued = vec![0; plan.order.len()];
    }
    let pi = p as usize;
    let start = w.map.starts[pi];
    let enemy = w.map.starts[1 - pi];
    let push = |out: &mut Vec<PlayerCmd>, cmd: Cmd| out.push(PlayerCmd { player: p, cmd });

    // -- resource assignment --
    let workers = w.workers(p);
    let my_alloy: Vec<u32> = w
        .map
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            n.kind == ResKind::Alloy && n.amount > 0 && {
                let d0 = (n.tx - start.0).abs() + (n.ty - start.1).abs() < 20;
                let near_hall = w.ents.iter().any(|e| {
                    e.alive
                        && e.player == p
                        && e.def != u16::MAX
                        && e.progress == 0
                        && w.pack.def(e.def).townhall
                        && {
                            let ht = e.pos.tile();
                            (n.tx - ht.0).abs() + (n.ty - ht.1).abs() < 14
                        }
                });
                d0 || near_hall
            }
        })
        .map(|(i, _)| i as u32)
        .collect();
    // the geyser to BUILD on: nearest flux to our start
    let my_flux_site: Option<u32> = w
        .map
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            n.kind == ResKind::Flux && (n.tx - start.0).abs() + (n.ty - start.1).abs() < 20
        })
        .min_by_key(|(_, n)| (n.tx - start.0).abs() + (n.ty - start.1).abs())
        .map(|(i, _)| i as u32);
    // the geyser to MINE: one that already carries our live extractor
    let my_flux: Option<u32> = w
        .map
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            n.kind == ResKind::Flux && n.taken_by != 0 && {
                let t = (n.taken_by - 1) as usize;
                t < w.ents.len() && {
                    let e = &w.ents[t];
                    e.alive && e.player == p && e.progress == 0
                }
            }
        })
        .map(|(i, _)| i as u32)
        .next();
    let extractor_done = w.count_named(p, gas_name(plan), true) > 0;
    let mut flux_workers = 0usize;
    for &wi in &workers {
        match w.ents[wi as usize].order {
            Order::HarvestGo(n) | Order::HarvestReturn(n) => {
                if w.map.nodes[n as usize].kind == ResKind::Flux {
                    flux_workers += 1;
                }
            }
            _ => {}
        }
    }
    let mut k = 0usize;
    for &wi in &workers {
        let e = &w.ents[wi as usize];
        if e.order == Order::Idle && !my_alloy.is_empty() {
            if extractor_done && (flux_workers as i32) < g.gas_workers {
                if let Some(f) = my_flux {
                    push(out, Cmd::Harvest { id: wi, node: f });
                    flux_workers += 1;
                    continue;
                }
            }
            let n = my_alloy[k % my_alloy.len()];
            k += 1;
            push(out, Cmd::Harvest { id: wi, node: n });
        }
    }
    // move two alloy miners onto flux once the extractor finishes
    if extractor_done && (flux_workers as i32) < g.gas_workers {
        if let Some(f) = my_flux {
            let mut moved = 0;
            for &wi in &workers {
                if ((moved + flux_workers) as i32) >= g.gas_workers {
                    break;
                }
                if let Order::HarvestGo(n) = w.ents[wi as usize].order {
                    if w.map.nodes[n as usize].kind == ResKind::Alloy {
                        push(out, Cmd::Harvest { id: wi, node: f });
                        moved += 1;
                    }
                }
            }
        }
    }

    // -- worker production (every hall) --
    let halls: Vec<u32> = w
        .ents
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.alive
                && e.player == p
                && e.def != u16::MAX
                && e.progress == 0
                && w.pack.def(e.def).townhall
        })
        .map(|(i, _)| i as u32)
        .collect();
    let target_workers =
        ((g.worker_base + g.worker_per_hall * halls.len() as i32) as usize).min(g.worker_cap as usize);
    if workers.len() < target_workers {
        for &h in &halls {
            if w.ents[h as usize].queue.is_empty() {
                push(
                    out,
                    Cmd::Train {
                        structure: h,
                        unit: w.pack.id(plan.worker),
                    },
                );
                break;
            }
        }
    }

    // -- expand to the nearest unclaimed cluster when the bank allows --
    let hall_def = w.pack.def(w.pack.id(match plan.worker {
        "ACR" => "ProcessingCore",
        "Scavenger" => "ScavengerNest",
        _ => "Citadel",
    }));
    let hall_cost = hall_def.cost;
    let expanding =
        halls.len() < w.count_named(p, &hall_def.name, false) + if plan.worker == "Converter" { 0 } else { 0 };
    let _ = expanding;
    let under_construction =
        w.count_named(p, &hall_def.name, false) > w.count_named(p, &hall_def.name, true);
    if (halls.len() as i32) < g.max_halls
        && !under_construction
        && workers.len() as i32 >= g.expand_min_workers
        && w.res[pi].0 > hall_cost.0 + g.expand_bank
        && w.tick >= st.expo_at
    {
        // nearest live alloy node that isn't near any of my halls yet
        let mut best: Option<(i32, (i32, i32))> = None;
        for n in w.map.nodes.iter() {
            if n.kind != ResKind::Alloy || n.amount <= 0 {
                continue;
            }
            let near_mine = halls
                .iter()
                .any(|&h| {
                    let ht = w.ents[h as usize].pos.tile();
                    (n.tx - ht.0).abs() + (n.ty - ht.1).abs() < 14
                });
            if near_mine {
                continue;
            }
            let d = (n.tx - start.0).abs() + (n.ty - start.1).abs();
            if best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, (n.tx, n.ty)));
            }
        }
        if let Some((_, c)) = best {
            let fp = hall_def.fp();
            if let (Some(spot), Some(b)) =
                (spot_clear(w, c, fp), pick_builder(w, &workers))
            {
                push(
                    out,
                    Cmd::Build {
                        worker: b,
                        unit: w.pack.id(&hall_def.name),
                        tx: spot.0,
                        ty: spot.1,
                    },
                );
                st.expo_at = w.tick + 40 * CADENCE;
            }
        }
    }

    // -- supply --
    let free2 = w.cap2[pi] - w.used2[pi];
    let supply_pending =
        w.count_named(p, plan.supply, false) - w.count_named(p, plan.supply, true);
    if free2 < 10 + 4 * halls.len() as i32 && supply_pending < 2 {
        if let Some(spot) = spot_near_bases(w, p, start, 3) {
            if let Some(b) = pick_builder(w, &workers) {
                push(
                    out,
                    Cmd::Build {
                        worker: b,
                        unit: w.pack.id(plan.supply),
                        tx: spot.0,
                        ty: spot.1,
                    },
                );
            }
        }
    }

    // -- build order --
    for (i, (name, min_w)) in plan.order.iter().enumerate() {
        if workers.len() < *min_w {
            break;
        }
        if w.count_named(p, name, false) > 0 {
            continue;
        }
        let last = st.order_issued[i];
        if last != 0 && w.tick.saturating_sub(last) < 200 {
            continue;
        }
        let def = w.pack.def(w.pack.id(name)).clone();
        let spot = if def.gas {
            match my_flux_site {
                Some(f) => {
                    let n = &w.map.nodes[f as usize];
                    if n.taken_by != 0 {
                        None
                    } else {
                        Some((n.tx - 1, n.ty - 1))
                    }
                }
                None => None,
            }
        } else {
            spot_near_bases(w, p, start, 3)
        };
        if let (Some(spot), Some(b)) = (spot, pick_builder(w, &workers)) {
            push(
                out,
                Cmd::Build {
                    worker: b,
                    unit: w.pack.id(name),
                    tx: spot.0,
                    ty: spot.1,
                },
            );
            st.order_issued[i] = w.tick.max(1);
        }
        break; // one build-order action per cadence
    }

    // -- factory scaling once the opener finished --
    let opener_done = plan
        .order
        .iter()
        .all(|(n, _)| w.count_named(p, n, false) > 0 || workers.len() < 14);
    let factories = w.count_named(p, plan.factory, false);
    if opener_done
        && (factories as i32) < g.factory_cap
        && w.res[pi].0 > g.factory_bank
        && workers.len() >= 14
    {
        if let (Some(spot), Some(b)) =
            (spot_near_bases(w, p, start, 3), pick_builder(w, &workers))
        {
            push(
                out,
                Cmd::Build {
                    worker: b,
                    unit: w.pack.id(plan.factory),
                    tx: spot.0,
                    ty: spot.1,
                },
            );
        }
    }

    // -- Genetron: keep every Manufacturer linked to an Uploader --
    if plan.worker == "ACR" {
        let uploaders: Vec<u32> = w
            .ents
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.alive
                    && e.player == p
                    && e.def != u16::MAX
                    && e.progress == 0
                    && w.pack.def(e.def).uploader
            })
            .map(|(i, _)| i as u32)
            .collect();
        let facilities: Vec<u32> = w
            .ents
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.alive
                    && e.player == p
                    && e.def != u16::MAX
                    && e.progress == 0
                    && w.pack.def(e.def).name == "Manufacturer"
            })
            .map(|(i, _)| i as u32)
            .collect();
        let linked_of = |f: u32| {
            uploaders
                .iter()
                .filter(|&&u| w.ents[u as usize].linked_to == f + 1)
                .count()
        };
        let mut free_up = uploaders
            .iter()
            .copied()
            .filter(|&u| w.ents[u as usize].linked_to == 0);
        let mut linked_something = false;
        for &f in &facilities {
            if linked_of(f) == 0 {
                if let Some(u) = free_up.next() {
                    push(out, Cmd::Link { uploader: u, facility: f });
                    linked_something = true;
                }
            }
        }
        // need more uploaders than we have facilities covered?
        let uncovered = facilities.iter().filter(|&&f| linked_of(f) == 0).count();
        if !linked_something
            && uncovered > 0
            && w.res[pi].0 >= 100
            && w.res[pi].1 >= 50
            && w.count_named(p, "Uploader", false) == w.count_named(p, "Uploader", true)
        {
            if let (Some(spot), Some(b)) =
                (spot_near_bases(w, p, start, 3), pick_builder(w, &workers))
            {
                push(
                    out,
                    Cmd::Build {
                        worker: b,
                        unit: w.pack.id("Uploader"),
                        tx: spot.0,
                        ty: spot.1,
                    },
                );
            }
        }
    }

    // -- army production: weighted sampling over the race's unit pool --
    let pool = Genome::pool(w, p);
    let roff = w.races[p as usize].min(2) * 12;
    let weights: Vec<i32> = pool
        .iter()
        .enumerate()
        .map(|(i, _)| g.comp_w.get(roff + i).copied().unwrap_or(0).max(0))
        .collect();
    let wsum: i32 = weights.iter().sum::<i32>().max(1);
    let pick = |k: usize| -> &str {
        let mut t = (k as i32 * 7919) % wsum;
        for (i, &wt) in weights.iter().enumerate() {
            if t < wt {
                return &pool[i];
            }
            t -= wt;
        }
        &pool[0]
    };
    let comp_len = pool.len().max(1);
    let mut comp_i = (w.tick / CADENCE) as usize;
    for (si, e) in w.ents.iter().enumerate() {
        if !(e.alive && e.player == p && e.def != u16::MAX && e.progress == 0) {
            continue;
        }
        let d = w.pack.def(e.def);
        if d.kind != Kind::Structure || d.trains.is_empty() {
            continue;
        }
        let slots = if d.parallel > 0 { d.parallel as usize } else { 1 };
        if e.queue.len() >= slots * 2 {
            continue;
        }
        for _ in 0..comp_len * 3 {
            let name = pick(comp_i);
            comp_i += 1;
            if !d.trains.iter().any(|t| t == name) {
                continue;
            }
            let ud = w.pack.def(w.pack.id(name));
            if w.res[pi].0 < ud.cost.0 || w.res[pi].1 < ud.cost.1 {
                continue;
            }
            push(
                out,
                Cmd::Train {
                    structure: si as u32,
                    unit: w.pack.id(name),
                },
            );
            break;
        }
    }

    // -- attack: escalating waves that hunt structures down --
    let s2 = w.army_supply2(p);
    let frenzy = w.tick as i32 > g.frenzy_t; // late game: no more sieges, only war
    let terminal = w.tick as i32 > g.terminal_t; // strangle the rebuild economy itself
    let threshold = if terminal {
        0
    } else if frenzy {
        30
    } else {
        (g.attack_base + (st.wave as i32) * g.wave_step).min(g.wave_cap)
    };
    let capped = w.used2[pi] >= w.cap2[pi] - 4 && s2 >= 40;
    if (s2 >= threshold || capped) && w.tick >= st.next_attack {
        let ids = w.army(p);
        let tgt = w
            .ents
            .iter()
            .find(|e| {
                e.alive
                    && e.player != p
                    && e.def != u16::MAX
                    && if terminal {
                        w.pack.def(e.def).townhall
                    } else {
                        w.pack.def(e.def).kind == Kind::Structure
                    }
            })
            .map(|e| e.pos.tile())
            .unwrap_or(enemy);
        let (mx, my) = milli_of_tile(tgt);
        push(out, Cmd::Attack { ids, mx, my });
        st.next_attack = w.tick + if terminal { 4 } else if frenzy { 8 } else { 20 } * CADENCE;
        st.wave += 1;
    }

    // -- casters: throw offensive actives at the nearest enemy --
    let mut casts = 0;
    for (ci, e) in w.ents.iter().enumerate() {
        if casts >= 2 {
            break;
        }
        if !(e.alive && e.player == p && e.def != u16::MAX && e.progress == 0) {
            continue;
        }
        let d = w.pack.def(e.def);
        for (slot, aid) in d.abil.iter().enumerate().take(3) {
            let a = &w.pack.abilities[w.pack.abil_by_id[aid] as usize];
            if a.kind != AbilKind::Active {
                continue;
            }
            let offensive = a.ops.iter().any(|o| {
                matches!(
                    o,
                    Op::AreaDamage { .. } | Op::ChainDamage { .. } | Op::Stun { .. } | Op::Dot { .. }
                )
            });
            if !offensive || e.abil_cd[slot] > 0 {
                continue;
            }
            if e.energy < echelon_sim::Fx::from_int(a.energy * g.cast_pct / 100) {
                continue;
            }
            let range2 = (a.range_m as i64) * (a.range_m as i64);
            let ep = e.pos;
            let mut best: Option<(i64, usize)> = None;
            for (ti, t) in w.ents.iter().enumerate() {
                if !(t.alive && t.player != p && t.def != u16::MAX) {
                    continue;
                }
                let dx = (t.pos.x.to_milli() - ep.x.to_milli()) as i64;
                let dy = (t.pos.y.to_milli() - ep.y.to_milli()) as i64;
                let d2 = dx * dx + dy * dy;
                if d2 <= range2 && best.map_or(true, |(bd, _)| d2 < bd) {
                    best = Some((d2, ti));
                }
            }
            if let Some((_, ti)) = best {
                let tp = w.ents[ti].pos;
                push(
                    out,
                    Cmd::Cast {
                        caster: ci as u32,
                        slot: slot as u8,
                        target: if a.target == AbilTarget::Enemy { ti as u32 } else { 0 },
                        mx: tp.x.to_milli() as i32,
                        my: tp.y.to_milli() as i32,
                    },
                );
                casts += 1;
                break;
            }
        }
    }
}

pub fn gas_name(p: &Plan) -> &'static str {
    match p.worker {
        "ACR" => "FilteringPlant",
        "Scavenger" => "Siphoner",
        _ => "Formulator",
    }
}

/// buildable footprint that also keeps a one-tile margin clear of every
/// ramp — never wall in your own army
pub fn spot_clear(w: &World, center: (i32, i32), fp: i32) -> Option<(i32, i32)> {
    for r in 2..16i32 {
        for dx in -r..=r {
            for dy in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let (tx, ty) = (center.0 + dx, center.1 + dy);
                if !w.map.area_buildable(tx, ty, fp) {
                    continue;
                }
                let mut near_ramp = false;
                'scan: for x in tx - 1..=tx + fp {
                    for y in ty - 1..=ty + fp {
                        if w.map.ramp(x, y).is_some() {
                            near_ramp = true;
                            break 'scan;
                        }
                    }
                }
                if !near_ramp {
                    return Some((tx, ty));
                }
            }
        }
    }
    None
}

/// try the start area first, then around every completed hall — bases
/// fill up, expansions have room
pub fn spot_near_bases(w: &World, p: u8, start: (i32, i32), fp: i32) -> Option<(i32, i32)> {
    if let Some(s) = spot_clear(w, start, fp) {
        return Some(s);
    }
    for e in w.ents.iter() {
        if e.alive
            && e.player == p
            && e.def != u16::MAX
            && e.progress == 0
            && w.pack.def(e.def).townhall
        {
            if let Some(s) = spot_clear(w, e.pos.tile(), fp) {
                return Some(s);
            }
        }
    }
    None
}

pub fn pick_builder(w: &World, workers: &[u32]) -> Option<u32> {
    for &wi in workers {
        match w.ents[wi as usize].order {
            Order::BuildGo(_) | Order::Constructing(_) => {}
            _ => return Some(wi),
        }
    }
    None
}

