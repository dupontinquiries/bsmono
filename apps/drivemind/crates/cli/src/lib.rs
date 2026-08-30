// drivemind-cli — dispatch in the gin style: a match over words, no
// argument-parsing framework, every command a small function. The engine's
// Event stream is printed as it happens (Exec lines prefixed `$`, Notes
// prefixed `!`), so a user watching the terminal sees the same loop the
// egui shell renders — one engine, two honest windows onto it.

use drivemind_core::{
    advisor, cleanup, cloneop, deltahostile, device, human_bytes, integrity, smart, video, Event,
    HostRunner,
};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    match cmd {
        "help" | "-h" | "--help" => {
            print!("{HELP}");
            0
        }
        "version" | "-V" | "--version" => {
            println!("dvm {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "devices" | "ls" => cmd_devices(),
        "smart" => match rest.first() {
            Some(d) => cmd_smart(d),
            None => err("usage: dvm smart /dev/<disk>"),
        },
        "health" => cmd_health(),
        "verify" => cmd_verify(&rest),
        "cleanup" => match rest.first() {
            Some(p) => cmd_cleanup(p),
            None => err("usage: dvm cleanup <path>"),
        },
        "videos" => cmd_videos(&rest),
        "gitscan" => match rest.first() {
            Some(p) => cmd_gitscan(p),
            None => err("usage: dvm gitscan <repo>"),
        },
        "clone" => cmd_clone(&rest),
        "advise" => cmd_advise(&rest),
        "selftest" => match rest.first() {
            Some(d) => cmd_selftest(d),
            None => err("usage: dvm selftest /dev/<disk>"),
        },
        other => err(&format!("unknown command: {other} (try `dvm help`)")),
    }
}

const HELP: &str = "\
dvm — drivemind: disk & data health

  devices                 list block devices with attach classification
  smart <dev>             full SMART report (USB-NVMe bridge ladder included)
  health                  probe every SMART-capable device, one verdict line each
  selftest <dev>          start a short SMART self-test (read result via `smart`)
  verify create <root> <manifest.jsonl>
  verify check  <root> <manifest.jsonl>
  cleanup <path>          reclaimable-space suggestions (never deletes)
  videos <path> [--batch out.txt]
                          re-encode candidates + av1tuner batch handoff
  gitscan <repo>          delta-hostile files → gitmind migration plan
  clone <src> <dst> [--run] [--unhealthy]
                          show a safety-checked clone plan; --run executes it
  advise <path>           run cleanup+videos+gitscan under <path>, plus device
                          health, and print one prioritized list
";

fn err(msg: &str) -> i32 {
    eprintln!("dvm: {msg}");
    2
}

/// The CLI's event sink: streams to stderr so stdout stays parseable.
fn sink() -> impl FnMut(Event) {
    let mut last_pct = u64::MAX;
    move |e: Event| match e {
        Event::Phase { name, subject } => eprintln!("== {name}: {subject}"),
        Event::Exec(c) => eprintln!("$ {c}"),
        Event::Note(n) => eprintln!("! {n}"),
        Event::Progress { done, total } => {
            if let Some(t) = total {
                // Only print on whole-percent changes; a million per-file
                // lines helps nobody and slows the scan.
                let pct = if t == 0 { 100 } else { done * 100 / t };
                if pct != last_pct {
                    last_pct = pct;
                    eprint!("\r   {pct}% ({done}/{t})");
                    if pct == 100 {
                        eprintln!();
                    }
                    let _ = std::io::stderr().flush();
                }
            }
        }
    }
}

fn sev_tag(s: smart::Severity) -> &'static str {
    match s {
        smart::Severity::Ok => "OK",
        smart::Severity::Advisory => "ADVISORY",
        smart::Severity::Warning => "WARNING",
        smart::Severity::Critical => "CRITICAL",
    }
}

fn cmd_devices() -> i32 {
    match device::enumerate(&HostRunner) {
        Ok(devs) => {
            for d in devs {
                println!(
                    "{:<14} {:>10}  {:<11} {}{}{}",
                    d.path,
                    human_bytes(d.size_bytes),
                    format!("{:?}", d.attach),
                    d.model.as_deref().unwrap_or("-"),
                    if d.mountpoints.is_empty() {
                        String::new()
                    } else {
                        format!("  mounted: {}", d.mountpoints.join(","))
                    },
                    if d.smart_capable() { "" } else { "  (no SMART)" },
                );
            }
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_smart(dev: &str) -> i32 {
    let mut s = sink();
    match smart::probe(&HostRunner, dev, &mut s) {
        Ok(rep) => {
            println!(
                "{}  {}  fw {}  {}",
                rep.model.as_deref().unwrap_or("?"),
                rep.serial.as_deref().unwrap_or("?"),
                rep.firmware.as_deref().unwrap_or("?"),
                rep.device_type_used
                    .as_deref()
                    .map(|t| format!("(via -d {t})"))
                    .unwrap_or_default()
            );
            if let Some(h) = rep.power_on_hours {
                println!("power-on: {h} h   temp: {}°C", rep.temperature_c.unwrap_or(-1));
            }
            for f in &rep.findings {
                println!("[{:<8}] {}", sev_tag(f.severity), f.message);
            }
            if rep.worst() >= smart::Severity::Warning {
                1
            } else {
                0
            }
        }
        Err(e) => err(&e),
    }
}

fn cmd_selftest(dev: &str) -> i32 {
    let mut s = sink();
    match smart::start_short_self_test(&HostRunner, dev, &mut s) {
        Ok(()) => {
            println!("short self-test started on {dev}; check `dvm smart {dev}` in ~2 min");
            0
        }
        Err(e) => err(&e),
    }
}

/// Probe everything probeable; return worst severity as the exit code
/// signal (0 clean, 1 something ≥ Warning) so this drops into cron/scripts.
fn cmd_health() -> i32 {
    let devs = match device::enumerate(&HostRunner) {
        Ok(d) => d,
        Err(e) => return err(&e),
    };
    let mut worst = smart::Severity::Ok;
    let mut s = sink();
    for d in devs.iter().filter(|d| d.smart_capable()) {
        match smart::probe(&HostRunner, &d.path, &mut s) {
            Ok(rep) => {
                let w = rep.worst();
                worst = worst.max(w);
                println!(
                    "{:<14} [{:<8}] {}",
                    d.path,
                    sev_tag(w),
                    rep.findings
                        .iter()
                        .filter(|f| f.severity >= smart::Severity::Advisory)
                        .map(|f| f.message.clone())
                        .collect::<Vec<_>>()
                        .join("; ")
                );
            }
            Err(e) => println!("{:<14} [UNPROBED] {e}", d.path),
        }
    }
    if worst >= smart::Severity::Warning {
        1
    } else {
        0
    }
}

fn cmd_verify(rest: &[String]) -> i32 {
    let usage = "usage: dvm verify create|check <root> <manifest.jsonl>";
    let (mode, root, man) = match (rest.first(), rest.get(1), rest.get(2)) {
        (Some(m), Some(r), Some(f)) => (m.as_str(), Path::new(r), Path::new(f)),
        _ => return err(usage),
    };
    let mut s = sink();
    match mode {
        "create" => match integrity::create(root, man, &mut s) {
            Ok(n) => {
                println!("manifest written: {n} file(s) → {}", man.display());
                0
            }
            Err(e) => err(&e),
        },
        "check" => match integrity::verify(root, man, &mut s) {
            Ok(rep) => {
                println!(
                    "ok {}  changed {}  missing {}  new {}  read-errors {}  ({} hashed)",
                    rep.ok,
                    rep.changed.len(),
                    rep.missing.len(),
                    rep.new.len(),
                    rep.read_errors.len(),
                    human_bytes(rep.bytes_hashed)
                );
                for (p, st) in &rep.changed {
                    if let integrity::FileStatus::Changed { expected, actual } = st {
                        println!("changed  {p}  {} → {}", &expected[..12], &actual[..12]);
                    }
                }
                for p in &rep.missing {
                    println!("missing  {p}");
                }
                for (p, e) in &rep.read_errors {
                    println!("READERR  {p}  {e}");
                }
                if rep.clean() {
                    0
                } else {
                    1
                }
            }
            Err(e) => err(&e),
        },
        _ => err(usage),
    }
}

fn cmd_cleanup(path: &str) -> i32 {
    let mut s = sink();
    let sugg = cleanup::scan(Path::new(path), &cleanup::CleanupConfig::default(), &mut s);
    if sugg.is_empty() {
        println!("nothing to suggest under {path}");
        return 0;
    }
    let total: u64 = sugg.iter().map(|x| x.reclaim_bytes).sum();
    for x in &sugg {
        println!("{:>10}  {}", human_bytes(x.reclaim_bytes), x.note);
        for p in &x.paths {
            println!("            {p}");
        }
        if let Some(c) = &x.command {
            println!("            → {c}");
        }
    }
    println!("total surfaced: {}", human_bytes(total));
    0
}

fn cmd_videos(rest: &[String]) -> i32 {
    let root = match rest.first() {
        Some(p) => PathBuf::from(p),
        None => return err("usage: dvm videos <path> [--batch out.txt]"),
    };
    let batch = flag_value(rest, "--batch").map(PathBuf::from);
    let mut s = sink();
    let scan = video::scan(&HostRunner, &root, &mut s);
    for c in &scan.candidates {
        println!(
            "{:>10}  {:<28} {}",
            human_bytes(c.size),
            c.why,
            c.path
        );
    }
    println!(
        "{} candidate(s), {} — est. {} recoverable via av1tuner (skipped {} already-modern)",
        scan.candidates.len(),
        human_bytes(scan.total_candidate_bytes),
        human_bytes(scan.est_savings_bytes()),
        scan.skipped_modern
    );
    if let Some(b) = batch {
        match scan.write_batch(&b) {
            Ok(msg) => println!("{msg}"),
            Err(e) => return err(&e),
        }
    }
    0
}

fn cmd_gitscan(repo: &str) -> i32 {
    let mut s = sink();
    match deltahostile::scan_repo(
        &HostRunner,
        Path::new(repo),
        &deltahostile::DeltaHostileConfig::default(),
        &mut s,
    ) {
        Ok(plan) => {
            for c in &plan.candidates {
                let why = match &c.reason {
                    deltahostile::Reason::CompressedFormat(e) => format!(".{e} (compressed format)"),
                    deltahostile::Reason::HighEntropy(b) => format!("entropy {b:.2} bits/byte"),
                };
                println!("{:>10}  {:<26} {}", human_bytes(c.size), why, c.path);
            }
            if plan.candidates.is_empty() {
                println!("no delta-hostile tracked files ≥ 1 MiB — git is fine for this repo");
            } else {
                println!(
                    "{} file(s), {} of {} tracked bytes — migrate with:\n  {}",
                    plan.candidates.len(),
                    human_bytes(plan.total_bytes),
                    human_bytes(plan.tracked_bytes),
                    plan.gin_command()
                );
            }
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_clone(rest: &[String]) -> i32 {
    let (src, dst) = match (rest.first(), rest.get(1)) {
        (Some(s), Some(d)) => (s.as_str(), d.as_str()),
        _ => return err("usage: dvm clone <src> <dst> [--run] [--unhealthy]"),
    };
    let run = rest.iter().any(|a| a == "--run");
    let unhealthy = rest.iter().any(|a| a == "--unhealthy");
    let devs = match device::enumerate(&HostRunner) {
        Ok(d) => d,
        Err(e) => return err(&e),
    };
    let plan = cloneop::plan(&devs, src, dst, unhealthy);
    println!("plan: {}", plan.command);
    for w in &plan.warnings {
        println!("warn: {w}");
    }
    for b in &plan.blockers {
        println!("BLOCKED: {b}");
    }
    if !plan.ok() {
        return 1;
    }
    if !run {
        println!("dry run — re-run with --run to execute the plan above");
        return 0;
    }
    let mut s = sink();
    match cloneop::execute(&HostRunner, &plan, true, &mut s) {
        Ok(()) => {
            println!("clone complete: {} → {}", plan.src, plan.dst);
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_advise(rest: &[String]) -> i32 {
    let root = match rest.first() {
        Some(p) => PathBuf::from(p),
        None => return err("usage: dvm advise <path>"),
    };
    let mut s = sink();

    let devices = device::enumerate(&HostRunner).unwrap_or_default();
    let mut smart_reports = Vec::new();
    for d in devices.iter().filter(|d| d.smart_capable()) {
        if let Ok(rep) = smart::probe(&HostRunner, &d.path, &mut s) {
            smart_reports.push(rep);
        }
    }
    let cleanup = cleanup::scan(&root, &cleanup::CleanupConfig::default(), &mut s);
    let videos = video::scan(&HostRunner, &root, &mut s);
    let batch_path = std::env::temp_dir().join("drivemind-av1tuner-batch.txt");
    if !videos.candidates.is_empty() {
        let _ = videos.write_batch(&batch_path);
    }
    // Only scan as a repo if it looks like one — the advisor shouldn't
    // print a git error for a plain media folder.
    let migration = if root.join(".git").exists() {
        deltahostile::scan_repo(
            &HostRunner,
            &root,
            &deltahostile::DeltaHostileConfig::default(),
            &mut s,
        )
        .ok()
    } else {
        None
    };

    let batch_str = batch_path.display().to_string();
    let recs = advisor::advise(&advisor::Inputs {
        devices: &devices,
        smart: &smart_reports,
        verify: None,
        cleanup: &cleanup,
        migration: migration.as_ref(),
        videos: Some(&videos),
        video_batch_path: Some(&batch_str),
    });

    if recs.is_empty() {
        println!("nothing to recommend — devices healthy, nothing reclaimable found");
        return 0;
    }
    for r in &recs {
        println!("[{:?}] {}", r.priority, r.title);
        println!("    {}", r.detail);
        if let Some(c) = &r.command {
            println!("    → {c}");
        }
    }
    0
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}
