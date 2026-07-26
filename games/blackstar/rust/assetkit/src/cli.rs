//! assetkit CLI — the Rust port of the python pipeline's measurement and
//! geometry tools. One binary, subcommands sharing the mesh core.
//!
//!   score   sweep a library and rank models (library_score)
//!   view    ascii silhouette, with the lying-down check (ascii_view)
//!   harvest connected-component catalog (part_harvest)
//!   morph   reassembly-swarm decomposition (voxel_morph)
//!   info    one model's numbers
//!
//! Verified against the python it replaces: on imported_jazz_part1.iqm both give
//! 398 verts / 480 tris / 2 components / compactness 12.5, and the IQM writer
//! round-trips through the reader.

mod json;
mod gltf;
mod mesh;
mod iqm;
mod harvest;
mod ascii;
mod pairs;
mod raids;
mod nn;
mod evolve;
mod rigscore;
mod hub;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 { return; }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { walk(&p, out, depth + 1); }
            else if let Some(x) = p.extension().and_then(|s| s.to_str()) {
                let x = x.to_ascii_lowercase();
                if x == "iqm" || x == "obj" || x == "gltf" || x == "glb" { out.push(p); }
            }
        }
    }
}

fn arg(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned()
}
fn argf(args: &[String], key: &str, d: f32) -> f32 {
    arg(args, key).and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn argu(args: &[String], key: &str, d: usize) -> usize {
    arg(args, key).and_then(|s| s.parse().ok()).unwrap_or(d)
}

fn cmd_score(args: &[String]) {
    let root = args.get(2).cloned().unwrap_or_else(|| ".".into());
    let top = argu(args, "--top", 30);
    let min_tris = argu(args, "--min-tris", 50);
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files, 0);
    eprintln!("{} candidate file(s) under {}", files.len(), root);

    let nt = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(4);
    let chunk = (files.len() + nt - 1) / nt.max(1);
    let mut handles = Vec::new();
    for c in files.chunks(chunk.max(1)) {
        let c: Vec<PathBuf> = c.to_vec();
        handles.push(std::thread::spawn(move || {
            let mut v = Vec::new();
            for p in c {
                if let Some(mut m) = mesh::load_any(&p) {
                    if m.f.len() < 32 { continue; }
                    let s = mesh::score(&mut m, min_tris);
                    if !s.scorable { continue; }
                    v.push((p.file_stem().unwrap_or_default().to_string_lossy().to_string(), s));
                }
            }
            v
        }));
    }
    let mut rows: Vec<(String, mesh::Score)> = Vec::new();
    for h in handles { if let Ok(mut v) = h.join() { rows.append(&mut v); } }
    rows.sort_by(|a, b| b.1.bony.total_cmp(&a.1.bony));

    println!("{:<32} {:>9} {:>8} {:>6} {:>8} {:>6} {:>8}  {}",
             "model", "bony", "compact", "coher", "members", "open%", "tris", "class");
    for (n, s) in rows.iter().take(top) {
        println!("{:<32} {:>9.1} {:>8.1} {:>6.3} {:>8} {:>5.0}% {:>8}  {}",
                 n.chars().take(32).collect::<String>(), s.bony, s.compactness,
                 s.coherence, s.members, s.open_frac * 100.0, s.tris, s.class);
    }
    let mut by: HashMap<&str, (usize, f32)> = HashMap::new();
    for (_n, s) in &rows {
        let e = by.entry(s.class).or_insert((0, 0.0));
        e.0 += 1; e.1 += s.compactness;
    }
    println!("\nby class:");
    let mut ks: Vec<_> = by.into_iter().collect();
    ks.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for (k, (n, sum)) in ks {
        println!("  {:<12} {:>5} models, mean compactness {:>7.1}", k, n, sum / n as f32);
    }
    if let Some(csv) = arg(args, "--csv") {
        let mut out = String::from("name,bony,compactness,coherence,members,bbox_fill,open_frac,tris,class\n");
        for (n, s) in &rows {
            out.push_str(&format!("{},{:.3},{:.3},{:.4},{},{:.5},{:.4},{},{}\n",
                n, s.bony, s.compactness, s.coherence, s.members, s.bbox_fill,
                s.open_frac, s.tris, s.class));
        }
        let _ = std::fs::write(&csv, out);
        eprintln!("wrote {}", csv);
    }
}

fn cmd_view(args: &[String]) {
    let p = match args.get(2) { Some(p) => PathBuf::from(p), None => { eprintln!("view <file>"); return; } };
    let width = argu(args, "--width", 48);
    let views = arg(args, "--views").unwrap_or_else(|| "front".into());
    let (rx, ry, rz) = (argf(args, "--rx", 0.0), argf(args, "--ry", 0.0), argf(args, "--rz", 0.0));
    let mut m = match mesh::load_any(&p) { Some(m) => m, None => { eprintln!("could not read"); return; } };
    mesh::normalise(&mut m);
    let v = ascii::rotate(&m.v, rx, ry, rz);
    let mut lo = [f32::MAX; 3]; let mut hi = [f32::MIN; 3];
    for q in &v { for k in 0..3 { lo[k] = lo[k].min(q[k]); hi[k] = hi[k].max(q[k]); } }
    let ext = [hi[0]-lo[0], hi[1]-lo[1], hi[2]-lo[2]];
    let verdict = if ascii::lying_down(ext) {
        format!("!! LYING DOWN -- height {:.2} is the smallest dimension", ext[1])
    } else if ext[1] >= ext[0].max(ext[2]) { "UPRIGHT (tallest axis is Y)".into() }
    else { "upright (widest in X/Z = arm span or bulk)".into() };
    println!("\n{}   {} tris   extent x{:.2} y{:.2} z{:.2}   {}",
             p.file_stem().unwrap_or_default().to_string_lossy(), m.f.len(),
             ext[0], ext[1], ext[2], verdict);
    for view in views.split(',') {
        println!("\n  [{}]", view.trim());
        for l in ascii::render(&m, &v, width, view.trim()) { println!("    {}", l); }
    }
}

fn cmd_harvest(args: &[String]) {
    let root = args.get(2).cloned().unwrap_or_else(|| ".".into());
    let min_tris = argu(args, "--min-tris", 50);
    let out = arg(args, "--out");
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files, 0);
    let mut total = 0usize;
    let mut emitted = 0usize;
    if let Some(o) = &out { let _ = std::fs::create_dir_all(format!("{}/meshes", o)); }
    for p in &files {
        if let Some(m) = mesh::load_any(p) {
            let comps = harvest::components(&m, min_tris);
            if comps.is_empty() { continue; }
            println!("  {:<44} {:5} components",
                     p.file_stem().unwrap_or_default().to_string_lossy()
                      .chars().take(44).collect::<String>(), comps.len());
            total += comps.len();
            if let Some(o) = &out {
                let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                for (i, c) in comps.iter().enumerate().take(64) {
                    let (v, uv, f) = iqm::box_uvs(&c.v, &c.f);
                    let path = format!("{}/meshes/{}_{}.iqm", o, stem, i);
                    if iqm::write_iqm(Path::new(&path), &v, &uv, &f, "plate_blue.png").is_ok() {
                        emitted += 1;
                    }
                }
            }
        }
    }
    println!("\n{} components across {} model(s)", total, files.len());
    if out.is_some() { println!("emitted {} part meshes", emitted); }
}

fn cmd_morph(args: &[String]) {
    let p = match args.get(2) { Some(p) => PathBuf::from(p), None => { eprintln!("morph <file>"); return; } };
    let out = arg(args, "--out").unwrap_or_else(|| "morph".into());
    let name = arg(args, "--name").unwrap_or_else(|| "morph".into());
    let k = argu(args, "--clusters", 96).min(126);
    let spread = argf(args, "--spread", 1.6);
    let swirl = argf(args, "--swirl", 0.8);
    let mut m = match mesh::load_any(&p) { Some(m) => m, None => { eprintln!("could not read"); return; } };
    mesh::normalise(&mut m);
    let _ = std::fs::create_dir_all(&out);

    // Lloyd relaxation on triangle centroids. Compactness matters: a stringy
    // cluster reads as debris when it moves, a compact one reads as plating.
    let cent: Vec<[f32; 3]> = m.f.iter().map(|t| {
        let (a, b, c) = (m.v[t[0] as usize], m.v[t[1] as usize], m.v[t[2] as usize]);
        [(a[0]+b[0]+c[0])/3.0, (a[1]+b[1]+c[1])/3.0, (a[2]+b[2]+c[2])/3.0]
    }).collect();
    let k = k.min(cent.len() / 4).max(1);
    let mut seed = 0x2545F491u64;
    let mut rnd = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;
                       (seed % 100000) as f32 / 100000.0 };
    let mut ctr: Vec<[f32; 3]> = (0..k).map(|i| cent[(i * cent.len()) / k]).collect();
    let mut lab = vec![0usize; cent.len()];
    for _ in 0..8 {
        for (i, c) in cent.iter().enumerate() {
            let mut best = 0usize; let mut bd = f32::MAX;
            for (j, q) in ctr.iter().enumerate() {
                let d = (c[0]-q[0]).powi(2) + (c[1]-q[1]).powi(2) + (c[2]-q[2]).powi(2);
                if d < bd { bd = d; best = j; }
            }
            lab[i] = best;
        }
        let mut acc = vec![[0.0f32; 3]; k];
        let mut cnt = vec![0usize; k];
        for (i, c) in cent.iter().enumerate() {
            for kk in 0..3 { acc[lab[i]][kk] += c[kk]; }
            cnt[lab[i]] += 1;
        }
        for j in 0..k { if cnt[j] > 0 { for kk in 0..3 { ctr[j][kk] = acc[j][kk] / cnt[j] as f32; } } }
    }

    let mut manifest = String::new();
    let mut written = 0usize;
    for j in 0..k {
        let faces: Vec<[u32; 3]> = m.f.iter().enumerate()
            .filter(|(i, _)| lab[*i] == j).map(|(_, t)| *t).collect();
        if faces.len() < 2 { continue; }
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let mut cv = Vec::new();
        let mut cf = Vec::new();
        for t in &faces {
            let mut nt = [0u32; 3];
            for kk in 0..3 {
                let e = remap.entry(t[kk]).or_insert_with(|| {
                    cv.push(m.v[t[kk] as usize]); (cv.len() - 1) as u32 });
                nt[kk] = *e;
            }
            cf.push(nt);
        }
        let mut piv = [0.0f32; 3];
        for q in &cv { for kk in 0..3 { piv[kk] += q[kk]; } }
        for kk in 0..3 { piv[kk] /= cv.len() as f32; }
        let loc: Vec<[f32; 3]> = cv.iter().map(|q| [q[0]-piv[0], q[1]-piv[1], q[2]-piv[2]]).collect();
        let rl = (piv[0]*piv[0] + piv[2]*piv[2]).sqrt().max(1e-5);
        let dir = [piv[0]/rl, 0.0, piv[2]/rl];
        let tan = [-dir[2], 0.0, dir[0]];
        let jit = (rnd() - 0.5) * 2.0;
        let disp = [piv[0] + dir[0]*spread + tan[0]*swirl*jit,
                    piv[1] + (rnd() - 0.5) * 0.8,
                    piv[2] + dir[2]*spread + tan[2]*swirl*jit];
        let (v, uv, f) = iqm::box_uvs(&loc, &cf);
        let file = format!("imported_{}_part{}.iqm", name, written);
        if iqm::write_iqm(Path::new(&format!("{}/{}", out, file)), &v, &uv, &f,
                          "plate_blue.png").is_ok() {
            manifest.push_str(&format!("{} plate_blue.png morph 0 20 {:.5} {:.5} {:.5} -1 {:.5} {:.5} {:.5}\n",
                file, piv[0], piv[1], piv[2], disp[0], disp[1], disp[2]));
            written += 1;
        }
    }
    let _ = std::fs::write(format!("{}/imported_{}_parts.txt", out, name), manifest.trim_end());
    println!("{} clusters -> {}/imported_{}_parts.txt", written, out, name);
}

fn cmd_pairs(args: &[String]) {
    let root = args.get(2).cloned().unwrap_or_else(|| ".".into());
    let out = arg(args, "--out").unwrap_or_else(|| "pairs".into());
    let min_tris = argu(args, "--min-tris", 50);
    let ratio = argu(args, "--ratio", 12);
    let previews = argu(args, "--preview", 8);
    // "box" is the user's two-stage proposal: degrade to the bounding box, so the
    // second network learns "what fills a volume of this shape" rather than
    // "sharpen this slightly blurry thing", which measured as unlearnable.
    let mode = arg(args, "--degrade").unwrap_or_else(|| "decimate".into());
    let _ = std::fs::create_dir_all(format!("{}/preview", out));
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files, 0);
    let ctx = arg(args, "--context").is_some();
    let mut all: Vec<(Vec<f32>, Vec<f32>)> = Vec::new();
    let mut tri: Vec<(Vec<f32>, Vec<f32>, Vec<f32>)> = Vec::new();
    for p in &files {
        let mut m = match mesh::load_any(p) { Some(m) => m, None => continue };
        mesh::normalise(&mut m);
        let mut plo = [f32::MAX; 3];
        let mut phi = [f32::MIN; 3];
        for q in &m.v { for k in 0..3 { plo[k] = plo[k].min(q[k]); phi[k] = phi[k].max(q[k]); } }
        let pspan = [(phi[0]-plo[0]).max(1e-6), (phi[1]-plo[1]).max(1e-6), (phi[2]-plo[2]).max(1e-6)];
        let comps = harvest::components(&m, min_tris);
        if comps.is_empty() { continue; }
        println!("  {:<44} {:5} components",
                 p.file_stem().unwrap_or_default().to_string_lossy()
                  .chars().take(44).collect::<String>(), comps.len());
        for c in comps {
            let raw = mesh::Mesh { v: c.v, f: c.f };
            let place = pairs::placement(&raw, plo, pspan);
            let mut cm = raw;
            mesh::normalise(&mut cm);
            let blob = if mode == "box" { pairs::boxify(&cm) }
                       else { pairs::decimate(&cm, (cm.f.len() / ratio.max(2)).max(12)) };
            let (eb, ed) = (pairs::encode(&blob), pairs::encode(&cm));
            if ctx { tri.push((eb.clone(), pairs::context_plane(&place), ed.clone())); }
            all.push((eb, ed));
        }
    }
    if all.is_empty() { println!("no components"); return; }
    if ctx {
        let _ = pairs::write_triples(Path::new(&format!("{}/triples.f32", out)), &tri);
        println!("  wrote {}/triples.f32 (blob, context, detail)", out);
    }
    let _ = pairs::write_pairs(Path::new(&format!("{}/pairs.f32", out)), &all);
    for (i, (b, d)) in all.iter().take(previews).enumerate() {
        let _ = pairs::write_pgm(Path::new(&format!("{}/preview/{:03}_blob.pgm", out, i)), b);
        let _ = pairs::write_pgm(Path::new(&format!("{}/preview/{:03}_detail.pgm", out, i)), d);
    }
    let _ = std::fs::write(format!("{}/pairs.json", out), format!(
        "{{\n \"count\": {},\n \"height\": {},\n \"width\": {},\n \"layout\": \"blob,detail interleaved f32 LE\",\n \"range\": [-1, 1]\n}}",
        all.len(), pairs::H, pairs::W));
    // report the gap: if the blob is not measurably less detailed the task is
    // degenerate, and no architecture rescues that
    let mut diff = 0.0f64;
    let mut relief_b = 0.0f64;
    let mut relief_d = 0.0f64;
    for (b, d) in &all {
        let mut s = 0.0f64;
        for i in 0..b.len() { s += (b[i] - d[i]).abs() as f64; }
        diff += s / b.len() as f64;
        let occ = |v: &Vec<f32>| -> f64 {
            let on: Vec<f32> = v.iter().copied().filter(|&x| x > -0.99).collect();
            if on.len() < 8 { return 0.0; }
            let mu = on.iter().map(|&x| x as f64).sum::<f64>() / on.len() as f64;
            (on.iter().map(|&x| (x as f64 - mu).powi(2)).sum::<f64>() / on.len() as f64).sqrt()
        };
        relief_b += occ(b); relief_d += occ(d);
    }
    let n = all.len() as f64;
    println!("\n{} pairs, tensors {}x{} in [-1,1]", all.len(), pairs::H, pairs::W);
    println!("  mean |blob-detail|      {:.3}", diff / n);
    println!("  depth relief blob {:.3} vs detail {:.3}  ({:+.0}% -- positive is correct)",
             relief_b / n, relief_d / n, 100.0 * (relief_d / relief_b - 1.0));
    println!("  wrote {}/pairs.f32 ({:.0} MB) + {} previews",
             out, (all.len() * pairs::H * pairs::W * 8) as f64 / 1e6, previews.min(all.len()) * 2);
}

fn cmd_raids(args: &[String]) {
    let maps_path = arg(args, "--maps").unwrap_or_else(|| "content/maps.json".into());
    let out = arg(args, "--out").unwrap_or_else(|| "content/raids.json".into());
    let hdr = arg(args, "--header").unwrap_or_else(|| "engine_src/exengine/raid_plan.h".into());
    let phases = argu(args, "--phases", 3);
    let seed = argu(args, "--seed", 7) as u64;
    let want = arg(args, "--raid").unwrap_or_else(|| "cherno_alpha".into());

    let added = raids::patch_maps(Path::new(&maps_path));
    if !added.is_empty() { println!("added missing maps: {}", added.join(", ")); }
    let maps = raids::map_names(Path::new(&maps_path));
    if maps.is_empty() { eprintln!("no maps found in {}", maps_path); return; }

    let rs = raids::generate(&maps, phases, seed);
    let _ = std::fs::write(&out, raids::to_json(&rs));
    println!("wrote {} ({} raids, {} phases each)", out, rs.len(), phases);

    let pick = rs.iter().find(|r| r.id == want).unwrap_or(&rs[0]);
    let _ = std::fs::write(&hdr, raids::to_header(pick));
    println!("baked '{}' into {}", pick.id, hdr);

    let missing = raids::validate(&rs, &maps);
    if !missing.is_empty() {
        println!("!! phases reference undefined maps: {}", missing.join(", "));
        std::process::exit(1);
    }
    println!("all referenced maps exist");
    let mut spread = [0usize; 4];
    for r in &rs { spread[r.mech as usize] += 1; }
    println!("mechanic spread: purge {} ground {} air {} sync {}",
             spread[0], spread[1], spread[2], spread[3]);
}

/// Stage 1 of the two-stage design: export where every part SITS, rather than what
/// it looks like. 9 numbers per part against a 32k-pixel image is a far
/// better-posed problem, and the target is exact rather than approximated.
fn cmd_place(args: &[String]) {
    let root = args.get(2).cloned().unwrap_or_else(|| ".".into());
    let out = arg(args, "--out").unwrap_or_else(|| "placement".into());
    let min_tris = argu(args, "--min-tris", 50);
    let _ = std::fs::create_dir_all(&out);
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files, 0);
    let mut rows: Vec<(String, usize, [f32; 9])> = Vec::new();
    for p in &files {
        let mut m = match mesh::load_any(p) { Some(m) => m, None => continue };
        mesh::normalise(&mut m);
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for q in &m.v { for k in 0..3 { lo[k] = lo[k].min(q[k]); hi[k] = hi[k].max(q[k]); } }
        let span = [(hi[0]-lo[0]).max(1e-6), (hi[1]-lo[1]).max(1e-6), (hi[2]-lo[2]).max(1e-6)];
        let comps = harvest::components(&m, min_tris);
        if comps.is_empty() { continue; }
        let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
        for (i, c) in comps.iter().enumerate() {
            let cm = mesh::Mesh { v: c.v.clone(), f: c.f.clone() };
            rows.push((name.clone(), i, pairs::placement(&cm, lo, span)));
        }
        println!("  {:<44} {:5} parts",
                 name.chars().take(44).collect::<String>(), comps.len());
    }
    if rows.is_empty() { println!("no parts"); return; }
    let mut csv = String::from("model,part,px,py,pz,sx,sy,sz,ax,ay,az\n");
    let mut raw: Vec<u8> = Vec::with_capacity(rows.len() * 36);
    for (n, i, v) in &rows {
        csv.push_str(&format!("{},{},{:.5},{:.5},{:.5},{:.5},{:.5},{:.5},{:.4},{:.4},{:.4}\n",
            n, i, v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8]));
        for x in v.iter() { raw.extend_from_slice(&x.to_le_bytes()); }
    }
    let _ = std::fs::write(format!("{}/placement.csv", out), csv);
    let _ = std::fs::write(format!("{}/placement.f32", out), &raw);
    // spread check: if every part reports the same position the descriptor is
    // broken and stage 1 would learn a constant
    let mut mn = [f32::MAX; 9];
    let mut mx = [f32::MIN; 9];
    for (_n, _i, v) in &rows {
        for k in 0..9 { mn[k] = mn[k].min(v[k]); mx[k] = mx[k].max(v[k]); }
    }
    println!("\n{} parts -> {}/placement.csv + .f32 (9 f32 each)", rows.len(), out);
    println!("  position range x {:.2}..{:.2}  y {:.2}..{:.2}  z {:.2}..{:.2}",
             mn[0], mx[0], mn[1], mx[1], mn[2], mx[2]);
    println!("  scale range    x {:.3}..{:.3} y {:.3}..{:.3} z {:.3}..{:.3}",
             mn[3], mx[3], mn[4], mx[4], mn[5], mx[5]);
    let flat = (0..9).all(|k| (mx[k] - mn[k]).abs() < 1e-3);
    if flat { println!("  !! descriptors are constant -- stage 1 would learn nothing"); }
    else { println!("  descriptors vary across parts, so there is something to predict"); }
}

/// Render a 64x32 depth plane as text. The whole reason this exists: every visual
/// fault in this project was invisible to the loss numbers and obvious on sight.
/// A falling L1 tells you nothing about whether the output looks like a part.
fn ascii_plane(img: &[f32], w: usize, h: usize, cols: usize) -> Vec<String> {
    const RAMP: &[u8] = b" .:-=+*#%@";
    let sx = (w / cols).max(1);
    let sy = (sx * 2).max(1);                 // characters are ~2:1, so halve rows
    (0..h / sy).map(|r| {
        (0..cols).map(|c| {
            let mut m = -1.0f32;
            for dy in 0..sy { for dx in 0..sx {
                let (y, x) = (r * sy + dy, c * sx + dx);
                if y < h && x < w { m = m.max(img[y * w + x]); }
            }}
            if m <= -0.98 { ' ' } else {
                RAMP[(((m + 1.0) * 0.5 * (RAMP.len() - 1) as f32) as usize)
                     .min(RAMP.len() - 1)] as char
            }
        }).collect::<String>()
    }).collect()
}

fn show_triptych(blob: &[f32], target: &[f32], out: &[f32], cols: usize) {
    let a = ascii_plane(blob, nn::IW, nn::IH, cols);
    let b = ascii_plane(target, nn::IW, nn::IH, cols);
    let c = ascii_plane(out, nn::IW, nn::IH, cols);
    println!("    {:<w$}  {:<w$}  {}", "INPUT (box)", "TARGET (part)", "NET OUTPUT", w = cols);
    for i in 0..a.len().max(b.len()).max(c.len()) {
        println!("    {:<w$}  {:<w$}  {}",
                 a.get(i).cloned().unwrap_or_default(),
                 b.get(i).cloned().unwrap_or_default(),
                 c.get(i).cloned().unwrap_or_default(), w = cols);
    }
}

fn cmd_train(args: &[String]) {
    let path = args.get(2).cloned().unwrap_or_else(|| "pairs/pairs.f32".into());
    let epochs = argu(args, "--epochs", 6);
    let l1w = argf(args, "--l1", 100.0);
    let lr = argf(args, "--lr", 0.002);
    let filters = argu(args, "--filters", 12);
    let limit = argu(args, "--limit", 400);
    let adv = arg(args, "--adv").is_some();

    let raw = match std::fs::read(&path) { Ok(r) => r, Err(e) => { eprintln!("{}: {}", path, e); return; } };
    let plane = pairs::H * pairs::W;
    // triples.f32 carries an extra CONTEXT plane between blob and detail
    let with_ctx = path.contains("triples");
    let pair_bytes = plane * (if with_ctx { 3 } else { 2 }) * 4;
    let n_all = raw.len() / pair_bytes;
    if n_all == 0 { eprintln!("no pairs in {}", path); return; }
    let n = n_all.min(limit);
    println!("{} pairs in file, using {} (downsampled to {}x{})", n_all, n, nn::IW, nn::IH);

    let rd = |i: usize, off: usize| -> Vec<f32> {
        let b = i * pair_bytes + off * plane * 4;
        (0..plane).map(|k| f32::from_le_bytes([raw[b+k*4], raw[b+k*4+1], raw[b+k*4+2], raw[b+k*4+3]]))
                  .collect()
    };
    let blobs: Vec<Vec<f32>> = (0..n).map(|i| nn::downsample(&rd(i, 0), pairs::W, pairs::H)).collect();
    let dets: Vec<Vec<f32>> = (0..n)
        .map(|i| nn::downsample(&rd(i, if with_ctx { 2 } else { 1 }), pairs::W, pairs::H)).collect();
    let ctxs: Vec<Vec<f32>> = if with_ctx {
        (0..n).map(|i| nn::downsample(&rd(i, 1), pairs::W, pairs::H)).collect()
    } else { Vec::new() };
    if with_ctx { println!("using CONTEXT plane (position/scale within the model)"); }
    // Real second CHANNEL: box plane then context plane, concatenated. Blending
    // them into one plane destroys both, which is what the first attempt measured.
    let inputs: Vec<Vec<f32>> = if with_ctx {
        blobs.iter().zip(&ctxs).map(|(b, c)| {
            let mut v = b.clone(); v.extend_from_slice(c); v
        }).collect()
    } else { blobs.clone() };

    // held-out split: a training loss that falls proves nothing on its own
    let n_te = (n / 5).max(1);
    let (te, tr) = (0..n_te, n_te..n);

    let l1 = |a: &[f32], b: &[f32]| -> f32 {
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32
    };
    let e_ident: f32 = te.clone().map(|i| l1(&blobs[i], &dets[i])).sum::<f32>() / n_te as f32;
    let mut mean = vec![0.0f32; nn::IH * nn::IW];
    for i in tr.clone() { for k in 0..mean.len() { mean[k] += dets[i][k]; } }
    for v in mean.iter_mut() { *v /= (n - n_te) as f32; }
    let e_mean: f32 = te.clone().map(|i| l1(&mean, &dets[i])).sum::<f32>() / n_te as f32;

    let mut rng = nn::Rng::new(1234);
    let mut g = nn::Gen::with_channels(if with_ctx { 2 } else { 1 }, filters, &mut rng);
    let mut d = nn::Disc::new(filters, &mut rng);

    println!("\n{:>5} {:>10} {:>10} {:>10}", "epoch", "train L1", "held-out", "D loss");
    for ep in 0..epochs {
        let mut tl = 0.0f32;
        let mut dl_sum = 0.0f32;
        for i in tr.clone() {
            let (out, cache) = g.forward(&inputs[i]);
            tl += l1(&out, &dets[i]);
            // L1 gradient, scaled — pure adversarial loss on a few hundred samples
            // collapses onto whichever donor dominates
            let m = out.len() as f32;
            let mut dl: Vec<f32> = out.iter().zip(&dets[i])
                .map(|(o, t)| l1w * (if o > t { 1.0 } else { -1.0 }) / m).collect();
            if adv {
                let (lr_, cr) = d.forward(&blobs[i], &dets[i]);
                let (lf, cf) = d.forward(&blobs[i], &out);
                let (l1r, g1r) = nn::bce(lr_, 0.9);
                let (l1f, g1f) = nn::bce(lf, 0.0);
                dl_sum += l1r + l1f;
                d.backward(&cr, g1r);
                d.backward(&cf, g1f);
                d.step(lr * 0.5);
                let (lf2, cf2) = d.forward(&blobs[i], &out);
                let (_, gg) = nn::bce(lf2, 1.0);
                let dadv = d.backward(&cf2, gg);
                for (k, v) in dadv.iter().enumerate() { dl[k] += *v; }
            }
            g.backward(&inputs[i], &cache, &out, &dl);
            g.step(lr);
        }
        let mut vl = 0.0f32;
        for i in te.clone() { let (o, _) = g.forward(&inputs[i]); vl += l1(&o, &dets[i]); }
        println!("{:>5} {:>10.4} {:>10.4} {:>10.4}", ep + 1,
                 tl / (n - n_te) as f32, vl / n_te as f32, dl_sum / (n - n_te) as f32);
    }

    let mut vl = 0.0f32;
    for i in te.clone() { let (o, _) = g.forward(&inputs[i]); vl += l1(&o, &dets[i]); }
    let e_net = vl / n_te as f32;

    // ASCII checkpoints on HELD-OUT examples. Numbers alone have repeatedly hidden
    // faults in this project; seeing the three planes side by side answers "is it
    // producing a part or a smear" in a way no loss value can.
    let n_show = argu(args, "--show", 3).min(n_te);
    if n_show > 0 {
        println!("\n  held-out checkpoints:");
        for i in te.clone().take(n_show) {
            let (o, _) = g.forward(&inputs[i]);
            println!();
            show_triptych(&blobs[i], &dets[i], &o, 30);
        }
    }
    println!("\n  {:<12} {:>9}", "baseline", "held-out L1");
    println!("  {:<12} {:>9.4}   return the blob unchanged", "IDENTITY", e_ident);
    println!("  {:<12} {:>9.4}   ignore the input", "MEAN", e_mean);
    println!("  {:<12} {:>9.4}   trained net", "NET", e_net);
    println!();
    // Beating MEAN is worthless -- returning the input beats the mean trivially.
    // The only baseline that matters is IDENTITY, and it has to be beaten by a
    // margin, not by noise. A net 1% better than copying its input has learned to
    // copy its input.
    // The net must beat BOTH baselines. Which one is the hard bar depends on the
    // degradation, and getting this wrong flatters the model in both directions:
    //   decimated blobs -> identity is strong (blob ~= target), mean is trivial
    //   box blobs       -> identity is trivial (a box is nothing like a part),
    //                      so MEAN becomes the real bar. A net that beats identity
    //                      by 35% while losing to the mean has learned "emit
    //                      something part-shaped" without reading its input.
    let g_id = 100.0 * (1.0 - e_net / e_ident);
    let g_mean = 100.0 * (1.0 - e_net / e_mean);
    println!("  vs IDENTITY: {:+.1}%     vs MEAN: {:+.1}%", g_id, g_mean);
    let hard = if e_ident < e_mean { "IDENTITY" } else { "MEAN" };
    println!("  (the hard bar here is {} -- the other is trivially beaten by this data)", hard);
    if g_id > 5.0 && g_mean > 5.0 {
        println!("  LEARNS a real mapping: it beats BOTH baselines. Worth GPU time.");
    } else if g_id > 5.0 || g_mean > 5.0 {
        println!("  PARTIAL. It beats one baseline and not the other, which means it has");
        println!("  found the output DISTRIBUTION but is not conditioning on the input.");
        println!("  More capacity or epochs may fix it; a GPU run is premature.");
    } else {
        println!("  HAS NOT LEARNED. Fix the DATA before spending GPU time -- check the");
        println!("  blob is measurably less detailed than the target (assetkit pairs");
        println!("  reports the relief gap).");
    }
}

/// Multi-pass genetic fill. Pass 1 fits a few large boxes over the whole region;
/// each later pass subdivides and adds finer genes. Coarse structure settles before
/// detail, which is why the result reads as built rather than scattered.
fn cmd_fill(args: &[String]) {
    // --batch runs every model under a directory, with names made unique. Doing
    // this by hand in a shell loop is how two models silently became one earlier
    // in this project; the tool owns the naming now.
    if let Some(dir) = arg(args, "--batch") {
        let mut files = Vec::new();
        walk(Path::new(&dir), &mut files, 0);
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut todo: Vec<(String, PathBuf)> = Vec::new();
        for p in files {
            // one entry per MODEL folder, not per file
            let base = p.parent().and_then(|d| d.file_name())
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "model".into());
            let mut nm: String = base.chars()
                .filter(|c| c.is_ascii_alphanumeric()).collect::<String>()
                .to_lowercase().chars().take(24).collect();
            if nm.is_empty() { nm = "model".into(); }
            let e = seen.entry(nm.clone()).or_insert(0);
            if *e > 0 { nm = format!("{}{}", nm, e); }
            *e += 1;
            todo.push((nm, p));
        }
        // keep the largest file per folder only
        let mut best: HashMap<String, (u64, PathBuf)> = HashMap::new();
        for (nm, p) in todo {
            let sz = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            let key = p.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or(nm.clone());
            let e = best.entry(key).or_insert((0, p.clone()));
            if sz > e.0 { *e = (sz, p); }
        }
        let out = arg(args, "--out").unwrap_or_else(|| "filled".into());
        let mut n_ok = 0;
        println!("{} model folder(s) under {}\n", best.len(), dir);
        for (i, (_k, (_sz, path))) in best.iter().enumerate() {
            let nm = format!("fill{:02}", i);
            let mut sub: Vec<String> = vec!["assetkit".into(), "fill".into(),
                path.to_string_lossy().to_string()];
            for k in ["--catalog", "--pop", "--gens", "--passes", "--region"] {
                if let Some(v) = arg(args, k) { sub.push(k.into()); sub.push(v); }
            }
            if arg(args, "--keep-shell").is_some() { sub.push("--keep-shell".into()); }
            sub.push("--out".into()); sub.push(out.clone());
            sub.push("--name".into()); sub.push(nm.clone());
            sub.push("--show".into()); sub.push("0".into());
            let label = path.parent().and_then(|d| d.file_name())
                            .map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            print!("  {:<40} ", label.chars().take(40).collect::<String>());
            cmd_fill(&sub);
            n_ok += 1;
        }
        println!("\n{} model(s) filled -> {}", n_ok, out);
        return;
    }
    let target_path = match args.get(2) { Some(p) => PathBuf::from(p), None => {
        eprintln!("fill <target.gltf|iqm> --catalog DIR [--out DIR]"); return; } };
    let cat_dir = arg(args, "--catalog").unwrap_or_else(|| "solidcat/meshes".into());
    let out = arg(args, "--out").unwrap_or_else(|| "evolved".into());
    let name = arg(args, "--name").unwrap_or_else(|| "evolved".into());
    let pop = argu(args, "--pop", 60);
    let gens = argu(args, "--gens", 40);
    let passes = argu(args, "--passes", 3);
    let seed = argu(args, "--seed", 5) as u64;
    let show = argu(args, "--show", 1);

    let mut tm = match mesh::load_any(&target_path) { Some(m) => m, None => {
        eprintln!("could not read {}", target_path.display()); return; } };
    mesh::normalise(&mut tm);
    // --region fills only part of the model. "chest" is the case this whole thread
    // began with: a hollow WFC torso that needs internal structure.
    let region = arg(args, "--region");
    let lat_ax = harvest::mirror_axis(&tm.v);
    if region.is_some() {
        println!("lateral axis: {} (detected, not assumed)", ["X", "Y", "Z"][lat_ax]);
    }
    let (rverts, lo, span) = match region.as_deref() {
        Some("chest") => evolve::region(&tm.v, 0.45, 0.80, 0.20, lat_ax),
        Some("legs")  => evolve::region(&tm.v, 0.00, 0.45, 0.45, lat_ax),
        Some("head")  => evolve::region(&tm.v, 0.82, 1.00, 0.25, lat_ax),
        Some(spec) => {
            // y0,y1,lateral
            let n: Vec<f32> = spec.split(',').filter_map(|x| x.parse().ok()).collect();
            if n.len() == 3 { evolve::region(&tm.v, n[0], n[1], n[2], lat_ax) }
            else { eprintln!("--region wants chest|legs|head or y0,y1,lateral"); return; }
        }
        None => {
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for p in &tm.v { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
            (tm.v.clone(), lo,
             [(hi[0]-lo[0]).max(1e-6), (hi[1]-lo[1]).max(1e-6), (hi[2]-lo[2]).max(1e-6)])
        }
    };
    if rverts.is_empty() { eprintln!("region selected no geometry"); return; }
    if region.is_some() {
        println!("region {}: {} of {} verts, box {:.2} x {:.2} x {:.2}",
                 region.as_deref().unwrap_or("-"), rverts.len(), tm.v.len(),
                 span[0], span[1], span[2]);
    }
    let shell = evolve::occupancy(&rverts, lo, span);
    // --interior targets the VOLUME the shell encloses rather than the shell
    // itself. Filling a surface with solids is near-impossible; filling a cavity
    // is what "structure inside a hollow torso" actually means.
    let target = if arg(args, "--interior").is_some() {
        let it = evolve::interior(&shell);
        let n = it.iter().filter(|b| **b).count();
        println!("interior: {} enclosed cells (shell had {})",
                 n, shell.iter().filter(|b| **b).count());
        if n < 20 {
            println!("  !! almost no enclosed volume -- this region is open, not hollow.");
            println!("     Filling its surface instead.");
            shell
        } else { it }
    } else { shell };
    let filled = target.iter().filter(|b| **b).count();

    let mut files = Vec::new();
    walk(Path::new(&cat_dir), &mut files, 0);
    let mut parts = Vec::new();
    let mut names = Vec::new();
    for f in files.iter().take(400) {
        if let Some(m) = mesh::load_any(f) {
            if m.f.len() >= 4 {
                parts.push(m);
                names.push(f.file_stem().unwrap_or_default().to_string_lossy().to_string());
            }
        }
    }
    if parts.is_empty() { eprintln!("no catalog parts under {}", cat_dir); return; }
    println!("target {} : {} tris, {}/{} occupancy cells filled",
             target_path.file_stem().unwrap_or_default().to_string_lossy(),
             tm.f.len(), filled, evolve::G.pow(3));
    println!("catalog: {} parts from {}\n", parts.len(), cat_dir);

    let cat = evolve::Catalog { parts, names };
    let mut best: Option<evolve::Genome> = None;
    println!("{:>5} {:>7} {:>8} {:>10} {:>10}", "pass", "genes", "scale", "start", "best");
    for p in 0..passes {
        // strides: each pass halves the box size and doubles the gene count
        let genes = 6 * (1 << p);
        let scale = 0.55 / (1.0 + p as f32);
        let cfg = evolve::Cfg { pop, gens, genes, elite: (pop / 10).max(2),
                                seed: seed + p as u64, scale };
        let (g, hist) = evolve::evolve(&cat, &target, lo, span, &cfg);
        println!("{:>5} {:>7} {:>8.2} {:>10.4} {:>10.4}",
                 p + 1, genes, scale, hist.first().copied().unwrap_or(0.0), g.fitness);
        // Keep the best across passes, not the last. More genes is not monotonically
        // better -- measured: 192 genes reached 0.385 and 384 genes fell back to
        // 0.366, because the search space grows faster than the budget to explore
        // it. Reporting the final pass would have hidden that.
        let improved = match &best { Some(b) => g.fitness > b.fitness, None => true };
        if !improved {
            println!("        (pass {} did not improve on {:.4} -- keeping the earlier best)",
                     p + 1, best.as_ref().map(|b| b.fitness).unwrap_or(0.0));
        }
        best = Some(match best {
            Some(b) if b.fitness >= g.fitness => b,
            _ => g,
        });
    }
    let best = match best { Some(b) => b, None => return };

    let mut baked = evolve::bake(&cat, &best);
    // Keeping the shell makes this ADDITIVE -- the original model plus evolved
    // structure -- rather than a replacement. At IoU ~0.38 the evolved geometry is
    // a rough reconstruction, so replacing a good model with it is a downgrade;
    // adding to it is not. Default on for regions, opt-in for whole models.
    let keep = arg(args, "--keep-shell").is_some()
               || (region.is_some() && arg(args, "--no-shell").is_none());
    if keep {
        // Fill without the shell is just a lump in space. Keeping it is what makes
        // this "Jazz with a built chest" rather than "a chest".
        let base = baked.v.len() as u32;
        baked.v.extend_from_slice(&tm.v);
        for t in &tm.f { baked.f.push([base + t[0], base + t[1], base + t[2]]); }
        println!("  kept the shell: +{} tris", tm.f.len());
    }
    let _ = std::fs::create_dir_all(&out);
    let (v, uv, f) = iqm::box_uvs(&baked.v, &baked.f);
    let file = format!("imported_{}_part0.iqm", name);
    let _ = iqm::write_iqm(Path::new(&format!("{}/{}", out, file)), &v, &uv, &f, "plate_blue.png");
    let _ = std::fs::write(format!("{}/imported_{}_parts.txt", out, name),
        format!("{} plate_blue.png body -1 0 0.00000 0.00000 0.00000 -1", file));

    println!("\n  best IoU-based fitness: {:.4}", best.fitness);
    println!("  {} parts placed, {} tris -> {}/{}", best.genes.len(), baked.f.len(), out, file);
    // A random assembly is the bar: if evolution cannot beat one, the search is
    // doing nothing and the fitness function is not guiding anything.
    let mut rng = evolve::Rng::new(seed ^ 0xabcd);
    let mut rand_best = f32::MIN;
    for _ in 0..40 {
        let genes: Vec<evolve::GenePart> = (0..best.genes.len()).map(|_| evolve::GenePart {
            part: rng.idx(cat.parts.len()),
            pos: [lo[0] + rng.f()*span[0], lo[1] + rng.f()*span[1], lo[2] + rng.f()*span[2]],
            scale: [span[0]*rng.range(0.1,0.5), span[1]*rng.range(0.1,0.5), span[2]*rng.range(0.1,0.5)],
            yaw: (rng.idx(4) as f32) * std::f32::consts::FRAC_PI_2,
        }).collect();
        let g = evolve::Genome { genes, fitness: 0.0 };
        rand_best = rand_best.max(evolve::score(&cat, &g, &target, lo, span));
    }
    println!("  best of 40 RANDOM assemblies: {:.4}", rand_best);
    let gain = 100.0 * (best.fitness - rand_best) / rand_best.abs().max(1e-3);
    if best.fitness > rand_best * 1.15 {
        println!("  evolution beats random by {:+.0}% -- the search is working", gain);
    } else {
        println!("  only {:+.0}% over random -- the fitness function is not guiding the", gain);
        println!("  search. Check the target occupancy is not nearly empty or nearly full.");
    }
    if show > 0 {
        let mut bm = baked.clone();
        mesh::normalise(&mut bm);
        let mut tn = tm.clone();
        mesh::normalise(&mut tn);
        println!("\n  TARGET                          EVOLVED");
        let a = ascii::render(&tn, &tn.v.clone(), 30, "front");
        let b = ascii::render(&bm, &bm.v.clone(), 30, "front");
        for i in 0..a.len().max(b.len()) {
            println!("  {:<30}  {}", a.get(i).cloned().unwrap_or_default(),
                     b.get(i).cloned().unwrap_or_default());
        }
    }
}

/// Score how well a model supports humanoid motion, BEFORE rigging it. The limb
/// splitter's failures were previously only discovered by measuring joint gaps
/// through a walk cycle, after the model had shipped.
fn cmd_rigscore(args: &[String]) {
    let p = args.get(2).cloned().unwrap_or_else(|| ".".into());
    let min_tris = argu(args, "--min-tris", 40);
    let path = PathBuf::from(&p);
    let mut files = Vec::new();
    if path.is_dir() { walk(&path, &mut files, 0); } else { files.push(path); }

    println!("{:<34} {:>6} {:>6} {:>6} {:>6} {:>7}  {}",
             "model", "sep", "sym", "prop", "artic", "TOTAL", "verdict");
    let mut rows = Vec::new();
    for f in &files {
        let mut m = match mesh::load_any(f) { Some(m) => m, None => continue };
        if m.f.len() < 200 { continue; }
        mesh::normalise(&mut m);
        let s = rigscore::score(&m, min_tris);
        let n = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
        println!("{:<34} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>7.2}  {}",
                 n.chars().take(34).collect::<String>(),
                 s.separability, s.symmetry, s.proportion, s.articulation, s.total,
                 rigscore::verdict(&s).split(" -- ").next().unwrap_or(""));
        rows.push((n, s));
    }
    if rows.len() == 1 {
        let (_n, s) = &rows[0];
        println!("\n  limb triangles: body {} head {} arm_l {} arm_r {} leg_l {} leg_r {}",
                 s.limb_tris[0], s.limb_tris[1], s.limb_tris[2],
                 s.limb_tris[3], s.limb_tris[4], s.limb_tris[5]);
        println!("  {}", rigscore::verdict(s));
        for n in &s.notes { println!("  !! {}", n); }
        println!("\n  weights: symmetry 0.35, separability 0.30, proportion 0.20, articulation 0.15");
        println!("  symmetry is weighted highest because it is what breaks the walk cycle --");
        println!("  a 4x limb imbalance threw parts a body-width off when animated.");
    } else if !rows.is_empty() {
        rows.sort_by(|a, b| b.1.total.total_cmp(&a.1.total));
        let ok = rows.iter().filter(|(_n, s)| s.total > 0.60).count();
        let marg = rows.iter().filter(|(_n, s)| s.total > 0.35 && s.total <= 0.60).count();
        println!("\n  {} riggable, {} marginal, {} not riggable (of {})",
                 ok, marg, rows.len() - ok - marg, rows.len());
    }
}

fn cmd_hub(args: &[String]) {
    let out = arg(args, "--out").unwrap_or_else(|| "content/hub.json".into());
    let hdr = arg(args, "--header").unwrap_or_else(|| "engine_src/exengine/hub_plan.h".into());
    let maps_path = arg(args, "--maps").unwrap_or_else(|| "content/maps.json".into());

    println!("{} rooms, {} destinations", hub::ROOMS.len(), hub::DESTS.len());
    for r in hub::ROOMS {
        println!("  {:<15} {:<10} ({:+5.1},{:+5.1}) -> {}",
                 r.id, r.kind.tag(), r.pos[0], r.pos[2], r.exits.join(", "));
    }

    // Three checks, each for a bug that is invisible until someone walks into it.
    let mut bad = 0;
    let unreach = hub::unreachable();
    if unreach.is_empty() { println!("\n  all rooms reachable from the bridge"); }
    else { println!("\n  !! UNREACHABLE from the bridge: {}", unreach.join(", ")); bad += 1; }

    let ow = hub::one_way();
    if ow.is_empty() { println!("  all exits are mutual"); }
    else {
        for (a, b) in &ow { println!("  !! one-way door: {} -> {}", a, b); }
        bad += 1;
    }

    let maps = raids::map_names(Path::new(&maps_path));
    let missing: Vec<&str> = hub::DESTS.iter().map(|d| d.map)
        .filter(|m| !maps.iter().any(|x| x == m)).collect();
    if missing.is_empty() { println!("  all destination maps exist in {}", maps_path); }
    else { println!("  !! destinations point at undefined maps: {}", missing.join(", ")); bad += 1; }

    // travel spread: if everything is one mode the drive/fly/land systems go unused
    let mut sp = [0usize; 4];
    for d in hub::DESTS { sp[d.travel.code() as usize] += 1; }
    println!("  travel: drive {} fly {} land {} walk {}", sp[0], sp[1], sp[2], sp[3]);

    // --ship fits the room graph into a real hull instead of using the authored
    // positions, so a stubby lander and a long cruiser both work from one layout.
    if let Some(sp) = arg(args, "--ship") {
        let mut sm = match mesh::load_any(Path::new(&sp)) {
            Some(m) => m, None => { println!("  !! could not read ship {}", sp); return; } };
        mesh::normalise(&mut sm);
        let shrink = argf(args, "--shrink", 0.72);
        let fitted = hub::fit_to_hull(&sm.v, shrink);
        println!("\n  fitted {} rooms into {} ({} tris)", fitted.len(),
                 Path::new(&sp).file_stem().unwrap_or_default().to_string_lossy(), sm.f.len());
        for r in &fitted {
            println!("    {:<15} centre ({:+5.2},{:+5.2},{:+5.2})  half ({:.2},{:.2},{:.2})",
                     r.id, r.centre[0], r.centre[1], r.centre[2], r.half[0], r.half[1], r.half[2]);
        }
        let ov = hub::overlaps(&fitted);
        if ov.is_empty() { println!("    no room overlaps"); }
        else {
            for (a, b) in &ov { println!("    !! {} overlaps {}", a, b); }
            bad += 1;
        }
        let out_h = hub::outside_hull(&fitted, &sm.v);
        if out_h.is_empty() { println!("    all rooms inside the hull"); }
        else { println!("    !! rooms outside the hull: {}", out_h.join(", ")); bad += 1; }
        if bad == 0 {
            let fh = arg(args, "--fit-header").unwrap_or_else(|| "engine_src/exengine/hub_fit.h".into());
            let _ = std::fs::write(&fh, hub::fitted_header(&fitted, &sp));
            println!("    wrote {}", fh);
        }
    }

    if bad > 0 { println!("\n  {} problem(s) -- not writing", bad); std::process::exit(1); }
    let _ = std::fs::write(&out, hub::to_json());
    let _ = std::fs::write(&hdr, hub::to_header());
    println!("\n  wrote {} and {}", out, hdr);
}

fn cmd_info(args: &[String]) {
    let p = match args.get(2) { Some(p) => PathBuf::from(p), None => { eprintln!("info <file>"); return; } };
    let mut m = match mesh::load_any(&p) { Some(m) => m, None => { eprintln!("could not read"); return; } };
    let comps = harvest::components(&m, 50);
    let ax = harvest::mirror_axis(&m.v);
    let s = mesh::score(&mut m, 50);
    println!("{}", p.display());
    println!("  {} verts, {} tris", s.verts, s.tris);
    println!("  components >=50 tris : {}", comps.len());
    println!("  mirror axis          : {}", ["X", "Y", "Z"][ax]);
    println!("  compactness          : {:.1}", s.compactness);
    println!("  coherence            : {:.3}", s.coherence);
    println!("  open edges           : {:.0}%", s.open_frac * 100.0);
    println!("  class                : {}", s.class);
    println!("  bony (filter, not a verdict) : {:.1}", s.bony);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("score") => cmd_score(&args),
        Some("view") => cmd_view(&args),
        Some("harvest") => cmd_harvest(&args),
        Some("morph") => cmd_morph(&args),
        Some("pairs") => cmd_pairs(&args),
        Some("raids") => cmd_raids(&args),
        Some("place") => cmd_place(&args),
        Some("train") => cmd_train(&args),
        Some("fill") => cmd_fill(&args),
        Some("rigscore") => cmd_rigscore(&args),
        Some("hub") => cmd_hub(&args),
        Some("info") => cmd_info(&args),
        _ => {
            eprintln!("assetkit — model pipeline tools\n");
            eprintln!("  score   <dir>  [--top N] [--csv f] [--min-tris N]");
            eprintln!("  view    <file> [--views front,side,top] [--width N] [--rx/--ry/--rz deg]");
            eprintln!("  harvest <dir>  [--out DIR] [--min-tris N]");
            eprintln!("  morph   <file> [--out DIR] [--name N] [--clusters N] [--spread F]");
            eprintln!("  pairs   <dir>  [--out DIR] [--ratio N] [--min-tris N]");
            eprintln!("  raids          [--raid ID] [--phases N] [--seed N] [--maps f] [--out f]");
            eprintln!("  place   <dir>  [--out DIR]   stage 1: position/scale/orientation");
            eprintln!("  train   <pairs.f32> [--epochs N] [--l1 F] [--adv] [--limit N]");
            eprintln!("  fill    <target> --catalog DIR [--region chest|legs|head|y0,y1,lat]");
            eprintln!("                          [--interior] [--keep-shell] [--no-shell]");
            eprintln!("                          [--pop N] [--gens N] [--passes N]");
            eprintln!("  rigscore <file|dir>          humanoid riggability, before rigging");
            eprintln!("  hub  [--ship HULL.iqm] [--shrink F]   rooms + destinations, fitted to a hull");
            eprintln!("  info    <file>");
        }
    }
}
