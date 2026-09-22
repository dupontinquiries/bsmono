//! boneforge — turn every vehicle in a folder into a bot, and report.
//!
//!     boneforge <dir-or-glb> [...]
//!
//! Prints one line per vehicle: what it was made of, what the bot came out
//! as, and anything that failed a check. The point is to run it over a
//! LIBRARY: a rule that works on one car and not on thirty is not a rule.

use boneforge::{anatomy, bot, glb, island, strip};
use std::path::{Path, PathBuf};

fn glbs(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        let mut e: Vec<_> = std::fs::read_dir(p).into_iter().flatten()
            .flatten().map(|x| x.path()).collect();
        e.sort();
        for c in e { glbs(&c, out); }
    } else if p.extension().is_some_and(|x| x == "glb") {
        out.push(p.to_path_buf());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("boneforge <dir-or-glb> [...]");
        std::process::exit(2);
    }
    // `--emit <dir>` writes fold strips blackstar-rs can load. Without it the
    // tool only reports, which is what you want when running over a library
    // to see whether the rules hold before writing anything.
    let mut emit: Option<PathBuf> = None;
    let mut frames = 5usize;
    let mut inputs: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--emit" => emit = it.next().map(PathBuf::from),
            "--frames" => frames = it.next().and_then(|v| v.parse().ok()).unwrap_or(5),
            other => inputs.push(other.to_string()),
        }
    }
    let mut files = Vec::new();
    for a in &inputs { glbs(Path::new(a), &mut files); }
    if files.is_empty() {
        eprintln!("no .glb found");
        std::process::exit(1);
    }

    println!("{:<26}{:>7}{:>7}{:>7}{:>8}{:>7}{:>8}  notes",
             "vehicle", "isl", "wheel", "panel", "height", "plates", "cover");
    let mut made = 0usize;
    let mut failed = 0usize;
    for f in &files {
        let name = f.file_stem().unwrap().to_string_lossy().to_string();
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(e) => { println!("  {name:<24}  read failed: {e}"); failed += 1; continue }
        };
        let mesh = match glb::load(&bytes) {
            Ok(m) => m,
            Err(e) => { println!("  {name:<24}  {e}"); failed += 1; continue }
        };
        let isl = island::split(&mesh.pos, &mesh.idx);
        if isl.is_empty() { println!("  {name:<24}  no geometry"); failed += 1; continue }

        let b = bot::build(&isl);
        let parts: Vec<anatomy::Part> =
            isl.iter().map(|i| anatomy::classify(i, &b.body)).collect();
        let corners = anatomy::corners(&isl, &parts, &b.body);
        let wheels = corners.len();
        let wheel_parts = b.carried.iter()
            .filter(|(_, p)| p.kind == anatomy::Kind::Wheel).count();
        let panels = b.carried.iter()
            .filter(|(_, p)| p.kind == anatomy::Kind::Panel).count();

        // coverage against the machine's own skin: 150-200% is armour
        // overlapping its neighbour, which is what scale armour does. The
        // vehicle's surface area is the wrong denominator -- it is a much
        // larger surface the bot does not have.
        let skin: f32 = b.bones.iter()
            .map(|x| std::f32::consts::TAU * x.radius * x.len).sum();
        let plated: f32 = b.plates.iter().map(|p| p.size[0] * p.size[1]).sum();
        let cover = if skin > 0.0 { plated / skin * 100.0 } else { 0.0 };

        let mut notes = Vec::new();
        if wheels == 0 { notes.push("no wheels found".to_string()); }
        if wheels % 2 != 0 { notes.push(format!("{wheels} corners, odd")); }
        if wheels > 0 && wheel_parts / wheels > 12 {
            notes.push(format!("{} parts per wheel", wheel_parts / wheels));
        }
        if !(90.0..320.0).contains(&cover) {
            notes.push(format!("coverage {cover:.0}%"));
        }
        if isl.len() < 4 {
            notes.push("nested shells, no parts to work with".to_string());
        }
        let mut wrote = 0usize;
        if let Some(root) = &emit {
            // The engine wants two or more pose dirs sorted vehicle-first.
            // A vehicle with nothing to segment still makes a bot; it just
            // makes a worse one, and that is the caller's call, not ours.
            let poses = bot::strip_poses(&b.bones, b.height, frames);
            match strip::write_chassis(root, &format!("bf_{name}"), &b, &poses, &name) {
                Ok(n) => wrote = n,
                Err(e) => notes.push(format!("write failed: {e}")),
            }
        }
        if wrote > 0 { notes.push(format!("{wrote} poses written")); }
        made += 1;
        println!("  {:<24}{:>7}{:>7}{:>7}{:>8.0}{:>7}{:>7.0}%  {}",
                 &name[..name.len().min(24)], isl.len(), wheels, panels,
                 b.height, b.plates.len(), cover, notes.join("; "));
    }
    println!("\n{made} bots from {} vehicles, {failed} unreadable", files.len());
}
