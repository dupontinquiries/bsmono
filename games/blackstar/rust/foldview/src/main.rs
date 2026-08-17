//! foldview — watch a blackstar frame fold between its two lives.
//!
//!   foldview --data engine_src/data --content content --variant watch --chassis raptor --t 0.63 --out f.png
//!   foldview ... --frames 8 --out outdir/          (sequence 0..1)
//!   foldview ... --sheet --t 0.63 --out sheet.png  (every chassis, side by side)
//!   foldview ... --gpu                             (interactive; build with --features gpu)
//!
//! Keys in the window: Space play/reverse, [ ] chassis, 1-3 variant, drag to orbit.

mod iqm;
mod rig;
mod fvx;
mod gltfx;
pub fn veh_prism_bridge(at: [f64; 3], axis: [f64; 3], len: f64, radius: f64)
    -> (Vec<[f64; 3]>, Vec<[f64; 3]>, Vec<u32>) {
    veh::prism_pub(at, axis, len, radius, 8)
}
mod veh;
mod soft;
#[cfg(feature = "gpu")]
mod gpu;

use anyhow::{bail, Context, Result};
use model_verify::motion::Profile;
use std::path::PathBuf;

struct Args {
    data: PathBuf,
    content: PathBuf,
    variant: String,
    chassis: String,
    profile: Option<Profile>,
    t: f64,
    frames: usize,
    sheet: bool,
    gpu: bool,
    out: PathBuf,
    size: (usize, usize),
    veh: Option<PathBuf>,
    census: Option<String>,
    vehposter: Option<String>,
    pick: Vec<String>,
    depth: usize,
    events: Option<PathBuf>,
    orientsheet: Option<String>,
    turnaround: Option<PathBuf>,
    front: bool,
    perf: Option<String>,
    salvo: bool,
    bake: Option<String>,
    lineup: Option<String>,
    ss: usize,
    gltf: Option<PathBuf>,
    strip: Option<PathBuf>,
    poses: usize,
}

fn parse() -> Result<Args> {
    let mut a = Args {
        veh: None, census: None, vehposter: None, pick: Vec::new(), depth: 5, events: None, orientsheet: None, turnaround: None, front: false, perf: None, salvo: false, bake: None, lineup: None, ss: 1, gltf: None, strip: None, poses: 5,
        data: "engine_src/data".into(),
        content: "content".into(),
        variant: "watch".into(),
        chassis: "standard".into(),
        profile: None,
        t: 0.63,
        frames: 0,
        sheet: false,
        gpu: false,
        out: "fold.png".into(),
        size: (960, 540),
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut val = || it.next().context("missing value");
        match k.as_str() {
            "--data" => a.data = val()?.into(),
            "--content" => a.content = val()?.into(),
            "--variant" => a.variant = val()?,
            "--chassis" => a.chassis = val()?,
            "--profile" => {
                a.profile = Some(match val()?.as_str() {
                    "fluid" => Profile::Fluid,
                    "combat" => Profile::Combat,
                    "anger" => Profile::Anger,
                    p => bail!("unknown profile {p}"),
                })
            }
            "--veh" => a.veh = Some(val()?.into()),
            "--census" => a.census = Some(val()?),
            "--vehposter" => a.vehposter = Some(val()?),
            "--pick" => a.pick = val()?.split(',').map(|s| s.to_string()).collect(),
            "--depth" => a.depth = val()?.parse()?,
            "--events" => a.events = Some(val()?.into()),
            "--orientsheet" => a.orientsheet = Some(val()?),
            "--turnaround" => a.turnaround = Some(val()?.into()),
            "--front" => a.front = true,
            "--perf" => a.perf = Some(val()?),
            "--salvo" => a.salvo = true,
            "--bake" => a.bake = Some(val()?),
            "--lineup" => a.lineup = Some(val()?),
            "--ss" => a.ss = val()?.parse()?,
            "--gltf" => a.gltf = Some(PathBuf::from(val()?)),
            "--strip" => a.strip = Some(PathBuf::from(val()?)),
            "--poses" => a.poses = val()?.parse()?,
            "--t" => a.t = val()?.parse()?,
            "--frames" => a.frames = val()?.parse()?,
            "--sheet" => a.sheet = true,
            "--gpu" => a.gpu = true,
            "--out" => a.out = val()?.into(),
            "--size" => {
                let v = val()?;
                let (w, h) = v.split_once('x').context("--size WxH")?;
                a.size = (w.parse()?, h.parse()?);
            }
            other => bail!("unknown flag {other}"),
        }
    }
    Ok(a)
}

fn veh_files(data: &PathBuf, cat: &str) -> Result<Vec<PathBuf>> {
    let prefix = format!("imported_{cat}_");
    let mut v: Vec<PathBuf> = std::fs::read_dir(data)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().map_or(false, |x| x == "iqm")
                && p.file_name().map_or(false, |n| n.to_string_lossy().starts_with(&prefix))
        })
        .collect();
    v.sort();
    Ok(v)
}

fn main() -> Result<()> {
    let a = parse()?;
    let prof = |c: &str| a.profile.unwrap_or_else(|| rig::profile_for(c));

    if a.gpu {
        #[cfg(feature = "gpu")]
        return gpu::run(&a.data, &a.content, &a.variant, &a.chassis);
        #[cfg(not(feature = "gpu"))]
        bail!("built without gpu — cargo run --features gpu -- --gpu ...");
    }

    if let Some(picks) = &a.lineup {
        // finished bots, shoulder to shoulder — the team photo
        let (tw, th) = (300usize, 480usize);
        let files: Vec<PathBuf> = picks.split(',').map(|p| a.data.join(p)).collect();
        let mut sheet = soft::Frame { w: tw * files.len(), h: th, rgb: vec![10; tw * files.len() * th * 3] };
        for (k, f) in files.iter().enumerate() {
            match veh::build(f, a.depth) {
                Ok(bot) => {
                    let shots = [(1.0, 0.9, 0.16)];
                    let (tw2, th2) = (tw * a.ss, th * a.ss);
                    let cam = soft::fit(&bot, &shots, tw2, th2, 0.94);
                    let fr = soft::render_cam(&bot, 1.0, &cam, tw2, th2, shots[0].1, shots[0].2);
                    let fr = soft::downsample(&fr, a.ss);
                    soft::blit(&mut sheet, &fr, k * tw, 0);
                }
                Err(e) => eprintln!("lineup: {} failed: {e}", f.display()),
            }
        }
        soft::save_png(&sheet, &a.out)?;
        eprintln!("lineup -> {}", a.out.display());
        return Ok(());
    }

    if let Some(what) = &a.bake {
        let files = if what.ends_with(".iqm") {
            vec![a.data.join(what)]
        } else {
            veh_files(&a.data, what)?
        };
        for f in &files {
            match veh::bake(f) {
                Ok(out) => eprintln!("baked {} -> {}", f.file_name().unwrap_or_default().to_string_lossy(), out.display()),
                Err(e) => eprintln!("bake failed for {}: {e}", f.display()),
            }
        }
        return Ok(());
    }

    if let Some(cat) = &a.orientsheet {
        // every vehicle in the category at rest, one fixed camera — the
        // fastest way for a human to flag upside-down/backwards/sideways
        let files = veh_files(&a.data, cat)?;
        if files.is_empty() { bail!("no models for {cat}"); }
        let (tw, th) = (256usize, 144usize);
        let cols = 6usize;
        let rows = (files.len() + cols - 1) / cols;
        let mut sheet = soft::Frame { w: tw * cols, h: th * rows, rgb: vec![10; tw * cols * th * rows * 3] };
        println!("orientation sheet index for {cat} (row,col -> model):");
        for (k, path) in files.iter().enumerate() {
            let (r, c) = (k / cols, k % cols);
            println!("  r{r} c{c}  {}", path.file_name().unwrap_or_default().to_string_lossy());
            match veh::build(path, a.depth) {
                Ok(bot) => {
                    let shots = [(0.0, 0.85, 0.26)];
                    let cam = soft::fit(&bot, &shots, tw, th, 0.90);
                    let f = soft::render_cam(&bot, 0.0, &cam, tw, th, shots[0].1, shots[0].2);
                    soft::blit(&mut sheet, &f, c * tw, r * th);
                }
                Err(_) => {}
            }
        }
        soft::save_png(&sheet, &a.out)?;
        eprintln!("orientation sheet -> {}", a.out.display());
        return Ok(());
    }

    if let Some(path) = &a.turnaround {
        let bot = veh::build(path, a.depth)?;
        let (tw, th) = (360usize, 405usize);
        let n = 6usize;
        let shots: Vec<(f64, f64, f64)> = (0..n)
            .map(|k| (1.0, k as f64 / n as f64 * std::f64::consts::TAU, 0.22))
            .collect();
        let (tw2, th2) = (tw * a.ss, th * a.ss);
        let cams = soft::fit_seq(&bot, &shots, tw2, th2, 0.92);
        let mut strip = soft::Frame { w: tw * n, h: th, rgb: vec![10; tw * n * th * 3] };
        for (k, &(t, yaw, pitch)) in shots.iter().enumerate() {
            let f = soft::render_cam(&bot, t, &cams[k], tw2, th2, yaw, pitch);
            let f = soft::downsample(&f, a.ss);
            soft::blit(&mut strip, &f, k * tw, 0);
        }
        soft::save_png(&strip, &a.out)?;
        eprintln!("turnaround -> {}", a.out.display());
        return Ok(());
    }

    if let Some(cat) = &a.census {
        for p in veh_files(&a.data, cat)? {
            println!("{}", veh::census_line(&p, a.depth));
        }
        return Ok(());
    }

    if let Some(path) = &a.veh {
        let pov = a.perf.as_deref().map(|p| match p {
            "roll" => veh::Performance::Roll, "spin" => veh::Performance::Spin,
            "combat" => veh::Performance::Combat, _ => veh::Performance::Clean,
        });
        let bot = veh::build_with(path, a.depth, pov, a.salvo)?;
        eprintln!("{} — {} pieces", bot.label, bot.parts.len());
        if let Some(ep) = &a.events {
            let mut ev: Vec<serde_json::Value> = Vec::new();
            for (i, p) in bot.parts.iter().enumerate() {
                let s = bot.slots[p.joint];
                ev.push(serde_json::json!({"part": p.name, "index": i,
                    "start": s.start, "end": s.end}));
            }
            std::fs::write(ep, serde_json::to_string_pretty(&serde_json::json!({
                "model": bot.label, "duration": bot.duration, "tracks": ev
            }))?)?;
            eprintln!("events -> {}", ep.display());
        }
        if let Some(dir) = &a.strip {
            let src = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let n = gltfx::export_strip(&bot, dir, &src, a.poses)?;
            eprintln!("strip: {} poses x {} merged prims from {} parts -> {}",
                a.poses, n, bot.parts.len(), dir.display());
            return Ok(());
        }
        if let Some(dir) = &a.gltf {
            gltfx::export(&bot, a.t, dir)?;
            eprintln!("exported {} parts at t={} -> {}", bot.parts.len(), a.t, dir.display());
            return Ok(());
        }
        if a.frames == 0 {
            let yaw0 = if a.front { std::f64::consts::PI } else { 0.75 + a.t * 0.9 };
            let shots = [(a.t, yaw0, if a.front { 0.05 } else { 0.30 })];
            let (w2, h2) = (a.size.0 * a.ss, a.size.1 * a.ss);
            let cam = soft::fit(&bot, &shots, w2, h2, 0.92);
            let f = soft::render_cam(&bot, a.t, &cam, w2, h2, shots[0].1, shots[0].2);
            let f = soft::downsample(&f, a.ss);
            soft::save_png(&f, &a.out)?;
            return Ok(());
        }
        std::fs::create_dir_all(&a.out)?;
        let n = a.frames.max(6);
        let shots: Vec<(f64, f64, f64)> = (0..n)
            .map(|i| { let t = i as f64 / (n - 1) as f64; (t, 0.75 + t * 0.9, 0.30) })
            .collect();
        let cams = soft::fit_seq(&bot, &shots, a.size.0, a.size.1, 0.92);
        for (i, &(t, yaw, pitch)) in shots.iter().enumerate() {
            let f = soft::render_cam(&bot, t, &cams[i], a.size.0, a.size.1, yaw, pitch);
            soft::save_png(&f, &a.out.join(format!("fold_{i:03}.png")))?;
        }
        eprintln!("{n} frames -> {}/", a.out.display());
        return Ok(());
    }

    if let Some(cat) = &a.vehposter {
        let files: Vec<_> = if a.pick.is_empty() {
            veh_files(&a.data, cat)?
        } else {
            a.pick.iter().map(|n| a.data.join(n)).collect()
        };
        if files.is_empty() { bail!("no models for category {cat}"); }
        let phases = [0.0, 0.22, 0.42, 0.63, 0.84, 1.0];
        let (tw, th) = (a.size.0, a.size.1);
        let mut poster = soft::Frame {
            w: tw * phases.len(), h: th * files.len(),
            rgb: vec![10; tw * phases.len() * th * files.len() * 3],
        };
        for (r, path) in files.iter().enumerate() {
            let bot = veh::build(path, a.depth)?;
            eprintln!("  {} — {} pieces", bot.label, bot.parts.len());
            let shots: Vec<(f64, f64, f64)> = phases.iter().map(|&t| (t, 0.75 + t * 0.9, 0.30)).collect();
            let cams = soft::fit_seq(&bot, &shots, tw, th, 0.92);
            for (c, &(t, yaw, pitch)) in shots.iter().enumerate() {
                let f = soft::render_cam(&bot, t, &cams[c], tw, th, yaw, pitch);
                soft::blit(&mut poster, &f, c * tw, r * th);
            }
        }
        soft::save_png(&poster, &a.out)?;
        eprintln!("poster -> {}", a.out.display());
        return Ok(());
    }

    if a.sheet {
        let all = rig::load_chassis(&a.content)?;
        let (tw, th) = (a.size.0, a.size.1);
        let cols = 4usize;
        let rows = (all.len() + cols - 1) / cols;
        let mut sheet = soft::Frame { w: tw * cols, h: th * rows, rgb: vec![10; tw * cols * th * rows * 3] };
        for (i, name) in all.keys().enumerate() {
            let bot = rig::build(&a.data, &a.content, &a.variant, name, prof(name))?;
            let f = soft::render(&bot, a.t, tw, th, 0.75 + 0.1 * i as f64, 0.32);
            soft::blit(&mut sheet, &f, (i % cols) * tw, (i / cols) * th);
            eprintln!("  {} — {}", name, bot.label);
        }
        soft::save_png(&sheet, &a.out)?;
        eprintln!("sheet -> {}", a.out.display());
        return Ok(());
    }

    let bot = rig::build(&a.data, &a.content, &a.variant, &a.chassis, prof(&a.chassis))?;
    eprintln!("{} — {} parts, {} joints", bot.label, bot.parts.len(), bot.joints.len());

    if a.frames > 0 {
        std::fs::create_dir_all(&a.out)?;
        let shots: Vec<(f64, f64, f64)> = (0..a.frames)
            .map(|i| { let t = i as f64 / (a.frames - 1).max(1) as f64; (t, 0.75 + t * 0.9, 0.30) })
            .collect();
        let cams = soft::fit_seq(&bot, &shots, a.size.0, a.size.1, 0.92);
        for (i, &(t, yaw, pitch)) in shots.iter().enumerate() {
            let f = soft::render_cam(&bot, t, &cams[i], a.size.0, a.size.1, yaw, pitch);
            soft::save_png(&f, &a.out.join(format!("fold_{i:03}.png")))?;
        }
        eprintln!("{} frames -> {}/", a.frames, a.out.display());
    } else {
        let f = soft::render(&bot, a.t, a.size.0, a.size.1, 0.75, 0.30);
        soft::save_png(&f, &a.out)?;
        eprintln!("t={} -> {}", a.t, a.out.display());
    }
    Ok(())
}
