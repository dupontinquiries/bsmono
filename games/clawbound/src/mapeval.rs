//! Map fairness — adapted from early-fps `mapeval.rs` (openness / flow / cover /
//! verticality → S–F grade). Extended with a hunter-vs-cat **fairness** axis:
//! cats need cover, hunters need sightline, so a good map balances the two.

use crate::world;

pub struct MapScore {
    pub openness: f32,
    pub flow: f32,
    pub cover: f32,
    pub verticality: f32,
    pub has_landmark: bool,
    pub total: f32,
    pub fairness: f32,
}

impl MapScore {
    pub fn grade(&self) -> char {
        match self.total as i32 {
            90..=200 => 'S', 80..=89 => 'A', 70..=79 => 'B',
            60..=69 => 'C', 45..=59 => 'D', _ => 'F',
        }
    }
}

pub fn score() -> MapScore {
    let n = 64usize;
    let half = world::SIZE * 0.8;
    let step = 2.0 * half / n as f32;
    let (mut land, mut open) = (0u32, 0u32);
    let mut cover_sum = 0.0f32;
    let mut hs: Vec<f32> = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let x = -half + i as f32 * step;
            let z = -half + j as f32 * step;
            let h = world::height_at(x, z);
            if h < world::WATER_Y { continue; } // water isn't walkable land
            land += 1; hs.push(h);
            let b = world::biome_at(x, z);
            cover_sum += b.tree;                 // foliage density ≈ cover
            let slope = (world::height_at(x + step, z) - h).abs() + (world::height_at(x, z + step) - h).abs();
            if b.tree < 0.2 && slope < 1.5 { open += 1; } // open, walkable = sightline
        }
    }
    let total_cells = (n * n) as f32;
    let flow = land as f32 / total_cells;
    let cover = if land > 0 { cover_sum / land as f32 } else { 0.0 };
    let openness = if land > 0 { open as f32 / land as f32 } else { 0.0 };
    let mean = hs.iter().sum::<f32>() / hs.len().max(1) as f32;
    let var = hs.iter().map(|h| (h - mean) * (h - mean)).sum::<f32>() / hs.len().max(1) as f32;
    let verticality = (var.sqrt() / 8.0).clamp(0.0, 1.0);
    let balance = 1.0 - (cover - openness).abs();
    let fairness = (balance * 100.0).clamp(0.0, 100.0);
    let total = (openness * 22.0 + flow * 22.0 + cover * 26.0 + verticality * 15.0 + balance * 15.0).clamp(0.0, 100.0);
    MapScore { openness, flow, cover, verticality, has_landmark: true, total, fairness }
}
