//! mapstudy — learn soldier map blueprints from 3D architecture scans (IQM).
//!
//! The reference-town pipeline proved the thesis: stamp REAL spatial design instead of
//! procedural guesswork. This tool extends it to 3D scans of real buildings: parse an IQM's
//! geometry, find the ground plane, rasterize the massing into a coarse grid, classify each
//! cell, and emit a height-aware blueprint the game stamps like `blueprints/town.txt`.
//!
//! Legend written (superset of the town legend):
//!   '^' outside the footprint (nature/scatter)      '.' open ground
//!   '+' paving (dense flat low geometry: plaza/street)
//!   'o' courtyard (open cell fully enclosed by massing — flood-fill from the border)
//!   'a' arch/gate (geometry only OVERHEAD: walkable under, lintel above)
//!   'c' column (small isolated massing surrounded by open floor — the cistern/temple/nave
//!       RHYTHM: reads as slender cover, not wall)
//!   'r' rampart (thin curtain-wall line ringing a courtyard — the castles' defensive
//!       CIRCULATION: stamps as a walkable wall-walk with parapet + stairs)
//!   '1'..'9' massing, tier = quantized height (curtain wall vs keep vs spire)
//!
//! Usage: mapstudy <dir-with-iqm> <out-blueprint-dir>
use std::path::Path;

fn u32le(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]]) }
fn f32le(b: &[u8], o: usize) -> f32 { f32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]]) }

struct Model { pos: Vec<[f32; 3]>, tris: Vec<[u32; 3]> }

fn parse_iqm(b: &[u8]) -> Option<Model> {
    if b.len() < 124 || &b[..16] != b"INTERQUAKEMODEL\0" { return None; }
    let num_vertexarrays = u32le(b, 44) as usize;
    let num_vertexes = u32le(b, 48) as usize;
    let ofs_vertexarrays = u32le(b, 52) as usize;
    let num_triangles = u32le(b, 56) as usize;
    let ofs_triangles = u32le(b, 60) as usize;
    let mut pos = Vec::new();
    for i in 0..num_vertexarrays {
        let r = ofs_vertexarrays + i * 20;
        let (ty, fmt, size, off) = (u32le(b, r), u32le(b, r+8), u32le(b, r+12) as usize, u32le(b, r+16) as usize);
        if ty == 0 && fmt == 7 && size >= 3 {              // POSITION, FLOAT
            pos.reserve(num_vertexes);
            for v in 0..num_vertexes {
                let o = off + v * size * 4;
                pos.push([f32le(b, o), f32le(b, o+4), f32le(b, o+8)]);
            }
        }
    }
    if pos.is_empty() { return None; }
    let mut tris = Vec::with_capacity(num_triangles);
    for t in 0..num_triangles {
        let o = ofs_triangles + t * 12;
        tris.push([u32le(b, o), u32le(b, o+4), u32le(b, o+8)]);
    }
    Some(Model { pos, tris })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (src, out) = (&args[1], &args[2]);
    for entry in std::fs::read_dir(src).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().and_then(|e| e.to_str()) != Some("iqm") { continue; }
        let bytes = std::fs::read(&p).unwrap();
        let Some(m) = parse_iqm(&bytes) else { eprintln!("skip (parse): {p:?}"); continue };
        let stem = p.file_stem().unwrap().to_string_lossy();
        let slug: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
            .collect::<String>().split('_').filter(|s| !s.is_empty()).take(3).collect::<Vec<_>>().join("_");
        study(&m, &stem, &slug, Path::new(out));
    }
}

fn study(m: &Model, name: &str, slug: &str, out: &Path) {
    // bbox
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &m.pos { for a in 0..3 { lo[a] = lo[a].min(p[a]); hi[a] = hi[a].max(p[a]); } }
    // Up axis: LET THE MODEL VOTE. A ground-hugging-face heuristic grabbed big flat WALLS on
    // two castles (up=X → solid blobs, courtyards lost). Instead rasterize under all three
    // candidates and keep the axis that yields the most PLAYABLE structure: ground cells +
    // courtyards + gates. The right orientation is the one that reads as a place to fight in.
    let classify = |up: usize| -> (Vec<Vec<char>>, usize, usize, usize) {
        classify_axis(m, &lo, &hi, up)
    };
    let (mut up, mut best_rows, mut best_score) = (1usize, Vec::new(), -1i64);
    for a in 0..3 {
        let (rows, ground, courts, gates) = classify(a);
        let score = ground as i64 + 3 * courts as i64 + 2 * gates as i64;
        if score > best_score { best_score = score; up = a; best_rows = rows; }
    }
    let rows = best_rows;
    let gh = rows.len(); let gw = rows[0].len();
    let mass: usize = rows.iter().flatten().filter(|c| c.is_ascii_digit()).count();
    let courts: usize = rows.iter().flatten().filter(|&&c| c == 'o').count();
    let cols: usize = rows.iter().flatten().filter(|&&c| c == 'c').count();
    let ramps: usize = rows.iter().flatten().filter(|&&c| c == 'r').count();
    let mut txt = format!("; learned from \"{name}\" — {}x{} grid, up-axis {}, {} massing cells, {} courtyard cells, {} columns, {} rampart cells\n; legend: ^ outside  . open  + paving  o courtyard  a arch/gate  c column  r rampart(wall-walk)  1-9 massing height tier\n",
        gw, gh, ["X", "Y", "Z"][up], mass, courts, cols, ramps);
    for r in &rows { txt.push_str(&r.iter().collect::<String>()); txt.push('\n'); }
    std::fs::write(out.join(format!("{slug}.txt")), &txt).unwrap();
    println!("── {name} ({gw}x{gh}, up={}, courts={courts}) → {slug}.txt", ["X","Y","Z"][up]);
    for r in rows.iter().step_by(2) { println!("  {}", r.iter().collect::<String>()); }
}

fn classify_axis(m: &Model, lo: &[f32; 3], hi: &[f32; 3], up: usize) -> (Vec<Vec<char>>, usize, usize, usize) {
    let (ax, az) = match up { 0 => (1, 2), 1 => (0, 2), _ => (0, 1) };
    let (sw, sh) = (hi[ax] - lo[ax], hi[az] - lo[az]);
    let span_up = (hi[up] - lo[up]).max(1e-6);
    let long = 56.0f32; let scale = long / sw.max(sh);
    let (gw, gh) = (((sw * scale).round() as usize).clamp(24, 56), ((sh * scale).round() as usize).clamp(24, 56));
    #[derive(Clone, Copy)] struct C { n: u32, minh: f32, maxh: f32, hist: [u16; 8] }
    let mut g = vec![C { n: 0, minh: f32::MAX, maxh: f32::MIN, hist: [0; 8] }; gw * gh];
    for t in &m.tris {
        let c = |i: usize| m.pos[t[i] as usize];
        let cx = (c(0)[ax] + c(1)[ax] + c(2)[ax]) / 3.0;
        let cz = (c(0)[az] + c(1)[az] + c(2)[az]) / 3.0;
        let cy = (c(0)[up] + c(1)[up] + c(2)[up]) / 3.0;
        let gx = (((cx - lo[ax]) / sw * gw as f32) as usize).min(gw - 1);
        let gz = (((cz - lo[az]) / sh * gh as f32) as usize).min(gh - 1);
        let h01 = (cy - lo[up]) / span_up;
        let cell = &mut g[gz * gw + gx];
        cell.n += 1; cell.minh = cell.minh.min(h01); cell.maxh = cell.maxh.max(h01);
        cell.hist[((h01 * 8.0) as usize).min(7)] += 1;
    }
    let dense = { let mut ns: Vec<u32> = g.iter().map(|c| c.n).filter(|&n| n > 0).collect(); ns.sort_unstable();
                  if ns.is_empty() { 1 } else { ns[ns.len() / 2] } };
    let mut rows: Vec<Vec<char>> = vec![vec!['^'; gw]; gh];
    for z in 0..gh { for x in 0..gw {
        let c = g[z * gw + x];
        rows[z][x] = if c.n == 0 { '^' }
            else if c.maxh < 0.06 { if c.n >= dense { '+' } else { '.' } }
            else if c.minh > 0.30 && c.maxh > 0.35 { 'a' }        // overhead-only (provisional)
            else { char::from_digit(((c.maxh * 9.0).ceil() as u32).clamp(1, 9), 10).unwrap() };
    }}
    // ── hollow halls (solid scans): floor+roof with an EMPTY MID-BAND ──
    // The Amman cistern and the big prayer halls scan with interior floors, so minh≈0 and the
    // overhead-only ('a') rule never sees them — every cell reads as solid wall. But a wall is
    // geometry at ALL heights; a roofed hall is geometry at the BOTTOM and the TOP with air
    // between. The per-cell height histogram tells them apart: hollow = populated low band +
    // populated high band + near-empty middle. A large connected hollow region is a hypostyle
    // hall: it becomes open floor, and the mid-FULL cells inside it (the shafts that DO cross
    // the air gap) are its columns — the exact rhythm the scan encodes.
    {
        let hollow = |c: &C| -> bool {
            if c.n < 6 { return false; }
            let low = c.hist[0] as u32 + c.hist[1] as u32;
            let high = c.hist[5] as u32 + c.hist[6] as u32 + c.hist[7] as u32;
            // vaulted ceilings (the cistern!) hang into the upper-middle band, so the "air"
            // test is the WALK band only: bins 2-4 (roughly chest height to under-vault)
            let mid: u32 = c.hist[2..5].iter().map(|&v| v as u32).sum();
            low > 0 && high > 0 && (mid as f32) < 0.22 * c.n as f32
        };
        let mut is_h = vec![false; gw * gh];
        for z in 0..gh { for x in 0..gw {
            if rows[z][x].is_ascii_digit() && hollow(&g[z * gw + x]) { is_h[z * gw + x] = true; }
        }}
        let mut seen = vec![false; gw * gh];
        for z0 in 0..gh { for x0 in 0..gw {
            if !is_h[z0 * gw + x0] || seen[z0 * gw + x0] { continue; }
            let mut region = Vec::new(); let mut st = vec![(x0, z0)];
            while let Some((x, z)) = st.pop() {
                if seen[z * gw + x] || !is_h[z * gw + x] { continue; }
                seen[z * gw + x] = true; region.push((x, z));
                if x > 0 { st.push((x - 1, z)); } if x + 1 < gw { st.push((x + 1, z)); }
                if z > 0 { st.push((x, z - 1)); } if z + 1 < gh { st.push((x, z + 1)); }
            }
            if region.len() < 30 { continue; }
            // Guard: count would-be columns FIRST. A "hall" whose supports exceed a quarter of
            // its area isn't a hypostyle reading — it's a cluttered interior mis-splitting
            // (Haunted Parish tagged 368 posts this way). Those stay solid massing.
            let mut col_count = 0usize;
            for &(x, z) in &region {
                for (dx, dz) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, nz) = (x as i32 + dx, z as i32 + dz);
                    if nx < 0 || nz < 0 || nx as usize >= gw || nz as usize >= gh { continue; }
                    if rows[nz as usize][nx as usize].is_ascii_digit() && !is_h[nz as usize * gw + nx as usize] {
                        col_count += 1;
                    }
                }
            }
            if col_count > region.len() / 4 { continue; }
            for &(x, z) in &region { rows[z][x] = '.'; }
            // columns: solid (mid-full) cells with ≥3 hall neighbors — shafts standing in the hall
            for &(x, z) in &region {
                for (dx, dz) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, nz) = (x as i32 + dx, z as i32 + dz);
                    if nx < 0 || nz < 0 || nx as usize >= gw || nz as usize >= gh { continue; }
                    let (nxu, nzu) = (nx as usize, nz as usize);
                    if !rows[nzu][nxu].is_ascii_digit() || is_h[nzu * gw + nxu] { continue; }
                    let halln = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)].iter().filter(|&&(ax, az)| {
                        let (bx, bz) = (nx + ax, nz + az);
                        bx >= 0 && bz >= 0 && (bx as usize) < gw && (bz as usize) < gh
                            && rows[bz as usize][bx as usize] == '.'
                    }).count();
                    if halln >= 3 { rows[nzu][nxu] = 'c'; }
                }
            }
        }}
    }
    // Overhead-only regions: a SMALL one is a gate/arch you walk under; a LARGE one is a roof —
    // and a roofed hall plays as solid massing the fight flows around (the town.txt precedent),
    // which also keeps courtyards as THE open spaces. Without this, every roofed interior read
    // as an arch and palaces dissolved into 'a' seas.
    {
        let mut seen = vec![false; gw * gh];
        for z0 in 0..gh { for x0 in 0..gw {
            if rows[z0][x0] != 'a' || seen[z0 * gw + x0] { continue; }
            let mut region = Vec::new(); let mut st = vec![(x0, z0)];
            while let Some((x, z)) = st.pop() {
                if seen[z * gw + x] || rows[z][x] != 'a' { continue; }
                seen[z * gw + x] = true; region.push((x, z));
                if x > 0 { st.push((x - 1, z)); } if x + 1 < gw { st.push((x + 1, z)); }
                if z > 0 { st.push((x, z - 1)); } if z + 1 < gh { st.push((x, z + 1)); }
            }
            if region.len() > 6 {
                // Two kinds of roofed region hide in a solid scan, and minh tells them apart.
                // A DWELLING/KEEP is roof over rooms-full-of-stuff: many cells reach the ground.
                // A HYPOSTYLE HALL (the Amman cistern, Thian Hock Keng's prayer hall) is roof over
                // OPEN floor held by point supports: few cells reach the ground, and those ARE the
                // columns. The first plays as solid massing (the town.txt precedent); the second is
                // the best interior playspace in the whole set — open it, keep its rhythm.
                let supports: Vec<(usize, usize)> = region.iter().copied()
                    .filter(|&(x, z)| g[z * gw + x].minh < 0.08).collect();
                let frac = supports.len() as f32 / region.len() as f32;
                if region.len() >= 40 && frac < 0.15 {
                    for &(x, z) in &region { rows[z][x] = '.'; }
                    for &(x, z) in &supports { rows[z][x] = 'c'; }
                } else {
                    for &(x, z) in &region {
                        let c = g[z * gw + x];
                        rows[z][x] = char::from_digit(((c.maxh * 9.0).ceil() as u32).clamp(1, 9), 10).unwrap();
                    }
                }
            }
        }}
    }
    // courtyards: open cells NOT reachable from the border through open cells
    let open = |ch: char| matches!(ch, '^' | '.' | '+' | 'a');   // arches are walk-under
    let mut seen = vec![false; gw * gh];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for x in 0..gw { for z in [0, gh - 1] { if open(rows[z][x]) { stack.push((x, z)); } } }
    for z in 0..gh { for x in [0, gw - 1] { if open(rows[z][x]) { stack.push((x, z)); } } }
    while let Some((x, z)) = stack.pop() {
        if seen[z * gw + x] || !open(rows[z][x]) { continue; }
        seen[z * gw + x] = true;
        if x > 0 { stack.push((x - 1, z)); } if x + 1 < gw { stack.push((x + 1, z)); }
        if z > 0 { stack.push((x, z - 1)); } if z + 1 < gh { stack.push((x, z + 1)); }
    }
    for z in 0..gh { for x in 0..gw {
        if open(rows[z][x]) && !seen[z * gw + x] { rows[z][x] = 'o'; }
    }}
    // ── columns: massing clusters of ≤2 cells whose entire boundary is open floor ──
    // The Amman cistern and Thian Hock Keng are FIELDS of point supports; as digits they
    // stamped into wall-stubs and the halls read as rubble. A column is architecture's way
    // of holding a roof without closing space — in gameplay terms: cover that preserves
    // sightline rhythm. Detected structurally, not by name, so any future scan with
    // freestanding supports learns the same way.
    {
        let is_mass = |c: char| c.is_ascii_digit();
        let openish = |c: char| matches!(c, '.' | '+' | 'o' | 'a' | '^');
        let mut candidates: Vec<Vec<(usize, usize)>> = Vec::new();
        let mut seen = vec![false; gw * gh];
        for z0 in 0..gh { for x0 in 0..gw {
            if !is_mass(rows[z0][x0]) || seen[z0 * gw + x0] { continue; }
            let mut region = Vec::new(); let mut st = vec![(x0, z0)];
            while let Some((x, z)) = st.pop() {
                if seen[z * gw + x] || !is_mass(rows[z][x]) { continue; }
                seen[z * gw + x] = true; region.push((x, z));
                if x > 0 { st.push((x - 1, z)); } if x + 1 < gw { st.push((x + 1, z)); }
                if z > 0 { st.push((x, z - 1)); } if z + 1 < gh { st.push((x, z + 1)); }
            }
            if region.len() <= 2 {
                let boundary_open = region.iter().all(|&(x, z)| {
                    [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)].iter().all(|&(dx, dz)| {
                        let (nx, nz) = (x as i32 + dx, z as i32 + dz);
                        if nx < 0 || nz < 0 || nx >= gw as i32 || nz >= gh as i32 { return true; }
                        let nc = rows[nz as usize][nx as usize];
                        openish(nc) || region.contains(&(nx as usize, nz as usize))
                    })
                });
                if boundary_open {
                    candidates.push(region.clone());
                }
            }
        }}
        // A FEW isolated piers are architecture (freestanding columns, gate posts); HUNDREDS
        // are a debris field (the Haunted Parish scan) and stamping them as columns turns
        // rubble into a bamboo forest. Cap: accept up to 40 isolated-pier cells per map,
        // largest-first; past that, the scatter stays massing.
        candidates.sort_by_key(|r| std::cmp::Reverse(r.len()));
        let mut budget = 40usize;
        for region in candidates {
            if region.len() > budget { break; }
            budget -= region.len();
            for (x, z) in region { rows[z][x] = 'c'; }
        }
    }
    // ── ramparts: the courtyard-FACING edge of massing becomes the wall-walk ──
    // Solid photogrammetry makes curtain walls THICK and TALL (Pioz's outer band quantizes
    // to tier 7-9), so "thin low wall line" finds nothing. The honest learning is simpler:
    // a castle's wall-walk is the inner edge of the massing ring around its bailey. Rule:
    // a massing cell (tier 2..=8 — not ground clutter, not the tallest spires) 4-adjacent
    // to a LARGE courtyard cluster (≥12 cells: real baileys, not lightwells) becomes 'r'.
    // Thin by construction — only the adjacency ring — and castle-selective by keying on
    // courtyards, so piazza edges and street walls stay plain massing.
    {
        let is_mass = |c: char| c.is_ascii_digit();
        // courtyard cluster sizes
        let mut cid = vec![usize::MAX; gw * gh];
        let mut sizes: Vec<usize> = Vec::new();
        for z0 in 0..gh { for x0 in 0..gw {
            if rows[z0][x0] != 'o' || cid[z0 * gw + x0] != usize::MAX { continue; }
            let id = sizes.len(); let mut n = 0usize; let mut st = vec![(x0, z0)];
            while let Some((x, z)) = st.pop() {
                if cid[z * gw + x] != usize::MAX || rows[z][x] != 'o' { continue; }
                cid[z * gw + x] = id; n += 1;
                if x > 0 { st.push((x - 1, z)); } if x + 1 < gw { st.push((x + 1, z)); }
                if z > 0 { st.push((x, z - 1)); } if z + 1 < gh { st.push((x, z + 1)); }
            }
            sizes.push(n);
        }}
        let mut mark = vec![false; gw * gh];
        for z in 0..gh { for x in 0..gw {
            let ch = rows[z][x];
            if !is_mass(ch) { continue; }
            let tier = ch.to_digit(10).unwrap();
            let _ = tier; // any massing tier: ruins scan low (Morton), temples tall — the
                          // courtyard-adjacency is the selective part, not the height
            let big_court = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dz)| {
                let (nx, nz) = (x as i32 + dx, z as i32 + dz);
                nx >= 0 && nz >= 0 && (nx as usize) < gw && (nz as usize) < gh
                    && rows[nz as usize][nx as usize] == 'o'
                    && sizes[cid[nz as usize * gw + nx as usize]] >= 6
            });
            if big_court { mark[z * gw + x] = true; }
        }}
        for z in 0..gh { for x in 0..gw { if mark[z * gw + x] { rows[z][x] = 'r'; } } }
    }
    let ground = rows.iter().flatten().filter(|&&c| c == '.' || c == '+').count();
    let courts = rows.iter().flatten().filter(|&&c| c == 'o').count();
    let gates = rows.iter().flatten().filter(|&&c| c == 'a').count();
    (rows, ground, courts, gates)
}
