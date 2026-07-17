//! skirmish.rs — slice 5: a runnable game. A minimal commander drives both
//! sides of a full match on the ported rules, so paperbound-core is not just
//! a library of laws but a crate that can PLAY: `cargo run --bin skirmish`
//! fights two factions and (optionally) renders the paper style to PNGs.
//!
//! The commander is deliberately simple — rts.py's AI has personality tiers;
//! this one is the skeleton those will hang from:
//!   • keep 4 workers harvesting (train scribes when short)
//!   • raise a troop table once affordable, then keep soldiers flowing
//!   • when the army reaches its muster size, march on the enemy hall
//!   • fighters auto-acquire on the way (match_sim's aggro does the rest)

use crate::data::Faction;
use crate::match_sim::{Job, Node, World};
use crate::sim::Refusal;
use crate::render::{render, save_png, Camera, Plateau};
use std::collections::BTreeMap;
use std::path::Path;

pub struct SkirmishReport {
    pub winner: Option<usize>,
    pub t: f64,
    pub ink: [f64; 2],
    pub units_left: [usize; 2],
    pub frames: Vec<std::path::PathBuf>,
}

struct Commander {
    team: usize,
    table: Option<usize>,
    muster: usize,
    wells: usize,
}

impl Commander {
    fn think(&mut self, w: &mut World) {
        let team = self.team;
        // find our hall
        let Some(hall) = w
            .buildings
            .iter()
            .position(|b| b.team == team && b.kind == "scriptorium" && b.alive())
        else {
            return;
        };
        // 1) worker line: keep 4 scribes
        let workers = w
            .units
            .iter()
            .filter(|u| u.team == team && u.worker && u.alive())
            .count();
        let queued_workers: usize = w.buildings[hall]
            .queue
            .iter()
            .filter(|(k, _)| k == "scribe")
            .count();
        if workers + queued_workers < 4 {
            let _ = w.queue_train(hall, "scribe");
        }
        // 2) tech: raise a table once (instant-complete; construction pacing
        //    is exercised elsewhere — the commander tests STRATEGY flow)
        if self.table.is_none() {
            let cost = w.facs[team].structures["table"].cost as f64;
            if w.ink[team] >= cost + 60.0 {
                w.ink[team] -= cost;
                let (hx, hy) = (w.buildings[hall].x, w.buildings[hall].y);
                let bi = w.add_building(team, "table", hx + 120.0, hy + 60.0, true);
                self.table = Some(bi);
            }
        }
        // 3) army line — and the classic RTS reflex: supply-blocked → raise
        //    an Ink Reservoir beside the hall (capped so it never well-spams)
        if let Some(t) = self.table {
            if w.buildings[t].alive() {
                if let Err(Refusal::SupplyBlocked) = w.queue_train(t, "soldier") {
                    let cost = w.facs[team].structures["well"].cost as f64;
                    if self.wells < 6 && w.ink[team] >= cost {
                        w.ink[team] -= cost;
                        let (hx, hy) = (w.buildings[hall].x, w.buildings[hall].y);
                        w.add_building(
                            team,
                            "well",
                            hx - 90.0 - self.wells as f64 * 50.0,
                            hy - 70.0,
                            true,
                        );
                        self.wells += 1;
                    }
                }
            }
        }
        // 4) muster & march
        let army: Vec<usize> = w
            .units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.team == team && !u.worker && u.alive())
            .map(|(i, _)| i)
            .collect();
        if army.len() >= self.muster {
            if let Some(target) = w
                .buildings
                .iter()
                .find(|b| b.team != team && b.kind == "scriptorium" && b.alive())
            {
                let (tx, ty) = (target.x, target.y);
                for i in army {
                    if matches!(w.units[i].job, Job::Idle | Job::Seek { .. }) {
                        w.units[i].job = Job::AttackMove { x: tx, y: ty };
                    }
                }
            }
        }
        // 5) siege: an army parked on the enemy hall breaks it (match_sim
        //    units only fight units; hall damage is the commander's siege
        //    order — rts.py units attack structures natively, next slice)
        let foes: Vec<(f64, f64, f64)> = w
            .buildings
            .iter()
            .filter(|b| b.team != team && b.alive())
            .map(|b| (b.x, b.y, b.r))
            .collect();
        let mut siege = 0.0;
        for &i in w
            .units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.team == team && !u.worker && u.alive())
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
            .iter()
        {
            let (ux, uy, dmg) = (w.units[i].x, w.units[i].y, w.units[i].dmg);
            for &(bx, by, br) in &foes {
                if ((ux - bx).powi(2) + (uy - by).powi(2)).sqrt() < br + 30.0 {
                    siege += dmg;
                }
            }
        }
        if siege > 0.0 {
            for b in w.buildings.iter_mut() {
                if b.team != team && b.alive() {
                    // 0.5s of the army's reach-in damage per think (2 Hz)
                    b.hp -= siege * 0.5 * 0.5;
                }
            }
        }
    }
}

/// Fight `fac_a` vs `fac_b` for up to `max_secs`; render a frame every
/// `frame_every` seconds into `out_dir` when given.
pub fn run_skirmish(
    book: &BTreeMap<String, Faction>,
    a: &str,
    b: &str,
    max_secs: f64,
    out_dir: Option<&Path>,
    frame_every: f64,
) -> SkirmishReport {
    let mut w = World::new(&book[a], &book[b]);
    w.add_building(0, "scriptorium", 300.0, 620.0, true);
    w.add_building(1, "scriptorium", 1500.0, 380.0, true);
    w.nodes.push(Node { x: 470.0, y: 560.0, amount: 3000.0 });
    w.nodes.push(Node { x: 1330.0, y: 430.0, amount: 3000.0 });
    w.nodes.push(Node { x: 900.0, y: 520.0, amount: 1500.0 });
    w.plateaus.push((900.0, 500.0, 140.0));
    for i in 0..3 {
        w.add_unit(0, "scribe", 330.0 + i as f64 * 24.0, 640.0);
        w.add_unit(1, "scribe", 1470.0 - i as f64 * 24.0, 400.0);
    }
    let mut c0 = Commander { team: 0, table: None, muster: 5, wells: 0 };
    let mut c1 = Commander { team: 1, table: None, muster: 5, wells: 0 };
    let plats: Vec<Plateau> = w
        .plateaus
        .iter()
        .map(|&(x, y, r)| Plateau { x, y, r })
        .collect();
    let cam = Camera { cam_x: 150.0, cam_y: 150.0, zoom: 0.72 };

    let dt = 1.0 / 30.0;
    let mut think_acc = 0.0;
    let mut frame_acc = frame_every; // render t=0 too
    let mut frames = Vec::new();
    while w.winner.is_none() && w.t < max_secs {
        think_acc += dt;
        if think_acc >= 0.5 {
            think_acc = 0.0;
            c0.think(&mut w);
            c1.think(&mut w);
        }
        if let Some(dir) = out_dir {
            frame_acc += dt;
            if frame_acc >= frame_every {
                frame_acc = 0.0;
                let f = render(&w, &plats, &cam, 1280, 800);
                let p = dir.join(format!("skirmish_{:04}.png", (w.t * 10.0) as u64));
                if save_png(&f, &p).is_ok() {
                    frames.push(p);
                }
            }
        }
        w.tick(dt);
    }
    SkirmishReport {
        winner: w.winner,
        t: w.t,
        ink: w.ink,
        units_left: [
            w.units.iter().filter(|u| u.team == 0 && u.alive()).count(),
            w.units.iter().filter(|u| u.team == 1 && u.alive()).count(),
        ],
        frames,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::load_factbook;

    #[test]
    fn a_full_skirmish_resolves() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let book = load_factbook(&dir).unwrap();
        let rep = run_skirmish(&book, "swarm", "rootbound", 600.0, None, 5.0);
        assert!(rep.winner.is_some(), "a decisive match within 10 minutes");
        assert!(rep.t > 20.0, "not instant — economies had to build");
        let win = rep.winner.unwrap();
        assert!(rep.units_left[win] > 0, "the winner should have an army left");
        assert_eq!(rep.units_left[1 - win], 0, "the loser's forces are unmade");
    }

    #[test]
    fn skirmish_renders_frames() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let book = load_factbook(&dir).unwrap();
        let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/skirmish");
        std::fs::create_dir_all(&out).unwrap();
        let rep = run_skirmish(&book, "press", "windwrit", 40.0, Some(&out), 15.0);
        assert!(rep.frames.len() >= 2, "at least two frames over 40s");
        for f in &rep.frames {
            assert!(f.metadata().unwrap().len() > 10_000);
        }
    }
}
