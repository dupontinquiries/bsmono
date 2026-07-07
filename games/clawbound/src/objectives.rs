//! Objectives — the cats' goals, placed on the map by what the terrain affords.
//! Rendezvous seeks a cover cluster (safe gather), Reach an exposed spot (a risky
//! dash the hunter can watch), Forage the leafy margins near water, Survive the
//! middle. Placement scans the map the same way `mapeval` grades it.

use crate::world;

#[derive(Clone, Copy)]
pub enum Kind { Reach, Rendezvous, Survive, Forage }

pub struct Obj { pub kind: Kind, pub x: f32, pub z: f32 }

pub fn color(k: Kind) -> u32 {
    match k {
        Kind::Reach => 0x5fd0ff,
        Kind::Rendezvous => 0xffd24a,
        Kind::Survive => 0xff6a8a,
        Kind::Forage => 0x8ee06a,
    }
}
pub fn name(k: Kind) -> &'static str {
    match k { Kind::Reach => "Reach", Kind::Rendezvous => "Rendezvous", Kind::Survive => "Survive", Kind::Forage => "Forage" }
}

pub fn place() -> Vec<Obj> {
    let n = 56usize;
    let half = 120.0;   // central playable area (not the world edges)
    let step = 2.0 * half / n as f32;
    let mut best_cover = (-1.0f32, 0.0f32, 0.0f32);
    let mut best_open = (-1.0f32, 0.0f32, 0.0f32);
    let mut best_prey = (-1.0f32, 0.0f32, 0.0f32);
    for j in 0..n {
        for i in 0..n {
            let x = -half + i as f32 * step;
            let z = -half + j as f32 * step;
            let h = world::height_at(x, z);
            if h < world::WATER_Y { continue; }
            let b = world::biome_at(x, z);
            let slope = (world::height_at(x + step, z) - h).abs();
            if b.tree > best_cover.0 { best_cover = (b.tree, x, z); }
            let openv = (1.0 - b.tree) - slope * 0.3;
            if openv > best_open.0 { best_open = (openv, x, z); }
            let depth_above = (h - world::WATER_Y).min(6.0) / 6.0;
            let preyv = b.tree * (1.0 - depth_above);   // leafy + near the waterline
            if preyv > best_prey.0 { best_prey = (preyv, x, z); }
        }
    }
    vec![
        Obj { kind: Kind::Rendezvous, x: best_cover.1, z: best_cover.2 },
        Obj { kind: Kind::Reach, x: best_open.1, z: best_open.2 },
        Obj { kind: Kind::Forage, x: best_prey.1, z: best_prey.2 },
        Obj { kind: Kind::Survive, x: 0.0, z: 0.0 },
    ]
}
