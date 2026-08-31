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
        "catalog" => cmd_catalog(&rest),
        "dupes" => cmd_dupes(&rest),
        "desync" => cmd_desync(&rest),
        "suggest" => cmd_suggest(&rest),
        "apply" => cmd_apply(&rest),
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

// ── catalogs / cross-mount memory ───────────────────────────────────

use drivemind_core::{assist, catalog, syncdiff};

fn flag_val(rest: &[String], name: &str) -> Option<String> {
    rest.iter().position(|a| a == name).and_then(|i| rest.get(i + 1).cloned())
}

fn store_dir(rest: &[String]) -> PathBuf {
    flag_val(rest, "--store").map(PathBuf::from).unwrap_or_else(catalog::default_store)
}

/// Best-effort filesystem identity for a mountpoint: findmnt for
/// UUID/LABEL, device enumeration for model/serial; explicit flags win.
fn identity_for_mount(mount: &str, rest: &[String]) -> catalog::DeviceIdentity {
    let mut id = catalog::DeviceIdentity {
        fs_uuid: flag_val(rest, "--uuid"),
        label: flag_val(rest, "--label"),
        model: None,
        serial: None,
    };
    if id.fs_uuid.is_none() || id.label.is_none() {
        if let Ok(out) = std::process::Command::new("findmnt")
            .args(["-rno", "UUID,LABEL", "--target", mount])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut it = text.split_whitespace();
            let u = it.next().unwrap_or("").to_string();
            let l = it.next().unwrap_or("").to_string();
            if id.fs_uuid.is_none() && !u.is_empty() {
                id.fs_uuid = Some(u);
            }
            if id.label.is_none() && !l.is_empty() {
                id.label = Some(l);
            }
        }
    }
    if let Ok(devs) = device::enumerate(&HostRunner) {
        for d in devs {
            if d.mountpoints.iter().any(|m| mount.starts_with(m.as_str()) && !m.is_empty()) {
                if id.model.is_none() {
                    id.model = d.model.clone();
                }
                if id.serial.is_none() {
                    id.serial = d.serial.clone();
                }
            }
        }
    }
    if id.fs_uuid.is_none() && id.label.is_none() {
        // Last resort: the mount dir name, so the catalog is still usable;
        // the operator can rename via --label next scan.
        let name = Path::new(mount)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into());
        eprintln!("note: no UUID/LABEL found for {mount}; keying catalog as '{name}' (pass --uuid/--label to pin identity)");
        id.label = Some(name);
    }
    id
}

fn cmd_catalog(rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("scan") => {
            let Some(mount) = rest.get(1) else {
                return err("usage: dvm catalog scan <mount> [--hash] [--uuid U] [--label L] [--photo IMG] [--store DIR]");
            };
            let hash = rest.iter().any(|a| a == "--hash");
            let root = Path::new(mount);
            if !root.is_dir() {
                return err(&format!("{mount} is not a directory"));
            }
            let identity = identity_for_mount(mount, rest);
            eprintln!("scanning {mount} as {} (hash: {hash}) …", identity.display());
            let entries = match catalog::scan(root, hash) {
                Ok(e) => e,
                Err(e) => return err(&e),
            };
            let meta = catalog::CatalogMeta {
                entry_count: entries.len() as u64,
                total_bytes: entries.values().map(|e| e.size).sum(),
                identity,
                taken_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                mount_root: mount.clone(),
                photo_file: None,
            };
            let key = meta.identity.key();
            let cat = catalog::Catalog { meta, entries };
            match catalog::save(&store_dir(rest), &cat) {
                Ok(dir) => {
                    println!(
                        "cataloged {} files / {} B as '{}' -> {}",
                        cat.meta.entry_count,
                        cat.meta.total_bytes,
                        key,
                        dir.display()
                    );
                    if let Some(img) = flag_val(rest, "--photo") {
                        match catalog::set_photo(&store_dir(rest), &key, Path::new(&img)) {
                            Ok(p) => println!("profile photo saved: {}", p.display()),
                            Err(e) => eprintln!("photo: {e}"),
                        }
                    }
                    0
                }
                Err(e) => err(&e),
            }
        }
        Some("list") => {
            let metas = catalog::list(&store_dir(rest));
            if metas.is_empty() {
                println!("no catalogs yet — run `dvm catalog scan <mount>` with a drive plugged in");
                return 0;
            }
            for m in metas {
                let photo = if m.photo_file.is_some() { " [photo]" } else { "" };
                println!(
                    "{:<28} {:<30} {:>8} files {:>14} B  scanned@{}{}",
                    m.identity.key(),
                    m.identity.display(),
                    m.entry_count,
                    m.total_bytes,
                    m.taken_at,
                    photo
                );
            }
            0
        }
        Some("photo") => {
            let (Some(key), Some(img)) = (rest.get(1), rest.get(2)) else {
                return err("usage: dvm catalog photo <device-key> <image> [--store DIR]");
            };
            match catalog::set_photo(&store_dir(rest), key, Path::new(img)) {
                Ok(p) => {
                    println!("photo for {key}: {}", p.display());
                    0
                }
                Err(e) => err(&e),
            }
        }
        _ => err("usage: dvm catalog <scan|list|photo> …"),
    }
}

fn load_all(store: &Path) -> Vec<catalog::Catalog> {
    catalog::list(store)
        .iter()
        .filter_map(|m| catalog::load(store, &m.identity.key()).ok())
        .collect()
}

fn cmd_dupes(rest: &[String]) -> i32 {
    let store = store_dir(rest);
    let cats = load_all(&store);
    if cats.len() < 2 {
        return err("need at least two catalogs (scan two drives first)");
    }
    let refs: Vec<&catalog::Catalog> = cats.iter().collect();
    let dups = syncdiff::duplicates(&refs);
    if dups.is_empty() {
        println!("no cross-drive duplicates found");
        return 0;
    }
    println!("{} duplicate groups across {} drives (largest first):", dups.len(), cats.len());
    for d in dups.iter().take(30) {
        let grade = match d.grade {
            syncdiff::MatchGrade::Confirmed => "confirmed",
            syncdiff::MatchGrade::Probable => "probable ",
        };
        println!("  [{}] {:>12} B  {}", grade, d.size, d.file_name);
        for (dev, path) in &d.locations {
            println!("      {dev}: {path}");
        }
    }
    0
}

fn cmd_desync(rest: &[String]) -> i32 {
    let (Some(ka), Some(kb)) = (rest.first(), rest.get(1)) else {
        return err("usage: dvm desync <device-key-a> <device-key-b> [--store DIR]");
    };
    let store = store_dir(rest);
    let (a, b) = match (catalog::load(&store, ka), catalog::load(&store, kb)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return err(&e),
    };
    let reps = syncdiff::desync(&a, &b);
    if reps.is_empty() {
        println!("no shared top-level directories between {ka} and {kb}");
        return 0;
    }
    for r in &reps {
        if r.in_sync() {
            println!("'{}': in sync", r.dir);
            continue;
        }
        println!(
            "'{}': {} only on {} ({} B) | {} only on {} ({} B) | {} differing",
            r.dir,
            r.only_a.len(),
            r.dev_a,
            r.only_a_bytes,
            r.only_b.len(),
            r.dev_b,
            r.only_b_bytes,
            r.differing.len()
        );
        for (rel, sz) in r.only_a.iter().take(5) {
            println!("    only {}: {rel} ({sz} B)", r.dev_a);
        }
        for (rel, sz) in r.only_b.iter().take(5) {
            println!("    only {}: {rel} ({sz} B)", r.dev_b);
        }
    }
    0
}

fn cmd_suggest(rest: &[String]) -> i32 {
    let (Some(ka), Some(kb)) = (rest.first(), rest.get(1)) else {
        return err("usage: dvm suggest <key-a> <key-b> [--logscope host:port key | --ollama host:port model] [--store DIR]");
    };
    let store = store_dir(rest);
    let (a, b) = match (catalog::load(&store, ka), catalog::load(&store, kb)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return err(&e),
    };
    let reps = syncdiff::desync(&a, &b);
    let refs: Vec<&catalog::Catalog> = [&a, &b].to_vec();
    let dups = syncdiff::duplicates(&refs);
    let suggestions = if let Some(pos) = rest.iter().position(|x| x == "--logscope") {
        // The in-tree wgpu engine (gpu-gemm netserve) — the fast path on
        // the operator's own hardware. Usage: --logscope host:port key
        let (Some(host), Some(key)) = (rest.get(pos + 1), rest.get(pos + 2)) else {
            return err("--logscope needs <host:port> <api-key>");
        };
        let mut lb = assist::LogscopeBackend::new(host, key);
        match assist::SuggestionBackend::suggest(&mut lb, &reps, &dups) {
            Ok(s) => {
                for r in &lb.rejected {
                    eprintln!("REJECTED (model proposed something outside the tool set): {r}");
                }
                s
            }
            Err(e) => return err(&format!("logscope-serve: {e} (rule backend works offline: drop --logscope)")),
        }
    } else if let Some(pos) = rest.iter().position(|x| x == "--ollama") {
        let (Some(host), Some(model)) = (rest.get(pos + 1), rest.get(pos + 2)) else {
            return err("--ollama needs <host:port> <model>");
        };
        let mut ob = assist::OllamaBackend::new(host, model);
        match assist::SuggestionBackend::suggest(&mut ob, &reps, &dups) {
            Ok(s) => {
                for r in &ob.rejected {
                    eprintln!("REJECTED (model proposed something outside the tool set): {r}");
                }
                s
            }
            Err(e) => return err(&format!("ollama: {e} (rule backend works offline: drop --ollama)")),
        }
    } else {
        let mut rb = assist::RuleBackend::default();
        assist::SuggestionBackend::suggest(&mut rb, &reps, &dups).unwrap_or_default()
    };
    if suggestions.is_empty() {
        println!("nothing to suggest — {ka} and {kb} look consistent");
        return 0;
    }
    for sg in &suggestions {
        println!("[{}] ({}) {} — {} B", sg.id, sg.kind, sg.human, sg.bytes);
        for act in &sg.actions {
            println!("    {}", serde_json::to_string(act).unwrap_or_default());
        }
    }
    let plan = store.join("plan.json");
    if let Ok(f) = std::fs::File::create(&plan) {
        let _ = serde_json::to_writer_pretty(f, &suggestions);
        println!("\nplan saved -> {} ; apply one with:\n  dvm apply <id> --from-mount <path-of-source-drive> --to-mount <path-of-dest-drive> --yes", plan.display());
    }
    0
}

fn cmd_apply(rest: &[String]) -> i32 {
    let Some(id) = rest.first() else {
        return err("usage: dvm apply <suggestion-id> --from-mount P --to-mount Q [--yes] [--store DIR]");
    };
    let store = store_dir(rest);
    let plan: Vec<assist::Suggestion> = match std::fs::File::open(store.join("plan.json")) {
        Ok(f) => match serde_json::from_reader(f) {
            Ok(p) => p,
            Err(e) => return err(&format!("plan.json: {e}")),
        },
        Err(_) => return err("no plan.json — run `dvm suggest` first"),
    };
    let Some(sg) = plan.iter().find(|s| &s.id == id) else {
        return err(&format!("no suggestion '{id}' in the current plan"));
    };
    let (from_m, to_m) = (flag_val(rest, "--from-mount"), flag_val(rest, "--to-mount"));
    let yes = rest.iter().any(|a| a == "--yes");
    println!("suggestion [{}]: {}", sg.id, sg.human);
    for act in &sg.actions {
        match act {
            assist::ToolAction::CopyFile { rel, .. } | assist::ToolAction::CopyTree { rel, .. } => {
                let (Some(fm), Some(tm)) = (&from_m, &to_m) else {
                    return err("copy actions need --from-mount and --to-mount (operator states where each drive is mounted right now)");
                };
                let src = Path::new(fm).join(rel);
                let dst = Path::new(tm).join(rel);
                if !yes {
                    println!("DRY RUN: would copy {} -> {} (add --yes to execute)", src.display(), dst.display());
                    continue;
                }
                let n = match copy_path(&src, &dst) {
                    Ok(n) => n,
                    Err(e) => return err(&e),
                };
                println!("copied {n} file(s): {} -> {}", src.display(), dst.display());
            }
            assist::ToolAction::VerifyPair { dev_a, dev_b, dir } => {
                println!("verify: re-scan both drives with hashing, then re-run desync:");
                println!("  dvm catalog scan <mount-of-{dev_a}> --hash");
                println!("  dvm catalog scan <mount-of-{dev_b}> --hash");
                println!("  dvm desync {dev_a} {dev_b}   # dir '{dir}' should show Confirmed matches");
            }
            assist::ToolAction::Note { text } => println!("note: {text}"),
        }
    }
    0
}

/// Copy a file or a directory tree; returns files copied. Refuses to
/// overwrite differing destinations unless the sizes already match
/// (idempotent re-runs stay safe; conflicting content is a decision for
/// the operator, not this tool).
fn copy_path(src: &Path, dst: &Path) -> Result<u64, String> {
    if src.is_file() {
        if let Ok(md) = std::fs::metadata(dst) {
            let sm = std::fs::metadata(src).map_err(|e| e.to_string())?;
            if md.len() != sm.len() {
                return Err(format!(
                    "{} exists with different size ({} vs {}) — resolve manually",
                    dst.display(),
                    md.len(),
                    sm.len()
                ));
            }
            return Ok(0);
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(src, dst).map_err(|e| format!("copy {}: {e}", src.display()))?;
        return Ok(1);
    }
    if src.is_dir() {
        let mut n = 0u64;
        for ent in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
            n += copy_path(&ent.path(), &dst.join(ent.file_name()))?;
        }
        return Ok(n);
    }
    Err(format!("{} does not exist", src.display()))
}
