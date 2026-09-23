//! comfortgov — a comfort governor for phones. Dry-run by default.
//!
//!   comfortgov --probe                 print what this system exposes, then exit
//!   comfortgov                         run, log decisions, write NOTHING
//!   comfortgov --apply                 run and write cpufreq / vm knobs (root)
//!   options: --interval-ms N (250)  --floor policy0=576000 (repeatable)
//!            --hint FILE   a file whose mtime is the last user input
//!                          (tideshell touches $XDG_RUNTIME_DIR/comfortgov.input)
//!            --bg-cgroup PATH   one cgroup v2 dir to reclaim from (memory.reclaim)
//!            --app-scopes DIR   the dir holding tideshell's per-app scopes
//!                          (app-tideshell-<id>-<pid>.scope, e.g.
//!                          /sys/fs/cgroup/user.slice/user-10000.slice/user@10000.service/app.slice);
//!                          reclaim rotates over them, sparing the app named in
//!            --foreground-hint FILE   (tideshell writes the front app's id there;
//!                          default $XDG_RUNTIME_DIR/comfortgov.foreground)
//!            --root DIR    sysfs/procfs root (tests; default /)

mod policy;
mod sysfs;

use policy::{Policy, Sample};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn mtime_ms(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let vals = |f: &str| -> Vec<String> { args.iter().enumerate().filter(|(_, a)| *a == f).filter_map(|(i, _)| args.get(i + 1).cloned()).collect() };
    let root = vals("--root").pop().unwrap_or_else(|| "/".into());
    let interval = Duration::from_millis(vals("--interval-ms").pop().and_then(|v| v.parse().ok()).unwrap_or(250));
    let floors: Vec<(String, u32)> = vals("--floor").iter().filter_map(|s| s.split_once('=').and_then(|(n, f)| f.parse().ok().map(|f| (n.to_string(), f)))).collect();
    let hint: Option<PathBuf> = vals("--hint").pop().map(PathBuf::from).or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(|r| PathBuf::from(r).join("comfortgov.input")));
    let bg_cgroup: Option<PathBuf> = vals("--bg-cgroup").pop().map(PathBuf::from);
    let app_scopes: Option<PathBuf> = vals("--app-scopes").pop().map(PathBuf::from);
    let fg_hint: Option<PathBuf> = vals("--foreground-hint").pop().map(PathBuf::from).or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(|r| PathBuf::from(r).join("comfortgov.foreground")));
    let mut scope_cursor = 0usize;
    let apply = has("--apply");

    let sys = sysfs::Sys::new(&root);
    let probe = sys.probe(&floors);
    if probe.clusters.is_empty() {
        eprintln!("comfortgov: no cpufreq policies under {root}sys/devices/system/cpu/cpufreq; nothing to govern");
        std::process::exit(1);
    }
    for (c, d) in probe.clusters.iter().zip(&probe.cluster_dirs) {
        let mhz: Vec<String> = c.freqs.iter().map(|f| (f / 1000).to_string()).collect();
        println!("{}: {} MHz; knee {} MHz{}  [{}]", c.name, mhz.join(" "), c.knee() / 1000, if c.floor_khz.is_some() { " (configured)" } else { " (heuristic: lowest OPP >= 30% of max; set --floor to the highest OPP at the minimum voltage)" }, d.display());
    }
    for (n, _, t) in &probe.zones {
        println!("thermal {n}: {}", t.map(|t| format!("{t:.1} C")).unwrap_or_else(|| "unreadable".into()));
    }
    match &probe.skin {
        Some((n, _, off)) if *off == 0.0 => println!("skin sensor: {n}"),
        Some((n, _, off)) => println!("skin sensor: none; using {n} minus {off} C as a proxy"),
        None => println!("skin sensor: none, no proxy: no thermal ceiling"),
    }
    println!("battery: {}", probe.battery.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none (treated as on charger)".into()));
    println!("mglru min_ttl_ms: {}   psi memory: {}   input hint: {}   background cgroup: {}", probe.has_mglru, probe.has_psi, hint.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none".into()), bg_cgroup.as_ref().or(app_scopes.as_ref()).map(|p| p.display().to_string()).unwrap_or_else(|| "none (no proactive reclaim)".into()));
    if has("--probe") {
        return;
    }
    println!("mode: {}", if apply { "APPLY (writing cpufreq/vm knobs)" } else { "dry run (decisions logged, nothing written)" });

    let mut policy = Policy::new(probe.clusters.clone());
    let mut cur: Vec<Option<policy::Window>> = vec![None; probe.clusters.len()];
    let mut last_jiffies = sys.cpu_jiffies();
    let mut last_util = 0.0f32;
    let mut last_why = String::new();
    let mut last_windows: Vec<policy::Window> = Vec::new();
    let t0 = Instant::now();
    let mut slow_tick = Instant::now();
    loop {
        std::thread::sleep(interval);
        let j = sys.cpu_jiffies();
        // No jiffies elapsed between two reads (a very short interval, or a
        // stalled counter): keep the last utilization rather than call it idle.
        let util = match (last_jiffies, j) {
            (Some((b0, t0)), Some((b1, t1))) if t1 > t0 => (b1 - b0) as f32 / (t1 - t0) as f32,
            _ => last_util,
        };
        last_util = util;
        last_jiffies = j;
        let (on_battery, battery_pct) = sys.battery(&probe.battery);
        let s = Sample {
            now_ms: now_ms(),
            last_input_ms: hint.as_ref().and_then(|p| mtime_ms(p)),
            util,
            skin_c: sys.skin_c(&probe.skin),
            on_battery,
            battery_pct,
            psi_mem_some: sys.psi_mem_some(),
            mem_avail_frac: sys.mem_avail_frac(),
        };
        let d = policy.decide(&s);
        // Log when something the user could notice changed: the state or a
        // window. The ceiling factor alone (1% per tick while the case
        // warms) is not worth a line each.
        let changed = d.windows.iter().zip(&cur).any(|(w, c)| Some(*w) != *c) || !apply && d.windows != last_windows;
        let state_key = format!("{:?}", d.state);
        if state_key != last_why || changed {
            let windows: Vec<String> = d.windows.iter().zip(&probe.clusters).map(|(w, c)| format!("{} {}-{}", c.name, w.min_khz / 1000, w.max_khz / 1000)).collect();
            println!("{:>8.1}s {}  -> {} swappiness={} ttl={}{}", t0.elapsed().as_secs_f32(), d.why, windows.join(", "), d.swappiness, d.min_ttl_ms, if d.reclaim_bytes > 0 { " reclaim" } else { "" });
            last_why = state_key;
            last_windows = d.windows.clone();
        }
        if apply {
            // Reclaim at most every 5 s: each step is real CPU work.
            let reclaim_due = slow_tick.elapsed() >= Duration::from_secs(5);
            let mut dd = d.clone();
            if !reclaim_due {
                dd.reclaim_bytes = 0;
            } else if d.reclaim_bytes > 0 {
                slow_tick = Instant::now();
            }
            // Per-app scopes: this tick's target is one background scope.
            let scope_target: Option<PathBuf> = match (&app_scopes, dd.reclaim_bytes > 0) {
                (Some(dir), true) => {
                    let scopes = sysfs::Sys::app_scopes(dir);
                    let fg = fg_hint.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
                    let t = sysfs::Sys::pick_reclaim_target(&scopes, fg.trim(), scope_cursor).map(|i| scopes[i].clone());
                    scope_cursor = scope_cursor.wrapping_add(1);
                    t
                }
                _ => None,
            };
            for line in sys.apply(&probe, &cur, &dd, scope_target.as_deref().or(bg_cgroup.as_deref())) {
                println!("         {line}");
            }
            for (i, w) in d.windows.iter().enumerate() {
                cur[i] = Some(*w);
            }
        }
    }
}
