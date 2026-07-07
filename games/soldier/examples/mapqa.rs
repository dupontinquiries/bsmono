//! mapqa — a single-command health dashboard for every authored map.
//!
//! Iterating on a growing roster of authored maps used to mean running `mesh_audit`, `sightline_report`,
//! `liveliness_report` and eyeballing `mapeval` separately, then cross-referencing. This rolls all of them
//! into one pass with pass/fail flags, so a regression on any map (a hole in the geometry, a disconnected
//! layout, a wave that can't spawn, a death-gallery sightline) shows up immediately in one table.
//!
//! Usage:
//!   cargo run --example mapqa                # score every authored preset
//!   cargo run --example mapqa -- <substr>    # only maps whose name contains <substr> (case-insensitive)
//!   cargo run --example mapqa -- --fail      # exit non-zero if any map fails a hard check (CI gate)
//!
//! Hard checks (a FAIL means the map is broken, not just mediocre):
//!   * connected      — every room reachable from spawn
//!   * spawns         — a full enemy wave can be placed (guards the wave-spam bug)
//!   * geometry       — no degenerate tris / bad normals (guards holes & shading glitches)
//!   * standable      — the sampled spawn/vantage points sit on real walkable surface
//! Soft metrics (quality, not correctness): mapeval grade, sightline structure, liveliness score.

use early_fps::enemy::populate;
use early_fps::level::{Level, PRESETS};
use early_fps::liveliness::analyze_preset;
use early_fps::mapeval::score;
use early_fps::math::Vec3;
use early_fps::mesh::Mesh3;
use early_fps::sightline;

struct GeomStat {
    degenerate: usize,
    bad_normals: usize,
    tris: usize,
}

fn geom_check(meshes: &[Mesh3]) -> GeomStat {
    let mut degenerate = 0usize;
    let mut bad_normals = 0usize;
    let mut tris = 0usize;
    for m in meshes {
        tris += m.tris.len();
        for tri in &m.tris {
            let a = m.verts[tri[0] as usize];
            let b = m.verts[tri[1] as usize];
            let c = m.verts[tri[2] as usize];
            let cross = (b - a).cross(c - a);
            let area = cross.len() * 0.5;
            if area < 1e-5 {
                degenerate += 1;
                continue;
            }
            let n = cross.normalized();
            if !n.x.is_finite() || !n.y.is_finite() || !n.z.is_finite() {
                bad_normals += 1;
            }
        }
    }
    GeomStat { degenerate, bad_normals, tris }
}

/// Collect the full renderable mesh set for a level (LOD field if present, else the flat list, plus
/// destructible meshes) — the same set the renderer and mesh_audit walk.
fn all_meshes(level: &Level) -> Vec<Mesh3> {
    let mut meshes: Vec<Mesh3> = Vec::new();
    if let Some(field) = &level.lod_field {
        for m in field.render_meshes() {
            meshes.push(m.clone());
        }
    } else {
        meshes.extend(level.meshes.iter().cloned());
    }
    for d in &level.destructibles {
        meshes.extend(d.mesh());
    }
    meshes
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fail_mode = args.iter().any(|a| a == "--fail");
    let filter: Option<String> = args.iter().find(|a| !a.starts_with("--")).map(|s| s.to_lowercase());

    println!(
        "{:<22} {:>5} {:>5} {:>4} {:>6} {:>6} {:>6}  {:<26}",
        "MAP", "grade", "struc", "live", "tris", "conn", "wave", "HARD CHECKS"
    );
    println!("{}", "─".repeat(100));

    let mut any_fail = false;
    let mut n_ok = 0usize;
    let mut n_total = 0usize;

    for p in PRESETS.iter() {
        if let Some(f) = &filter {
            if !p.name.to_lowercase().contains(f.as_str()) {
                continue;
            }
        }
        n_total += 1;
        let level = Level::generate(p.config(7));

        // ── hard checks ────────────────────────────────────────────────────────────────────────────────
        let sc = score(&level);
        let connected = sc.connected;

        let wave = populate(&level, 13, 42);
        let spawns_ok = wave.len() >= 10;
        // every spawned enemy must stand on a real surface
        let standable = wave.iter().all(|e| level.surface_at(early_fps::math::Vec2::new(e.pos.x, e.pos.z)).is_some());

        let meshes = all_meshes(&level);
        let g = geom_check(&meshes);
        let geom_ok = g.degenerate == 0 && g.bad_normals == 0;

        let hard_ok = connected && spawns_ok && standable && geom_ok;
        if hard_ok {
            n_ok += 1;
        } else {
            any_fail = true;
        }

        // ── soft metrics ───────────────────────────────────────────────────────────────────────────────
        let sr = sightline::analyze(&level);
        let live = analyze_preset(p.name, 7, 240).map(|r| r.score).unwrap_or(0.0);

        // ── build the hard-check annotation ──────────────────────────────────────────────────────────────
        let mut flags: Vec<&str> = Vec::new();
        if !connected {
            flags.push("DISCONNECTED");
        }
        if !spawns_ok {
            flags.push("WAVE-SPAM");
        }
        if !standable {
            flags.push("SPAWN-VOID");
        }
        if g.degenerate > 0 {
            flags.push("DEGEN-TRIS");
        }
        if g.bad_normals > 0 {
            flags.push("BAD-NORMALS");
        }
        let checks = if flags.is_empty() { "ok".to_string() } else { flags.join(" ") };
        let mark = if hard_ok { " " } else { "⚠" };

        println!(
            "{}{:<21} {:>5} {:>5.2} {:>4.2} {:>6} {:>6} {:>6}  {:<26}",
            mark,
            p.name,
            sc.grade(),
            sr.structure_score,
            live,
            g.tris,
            if connected { "yes" } else { "NO" },
            wave.len(),
            checks
        );
    }

    println!("{}", "─".repeat(100));
    println!("{}/{} maps pass all hard checks.", n_ok, n_total);
    let _ = Vec3::ZERO;

    if fail_mode && any_fail {
        eprintln!("FAIL: one or more maps failed a hard check.");
        std::process::exit(1);
    }
}
