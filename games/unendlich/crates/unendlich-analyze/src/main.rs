//! `unendlich-analyze` — CLI for querying structured event logs.
//!
//! Reads `events.jsonl` and produces summaries / filters / comparisons.
//! Counterpart to the human-readable terminal log + run_summaries.txt;
//! intended for machine analysis and bisects.
//!
//! ## Usage
//!
//! ```text
//! unendlich-analyze list
//!     List all runs in events.jsonl (uuid, version, duration).
//!
//! unendlich-analyze summarize r-<uuid>
//!     Print a digest of one run's events.
//!
//! unendlich-analyze where <jq-like-path>=<value>
//!     Filter runs whose `start` or `flag` events match the constraint.
//!     Example: `unendlich-analyze where flag.UNENDLICH_RENDER_DISTANCE=8`
//! ```
//!
//! ## c.8 scope
//!
//! Minimal: `list` + `summarize`. The `where` / `compare` / richer query
//! commands are deferred to the c.10/c.11 launcher work, since the
//! launcher will need most of that machinery anyway and we'd rather
//! build it once.
//!
//! ## File path
//!
//! Defaults to `events.jsonl` in the current working directory. Override
//! via `UNENDLICH_EVENTS_PATH` (matches the writer side).

use std::collections::BTreeMap;
use std::io::BufRead;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_help();
        std::process::exit(if args.is_empty() { 1 } else { 0 });
    }

    // optimize doesn't read events.jsonl — it takes a result file
    // path argument directly. All other commands need events.jsonl.
    if args[0] != "optimize" {
        let path = std::env::var("UNENDLICH_EVENTS_PATH")
            .unwrap_or_else(|_| "events.jsonl".to_string());

        let events = match load_events(&path) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("Could not read {}: {}", path, e);
                std::process::exit(2);
            }
        };

        match args[0].as_str() {
            "list" => cmd_list(&events),
            "summarize" => {
                if args.len() < 2 {
                    eprintln!("summarize requires a UUID argument");
                    std::process::exit(1);
                }
                cmd_summarize(&events, &args[1]);
            }
            "wait_spikes" => {
                let uuid = args.get(1).map(|s| s.as_str());
                cmd_wait_spikes(&events, uuid);
            }
            "gpu_spikes" => {
                let uuid = args.get(1).map(|s| s.as_str());
                cmd_gpu_spikes(&events, uuid);
            }
            "upload_pressure" => {
                let uuid = args.get(1).map(|s| s.as_str());
                cmd_upload_pressure(&events, uuid);
            }
            "cull_count" => {
                let uuid = args.get(1).map(|s| s.as_str());
                cmd_cull_count(&events, uuid);
            }
            "shadow_stride" => {
                let uuid = args.get(1).map(|s| s.as_str());
                cmd_shadow_stride(&events, uuid);
            }
            cmd => {
                eprintln!("Unknown command: {}", cmd);
                print_help();
                std::process::exit(1);
            }
        }
    } else {
        if args.len() < 2 {
            eprintln!("optimize requires a path to a .unendresult.json file");
            std::process::exit(1);
        }
        cmd_optimize(&args[1]);
    }
}

fn print_help() {
    println!("unendlich-analyze {} — query structured event logs", env!("CARGO_PKG_VERSION"));
    println!();
    println!("USAGE:");
    println!("    unendlich-analyze list");
    println!("    unendlich-analyze summarize <uuid>");
    println!("    unendlich-analyze wait_spikes [<uuid>]");
    println!("    unendlich-analyze gpu_spikes [<uuid>]");
    println!("    unendlich-analyze upload_pressure [<uuid>]");
    println!("    unendlich-analyze cull_count [<uuid>]");
    println!("    unendlich-analyze shadow_stride [<uuid>]");
    println!("    unendlich-analyze optimize <path-to-.unendresult.json>");
    println!();
    println!("DESCRIPTIONS:");
    println!("    list             List runs in events.jsonl (uuid, version, duration).");
    println!("    summarize        Print a digest of one run's events.");
    println!("    wait_spikes      Summarize `wait_fences_spike` diagnostic events");
    println!("                     (chug investigation: which frames blocked the render");
    println!("                     thread, and whether the GPU or CPU was the bottleneck).");
    println!("                     If no UUID given, summarizes events from all runs.");
    println!("    gpu_spikes       Summarize `gpu_spike` diagnostic events");
    println!("                     (which GPU pass was the culprit for each chug frame).");
    println!("                     Pairs with wait_spikes for full diagnosis.");
    println!("    upload_pressure  Summarize `upload_pressure` diagnostic events");
    println!("                     (which frames pushed lots of mesh bytes to GPU).");
    println!("                     Tests the upload-contention chug hypothesis.");
    println!("    cull_count       Summarize `cull_count` diagnostic events");
    println!("                     (CPU-side count of how many chunks pass shadow");
    println!("                     vs main cull frustums each frame). Tests whether");
    println!("                     shadow is drawing significantly more chunks than");
    println!("                     main, which would explain shadow's dominant cost.");
    println!("    shadow_stride    Summarize `shadow_stride` diagnostic events");
    println!("                     (which frames used temporal stride vs forced full");
    println!("                     render, and why). Confirms the stride mechanism");
    println!("                     is engaging when configured.");
    println!("    optimize         Read a .unendresult.json and rank perf zones");
    println!("                     by mean contribution + tail-to-median spread.");
    println!("                     Surfaces 'where should I focus optimization' at a glance.");
    println!();
    println!("ENV:");
    println!("    UNENDLICH_EVENTS_PATH    Path to events.jsonl (default: ./events.jsonl)");
}

/// optimize: rank perf zones in a result file by:
///   1. Mean contribution to total frame time (where time is spent).
///   2. p99/p50 ratio (tail-latency volatility).
///
/// The box plots in the binary's log already show this visually for each
/// zone; this command makes the ranking explicit and cross-zone, so
/// "where should I focus" becomes a one-line read.
///
/// Input: path to a `.unendresult.json` file (produced by spec runs).
fn cmd_optimize(path: &str) {
    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Could not read {}: {}", path, e);
            std::process::exit(2);
        }
    };
    let result: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Could not parse JSON in {}: {}", path, e);
            std::process::exit(2);
        }
    };

    let stats = match result.get("stats").and_then(|s| s.as_object()) {
        Some(s) => s,
        None => {
            eprintln!("Result file has no 'stats' object: {}", path);
            std::process::exit(2);
        }
    };

    // c.13-instrumentation: surface workload context FIRST so the
    // reader has scale-of-the-problem context before reading the
    // ranked tables. Without this, a 91ms shadow at rd=8 looks the
    // same as 91ms shadow at rd=2 — but they mean very different
    // things.
    if let Some(workload) = stats.get("workload").and_then(|w| w.as_object()) {
        let mut printed_header = false;
        let maybe_print_header = |this_printed_header: &mut bool| {
            if !*this_printed_header {
                println!("Workload context");
                println!("────────────────");
                *this_printed_header = true;
            }
        };

        // loaded / rendering chunk distributions.
        for (label, field) in &[("loaded chunks", "loaded"), ("rendering chunks", "rendering")] {
            if let Some(dist) = workload.get(*field).and_then(|v| v.as_object()) {
                let n = dist.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
                if n == 0 { continue; }
                let min = dist.get("min").and_then(|v| v.as_u64()).unwrap_or(0);
                let p50 = dist.get("p50").and_then(|v| v.as_u64()).unwrap_or(0);
                let max = dist.get("max").and_then(|v| v.as_u64()).unwrap_or(0);
                maybe_print_header(&mut printed_header);
                println!("  {:<18} min={} p50={} max={} (n={} samples)",
                    label, min, p50, max, n);
            }
        }

        // Snapshot (start + end + delta).
        if let Some(snap) = workload.get("snapshot").and_then(|s| s.as_object()) {
            maybe_print_header(&mut printed_header);
            if let Some(start) = snap.get("start").and_then(|s| s.as_object()) {
                let lc = start.get("loaded_chunks").and_then(|v| v.as_u64()).unwrap_or(0);
                let nb = start.get("non_air_blocks").and_then(|v| v.as_u64()).unwrap_or(0);
                println!("  start state         loaded_chunks={} non_air_blocks={}", lc, nb);
            } else if snap.contains_key("start") {
                // start is present but null — boot didn't finish.
                println!("  start state         (run too short for boot/warmup snapshot)");
            }
            if let Some(end) = snap.get("end").and_then(|s| s.as_object()) {
                let lc = end.get("loaded_chunks").and_then(|v| v.as_u64()).unwrap_or(0);
                let nb = end.get("non_air_blocks").and_then(|v| v.as_u64()).unwrap_or(0);
                println!("  end state           loaded_chunks={} non_air_blocks={}", lc, nb);
            }
            if let Some(delta) = snap.get("delta").and_then(|d| d.as_object()) {
                let dc = delta.get("loaded_chunks").and_then(|v| v.as_i64()).unwrap_or(0);
                let db = delta.get("non_air_blocks").and_then(|v| v.as_i64()).unwrap_or(0);
                let dc_sign = if dc >= 0 { "+" } else { "" };
                let db_sign = if db >= 0 { "+" } else { "" };
                println!("  delta               loaded_chunks={}{} non_air_blocks={}{}",
                    dc_sign, dc, db_sign, db);
            }
        }
        if printed_header {
            println!();
        }
    }

    // Gather zones from both gpu and cpu maps. Each entry has
    // mean_ms, p50_ms, p90_ms, p99_ms, min_ms, max_ms.
    #[derive(Debug, Clone)]
    struct ZoneStat {
        category: &'static str,
        name: String,
        mean_ms: f64,
        p50_ms: f64,
        p99_ms: f64,
        max_ms: f64,
        n: u64,
    }

    let mut zones: Vec<ZoneStat> = Vec::new();

    for (category, key) in &[("gpu", "gpu"), ("cpu", "cpu")] {
        let map = match stats.get(*key).and_then(|m| m.as_object()) {
            Some(m) => m,
            None => continue,
        };
        for (zname, zval) in map {
            let zone_obj = match zval.as_object() {
                Some(o) => o,
                None => continue,
            };
            let f = |k: &str| -> f64 {
                zone_obj.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0)
            };
            let n = zone_obj.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
            if n == 0 { continue; }
            zones.push(ZoneStat {
                category,
                name: zname.clone(),
                mean_ms: f("mean_ms"),
                p50_ms: f("p50_ms"),
                p99_ms: f("p99_ms"),
                max_ms: f("max_ms"),
                n,
            });
        }
    }

    if zones.is_empty() {
        println!("No zones found in {}. Nothing to optimize.", path);
        return;
    }

    // Find total-by-category for percentage contribution. We use mean_ms
    // because that captures "long-run cost" rather than a single value's
    // contribution. p50 would underweight rare-but-expensive zones.
    let total_gpu_mean: f64 = zones.iter()
        .filter(|z| z.category == "gpu" && z.name != "total")
        .map(|z| z.mean_ms).sum();
    let total_cpu_mean: f64 = zones.iter()
        .filter(|z| z.category == "cpu")
        .map(|z| z.mean_ms).sum();

    // Ranking 1: by mean contribution.
    let mut by_mean = zones.clone();
    by_mean.sort_by(|a, b| b.mean_ms.partial_cmp(&a.mean_ms).unwrap_or(std::cmp::Ordering::Equal));

    println!("Where time is spent (ranked by mean contribution)");
    println!("───────────────────────────────────────────────────");
    println!("{:<8} {:<16} {:>7} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "cat", "zone", "n", "mean_ms", "p50_ms", "p99_ms", "max_ms", "% of cat");
    for z in &by_mean {
        let total = if z.category == "gpu" { total_gpu_mean } else { total_cpu_mean };
        // For 'total' GPU pseudo-zone, skip the % (it's the denominator).
        let pct_str = if z.name == "total" {
            "—".to_string()
        } else if total > 0.0 {
            format!("{:.0}%", 100.0 * z.mean_ms / total)
        } else {
            "—".to_string()
        };
        println!("{:<8} {:<16} {:>7} {:>10.3} {:>10.3} {:>10.3} {:>10.3} {:>10}",
            z.category, z.name, z.n, z.mean_ms, z.p50_ms, z.p99_ms, z.max_ms, pct_str);
    }
    println!();

    // Ranking 2: by tail-to-median ratio. Indicates volatility.
    let mut by_tail: Vec<ZoneStat> = zones.iter()
        .filter(|z| z.p50_ms > 0.001)  // ignore zones with p50 ≈ 0 (avoid div-by-near-zero)
        .cloned().collect();
    by_tail.sort_by(|a, b| {
        let ra = a.p99_ms / a.p50_ms;
        let rb = b.p99_ms / b.p50_ms;
        rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
    });

    println!("Tail-latency volatility (ranked by p99/p50)");
    println!("───────────────────────────────────────────");
    println!("{:<8} {:<16} {:>10} {:>10} {:>10}",
        "cat", "zone", "p50_ms", "p99_ms", "p99/p50");
    let mut printed = 0;
    for z in &by_tail {
        let ratio = z.p99_ms / z.p50_ms;
        // Surface only meaningful spreads. <2× is benign noise.
        if ratio < 2.0 { continue; }
        println!("{:<8} {:<16} {:>10.3} {:>10.3} {:>9.1}×",
            z.category, z.name, z.p50_ms, z.p99_ms, ratio);
        printed += 1;
        if printed >= 10 { break; } // cap output
    }
    if printed == 0 {
        println!("(no zones with p99/p50 ≥ 2× — workload is steady)");
    }
    println!();

    // Brief interpretation guide.
    println!("Reading this:");
    println!("  • Top of the first table = where you'd spend optimization time.");
    println!("  • Top of the second table = zones with hidden spikes; check");
    println!("    if a single bad frame is hurting your tail.");
}

/// One event line, parsed.
type Event = serde_json::Map<String, serde_json::Value>;

/// Read all events from the jsonl file. Malformed lines log a warning
/// to stderr and are skipped — better than failing the whole read.
fn load_events(path: &str) -> std::io::Result<Vec<Event>> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut out = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() { continue; }
        match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(serde_json::Value::Object(obj)) => out.push(obj),
            Ok(_) => {
                eprintln!("Warning: {}:{}: not a JSON object, skipped", path, lineno + 1);
            }
            Err(e) => {
                eprintln!("Warning: {}:{}: parse error: {} — skipped", path, lineno + 1, e);
            }
        }
    }
    Ok(out)
}

/// Get a string field from an event, or empty string if missing/wrong type.
fn s(e: &Event, key: &str) -> String {
    match e.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

/// `list` — group events by uuid, print one row per run with version + duration.
fn cmd_list(events: &[Event]) {
    // Group events by uuid, preserving first-seen order.
    let mut by_uuid: BTreeMap<String, RunSummary> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for e in events {
        let uuid = s(e, "uuid");
        if uuid.is_empty() { continue; }
        if !by_uuid.contains_key(&uuid) {
            order.push(uuid.clone());
        }
        let entry = by_uuid.entry(uuid).or_default();
        let kind = s(e, "kind");
        match kind.as_str() {
            "start" => {
                entry.version = s(e, "version");
                entry.binary = s(e, "binary");
            }
            "shutdown" => {
                entry.shutdown_reason = s(e, "reason");
                if let Some(serde_json::Value::Number(n)) = e.get("duration_secs") {
                    entry.duration_secs = n.as_f64().unwrap_or(0.0);
                }
            }
            "frame_window" => {
                entry.frame_windows += 1;
            }
            _ => {}
        }
    }

    if order.is_empty() {
        println!("(no runs found)");
        return;
    }

    println!("{:<20} {:<14} {:>8}  {:>4}  {}",
        "UUID", "VERSION", "SECS", "WIN", "SHUTDOWN");
    for uuid in &order {
        let r = &by_uuid[uuid];
        println!("{:<20} {:<14} {:>8.1}  {:>4}  {}",
            uuid,
            if r.version.is_empty() { "(unknown)" } else { &r.version },
            r.duration_secs,
            r.frame_windows,
            if r.shutdown_reason.is_empty() { "(no shutdown event — crashed?)" } else { &r.shutdown_reason },
        );
    }
    println!();
    println!("{} run(s).", order.len());
}

#[derive(Default)]
struct RunSummary {
    version: String,
    binary: String,
    duration_secs: f64,
    shutdown_reason: String,
    frame_windows: usize,
}

/// `summarize` — print everything about one run.
fn cmd_summarize(events: &[Event], uuid: &str) {
    let matching: Vec<&Event> = events.iter()
        .filter(|e| s(e, "uuid") == uuid)
        .collect();
    if matching.is_empty() {
        eprintln!("No events found for {}", uuid);
        std::process::exit(3);
    }

    // Header from `start`.
    if let Some(start) = matching.iter().find(|e| s(e, "kind") == "start") {
        println!("Run {}", uuid);
        println!("  version:  {}", s(start, "version"));
        println!("  binary:   {}", s(start, "binary"));
        println!("  cmdline:  {}", s(start, "cmdline"));
    } else {
        println!("Run {} (no start event)", uuid);
    }

    // Flags.
    let flag_events: Vec<&&Event> = matching.iter()
        .filter(|e| s(e, "kind") == "flag")
        .collect();
    if !flag_events.is_empty() {
        println!();
        println!("  Flags ({}):", flag_events.len());
        for f in &flag_events {
            let source_marker = if s(f, "source") == "default" { " (default)" } else { "" };
            println!("    {} = {}{}", s(f, "name"), s(f, "value"), source_marker);
        }
    }

    // Frame windows: aggregate stats from per-window samples.
    let windows: Vec<&&Event> = matching.iter()
        .filter(|e| s(e, "kind") == "frame_window")
        .collect();
    if !windows.is_empty() {
        println!();
        println!("  Frame windows: {}", windows.len());
        // Compute min/p50/max for a couple of key fields.
        let mut frame_p50s: Vec<f64> = Vec::new();
        let mut renderings: Vec<u64> = Vec::new();
        let mut loadeds: Vec<u64> = Vec::new();
        for w in &windows {
            if let Some(serde_json::Value::Number(n)) = w.get("frame_p50_ms") {
                if let Some(v) = n.as_f64() { frame_p50s.push(v); }
            }
            if let Some(serde_json::Value::Number(n)) = w.get("rendering") {
                if let Some(v) = n.as_u64() { renderings.push(v); }
            }
            if let Some(serde_json::Value::Number(n)) = w.get("loaded") {
                if let Some(v) = n.as_u64() { loadeds.push(v); }
            }
        }
        print_distribution_f("    frame_p50_ms ", &frame_p50s);
        print_distribution_u("    rendering    ", &renderings);
        print_distribution_u("    loaded       ", &loadeds);
    }

    // Shutdown.
    if let Some(sd) = matching.iter().find(|e| s(e, "kind") == "shutdown") {
        println!();
        println!("  Shutdown:");
        println!("    reason:        {}", s(sd, "reason"));
        if let Some(serde_json::Value::Number(n)) = sd.get("duration_secs") {
            println!("    duration_secs: {}", n);
        }
    } else {
        println!();
        println!("  (no shutdown event — crashed or still running?)");
    }
}

fn print_distribution_f(label: &str, samples: &[f64]) {
    if samples.is_empty() {
        println!("{}: (no samples)", label);
        return;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let min = sorted[0];
    let p50 = sorted[n / 2];
    let p90 = sorted[(n * 9 / 10).min(n - 1)];
    let max = *sorted.last().unwrap();
    println!("{}: n={}  min={:.1}  p50={:.1}  p90={:.1}  max={:.1}", label, n, min, p50, p90, max);
}

fn print_distribution_u(label: &str, samples: &[u64]) {
    if samples.is_empty() {
        println!("{}: (no samples)", label);
        return;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let min = sorted[0];
    let p50 = sorted[n / 2];
    let p90 = sorted[(n * 9 / 10).min(n - 1)];
    let max = *sorted.last().unwrap();
    println!("{}: n={}  min={}  p50={}  p90={}  max={}", label, n, min, p50, p90, max);
}

/// `wait_spikes` — summarize `wait_fences_spike` events.
///
/// These events are emitted by the engine (v0.24.3+) whenever
/// `vkWaitForFences` blocks the render thread for more than 10ms.
/// Each event carries:
///   - wait_ms: how long the wait was.
///   - last_submit_to_wait_ms: time between submitting frame N-1 and
///     starting the wait on its fence.
///
/// Interpretation:
///   - If last_submit_to_wait_ms is small (near 0), the CPU hit
///     wait_for_fences immediately after submitting. The GPU was the
///     bottleneck — long shadow/main pass, lots of geometry, etc.
///   - If last_submit_to_wait_ms is large (comparable to wait_ms), the
///     CPU spent time doing OTHER work between submit and wait. The
///     wait then had less GPU work left to do. CPU side was the
///     bottleneck for the interval — likely mesh uploads, game tick,
///     descriptor updates.
///   - Both large = both were slow. Look at GPU lifetime stats from
///     the same run to see if a specific zone (shadow/main/froxel)
///     spiked.
///
/// Output: distributions over all wait spikes in the run(s), plus
/// the top 10 worst spikes by wait_ms with their last_submit_to_wait_ms
/// for case-by-case inspection.
fn cmd_wait_spikes(events: &[Event], uuid_filter: Option<&str>) {
    let matching: Vec<&Event> = events.iter()
        .filter(|e| s(e, "kind") == "wait_fences_spike")
        .filter(|e| uuid_filter.map_or(true, |u| s(e, "uuid") == u))
        .collect();

    if matching.is_empty() {
        match uuid_filter {
            Some(u) => println!("No wait_fences_spike events found for run {}", u),
            None    => println!("No wait_fences_spike events found in events.jsonl"),
        }
        println!();
        println!("These are only emitted by engines at v0.24.3 or later.");
        println!("Earlier runs have no chug-investigation data.");
        return;
    }

    let label = match uuid_filter {
        Some(u) => format!("run {}", u),
        None    => format!("all runs in events.jsonl"),
    };
    println!("wait_fences_spike events ({}): n={}", label, matching.len());
    println!();

    // Distribution of wait durations.
    let mut wait_ms: Vec<f64> = Vec::new();
    let mut last_submit_ms: Vec<f64> = Vec::new();
    let mut both_paired: Vec<(f64, f64)> = Vec::new();

    for e in &matching {
        let w = e.get("wait_ms").and_then(|v| v.as_f64());
        let s_ = e.get("last_submit_to_wait_ms").and_then(|v| v.as_f64());
        if let Some(w) = w {
            wait_ms.push(w);
            if let Some(s_) = s_ {
                last_submit_ms.push(s_);
                both_paired.push((w, s_));
            }
        }
    }

    print_distribution_f("  wait_ms                  ", &wait_ms);
    print_distribution_f("  last_submit_to_wait_ms   ", &last_submit_ms);
    println!();

    // Worst 10 by wait_ms — show submit-to-wait alongside for each
    // so the user can read whether GPU or CPU was the limit.
    both_paired.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    println!("  Worst 10 spikes:");
    println!("    {:>10}  {:>14}  {:>20}", "wait_ms", "submit→wait_ms", "interpretation");
    for (w, s_) in both_paired.iter().take(10) {
        let interp = if *s_ < w * 0.2 {
            "GPU-bound (CPU waited)"
        } else if *s_ > w * 0.8 {
            "CPU-bound (work between)"
        } else {
            "mixed"
        };
        println!("    {:>10.1}  {:>14.1}  {:>20}", w, s_, interp);
    }
    println!();

    // Quick summary diagnosis.
    let gpu_bound = both_paired.iter().filter(|(w, s_)| *s_ < w * 0.2).count();
    let cpu_bound = both_paired.iter().filter(|(w, s_)| *s_ > w * 0.8).count();
    let mixed     = both_paired.len().saturating_sub(gpu_bound + cpu_bound);

    println!("  Diagnosis:");
    println!("    GPU-bound spikes (CPU waited immediately): {}", gpu_bound);
    println!("    CPU-bound spikes (CPU did work between):   {}", cpu_bound);
    println!("    Mixed:                                      {}", mixed);
    println!();
    println!("  See `docs/v0.24.3-chug-investigation-plan.md` for what this means.");
}

/// `gpu_spikes` — summarize `gpu_spike` events.
///
/// These events are emitted by the engine (v0.24.3-chug-debug-2+)
/// whenever a frame's `total` GPU zone exceeds 100ms. Each event
/// carries:
///   - frame_idx: which frame slot.
///   - total_us: total GPU time for that frame.
///   - shadow_us / main_us / froxel_us / cull_us: per-zone breakdown.
///
/// Interpretation: for each chug, this tells us which render pass
/// took the time. Combined with `wait_spikes` (which says CPU or GPU
/// was the bottleneck), `gpu_spikes` says WHICH GPU pass.
///
/// Output: distribution of total spike times + which zone dominated
/// each spike, ranked by total_us descending.
fn cmd_gpu_spikes(events: &[Event], uuid_filter: Option<&str>) {
    let matching: Vec<&Event> = events.iter()
        .filter(|e| s(e, "kind") == "gpu_spike")
        .filter(|e| uuid_filter.map_or(true, |u| s(e, "uuid") == u))
        .collect();

    if matching.is_empty() {
        match uuid_filter {
            Some(u) => println!("No gpu_spike events found for run {}", u),
            None    => println!("No gpu_spike events found in events.jsonl"),
        }
        println!();
        println!("These are only emitted by engines at v0.24.3-chug-debug-2 or later.");
        return;
    }

    let label = match uuid_filter {
        Some(u) => format!("run {}", u),
        None    => format!("all runs in events.jsonl"),
    };
    println!("gpu_spike events ({}): n={}", label, matching.len());
    println!();

    // Distributions per zone (ms, not us).
    let zone_us = |e: &Event, key: &str| -> Option<f64> {
        e.get(key).and_then(|v| v.as_u64()).map(|u| u as f64 / 1000.0)
    };
    let mut totals: Vec<f64> = Vec::new();
    let mut shadows: Vec<f64> = Vec::new();
    let mut mains: Vec<f64> = Vec::new();
    let mut froxels: Vec<f64> = Vec::new();
    let mut culls: Vec<f64> = Vec::new();
    let mut per_event: Vec<(f64, f64, f64, f64, f64, u64)> = Vec::new(); // (total, shadow, main, froxel, cull, frame_idx)

    for e in &matching {
        let t  = zone_us(e, "total_us").unwrap_or(0.0);
        let sh = zone_us(e, "shadow_us").unwrap_or(0.0);
        let mn = zone_us(e, "main_us").unwrap_or(0.0);
        let fx = zone_us(e, "froxel_us").unwrap_or(0.0);
        let cl = zone_us(e, "cull_us").unwrap_or(0.0);
        let fi = e.get("frame_idx").and_then(|v| v.as_u64()).unwrap_or(0);
        totals.push(t);
        shadows.push(sh);
        mains.push(mn);
        froxels.push(fx);
        culls.push(cl);
        per_event.push((t, sh, mn, fx, cl, fi));
    }

    print_distribution_f("  total_ms         ", &totals);
    print_distribution_f("    shadow_ms      ", &shadows);
    print_distribution_f("    main_ms        ", &mains);
    print_distribution_f("    froxel_ms      ", &froxels);
    print_distribution_f("    cull_ms        ", &culls);
    println!();

    // Worst 10 spikes by total, with culprit identification.
    per_event.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    println!("  Worst 10 spikes (zone with highest %% of total = the culprit):");
    println!("    {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>10}",
        "total", "shadow", "main", "froxel", "cull", "culprit");
    for (t, sh, mn, fx, cl, _fi) in per_event.iter().take(10) {
        let max_zone = [
            ("shadow", *sh),
            ("main",   *mn),
            ("froxel", *fx),
            ("cull",   *cl),
        ].iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(n, _)| *n).unwrap_or("?");
        let max_val = sh.max(*mn).max(*fx).max(*cl);
        let pct = if *t > 0.0 { (max_val / t) * 100.0 } else { 0.0 };
        let culprit = format!("{} ({:.0}%)", max_zone, pct);
        println!("    {:>8.1}  {:>8.1}  {:>8.1}  {:>8.1}  {:>8.1}  {:>10}",
            t, sh, mn, fx, cl, culprit);
    }
    println!();

    // Aggregate by culprit zone.
    let mut culprit_counts: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for (_t, sh, mn, fx, cl, _) in &per_event {
        let max_zone = [
            ("shadow", *sh),
            ("main",   *mn),
            ("froxel", *fx),
            ("cull",   *cl),
        ].iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(n, _)| *n).unwrap_or("?");
        *culprit_counts.entry(max_zone).or_insert(0) += 1;
    }
    println!("  Culprit summary:");
    let mut culprit_v: Vec<_> = culprit_counts.iter().collect();
    culprit_v.sort_by(|a, b| b.1.cmp(a.1));
    for (zone, count) in culprit_v {
        let pct = (*count as f64 / matching.len() as f64) * 100.0;
        println!("    {:>10}: {} spikes ({:.0}%)", zone, count, pct);
    }
    println!();
    println!("  See `docs/v0.24.3-chug-investigation-plan.md` for next steps.");
}

/// `upload_pressure` — summarize `upload_pressure` events.
///
/// These events are emitted by the engine (v0.24.3-upload-pressure+)
/// whenever a single frame's mesh upload bytes exceeds a threshold.
/// Each event carries:
///   - frame: monotonic wall-clock frame counter for ordering.
///   - upload_bytes: total bytes memcpy'd into the persistent-mapped
///     pool buffer this frame.
///   - upload_chunks: how many chunks contributed.
///   - upload_ms: elapsed CPU time for the memcpy loop.
///
/// Interpretation: the upload-contention hypothesis says heavy CPU
/// writes to HOST_COHERENT mapped memory force the GPU's caches to
/// invalidate before its next read, slowing shadow + main passes
/// together (matching the gpu_spike pattern).
///
/// We report:
///   - Distribution of upload bytes, chunks, ms per pressure event.
///   - Count comparison against gpu_spike events from the SAME run.
///   - If counts are comparable, that's circumstantial evidence the
///     hypothesis is correct.
///
/// Note on correlation: we don't have a precise per-frame join key
/// (FIF=1 makes frame_idx useless, and frame counter wasn't propagated
/// to wait_fences_spike events). Count comparison is a coarse signal;
/// for tight correlation, look at events.jsonl ordering — upload_pressure
/// events should appear near gpu_spike events in time.
fn cmd_upload_pressure(events: &[Event], uuid_filter: Option<&str>) {
    let matching: Vec<&Event> = events.iter()
        .filter(|e| s(e, "kind") == "upload_pressure")
        .filter(|e| uuid_filter.map_or(true, |u| s(e, "uuid") == u))
        .collect();

    if matching.is_empty() {
        match uuid_filter {
            Some(u) => println!("No upload_pressure events found for run {}", u),
            None    => println!("No upload_pressure events found in events.jsonl"),
        }
        println!();
        println!("These are only emitted by engines at v0.24.3-upload-pressure or later.");
        return;
    }

    let label = match uuid_filter {
        Some(u) => format!("run {}", u),
        None    => format!("all runs in events.jsonl"),
    };
    println!("upload_pressure events ({}): n={}", label, matching.len());
    println!();

    // Distributions.
    let mut bytes_mb: Vec<f64> = Vec::new();
    let mut chunks: Vec<f64> = Vec::new();
    let mut elapsed_ms: Vec<f64> = Vec::new();
    let mut per_event: Vec<(f64, u64, f64, u64)> = Vec::new(); // (bytes_mb, chunks, ms, frame)

    for e in &matching {
        let b = e.get("upload_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as f64 / (1024.0 * 1024.0);
        let c = e.get("upload_chunks").and_then(|v| v.as_u64()).unwrap_or(0);
        let m = e.get("upload_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let f = e.get("frame").and_then(|v| v.as_u64()).unwrap_or(0);
        bytes_mb.push(b);
        chunks.push(c as f64);
        elapsed_ms.push(m);
        per_event.push((b, c, m, f));
    }

    print_distribution_f("  upload_bytes_MB ", &bytes_mb);
    print_distribution_f("  upload_chunks   ", &chunks);
    print_distribution_f("  upload_ms       ", &elapsed_ms);
    println!();

    // Worst 10 by bytes.
    per_event.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    println!("  Worst 10 upload-pressure frames (by bytes):");
    println!("    {:>10}  {:>8}  {:>8}  {:>10}", "bytes_MB", "chunks", "ms", "frame");
    for (b, c, m, f) in per_event.iter().take(10) {
        println!("    {:>10.2}  {:>8}  {:>8.1}  {:>10}", b, c, m, f);
    }
    println!();

    // Count comparison with gpu_spike events from the same run filter.
    // Hypothesis test: if upload_pressure count is comparable to
    // gpu_spike count (within ~2x), upload pressure is plausibly
    // co-occurring with the chugs.
    let gpu_spike_count = events.iter()
        .filter(|e| s(e, "kind") == "gpu_spike")
        .filter(|e| uuid_filter.map_or(true, |u| s(e, "uuid") == u))
        .count();
    let wait_spike_count = events.iter()
        .filter(|e| s(e, "kind") == "wait_fences_spike")
        .filter(|e| uuid_filter.map_or(true, |u| s(e, "uuid") == u))
        .count();

    println!("  Cross-reference (same run filter):");
    println!("    upload_pressure events: {}", matching.len());
    println!("    gpu_spike events:       {}", gpu_spike_count);
    println!("    wait_fences_spike:      {}", wait_spike_count);
    println!();
    if matching.len() > 0 && gpu_spike_count > 0 {
        let ratio = matching.len() as f64 / gpu_spike_count as f64;
        println!("  Hypothesis check:");
        if ratio >= 0.5 && ratio <= 2.0 {
            println!("    upload_pressure / gpu_spike ratio = {:.2}", ratio);
            println!("    -> Comparable counts; upload pressure plausibly co-occurs with chugs.");
            println!("       Look at events.jsonl for time-clustering of the two event kinds");
            println!("       to confirm. If they cluster, reducing UNENDLICH_UPLOAD_BYTES_BUDGET");
            println!("       (default 8MB) should reduce chug count.");
        } else if ratio < 0.5 {
            println!("    upload_pressure ({}) much fewer than gpu_spike ({}).", matching.len(), gpu_spike_count);
            println!("    -> Most chugs are NOT during upload-pressure frames. Hypothesis weaker.");
        } else {
            println!("    upload_pressure ({}) much more than gpu_spike ({}).", matching.len(), gpu_spike_count);
            println!("    -> Many high-upload frames did NOT cause chugs. Threshold may be too low,");
            println!("       or upload pressure alone isn't sufficient to chug.");
        }
    }
    println!();
    println!("  See `docs/v0.24.3-chug-investigation-plan.md` for next steps.");
}

/// `cull_count` — summarize `cull_count` events.
///
/// Each event carries:
///   - shadow_count: chunks passing the shadow cull's frustum test.
///   - main_count: chunks passing the camera cull's frustum test.
///   - total_count: total chunks in the renderer's pool.
///   - ratio: shadow_count / main_count.
///   - frame_idx: per-FIF slot (always 0 with MAX_FRAMES_IN_FLIGHT=1).
///
/// Events are emitted only when shadow_count > 1.5× main_count OR
/// shadow_count > 50 — to focus on frames where shadow load is the
/// dominant factor.
///
/// Interpretation: if shadow_count is consistently much larger than
/// main_count (say 2-3×), the shadow pass is paying for far more
/// chunks than the visible scene needs. The fix would be shadow-
/// specific culling (only draw chunks whose shadows actually fall
/// into the visible scene) or shadow LOD (draw distant casters at
/// coarser detail).
///
/// If shadow_count ≈ main_count, shadow is rendering the same
/// chunks as main but is still much more expensive. That points at
/// per-draw or per-chunk fixed costs (state changes, tile-binning
/// per draw, etc) rather than chunk count.
fn cmd_cull_count(events: &[Event], uuid_filter: Option<&str>) {
    let matching: Vec<&Event> = events.iter()
        .filter(|e| s(e, "kind") == "cull_count")
        .filter(|e| uuid_filter.map_or(true, |u| s(e, "uuid") == u))
        .collect();

    if matching.is_empty() {
        match uuid_filter {
            Some(u) => println!("No cull_count events found for run {}", u),
            None    => println!("No cull_count events found in events.jsonl"),
        }
        println!();
        println!("These are only emitted by engines at v0.24.3-cull-count or later.");
        println!("Events fire only when shadow_count > 1.5× main_count or shadow_count > 50.");
        return;
    }

    let label = match uuid_filter {
        Some(u) => format!("run {}", u),
        None    => format!("all runs in events.jsonl"),
    };
    println!("cull_count events ({}): n={}", label, matching.len());
    println!();

    let mut shadows: Vec<f64> = Vec::new();
    let mut mains: Vec<f64> = Vec::new();
    let mut totals: Vec<f64> = Vec::new();
    let mut ratios: Vec<f64> = Vec::new();
    let mut per_event: Vec<(u64, u64, u64, f64)> = Vec::new(); // (shadow, main, total, ratio)

    for e in &matching {
        let sh = e.get("shadow_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let mn = e.get("main_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let tt = e.get("total_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let r  = e.get("ratio").and_then(|v| v.as_f64()).unwrap_or(0.0);
        shadows.push(sh as f64);
        mains.push(mn as f64);
        totals.push(tt as f64);
        ratios.push(r);
        per_event.push((sh, mn, tt, r));
    }

    print_distribution_f("  shadow_count    ", &shadows);
    print_distribution_f("  main_count      ", &mains);
    print_distribution_f("  total_count     ", &totals);
    print_distribution_f("  ratio (shdw/mn) ", &ratios);
    println!();

    // Worst 10 by shadow_count.
    per_event.sort_by(|a, b| b.0.cmp(&a.0));
    println!("  Worst 10 frames (by shadow_count):");
    println!("    {:>10}  {:>8}  {:>8}  {:>8}", "shadow", "main", "total", "ratio");
    for (sh, mn, tt, r) in per_event.iter().take(10) {
        println!("    {:>10}  {:>8}  {:>8}  {:>8.2}", sh, mn, tt, r);
    }
    println!();

    // Quick verdict.
    let mean_ratio: f64 = ratios.iter().sum::<f64>() / ratios.len() as f64;
    println!("  Mean shadow:main ratio across emitted events: {:.2}", mean_ratio);
    if mean_ratio > 2.0 {
        println!("  -> Shadow is drawing significantly more chunks than main.");
        println!("     Likely fix: shadow-specific LOD or cull tightening.");
    } else if mean_ratio > 1.3 {
        println!("  -> Shadow draws somewhat more chunks than main (modest).");
        println!("     Per-chunk fixed cost likely matters too.");
    } else {
        println!("  -> Shadow draws roughly same chunks as main.");
        println!("     Chunk count is NOT the explanation; per-draw or vertex cost is.");
    }
    println!();
    println!("  See `docs/v0.24.3-chug-investigation-plan.md` for next steps.");
}

/// `shadow_stride` — summarize `shadow_stride` events.
///
/// Each event carries:
///   - frame: monotonic frame counter.
///   - stride_setting: env-configured stride (1-16).
///   - effective_stride: what the cull actually used this frame
///     (1 when force_full is true, stride_setting otherwise).
///   - phase: residue class admitted this frame (0..stride).
///   - force_full: whether this frame bypassed the stride.
///   - warmup_force: forced because we're in the first N frames.
///   - cascade_shift_force: forced because camera moved > 1 chunk.
///
/// Events emit only when stride_setting > 1 (= the feature is on).
///
/// Interpretation: if the data shows mostly `effective_stride > 1`
/// frames, the stride is engaging — compare GPU `shadow` zone time
/// to a stride=1 baseline run. If many frames force-full due to
/// cascade-shift, the player is moving fast enough to bypass
/// temporal amortization, and the chug returns.
fn cmd_shadow_stride(events: &[Event], uuid_filter: Option<&str>) {
    let matching: Vec<&Event> = events.iter()
        .filter(|e| s(e, "kind") == "shadow_stride")
        .filter(|e| uuid_filter.map_or(true, |u| s(e, "uuid") == u))
        .collect();

    if matching.is_empty() {
        match uuid_filter {
            Some(u) => println!("No shadow_stride events found for run {}", u),
            None    => println!("No shadow_stride events found in events.jsonl"),
        }
        println!();
        println!("Set UNENDLICH_SHADOW_CHUNK_STRIDE > 1 to enable the feature.");
        println!("These events are emitted only when stride > 1.");
        return;
    }

    let label = match uuid_filter {
        Some(u) => format!("run {}", u),
        None    => format!("all runs in events.jsonl"),
    };
    println!("shadow_stride events ({}): n={}", label, matching.len());
    println!();

    let mut force_full_count = 0u32;
    let mut warmup_count = 0u32;
    let mut cascade_shift_count = 0u32;
    let mut strided_count = 0u32;
    let mut effective_strides: Vec<f64> = Vec::new();
    let mut phase_distribution: std::collections::HashMap<u64, u32> =
        std::collections::HashMap::new();

    for e in &matching {
        let force_full = e.get("force_full")
            .and_then(|v| v.as_bool()).unwrap_or(false);
        let warmup = e.get("warmup_force")
            .and_then(|v| v.as_bool()).unwrap_or(false);
        let casc_shift = e.get("cascade_shift_force")
            .and_then(|v| v.as_bool()).unwrap_or(false);
        let eff_stride = e.get("effective_stride")
            .and_then(|v| v.as_u64()).unwrap_or(1);
        let phase = e.get("phase").and_then(|v| v.as_u64()).unwrap_or(0);

        if force_full { force_full_count += 1; }
        if warmup { warmup_count += 1; }
        if casc_shift { cascade_shift_count += 1; }
        if !force_full { strided_count += 1; }
        effective_strides.push(eff_stride as f64);
        *phase_distribution.entry(phase).or_insert(0) += 1;
    }

    let n = matching.len();
    let setting = matching[0].get("stride_setting")
        .and_then(|v| v.as_u64()).unwrap_or(1);
    println!("  Configured stride: {}", setting);
    println!();
    println!("  Frame disposition (n={}):", n);
    println!("    strided (cull skipped chunks):    {} ({:.1}%)",
        strided_count, strided_count as f64 / n as f64 * 100.0);
    println!("    forced full render:               {} ({:.1}%)",
        force_full_count, force_full_count as f64 / n as f64 * 100.0);
    println!("      reason: warmup (first N frames): {}", warmup_count);
    println!("      reason: cascade shifted > 1 chunk: {}", cascade_shift_count);
    println!();

    print_distribution_f("  effective_stride", &effective_strides);
    println!();

    let mut phase_vec: Vec<_> = phase_distribution.iter().collect();
    phase_vec.sort_by_key(|&(p, _)| *p);
    println!("  Phase distribution:");
    for (phase, count) in phase_vec {
        println!("    phase {}: {} frames ({:.1}%)",
            phase, count, *count as f64 / n as f64 * 100.0);
    }
    println!();
    if cascade_shift_count > strided_count {
        println!("  Warning: more cascade-shift force-fulls than strided frames.");
        println!("  Player is moving too fast for temporal amortization to engage.");
        println!("  Either reduce movement speed or accept full render on motion.");
    }
}
