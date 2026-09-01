//! PERFBISECT — find why the same engine runs worse on one machine than
//! another, without bisecting by hand.
//!
//! The situation this is for: unendlich runs poorly on the desktop and
//! almost as well on the phone with no flags set. That is not "the
//! desktop is slow"; a desktop GPU is not slower than a phone's. It is
//! one stage doing extra work on one machine — a validation or overlay
//! layer loaded implicitly, a present mode fighting the compositor, the
//! GPU worldgen path the phone does not take (FP32 vs the desktop's
//! FP64), a shadow map sized for the wrong screen. Each of those has a
//! knob, and the engine already ships the probes. What was missing is
//! the thing that turns the knobs on BOTH machines the same way and
//! then divides.
//!
//! ## How the bisection works
//!
//! `run` writes a report: a machine fingerprint (CPU, GPU, driver,
//! session, every VK_/MESA_/UNENDLICH_ variable in the environment, the
//! implicit Vulkan layers on disk — the usual culprit) and one line per
//! probe. Probes are three kinds:
//!
//! * **calibration**, in-process: a serial hash loop, a threaded one, a
//!   memcpy. These measure the MACHINE, so a phone's slower CPU can be
//!   divided out of everything else.
//! * **the engine's headless probes**: `perf-probe` (worldgen / octree
//!   per chunk), `world-bench` (CPU vs GPU generator), the mesh and
//!   octree benches. Parsed where their output is structured, wall-timed
//!   where it is not.
//! * **the game itself**, `unendlich-world`, run for a fixed
//!   `UNENDLICH_MAX_DURATION_MS` under a KNOB MATRIX: base; vsync fifo /
//!   immediate; generator cpu / gpu; mesher cpu / gpu; shadow map small /
//!   big; fog off; render scale half; render distance near / far;
//!   validation layer on / all layers forced off; indirect draw off. The
//!   metric is the profiler's frame p50 (`zone=frame ... p50=`) or the
//!   GPU timer's `total` line, whichever the build prints.
//!
//! `compare` then, for every probe both reports have, computes A/B and
//! divides by the calibration ratio. A probe whose normalized ratio is
//! near 1 is explained by the CPU; one far from 1 is the anomaly, and
//! the matrix says which knob moves it. A few rules turn the largest
//! departures into named suspects: render-scale-half sensitivity means
//! fragment-bound (driver, resolution, layer); forced-off layers faster
//! than base means a layer is loading implicitly; vsync variants that
//! all equal base mean present mode is not it; a generator gap that only
//! one machine shows means the GPU worldgen path.
//!
//! Nothing here is subtle. It is the loop you would do by hand, done
//! the same way twice, with the division at the end.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ------------------------------------------------------------- report ---

/// One measured number.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub probe: String,
    pub value: f64,
    pub unit: String,
    pub note: String,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub meta: BTreeMap<String, String>,
    pub rows: Vec<Row>,
}

impl Report {
    pub fn get(&self, probe: &str) -> Option<f64> {
        self.rows.iter().find(|r| r.probe == probe).map(|r| r.value)
    }

    pub fn to_tsv(&self) -> String {
        let mut s = String::new();
        s.push_str("# perfbisect report v1\n");
        for (k, v) in &self.meta {
            s.push_str(&format!("# {}\t{}\n", k, v.replace('\n', " ")));
        }
        s.push_str("probe\tvalue\tunit\tnote\n");
        for r in &self.rows {
            s.push_str(&format!("{}\t{}\t{}\t{}\n", r.probe, r.value, r.unit, r.note.replace('\t', " ")));
        }
        s
    }

    pub fn from_tsv(text: &str) -> Report {
        let mut rep = Report::default();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("# ") {
                if let Some((k, v)) = rest.split_once('\t') {
                    rep.meta.insert(k.to_string(), v.to_string());
                }
                continue;
            }
            if line.starts_with("probe\t") || line.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 3 {
                continue;
            }
            if let Ok(v) = f[1].parse::<f64>() {
                rep.rows.push(Row { probe: f[0].to_string(), value: v, unit: f[2].to_string(), note: f.get(3).unwrap_or(&"").to_string() });
            }
        }
        rep
    }
}

// -------------------------------------------------------- fingerprint ---

fn sh(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).stdin(Stdio::null()).output().ok()?;
    if !out.status.success() && out.stdout.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn fingerprint(rep: &mut Report) {
    let m = &mut rep.meta;
    m.insert("host".into(), sh("hostname", &[]).unwrap_or_default().trim().to_string());
    m.insert("uname".into(), sh("uname", &["-srm"]).unwrap_or_default().trim().to_string());
    m.insert("nproc".into(), std::thread::available_parallelism().map(|n| n.get().to_string()).unwrap_or_default());
    if let Ok(cpu) = fs::read_to_string("/proc/cpuinfo") {
        let model = cpu.lines().find(|l| l.starts_with("model name") || l.starts_with("Hardware") || l.starts_with("CPU part")).unwrap_or("").to_string();
        m.insert("cpu".into(), model.split(':').nth(1).unwrap_or("").trim().to_string());
    }
    if let Ok(mem) = fs::read_to_string("/proc/meminfo") {
        if let Some(l) = mem.lines().find(|l| l.starts_with("MemTotal")) {
            m.insert("mem".into(), l.split_whitespace().nth(1).unwrap_or("").to_string() + " kB");
        }
    }
    // the GPU and its driver: vulkaninfo's summary is the authority
    if let Some(vi) = sh("vulkaninfo", &["--summary"]) {
        for key in ["deviceName", "driverName", "driverInfo", "apiVersion", "driverVersion", "deviceType"] {
            let vals: Vec<String> = vi
                .lines()
                .filter(|l| l.trim_start().starts_with(key))
                .map(|l| l.split('=').nth(1).unwrap_or("").trim().to_string())
                .collect();
            if !vals.is_empty() {
                m.insert(format!("vk.{key}"), vals.join(" | "));
            }
        }
        // vulkaninfo lists layers it finds; a desktop with three is the story
        let layers: Vec<&str> = vi.lines().filter(|l| l.contains("VK_LAYER_")).map(|l| l.trim()).collect();
        m.insert("vk.layers_seen".into(), layers.join(" | "));
    } else {
        m.insert("vk".into(), "vulkaninfo not found".into());
    }
    // implicit layers on disk: loaded into EVERY vulkan app without asking
    let mut implicit = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();
    for d in ["/usr/share/vulkan/implicit_layer.d", "/etc/vulkan/implicit_layer.d", "/usr/local/share/vulkan/implicit_layer.d"] {
        if let Ok(rd) = fs::read_dir(d) {
            implicit.extend(rd.flatten().map(|e| e.path().display().to_string()));
        }
    }
    if let Ok(rd) = fs::read_dir(format!("{home}/.local/share/vulkan/implicit_layer.d")) {
        implicit.extend(rd.flatten().map(|e| e.path().display().to_string()));
    }
    m.insert("vk.implicit_layers".into(), implicit.join(" | "));
    // the environment that steers a vulkan app or the engine
    let mut env: Vec<String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("VK_") || k.starts_with("MESA_") || k.starts_with("UNENDLICH_") || k == "DISPLAY" || k == "WAYLAND_DISPLAY" || k == "XDG_SESSION_TYPE" || k == "__GL_SYNC_TO_VBLANK" || k == "DXVK_HUD" || k == "MANGOHUD" || k == "ENABLE_VKBASALT")
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    env.sort();
    m.insert("env".into(), env.join(" | "));
    m.insert("when".into(), sh("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]).unwrap_or_default().trim().to_string());
}

// -------------------------------------------------------- calibration ---

fn calib(rep: &mut Report) {
    // serial: one core's integer speed
    let t = Instant::now();
    let mut x: u64 = 0x9E3779B97F4A7C15;
    for i in 0..(1u64 << 26) {
        x = (x ^ i).wrapping_mul(0xBF58476D1CE4E5B9).rotate_left(23);
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    rep.rows.push(Row { probe: "calib.cpu.serial".into(), value: ms, unit: "ms".into(), note: format!("hash {:x}", x & 0xff) });

    // threaded: the whole machine's integer throughput
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let t = Instant::now();
    let hs: Vec<_> = (0..n)
        .map(|k| {
            std::thread::spawn(move || {
                let mut x: u64 = 0x9E3779B97F4A7C15 ^ k as u64;
                for i in 0..(1u64 << 25) {
                    x = (x ^ i).wrapping_mul(0xBF58476D1CE4E5B9).rotate_left(23);
                }
                x
            })
        })
        .collect();
    let mut acc = 0u64;
    for h in hs {
        acc ^= h.join().unwrap_or(0);
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    rep.rows.push(Row { probe: "calib.cpu.threads".into(), value: ms, unit: "ms".into(), note: format!("{n} threads, {:x}", acc & 0xff) });

    // memory: a copy the size of a few chunk pools
    let src = vec![1u8; 128 << 20];
    let mut dst = vec![0u8; 128 << 20];
    let t = Instant::now();
    dst.copy_from_slice(&src);
    let s = t.elapsed().as_secs_f64().max(1e-6);
    rep.rows.push(Row { probe: "calib.mem.copy".into(), value: 0.128 / s, unit: "GB/s".into(), note: format!("{}", dst[dst.len() / 2]) });
}

// -------------------------------------------------------- the engine ---

struct Engine {
    dir: PathBuf,
}

impl Engine {
    fn bin(&self, name: &str) -> Option<PathBuf> {
        for prof in ["release", "debug"] {
            let p = self.dir.join("target").join(prof).join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }
}

/// Run a binary with extra env, capture stdout+stderr and wall time.
fn run_bin(bin: &Path, args: &[&str], env: &[(&str, &str)], clear: &[&str], timeout: Duration) -> (String, f64, bool) {
    let mut cmd = Command::new(bin);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for k in clear {
        cmd.env_remove(k);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let t = Instant::now();
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (format!("spawn failed: {e}"), 0.0, false),
    };
    // a wall clock on the child: the game exits on its own duration knob,
    // the probes are bounded, but a hung driver is not our bisection
    let out = wait_with_timeout(child, timeout);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    match out {
        Some(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).to_string();
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            (text, ms, o.status.success())
        }
        None => ("timed out".into(), ms, false),
    }
}

fn wait_with_timeout(mut child: std::process::Child, timeout: Duration) -> Option<std::process::Output> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

/// `perf-probe`'s CSV: radius,chunks,stage,total_ms,per_chunk_us,...
pub fn parse_perf_probe(text: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for l in text.lines() {
        let f: Vec<&str> = l.split(',').collect();
        if f.len() < 5 || f[0] == "radius" {
            continue;
        }
        if let (Ok(r), Ok(us)) = (f[0].parse::<i32>(), f[4].parse::<f64>()) {
            out.push((format!("probe.{}.r{}", f[2].to_lowercase(), r), us));
        }
    }
    out
}

/// The engine profiler's `zone=NAME n=.. mean=.. p50=.. p95=.. p99=.. max=..`
/// lines, and the GPU timer's `total ... p50=.. p99=..` line.
pub fn parse_game(text: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let grab = |l: &str, key: &str| -> Option<f64> {
        let i = l.find(key)?;
        let tok = l[i + key.len()..].trim_start().split(|c: char| c.is_whitespace() || c == ',' || c == ')').next()?;
        // "4.20x", "7.01ms": the unit rides on the number
        tok.trim_end_matches(|c: char| !(c.is_ascii_digit() || c == '.')).parse::<f64>().ok()
    };
    for l in text.lines() {
        if let Some(z) = l.split_whitespace().find(|w| w.starts_with("zone=")) {
            let name = z.trim_start_matches("zone=").to_lowercase();
            if let Some(p50) = grab(l, "p50=") {
                out.push((format!("game.zone.{name}.p50"), p50));
            }
            if let Some(p99) = grab(l, "p99=") {
                out.push((format!("game.zone.{name}.p99"), p99));
            }
        } else if l.trim_start().starts_with("total") && l.contains("p50=") {
            if let Some(p50) = grab(l, "p50=") {
                out.push(("game.gpu.total.p50".into(), p50));
            }
            if let Some(p99) = grab(l, "p99=") {
                out.push(("game.gpu.total.p99".into(), p99));
            }
        } else if l.contains("speedup vs CPU:") {
            if let Some(x) = grab(l, "speedup vs CPU:") {
                out.push(("bench.world.gpu_speedup".into(), x));
            }
        }
    }
    out
}

/// The engine's events.jsonl: count the `gpu_spike` events after
/// `warmup_complete` and the largest one, in ms. No JSON parser — the
/// engine writes one flat object per line and two field names are all
/// this needs.
pub fn parse_spikes(text: &str) -> (usize, f64) {
    let mut warm = false;
    let mut n = 0usize;
    let mut max_us = 0.0f64;
    let field = |l: &str, k: &str| -> Option<f64> {
        let i = l.find(&format!("\"{k}\":"))?;
        l[i + k.len() + 3..].trim_start().split(|c: char| c == ',' || c == '}').next()?.parse::<f64>().ok()
    };
    for l in text.lines() {
        if l.contains("\"kind\":\"warmup_complete\"") {
            warm = true;
        }
        if warm && l.contains("\"kind\":\"gpu_spike\"") {
            n += 1;
            if let Some(t) = field(l, "total_us") {
                max_us = max_us.max(t);
            }
        }
    }
    (n, max_us / 1000.0)
}

/// The knob matrix. Each variant isolates one suspect; the metric is the
/// same for all of them so the compare can divide.
pub fn matrix() -> Vec<(&'static str, Vec<(&'static str, &'static str)>, Vec<&'static str>)> {
    vec![
        ("base", vec![], vec![]),
        ("vsync.fifo", vec![("UNENDLICH_VSYNC", "1")], vec![]),
        ("vsync.immediate", vec![("UNENDLICH_VSYNC", "immediate")], vec![]),
        ("gen.cpu", vec![("UNENDLICH_GEN_BACKEND", "cpu")], vec![]),
        ("gen.gpu", vec![("UNENDLICH_GEN_BACKEND", "gpu")], vec![]),
        ("mesher.cpu", vec![("UNENDLICH_GPU_MESHER", "0")], vec![]),
        ("mesher.gpu", vec![("UNENDLICH_GPU_MESHER", "1")], vec![]),
        ("shadows.small", vec![("UNENDLICH_SHADOW_MAP_SIZE", "512")], vec![]),
        ("shadows.big", vec![("UNENDLICH_SHADOW_MAP_SIZE", "4096")], vec![]),
        ("fog.off", vec![("UNENDLICH_NO_FOG", "1")], vec![]),
        ("scale.half", vec![("UNENDLICH_RENDER_SCALE", "0.5")], vec![]),
        ("dist.near", vec![("UNENDLICH_RENDER_DISTANCE", "4")], vec![]),
        ("dist.far", vec![("UNENDLICH_RENDER_DISTANCE", "12")], vec![]),
        ("indirect.off", vec![("UNENDLICH_INDIRECT", "0")], vec![]),
        ("validation.on", vec![("VK_INSTANCE_LAYERS", "VK_LAYER_KHRONOS_validation")], vec![]),
        // every layer off, including implicit ones a desktop loads without asking
        ("layers.forced_off", vec![("VK_LOADER_LAYERS_DISABLE", "~all~"), ("VK_INSTANCE_LAYERS", "")], vec!["MANGOHUD", "ENABLE_VKBASALT", "DXVK_HUD"]),
        ("frames.one", vec![("UNENDLICH_FRAMES_IN_FLIGHT", "1")], vec![]),
        ("frames.three", vec![("UNENDLICH_FRAMES_IN_FLIGHT", "3")], vec![]),
    ]
}

fn run(args: &[String]) -> i32 {
    let mut engine = PathBuf::from("games/unendlich");
    let mut out = PathBuf::from("perfbisect.tsv");
    let mut seconds = 12u64;
    let mut only: Option<Vec<String>> = None;
    let mut quick = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--engine" => {
                engine = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--out" => {
                out = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--seconds" => {
                seconds = args[i + 1].parse().unwrap_or(12);
                i += 1;
            }
            "--only" => {
                only = Some(args[i + 1].split(',').map(|s| s.to_string()).collect());
                i += 1;
            }
            "--quick" => quick = true,
            _ => {}
        }
        i += 1;
    }
    let want = |k: &str| only.as_ref().map(|o| o.iter().any(|x| x == k)).unwrap_or(true);
    let eng = Engine { dir: engine };
    let mut rep = Report::default();
    fingerprint(&mut rep);
    rep.meta.insert("engine_dir".into(), eng.dir.display().to_string());
    rep.meta.insert("game_seconds".into(), seconds.to_string());

    if want("calib") {
        eprintln!("perfbisect: calibration");
        calib(&mut rep);
    }

    // the engine's headless probes
    if want("probe") {
        match eng.bin("perf-probe") {
            Some(b) => {
                let radii = if quick { "1,2" } else { "1,2,3" };
                eprintln!("perfbisect: perf-probe {radii}");
                let (text, ms, ok) = run_bin(&b, &[radii, "--repeats", "2"], &[], &[], Duration::from_secs(600));
                rep.rows.push(Row { probe: "probe.wall".into(), value: ms, unit: "ms".into(), note: if ok { "ok".into() } else { "failed".into() } });
                for (k, v) in parse_perf_probe(&text) {
                    rep.rows.push(Row { probe: k, value: v, unit: "us/chunk".into(), note: String::new() });
                }
            }
            None => rep.meta.insert("missing.perf-probe".into(), "not built".into()).map(|_| ()).unwrap_or(()),
        }
        for name in ["world-bench", "mesh-bench", "octree-bench", "gpu-mesh-bench", "gpu-headless-probe"] {
            let Some(b) = eng.bin(name) else {
                rep.meta.insert(format!("missing.{name}"), "not built".into());
                continue;
            };
            eprintln!("perfbisect: {name}");
            let (text, ms, ok) = run_bin(&b, &[], &[], &[], Duration::from_secs(600));
            let key = format!("bench.{}", name.replace('-', "_"));
            rep.rows.push(Row { probe: format!("{key}.wall"), value: ms, unit: "ms".into(), note: if ok { "ok".into() } else { "failed".into() } });
            for (k, v) in parse_game(&text) {
                rep.rows.push(Row { probe: k, value: v, unit: "x".into(), note: name.into() });
            }
        }
    }

    // the game, under the matrix. `unendlich-fullgame` is the maintained
    // bin (it followed the GLFW-decoupling refactor of input.rs;
    // `unendlich-world` did not and no longer builds), so it is tried
    // first; the older bin is the fallback for a tree where it still
    // compiles.
    if want("game") {
        let game = eng.bin("unendlich-fullgame").or_else(|| eng.bin("unendlich-world"));
        if let Some(b) = &game {
            rep.meta.insert("game_bin".into(), b.display().to_string());
        }
        match game {
            Some(b) => {
                let dur = (seconds * 1000).to_string();
                let warm = ((seconds * 1000) / 4).to_string();
                for (name, env, clear) in matrix() {
                    if quick && !matches!(name, "base" | "scale.half" | "layers.forced_off" | "vsync.immediate" | "gen.cpu") {
                        continue;
                    }
                    eprintln!("perfbisect: game.{name}");
                    let events_path = std::env::temp_dir().join(format!("perfbisect-events-{name}.jsonl"));
                    let _ = fs::remove_file(&events_path);
                    let events_str = events_path.display().to_string();
                    let mut e: Vec<(&str, &str)> = vec![("UNENDLICH_MAX_DURATION_MS", dur.as_str()), ("UNENDLICH_WARMUP_MS", warm.as_str()), ("UNENDLICH_EVENTS_PATH", events_str.as_str())];
                    e.extend(env.iter().copied());
                    let (text, ms, ok) = run_bin(&b, &[], &e, &clear, Duration::from_secs(seconds + 90));
                    let parsed = parse_game(&text);
                    // THE LAST BLOCK WINS. The engine prints its zone summary
                    // more than once (a boot window, then steady state); the
                    // first is the load and the last is the game. The desktop
                    // report keyed on the first and read a 635 ms frame that
                    // was chunk loading.
                    let last = |k: &str| parsed.iter().rev().find(|(n, _)| n == k).map(|(_, v)| *v);
                    // frame_total is the CPU frame; the GPU timer's total is
                    // the fallback, and p99 is a first-class row because the
                    // desktop's whole problem was a 4 ms p50 with a 1.6 s p99
                    let frame = last("game.zone.frame_total.p50").or_else(|| last("game.zone.frame.p50")).or_else(|| last("game.gpu.total.p50"));
                    let frame99 = last("game.zone.frame_total.p99").or_else(|| last("game.zone.frame.p99")).or_else(|| last("game.gpu.total.p99"));
                    match frame {
                        Some(v) => rep.rows.push(Row { probe: format!("game.{name}.frame_p50"), value: v, unit: "ms".into(), note: if ok { "ok".into() } else { "exit!=0".into() } }),
                        None => rep.rows.push(Row { probe: format!("game.{name}.frame_p50"), value: f64::NAN, unit: "ms".into(), note: format!("no summary in output (wall {ms:.0} ms, ok {ok})") }),
                    }
                    if let Some(v) = frame99 {
                        rep.rows.push(Row { probe: format!("game.{name}.frame_p99"), value: v, unit: "ms".into(), note: "the hitch".into() });
                    }
                    if let Some(v) = last("game.zone.draw.p99") {
                        rep.rows.push(Row { probe: format!("game.{name}.draw_p99"), value: v, unit: "ms".into(), note: String::new() });
                    }
                    if let Some(v) = last("game.gpu.total.p50") {
                        rep.rows.push(Row { probe: format!("game.{name}.gpu_p50"), value: v, unit: "ms".into(), note: String::new() });
                    }
                    // the engine's own event log: GPU spikes after warmup are
                    // the hitches, counted and sized
                    if let Ok(ev) = fs::read_to_string(&events_path) {
                        let (n, max_ms) = parse_spikes(&ev);
                        rep.rows.push(Row { probe: format!("game.{name}.gpu_spikes"), value: n as f64, unit: "count".into(), note: "after warmup_complete".into() });
                        rep.rows.push(Row { probe: format!("game.{name}.spike_max"), value: max_ms, unit: "ms".into(), note: String::new() });
                    }
                    // every zone, last block only
                    let mut seen = std::collections::HashSet::new();
                    for (k, v) in parsed.iter().rev() {
                        if seen.insert(k.clone()) {
                            rep.rows.push(Row { probe: format!("game.{name}.{}", k.trim_start_matches("game.")), value: *v, unit: "ms".into(), note: String::new() });
                        }
                    }
                }
            }
            None => {
                rep.meta.insert("missing.game".into(), "no unendlich-fullgame or unendlich-world in target/ (cargo build --release --features \"render window ui-overlay\" --bin unendlich-fullgame)".into());
            }
        }
    }

    let text = rep.to_tsv();
    if let Err(e) = fs::write(&out, &text) {
        eprintln!("perfbisect: cannot write {}: {e}", out.display());
        return 2;
    }
    println!("{text}");
    eprintln!("perfbisect: wrote {}", out.display());
    0
}

// ------------------------------------------------------------ compare ---

/// One probe across the two machines.
#[derive(Clone, Debug, PartialEq)]
pub struct Cmp {
    pub probe: String,
    pub a: f64,
    pub b: f64,
    /// a / b
    pub ratio: f64,
    /// a / b divided by what the CPU calibration ratio predicts
    pub anomaly: f64,
}

pub fn compare(a: &Report, b: &Report) -> Vec<Cmp> {
    let cal = match (a.get("calib.cpu.serial"), b.get("calib.cpu.serial")) {
        (Some(x), Some(y)) if y > 0.0 => x / y,
        _ => 1.0,
    };
    let mut out = Vec::new();
    for ra in &a.rows {
        let Some(vb) = b.get(&ra.probe) else { continue };
        if !ra.value.is_finite() || !vb.is_finite() || vb == 0.0 || ra.probe.starts_with("calib.") {
            continue;
        }
        let ratio = ra.value / vb;
        // GB/s is bigger-is-better; everything else is a time
        let anomaly = if ra.unit == "GB/s" { (1.0 / ratio) / cal } else { ratio / cal };
        out.push(Cmp { probe: ra.probe.clone(), a: ra.value, b: vb, ratio, anomaly });
    }
    out.sort_by(|x, y| y.anomaly.ln().abs().partial_cmp(&x.anomaly.ln().abs()).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// The rules: which departures name a suspect. Each verdict carries the
/// numbers that triggered it, so it can be disbelieved on evidence.
pub fn verdicts(a: &Report, b: &Report, cmps: &[Cmp]) -> Vec<String> {
    let mut v = Vec::new();
    let g = |r: &Report, k: &str| r.get(&format!("game.{k}.frame_p50")).filter(|x| x.is_finite());
    let name_a = a.meta.get("host").cloned().unwrap_or("A".into());
    let name_b = b.meta.get("host").cloned().unwrap_or("B".into());

    // implicit layers present on one machine only
    let la = a.meta.get("vk.implicit_layers").cloned().unwrap_or_default();
    let lb = b.meta.get("vk.implicit_layers").cloned().unwrap_or_default();
    if la != lb {
        v.push(format!("implicit Vulkan layers differ: {name_a} has [{la}], {name_b} has [{lb}] — a layer loads into every app without asking; this is the first thing to disprove"));
    }
    if let (Some(base), Some(off)) = (g(a, "base"), g(a, "layers.forced_off")) {
        if off > 0.0 && base / off > 1.25 {
            v.push(format!("{name_a}: forcing all layers off takes the frame from {base:.1} to {off:.1} ms (x{:.2}) — a layer is loaded by default there", base / off));
        }
    }
    if let (Some(base), Some(half)) = (g(a, "base"), g(a, "scale.half")) {
        if half > 0.0 && base / half > 1.6 {
            v.push(format!("{name_a}: half render scale takes the frame from {base:.1} to {half:.1} ms (x{:.2}) — fragment-bound: driver, resolution or a layer, not the CPU", base / half));
        } else if half > 0.0 && base / half < 1.15 {
            v.push(format!("{name_a}: half render scale barely moves the frame ({base:.1} -> {half:.1} ms) — not fragment-bound; look at present mode, the CPU side, or uploads"));
        }
    }
    // fifo is CAPPED at the refresh by definition, so it is not evidence
    // either way; immediate against base is the question (the first
    // version called an 8.5 ms fifo frame a present-mode problem)
    if let (Some(base), Some(imm)) = (g(a, "base"), g(a, "vsync.immediate")) {
        let spread = base.max(imm) / base.min(imm).max(1e-6);
        if spread < 1.15 {
            v.push(format!("{name_a}: immediate and base within 15% ({imm:.1}/{base:.1} ms) — present mode is not the cause"));
        } else {
            v.push(format!("{name_a}: immediate {imm:.1} vs base {base:.1} ms — the swapchain mode or the compositor is in the loop"));
        }
    }
    if let (Some(cpu), Some(gpu)) = (g(a, "gen.cpu"), g(a, "gen.gpu")) {
        let bcpu = g(b, "gen.cpu");
        let bgpu = g(b, "gen.gpu");
        let ra = gpu / cpu.max(1e-6);
        if let (Some(bc), Some(bg)) = (bcpu, bgpu) {
            let rb = bg / bc.max(1e-6);
            if (ra / rb.max(1e-6)) > 1.4 {
                v.push(format!("GPU worldgen costs {name_a} x{ra:.2} vs cpu gen, but {name_b} only x{rb:.2} — the compute-shader path (FP64 vs the FP32 port; see STATUS.md) differs between the two drivers"));
            }
        } else if ra > 1.4 {
            v.push(format!("{name_a}: GPU worldgen is slower than CPU worldgen there (x{ra:.2}) — the compute path is the suspect"));
        }
    }
    if let (Some(cpu), Some(gpu)) = (g(a, "mesher.cpu"), g(a, "mesher.gpu")) {
        if gpu / cpu.max(1e-6) > 1.4 {
            v.push(format!("{name_a}: the GPU mesher is slower than the CPU one there (x{:.2})", gpu / cpu.max(1e-6)));
        }
    }
    // HITCHING, NOT SLOW: a fine p50 with a p99 twenty times it is a
    // stall, and the stall has a knob if one variant's p99 is sane
    let g99 = |r: &Report, k: &str| r.get(&format!("game.{k}.frame_p99")).filter(|x| x.is_finite());
    if let (Some(p50), Some(p99)) = (g(a, "base"), g99(a, "base")) {
        if p50 > 0.0 && p99 / p50 > 20.0 {
            v.push(format!("{name_a}: not slow, HITCHING — base frame p50 {p50:.1} ms but p99 {p99:.0} ms (x{:.0}). The p50 is a fine machine; the p99 is a stall", p99 / p50));
            let mut calm: Vec<(String, f64)> = matrix()
                .iter()
                .filter_map(|(n, _, _)| g99(a, n).map(|x| (n.to_string(), x)))
                .filter(|(_, x)| *x < p99 * 0.1)
                .collect();
            calm.sort_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal));
            if let Some((n, x)) = calm.first() {
                v.push(format!("{name_a}: the stall has a knob — game.{n} has frame p99 {x:.1} ms while base has {p99:.0}; that variant's one knob is the suspect"));
            }
        }
    }
    if let (Some(sc), Some(sg)) = (a.get("game.mesher.cpu.gpu_spikes"), a.get("game.mesher.gpu.gpu_spikes")) {
        if sg >= 3.0 && sc <= 1.0 {
            v.push(format!("{name_a}: {sg:.0} GPU spikes after warmup with the GPU mesher, {sc:.0} with the CPU mesher — the mesher's chunk arrival stalls the graphics queue"));
        }
    }
    // roamer_tick variance across variants: world-state luck, not a knob
    let roam: Vec<f64> = matrix().iter().filter_map(|(n, _, _)| a.get(&format!("game.{n}.zone.roamer_tick.p50"))).filter(|x| x.is_finite()).collect();
    if roam.len() >= 4 {
        let (lo, hi) = roam.iter().fold((f64::MAX, 0.0f64), |(l, h), x| (l.min(*x), h.max(*x)));
        if hi > 8.0 && hi / lo.max(0.1) > 3.0 {
            v.push(format!("{name_a}: roamer_tick p50 ranges {lo:.1}–{hi:.1} ms across variants that do not touch it — creature AI is world-state luck and, when heavy, most of a frame; cap its per-frame work"));
        }
    }
    // the biggest anomaly overall, if it is big
    if let Some(top) = cmps.first() {
        if top.anomaly.ln().abs() > (1.5f64).ln() {
            v.push(format!("largest departure from what the CPU explains: {} at x{:.2} ({name_a} {:.2} vs {name_b} {:.2})", top.probe, top.anomaly, top.a, top.b));
        }
    }
    if v.is_empty() {
        v.push("no rule fired: the two machines differ roughly as their CPUs do".into());
    }
    v
}

fn cmd_compare(args: &[String]) -> i32 {
    if args.len() < 2 {
        eprintln!("usage: perfbisect compare A.tsv B.tsv");
        return 2;
    }
    let a = Report::from_tsv(&fs::read_to_string(&args[0]).unwrap_or_default());
    let b = Report::from_tsv(&fs::read_to_string(&args[1]).unwrap_or_default());
    let cmps = compare(&a, &b);
    let na = a.meta.get("host").cloned().unwrap_or("A".into());
    let nb = b.meta.get("host").cloned().unwrap_or("B".into());
    println!("perfbisect compare — A = {na} ({}), B = {nb} ({})", a.meta.get("vk.deviceName").cloned().unwrap_or("?".into()), b.meta.get("vk.deviceName").cloned().unwrap_or("?".into()));
    let cal = a.get("calib.cpu.serial").zip(b.get("calib.cpu.serial")).map(|(x, y)| x / y).unwrap_or(1.0);
    println!("CPU calibration ratio A/B = x{cal:.2}  (everything below is divided by it)\n");
    println!("{:<40} {:>10} {:>10} {:>8} {:>9}", "probe", na, nb, "A/B", "anomaly");
    for c in &cmps {
        let flag = if c.anomaly.ln().abs() > (1.5f64).ln() { "  <-" } else { "" };
        println!("{:<40} {:>10.2} {:>10.2} {:>8.2} {:>9.2}{flag}", c.probe, c.a, c.b, c.ratio, c.anomaly);
    }
    println!("\nverdicts:");
    for v in verdicts(&a, &b, &cmps) {
        println!("  * {v}");
    }
    0
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let code = match argv.get(1).map(|s| s.as_str()) {
        Some("run") => run(&argv[2..]),
        Some("compare") => cmd_compare(&argv[2..]),
        _ => {
            let mut e = std::io::stderr();
            let _ = writeln!(e, "perfbisect — bisect a cross-machine performance disparity\n");
            let _ = writeln!(e, "  perfbisect run [--engine games/unendlich] [--out host.tsv] [--seconds 12] [--only calib,probe,game] [--quick]");
            let _ = writeln!(e, "  perfbisect compare desktop.tsv phone.tsv\n");
            let _ = writeln!(e, "build the engine's probes first: cd games/unendlich && cargo build --release --features \"render window ui-overlay\"");
            let _ = writeln!(e, "  --bin unendlich-fullgame --bin perf-probe --bin world-bench --bin mesh-bench --bin octree-bench");
            let _ = writeln!(e, "run from the MONOREPO ROOT: --manifest-path and --engine are relative to it");
            2
        }
    };
    std::process::exit(code);
}

// -------------------------------------------------------------- tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_survives_the_tsv_round_trip() {
        let mut r = Report::default();
        r.meta.insert("host".into(), "desk".into());
        r.rows.push(Row { probe: "calib.cpu.serial".into(), value: 120.5, unit: "ms".into(), note: "x".into() });
        r.rows.push(Row { probe: "game.base.frame_p50".into(), value: 7.25, unit: "ms".into(), note: String::new() });
        let back = Report::from_tsv(&r.to_tsv());
        assert_eq!(back.meta.get("host").map(|s| s.as_str()), Some("desk"));
        assert_eq!(back.rows, r.rows);
    }

    #[test]
    fn perf_probe_csv_parses_per_chunk_microseconds() {
        let text = "radius,chunks,stage,total_ms,per_chunk_us,chunks_per_sec,extra\n2,125,worldgen,300.000,2400.0,416,\n2,125,octree,50.0,400.0,2500,nodes=1\n";
        let rows = parse_perf_probe(text);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("probe.worldgen.r2".into(), 2400.0));
        assert_eq!(rows[1].0, "probe.octree.r2");
    }

    #[test]
    fn the_profiler_and_gpu_timer_lines_are_read() {
        let text = "zone=frame n=700 mean=8.10 p50=7.90 p95=12.0 p99=30.5 max=90\n  total     p50= 7.01 ms  p99= 92.82 ms  over_16.6ms=21 (3.0%)\n  speedup vs CPU: 4.20x\n";
        let rows = parse_game(text);
        let get = |k: &str| rows.iter().find(|(n, _)| n == k).map(|(_, v)| *v);
        assert_eq!(get("game.zone.frame.p50"), Some(7.90));
        assert_eq!(get("game.zone.frame.p99"), Some(30.5));
        assert_eq!(get("game.gpu.total.p50"), Some(7.01));
        assert_eq!(get("game.gpu.total.p99"), Some(92.82));
        assert_eq!(get("bench.world.gpu_speedup"), Some(4.20));
    }

    fn rep(host: &str, cal: f64, rows: &[(&str, f64)]) -> Report {
        let mut r = Report::default();
        r.meta.insert("host".into(), host.into());
        r.rows.push(Row { probe: "calib.cpu.serial".into(), value: cal, unit: "ms".into(), note: String::new() });
        for (k, v) in rows {
            r.rows.push(Row { probe: k.to_string(), value: *v, unit: "ms".into(), note: String::new() });
        }
        r
    }

    #[test]
    fn the_anomaly_divides_out_the_cpu_and_ranks_the_departure_first() {
        // the phone's CPU is 3x slower; its worldgen is 3x slower (explained);
        // the desktop's frame is 4x slower than the phone's (unexplained)
        let desk = rep("desk", 100.0, &[("probe.worldgen.r2", 1000.0), ("game.base.frame_p50", 40.0)]);
        let phone = rep("phone", 300.0, &[("probe.worldgen.r2", 3000.0), ("game.base.frame_p50", 10.0)]);
        let c = compare(&desk, &phone);
        assert_eq!(c[0].probe, "game.base.frame_p50");
        assert!((c[0].anomaly - 12.0).abs() < 1e-6, "{:?}", c[0]);
        let wg = c.iter().find(|x| x.probe == "probe.worldgen.r2").unwrap();
        assert!((wg.anomaly - 1.0).abs() < 1e-6, "explained by the cpu: {:?}", wg);
    }

    #[test]
    fn the_rules_name_a_layer_and_a_fragment_bound_frame() {
        let mut desk = rep("desk", 100.0, &[("game.base.frame_p50", 40.0), ("game.layers.forced_off.frame_p50", 10.0), ("game.scale.half.frame_p50", 12.0), ("game.vsync.fifo.frame_p50", 40.5), ("game.vsync.immediate.frame_p50", 39.8)]);
        desk.meta.insert("vk.implicit_layers".into(), "/usr/share/vulkan/implicit_layer.d/MangoHud.json".into());
        let mut phone = rep("phone", 300.0, &[("game.base.frame_p50", 10.0), ("game.layers.forced_off.frame_p50", 10.0), ("game.scale.half.frame_p50", 9.5), ("game.vsync.fifo.frame_p50", 10.0), ("game.vsync.immediate.frame_p50", 10.0)]);
        phone.meta.insert("vk.implicit_layers".into(), String::new());
        let c = compare(&desk, &phone);
        let v = verdicts(&desk, &phone, &c);
        let all = v.join("\n");
        assert!(all.contains("implicit Vulkan layers differ"), "{all}");
        assert!(all.contains("a layer is loaded by default"), "{all}");
        assert!(all.contains("fragment-bound"), "{all}");
        assert!(all.contains("present mode is not the cause"), "{all}");
    }

    #[test]
    fn spikes_are_counted_after_warmup_only_and_the_largest_kept() {
        let ev = "{\"kind\":\"gpu_spike\",\"main_us\":900000,\"total_us\":950000,\"ts_ms\":100}\n{\"kind\":\"warmup_complete\",\"ts_ms\":3000}\n{\"kind\":\"gpu_spike\",\"main_us\":545824,\"shadow_us\":171902,\"total_us\":717917,\"ts_ms\":7010}\n{\"kind\":\"gpu_spike\",\"total_us\":1017431,\"ts_ms\":7198}\n";
        let (n, max_ms) = parse_spikes(ev);
        assert_eq!(n, 2, "the boot spike counted");
        assert!((max_ms - 1017.431).abs() < 1e-3);
    }

    #[test]
    fn a_fine_p50_with_a_huge_p99_is_called_hitching_and_the_calm_variant_is_named() {
        let mut desk = rep("desk", 100.0, &[("game.base.frame_p50", 19.8), ("game.base.frame_p99", 1600.0), ("game.mesher.cpu.frame_p99", 12.9), ("game.mesher.gpu.frame_p99", 766.0), ("game.fog.off.frame_p99", 991.0)]);
        desk.rows.push(Row { probe: "game.mesher.cpu.gpu_spikes".into(), value: 0.0, unit: "count".into(), note: String::new() });
        desk.rows.push(Row { probe: "game.mesher.gpu.gpu_spikes".into(), value: 6.0, unit: "count".into(), note: String::new() });
        let phone = rep("phone", 300.0, &[("game.base.frame_p50", 30.0), ("game.base.frame_p99", 45.0)]);
        let c = compare(&desk, &phone);
        let all = verdicts(&desk, &phone, &c).join("\n");
        assert!(all.contains("HITCHING"), "{all}");
        assert!(all.contains("game.mesher.cpu has frame p99 12.9"), "{all}");
        assert!(all.contains("stalls the graphics queue"), "{all}");
    }

    #[test]
    fn the_matrix_isolates_one_knob_per_variant() {
        for (name, env, _) in matrix() {
            assert!(env.len() <= 2, "{name} turns {} knobs", env.len());
        }
        assert!(matrix().iter().any(|(n, _, _)| *n == "base"));
    }
}
