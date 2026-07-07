//! mapeval.rs — a headless **map-rating harness**. It scores a generated [`Level`] on a handful of design
//! metrics (openness/sky exposure, sightline flow, cover balance, verticality, connectivity, scale, and
//! whether it has a centerpiece), then folds them into a single 0–100 number with a letter grade.
//!
//! The point is to *iterate*: generate maps, score them, find the weak ones, change the generator, and
//! re-score to confirm the change helped — instead of eyeballing screenshots. The weighting philosophy is
//! borrowed from the logscope `bestregion` selector: connectivity is a hard gate (a map you can't traverse
//! is worthless however pretty), and we reward a *substantial, balanced* map over one that maxes a single
//! axis. Everything here is pure and deterministic so it runs in tests across every preset.

use crate::level::{Cell, Level, MapTheme};

/// A map's design scorecard. Sub-scores are 0..1; `total` is 0..100.
#[derive(Clone, Copy, Debug)]
pub struct MapScore {
    /// Sky exposure: how un-dungeon-like it feels (open biome = 1; built biome = skylights + ceiling height).
    pub openness: f32,
    /// Sightline flow: a healthy mix of short/medium/long engagement ranges (not all corridors, not all field).
    pub flow: f32,
    /// Cover balance: obstacle density near the sweet spot (neither barren nor cluttered).
    pub cover: f32,
    /// Verticality: terrain height variation + raised platforms (multi-level interest).
    pub verticality: f32,
    /// Every room reachable from spawn — a hard requirement.
    pub connected: bool,
    /// Walkable floor tiles (scale).
    pub area: usize,
    /// Has a built centerpiece landmark.
    pub has_landmark: bool,
    /// Weighted 0..100 overall.
    pub total: f32,
}

impl MapScore {
    pub fn grade(&self) -> char {
        match self.total as i32 {
            90..=100 => 'S',
            80..=89 => 'A',
            70..=79 => 'B',
            60..=69 => 'C',
            45..=59 => 'D',
            _ => 'F',
        }
    }

    /// The metric profile that defines how a map *plays* — used to compare maps to a role model. Connectivity
    /// is excluded (it's a binary gate, not a style axis).
    pub fn profile(&self) -> [f32; 4] {
        [self.openness, self.flow, self.cover, self.verticality]
    }
}

/// Flood-fill connectivity: is every room centre reachable from room 0 over walkable cells?
fn all_rooms_connected(level: &Level) -> bool {
    if level.rooms.len() < 2 {
        return true;
    }
    let (sx, sz) = level.rooms[0].rect.center();
    let mut seen = vec![false; (level.w * level.h) as usize];
    let mut stack = vec![(sx, sz)];
    while let Some((x, z)) = stack.pop() {
        if !level.in_bounds(x, z) {
            continue;
        }
        let idx = (z * level.w + x) as usize;
        if seen[idx] || !level.cell(x, z).walkable() {
            continue;
        }
        seen[idx] = true;
        stack.extend_from_slice(&[(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]);
    }
    level.rooms.iter().all(|r| {
        let (cx, cz) = r.rect.center();
        level.in_bounds(cx, cz) && seen[(cz * level.w + cx) as usize]
    })
}

/// Score one generated level.
pub fn score(level: &Level) -> MapScore {
    let (w, h) = (level.w, level.h);
    let ts = level.ts();

    // Collect floor tiles.
    let mut floor: Vec<(i32, i32)> = Vec::new();
    for z in 0..h {
        for x in 0..w {
            if level.cell(x, z) == Cell::Floor {
                floor.push((x, z));
            }
        }
    }
    let area = floor.len().max(1);

    // ── Openness ────────────────────────────────────────────────────────────────────────────────────────
    let openness = if level.cfg.theme.open_sky() || level.open_air {
        1.0
    } else {
        // built biome: roof openings (skylights) + a comfort factor for ceiling height
        let sky_tiles = floor.iter().filter(|&&(x, z)| level.skylights.iter().any(|r| x >= r.x0 && x < r.x1 && z >= r.z0 && z < r.z1)).count();
        let sky_frac = (sky_tiles as f32 / area as f32).min(0.45);
        let ceil_factor = ((level.ceil_y - 4.0) / 6.0).clamp(0.0, 1.0);
        (sky_frac * 2.0 + ceil_factor * 0.45).clamp(0.0, 1.0)
    };

    // ── Flow (sightline variety) ──────────────────────────────────────────────────────────────────────
    // Sample floor tiles; the longest open cardinal run from each lands in short / mid / long buckets.
    // A run is broken not only by walls but by *tall* cover (an obstacle that reaches eye height) — low cover
    // you can see and shoot over doesn't count. This is what lets deliberate cover actually shape engagement
    // ranges, the way it does in the real game where obstacles block line-of-sight.
    let eye = level.floor_y + 1.5;
    let mut tall = vec![false; (level.w * level.h) as usize];
    for o in &level.obstacles {
        if o.center.y + o.half.y < eye || o.center.y - o.half.y > eye {
            continue; // sits entirely below or above eye level → doesn't break a standing sightline
        }
        let tx0 = ((o.center.x - o.half.x) / ts) as i32;
        let tx1 = ((o.center.x + o.half.x) / ts) as i32;
        let tz0 = ((o.center.z - o.half.z) / ts) as i32;
        let tz1 = ((o.center.z + o.half.z) / ts) as i32;
        for tz in tz0..=tz1 {
            for tx in tx0..=tx1 {
                if level.in_bounds(tx, tz) {
                    tall[(tz * level.w + tx) as usize] = true;
                }
            }
        }
    }
    let dirs = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)];
    let (mut s_short, mut s_mid, mut s_long, mut n) = (0u32, 0u32, 0u32, 0u32);
    let step = (area / 400).max(1);
    for (i, &(x, z)) in floor.iter().enumerate() {
        if i % step != 0 {
            continue;
        }
        let mut run = 0;
        for (dx, dz) in dirs {
            let mut d = 1;
            while d < 60 && level.cell(x + dx * d, z + dz * d).walkable() {
                let (nx, nz) = (x + dx * d, z + dz * d);
                if level.in_bounds(nx, nz) && tall[(nz * level.w + nx) as usize] {
                    break; // tall cover ends the sightline here
                }
                d += 1;
            }
            run = run.max(d);
        }
        let world = run as f32 * ts;
        if world < 9.0 {
            s_short += 1;
        } else if world < 26.0 {
            s_mid += 1;
        } else {
            s_long += 1;
        }
        n += 1;
    }
    let nf = n.max(1) as f32;
    let (fs, fm, fl) = (s_short as f32 / nf, s_mid as f32 / nf, s_long as f32 / nf);
    // Ideal spread: mostly medium engagement ranges, some short rooms, a few long lanes.
    let (is, im, il) = (0.30f32, 0.45f32, 0.25f32);
    let flow = (1.0 - 0.5 * ((fs - is).abs() + (fm - im).abs() + (fl - il).abs())).clamp(0.0, 1.0);

    // ── Cover balance ───────────────────────────────────────────────────────────────────────────────────
    // Obstacles per floor tile; sweet spot ~1 per 22 tiles. For authored maps, use cover_obstacles
    // (only freestanding props, not building shells) so the density reading isn't inflated to zero.
    let cover_count = if level.cover_obstacles.is_empty() {
        level.obstacles.len()
    } else {
        level.cover_obstacles.len()
    };
    let density = cover_count as f32 / area as f32;
    let ideal_d = 1.0 / 22.0;
    let cover = (1.0 - ((density - ideal_d).abs() / (ideal_d * 1.8)).min(1.0)).clamp(0.0, 1.0);

    // ── Verticality ─────────────────────────────────────────────────────────────────────────────────────
    // Height spread across floor tiles + a bonus for raised platforms.
    let mut hmin = f32::MAX;
    let mut hmax = f32::MIN;
    let mut hsum = 0.0f32;
    for &(x, z) in &floor {
        let hgt = level.tile_h(x, z);
        hmin = hmin.min(hgt);
        hmax = hmax.max(hgt);
        hsum += hgt;
    }
    let _ = hsum;
    let spread = if hmax > hmin { (hmax - hmin).min(8.0) / 8.0 } else { 0.0 };
    let plat_bonus = (level.platforms.len() as f32 / 6.0).min(1.0) * 0.4;
    let verticality = (spread * 0.8 + plat_bonus).clamp(0.0, 1.0);

    // ── Connectivity + landmark + scale ─────────────────────────────────────────────────────────────────
    let connected = all_rooms_connected(level);
    // A landmark is a distinctive focal structure that helps players orient. Count either a bright emissive
    // feature (a beacon, sign, glowing core) OR a physically large-and-tall structure (a tower, cabin, monument
    // — a chunky mesh that rises well above the floor). The old test only caught emissive ones, so natural
    // maps with a real built landmark (e.g. Redwood Canyon's ranger cabin) were scored as landmark-less.
    let floor_y = level.floor_y;
    let has_landmark = level.meshes.iter().any(|m| {
        let emissive_beacon = m.emissive > 0.4 && m.tri_count() > 24;
        let tall_structure = m.tri_count() > 40 && {
            // does this mesh rise more than ~3.5m above the floor and have real footprint?
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            let (mut xlo, mut xhi, mut zlo, mut zhi) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
            for v in &m.verts {
                lo = lo.min(v.y);
                hi = hi.max(v.y);
                xlo = xlo.min(v.x);
                xhi = xhi.max(v.x);
                zlo = zlo.min(v.z);
                zhi = zhi.max(v.z);
            }
            hi - floor_y > 3.5 && (xhi - xlo) > 2.0 && (zhi - zlo) > 2.0
        };
        emissive_beacon || tall_structure
    });
    let scale = ((area as f32) / 4000.0).min(1.0);

    // ── Weighted total (bestregion philosophy: connectivity gates everything) ─────────────────────────────
    let weighted = openness * 0.30 + flow * 0.22 + cover * 0.16 + verticality * 0.16 + scale * 0.06 + if has_landmark { 0.10 } else { 0.0 };
    let gate = if connected { 1.0 } else { 0.25 };
    let total = (weighted * gate * 100.0).clamp(0.0, 100.0);

    MapScore { openness, flow, cover, verticality, connected, area, has_landmark, total }
}

/// Score every preset (one seed each) and return them sorted best-first. Deterministic.
pub fn rank_presets(seed: u64) -> Vec<(&'static str, MapScore)> {
    let mut out: Vec<(&'static str, MapScore)> = crate::level::PRESETS
        .iter()
        .map(|p| (p.name, score(&Level::generate(p.config(seed)))))
        .collect();
    out.sort_by(|a, b| b.1.total.partial_cmp(&a.1.total).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// A compact text report of the ranking — handy to print from a test while iterating.
pub fn report(seed: u64) -> String {
    let mut s = String::from("MAP RATINGS  (open / flow / cover / vert)\n");
    for (name, sc) in rank_presets(seed) {
        s.push_str(&format!(
            "{:>5.1} {}  {:<22} o{:.2} f{:.2} c{:.2} v{:.2}{}\n",
            sc.total,
            sc.grade(),
            name,
            sc.openness,
            sc.flow,
            sc.cover,
            sc.verticality,
            if sc.connected { "" } else { "  !DISCONNECTED" },
        ));
    }
    s
}

// ── Role-model-guided selection ──────────────────────────────────────────────────────────────────────────
// Instead of judging candidates only by raw score, hold up a known-good map as the *exemplar* and rank new
// candidates by how closely they resemble how it plays. This is how a designer says "make more like THIS one"
// — the role model's metric profile becomes the target the factory filters toward.

/// How alike two maps play: Euclidean distance between their metric profiles (0 = identical). A disconnected
/// candidate is pushed to the back regardless of profile, since it can't actually be the role model's peer.
pub fn profile_distance(role: &MapScore, cand: &MapScore) -> f32 {
    if !cand.connected {
        return f32::MAX;
    }
    let (a, b) = (role.profile(), cand.profile());
    a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
}

/// The metric profile of a named role-model preset (e.g. "Blighted Watchtower"). `None` if no such preset.
pub fn role_model(name: &str, seed: u64) -> Option<MapScore> {
    crate::level::PRESETS.iter().find(|p| p.name == name).map(|p| score(&Level::generate(p.config(seed))))
}

/// Rank every preset by resemblance to a role model (closest first): `(name, distance, score)`. The role model
/// itself sorts to the top (distance ~0). Lets you see which existing archetypes already play like the
/// exemplar and which are furthest from it.
pub fn rank_like(role: &MapScore, seed: u64) -> Vec<(&'static str, f32, MapScore)> {
    let mut out: Vec<(&'static str, f32, MapScore)> = crate::level::PRESETS
        .iter()
        .map(|p| {
            let sc = score(&Level::generate(p.config(seed)));
            (p.name, profile_distance(role, &sc), sc)
        })
        .collect();
    out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Scan generated candidates (graph-skinned maps across seeds) and keep the `keep` that play most like the
/// role model — the factory's "make new maps in the spirit of THIS one" loop. Returns `(seed, distance,
/// score)` closest-first. Each candidate is a graph map on `theme`, so its plan is fresh but its feel is
/// filtered to match the exemplar.
pub fn candidates_like(role: &MapScore, theme: MapTheme, keep: usize, budget: usize, base_seed: u64) -> Vec<(u64, f32, MapScore)> {
    let mut scored: Vec<(u64, f32, MapScore)> = Vec::new();
    for i in 0..budget {
        let seed = base_seed.wrapping_add(i as u64).wrapping_mul(0x100_0000_01B3);
        let cfg = crate::level::MapConfig { seed, theme, size: 152, ..Default::default() };
        // skin a fresh topology graph so candidates differ in plan, not just palette
        let mut cfg = cfg;
        cfg.graph = true;
        let sc = score(&Level::generate(cfg));
        let d = profile_distance(role, &sc);
        if d.is_finite() {
            scored.push((seed, d, sc));
        }
    }
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(keep);
    scored
}

/// A report of which presets most resemble a role-model map (and which least).
pub fn role_model_report(name: &str, seed: u64) -> String {
    let Some(role) = role_model(name, seed) else {
        return format!("no role model named '{}'\n", name);
    };
    let ranked = rank_like(&role, seed);
    let p = role.profile();
    let mut s = format!(
        "ROLE MODEL: {}  (open {:.2} / flow {:.2} / cover {:.2} / vert {:.2})\nclosest matches:\n",
        name, p[0], p[1], p[2], p[3]
    );
    for (n, d, sc) in ranked.iter().take(8) {
        let pp = sc.profile();
        s.push_str(&format!("  d{:.2}  {:<22} o{:.2} f{:.2} c{:.2} v{:.2}\n", d, n, pp[0], pp[1], pp[2], pp[3]));
    }
    s.push_str("furthest:\n");
    for (n, d, _) in ranked.iter().rev().take(3) {
        s.push_str(&format!("  d{:.2}  {}\n", d, n));
    }
    s
}

/// Average openness of the built (ceiling-bound) biomes — the metric the openness pass should lift.
pub fn built_openness(seed: u64) -> f32 {
    let built = [
        MapTheme::Industrial,
        MapTheme::Bunker,
        MapTheme::Warehouse,
        MapTheme::Urban,
        MapTheme::Military,
    ];
    let mut sum = 0.0;
    for th in built {
        let lvl = Level::generate(crate::level::MapConfig { seed, theme: th, size: 150, ..Default::default() });
        sum += score(&lvl).openness;
    }
    sum / built.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::{MapConfig, MapTheme};

    #[test]
    fn open_biomes_score_high_openness_built_ones_lower() {
        let meadow = score(&Level::generate(MapConfig { seed: 1, theme: MapTheme::Meadow, size: 160, ..Default::default() }));
        let bunker = score(&Level::generate(MapConfig { seed: 1, theme: MapTheme::Bunker, size: 150, ..Default::default() }));
        assert!(meadow.openness > 0.9, "meadow should read fully open");
        assert!(bunker.openness < meadow.openness, "a sealed bunker should be less open than a meadow");
    }

    #[test]
    fn no_authored_map_emits_degenerate_geometry() {
        // Regression guard (found via the `mapqa` harness): several maps were emitting zero-area triangles
        // from the LOD impostor pass on thin-vertical chunks. Degenerate tris waste fill and can produce
        // garbage normals/shading. Scan a spread of presets across the roster — including the LOD field and
        // destructibles, the same geometry the renderer walks — and require zero degenerate triangles.
        use crate::level::PRESETS;
        use crate::mesh::Mesh3;
        let collect = |level: &Level| -> Vec<Mesh3> {
            let mut m: Vec<Mesh3> = Vec::new();
            if let Some(f) = &level.lod_field {
                for x in f.render_meshes() {
                    m.push(x.clone());
                }
            } else {
                m.extend(level.meshes.iter().cloned());
            }
            for d in &level.destructibles {
                m.extend(d.mesh());
            }
            m
        };
        // stride through the roster so the test stays fast but broad
        for p in PRESETS.iter().step_by(3) {
            let level = Level::generate(p.config(7));
            let mut degen = 0usize;
            for mesh in collect(&level) {
                for tri in &mesh.tris {
                    let a = mesh.verts[tri[0] as usize];
                    let b = mesh.verts[tri[1] as usize];
                    let c = mesh.verts[tri[2] as usize];
                    if (b - a).cross(c - a).len() * 0.5 < 1e-5 {
                        degen += 1;
                    }
                }
            }
            assert_eq!(degen, 0, "{} emits {} degenerate triangle(s)", p.name, degen);
        }
    }

    #[test]
    fn every_preset_clears_the_quality_bar_and_connects() {
        // The ranking doubles as a regression guard: nothing should be disconnected or score like garbage.
        for (name, sc) in rank_presets(7) {
            assert!(sc.connected, "{} generated disconnected", name);
            assert!(sc.total >= 35.0, "{} scored too low: {:.1} (o{:.2} f{:.2} c{:.2} v{:.2})", name, sc.total, sc.openness, sc.flow, sc.cover, sc.verticality);
        }
    }

    #[test]
    fn the_openness_pass_unsealed_the_built_biomes() {
        // The whole point of the harness: confirm the skylight pass lifted built biomes far off the floor.
        // (Baseline before skylights was ~0.015 average; the pass should put it well above 0.4.)
        assert!(built_openness(7) > 0.4, "built biomes should no longer feel sealed: {:.3}", built_openness(7));
    }

    #[test]
    fn the_report_renders() {
        let r = report(3);
        assert!(r.contains("MAP RATINGS"));
        assert!(r.lines().count() > 10);
    }
#[test]
fn the_role_model_is_its_own_closest_match_and_breeds_lookalikes() {
    use crate::mapeval::{candidates_like, rank_like, role_model};
    let role = role_model("Blighted Watchtower", 7).expect("role model exists");
    let ranked = rank_like(&role, 7);
    // the exemplar sorts to the top (distance ~0)
    assert_eq!(ranked[0].0, "Blighted Watchtower");
    assert!(ranked[0].1 < 0.01, "role model should match itself, d={}", ranked[0].1);
    // distances are sorted ascending
    for w in ranked.windows(2) {
        assert!(w[0].1 <= w[1].1, "rank_like must be closest-first");
    }
    // breeding fresh graph candidates yields connected lookalikes that genuinely resemble the exemplar
    let bred = candidates_like(&role, crate::level::MapTheme::Meadow, 3, 40, 1);
    assert!(bred.len() >= 1, "should breed at least one lookalike");
    assert!(bred.iter().all(|(_, d, s)| s.connected && *d < 0.3), "bred candidates should play like the role model");
}

}
