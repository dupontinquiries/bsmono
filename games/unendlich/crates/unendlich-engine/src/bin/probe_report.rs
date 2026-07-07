//! probe-report — analyze CSV files from the thread_probe sampler.
//!
//! Usage:
//!   probe-report path/to/probe-NNNN.csv
//!   probe-report path/to/probe-NNNN.csv --window 30000:60000   (slice ms)
//!   probe-report path/to/probe-NNNN.csv --thread render
//!   probe-report path/to/probe-NNNN.csv --hot-deltas
//!
//! Reads the CSV the sampler writes, produces terminal summaries that
//! Claude or a human can read directly without browsing thousands of
//! rows.
//!
//! Output sections:
//!   1. Overview — duration, threads observed, FPS distribution, chug%
//!   2. Per-thread activity histogram (what fraction of time in each
//!      state, per registered thread)
//!   3. Per-thread CPU sleep% — derived from cpu_ticks_delta vs the
//!      sample interval, tells us how much each thread was actually
//!      doing CPU work
//!   4. Hot deltas — moments when many threads changed state
//!      simultaneously. The magic-switch moment is the canonical
//!      example. Top 10 reported with surrounding context rows.
//!   5. Chug periods — contiguous spans where the engine flagged
//!      itself as chugging. Per-thread state distribution during each.
//!
//! Designed to print compactly (≤ ~100 lines) so the output can be
//! pasted into a chat without burning context.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};

#[derive(Debug, Clone)]
// Fields are parsed from the telemetry CSV; not every column is read by every
// report, so the unread ones are expected.
#[allow(dead_code)]
struct Row {
    ms: u64,
    wall_dt: u64,
    thread: String,
    tid: u32,
    activity: String,
    context: u64,
    cpu_ticks_delta: u64,
    last_cpu: i32,
    wchan: String,
    fps_tenths: u32,
    frame_p50_us: u32,
    wait_fences_us: u32,
    mesh_q_size: u32,
    mesh_q_in_flight: u32,
    gen_pending: u32,
    rendering: u32,
    loaded: u32,
    chug: bool,
}

fn parse_row(s: &str) -> Option<Row> {
    let f: Vec<&str> = s.trim().split(',').collect();
    if f.len() < 18 { return None; }
    Some(Row {
        ms: f[0].parse().ok()?,
        wall_dt: f[1].parse().ok()?,
        thread: f[2].to_string(),
        tid: f[3].parse().ok()?,
        activity: f[4].to_string(),
        context: f[5].parse().ok()?,
        cpu_ticks_delta: f[6].parse().ok()?,
        last_cpu: f[7].parse().ok()?,
        wchan: f[8].to_string(),
        fps_tenths: f[9].parse().ok()?,
        frame_p50_us: f[10].parse().ok()?,
        wait_fences_us: f[11].parse().ok()?,
        mesh_q_size: f[12].parse().ok()?,
        mesh_q_in_flight: f[13].parse().ok()?,
        gen_pending: f[14].parse().ok()?,
        rendering: f[15].parse().ok()?,
        loaded: f[16].parse().ok()?,
        chug: f[17] == "1",
    })
}

fn parse_window(s: &str) -> Option<(u64, u64)> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 { return None; }
    Some((parts[0].parse().ok()?, parts[1].parse().ok()?))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("usage: probe-report PATH [--window START_MS:END_MS] [--thread NAME] [--hot-deltas]");
        std::process::exit(if args.is_empty() { 2 } else { 0 });
    }
    let path = &args[0];
    let mut window: Option<(u64, u64)> = None;
    let mut thread_filter: Option<String> = None;
    let mut hot_deltas_only = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--window" if i + 1 < args.len() => {
                window = parse_window(&args[i + 1]);
                i += 2;
            }
            "--thread" if i + 1 < args.len() => {
                thread_filter = Some(args[i + 1].clone());
                i += 2;
            }
            "--hot-deltas" => { hot_deltas_only = true; i += 1; }
            other => {
                eprintln!("unknown arg: {}", other);
                std::process::exit(2);
            }
        }
    }

    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("could not open {}: {}", path, e);
            std::process::exit(1);
        }
    };
    let reader = BufReader::new(file);
    let mut rows: Vec<Row> = Vec::new();
    for (idx, line) in reader.lines().enumerate() {
        if idx == 0 { continue; } // header
        let Ok(line) = line else { continue };
        let Some(row) = parse_row(&line) else { continue };
        if let Some((a, b)) = window {
            if row.ms < a || row.ms > b { continue; }
        }
        if let Some(t) = &thread_filter {
            if row.thread != *t { continue; }
        }
        rows.push(row);
    }
    if rows.is_empty() {
        eprintln!("no rows matched filters");
        std::process::exit(1);
    }

    if hot_deltas_only {
        print_hot_deltas(&rows);
        return;
    }
    print_overview(&rows);
    println!();
    print_activity_histogram(&rows);
    println!();
    print_cpu_sleep(&rows);
    println!();
    print_hot_deltas(&rows);
    println!();
    print_chug_periods(&rows);
}

fn print_overview(rows: &[Row]) {
    let min_ms = rows.iter().map(|r| r.ms).min().unwrap_or(0);
    let max_ms = rows.iter().map(|r| r.ms).max().unwrap_or(0);
    let duration = (max_ms - min_ms) as f64 / 1000.0;
    let mut threads: BTreeMap<String, u32> = BTreeMap::new();
    let mut chug_rows = 0;
    let mut fps_sum: u64 = 0;
    let mut fps_n: u64 = 0;
    let mut min_fps = u32::MAX;
    let mut max_fps = 0u32;
    for r in rows {
        *threads.entry(r.thread.clone()).or_insert(0) += 1;
        if r.thread == "render" {
            if r.chug { chug_rows += 1; }
            if r.fps_tenths > 0 {
                fps_sum += r.fps_tenths as u64;
                fps_n += 1;
                min_fps = min_fps.min(r.fps_tenths);
                max_fps = max_fps.max(r.fps_tenths);
            }
        }
    }
    let avg_fps = if fps_n > 0 { fps_sum as f64 / fps_n as f64 / 10.0 } else { 0.0 };
    let render_rows = threads.get("render").copied().unwrap_or(0);
    let chug_pct = if render_rows > 0 {
        chug_rows as f64 / render_rows as f64 * 100.0
    } else { 0.0 };

    println!("─ Overview ────────────────────────────────────────");
    println!("  duration:  {:.1}s  ({} → {} ms)", duration, min_ms, max_ms);
    println!("  fps:       avg {:.1}  min {:.1}  max {:.1}",
        avg_fps, min_fps as f64 / 10.0, max_fps as f64 / 10.0);
    println!("  chug:      {:.1}%  ({} of {} render-thread samples)",
        chug_pct, chug_rows, render_rows);
    println!("  threads observed ({}):", threads.len());
    for (name, count) in &threads {
        println!("    {:<28}  {} samples", name, count);
    }
}

fn print_activity_histogram(rows: &[Row]) {
    // Per-thread, count of each activity tag, expressed as % of that
    // thread's total samples.
    println!("─ Per-thread activity distribution ─────────────────");
    let mut by_thread: BTreeMap<String, HashMap<String, u32>> = BTreeMap::new();
    for r in rows {
        let m = by_thread.entry(r.thread.clone()).or_default();
        *m.entry(r.activity.clone()).or_insert(0) += 1;
    }
    for (thread, hist) in &by_thread {
        let total: u32 = hist.values().sum();
        if total == 0 { continue; }
        println!("  {} ({} samples)", thread, total);
        let mut pairs: Vec<(&String, &u32)> = hist.iter().collect();
        pairs.sort_by(|a, b| b.1.cmp(a.1));
        for (act, n) in pairs {
            let pct = *n as f64 / total as f64 * 100.0;
            // Skip tiny tail entries to keep output compact.
            if pct < 0.5 { continue; }
            println!("    {:<14}  {:>6.1}%  ({} samples)", act, pct, n);
        }
    }
}

fn print_cpu_sleep(rows: &[Row]) {
    // CPU sleep% per thread: 1 - (avg cpu_ticks_delta / clock_ticks_per_interval).
    // We can compute clock_ticks_per_interval from wall_dt: at 100Hz
    // clock (typical Linux), 100ms = 10 ticks. We use 100Hz as
    // assumption; if the host uses a different HZ (250, 1000), the
    // percentages would be scaled wrong. The relative ordering is
    // still meaningful regardless.
    //
    // Sometimes USER_HZ is 250 or 1000. Linux today defaults to 250 on
    // most distros and 100 on some embedded. We can't detect from
    // userspace without sysconf(_SC_CLK_TCK). To stay portable across
    // the analysis tool, we report cpu_ticks_per_sec_avg and let the
    // reader scale mentally.
    println!("─ Per-thread CPU work (ticks/sec averaged) ─────────");
    println!("  (high = busy thread; low = mostly sleeping)");
    let mut by_thread: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for r in rows {
        let e = by_thread.entry(r.thread.clone()).or_insert((0, 0));
        e.0 += r.cpu_ticks_delta;
        e.1 += r.wall_dt;
    }
    for (thread, (ticks, wall_ms)) in &by_thread {
        if *wall_ms == 0 { continue; }
        let ticks_per_sec = *ticks as f64 / (*wall_ms as f64 / 1000.0);
        println!("    {:<28}  {:>7.1} ticks/sec", thread, ticks_per_sec);
    }
}

fn print_hot_deltas(rows: &[Row]) {
    // For each sample interval, compute an "activity flux" score = how
    // many threads changed their activity tag since the previous
    // interval. Sort intervals by score descending; print top 10 with
    // a few rows of context.
    //
    // Algorithm: group rows by ms. For each (ms_t, ms_t+1) pair,
    // count threads whose activity differs.
    println!("─ Hot deltas (intervals with many simultaneous state changes) ─");
    let mut by_ms: BTreeMap<u64, HashMap<String, String>> = BTreeMap::new();
    for r in rows {
        by_ms.entry(r.ms).or_default().insert(r.thread.clone(), r.activity.clone());
    }
    let ms_list: Vec<u64> = by_ms.keys().copied().collect();
    let mut deltas: Vec<(u64, u64, u32)> = Vec::new(); // (prev_ms, cur_ms, change_count)
    for w in ms_list.windows(2) {
        let prev = &by_ms[&w[0]];
        let cur = &by_ms[&w[1]];
        let mut changed = 0u32;
        for (thread, act) in cur {
            if prev.get(thread).map(|p| p != act).unwrap_or(true) {
                changed += 1;
            }
        }
        deltas.push((w[0], w[1], changed));
    }
    deltas.sort_by(|a, b| b.2.cmp(&a.2));
    let top = deltas.into_iter().take(10);
    for (prev_ms, cur_ms, n) in top {
        if n == 0 { continue; }
        // Find a representative render-thread row at cur_ms to print fps/queue context.
        let ctx_row = rows.iter().find(|r| r.ms == cur_ms && r.thread == "render");
        let (fps, mq, gp) = if let Some(r) = ctx_row {
            (r.fps_tenths as f64 / 10.0, r.mesh_q_size, r.gen_pending)
        } else {
            (0.0, 0, 0)
        };
        println!("  t={:>6}ms → {:>6}ms  {:>2} thread state changes  fps={:>5.1} mesh_q={} gen={}",
            prev_ms, cur_ms, n, fps, mq, gp);
    }
}

fn print_chug_periods(rows: &[Row]) {
    // Contiguous spans where any render-thread row has chug=1.
    // For each chug span, print start/end ms, duration, fps at edges,
    // and per-thread state distribution within the span.
    println!("─ Chug periods (engine-flagged: fps<20 OR wait_fences>50ms) ─");
    let mut render_rows: Vec<&Row> = rows.iter().filter(|r| r.thread == "render").collect();
    render_rows.sort_by_key(|r| r.ms);

    let mut spans: Vec<(u64, u64)> = Vec::new(); // (start_ms, end_ms) inclusive
    let mut cur: Option<(u64, u64)> = None;
    for r in &render_rows {
        if r.chug {
            cur = Some(match cur {
                Some((s, _)) => (s, r.ms),
                None => (r.ms, r.ms),
            });
        } else if let Some(span) = cur.take() {
            spans.push(span);
        }
    }
    if let Some(span) = cur { spans.push(span); }

    if spans.is_empty() {
        println!("  no chug periods detected");
        return;
    }
    for (i, (start, end)) in spans.iter().enumerate() {
        let duration = (end - start) as f64 / 1000.0;
        if duration < 0.5 { continue; }  // skip noise
        let start_row = render_rows.iter().find(|r| r.ms == *start);
        let end_row = render_rows.iter().find(|r| r.ms == *end);
        let fps_start = start_row.map(|r| r.fps_tenths as f64 / 10.0).unwrap_or(0.0);
        let fps_end = end_row.map(|r| r.fps_tenths as f64 / 10.0).unwrap_or(0.0);
        let mq_at_end = end_row.map(|r| r.mesh_q_size).unwrap_or(0);
        let gp_at_end = end_row.map(|r| r.gen_pending).unwrap_or(0);
        println!("  #{}: {} → {} ms ({:.1}s)  fps {:.1}→{:.1}  mesh_q@end={} gen@end={}",
            i, start, end, duration, fps_start, fps_end, mq_at_end, gp_at_end);

        // Activity distribution during this span, per non-render thread.
        let mut span_threads: BTreeMap<String, HashMap<String, u32>> = BTreeMap::new();
        for r in rows {
            if r.ms < *start || r.ms > *end { continue; }
            if r.thread == "render" { continue; }
            *span_threads.entry(r.thread.clone()).or_default().entry(r.activity.clone()).or_insert(0) += 1;
        }
        for (thread, hist) in &span_threads {
            let total: u32 = hist.values().sum();
            if total == 0 { continue; }
            let mut pairs: Vec<(&String, &u32)> = hist.iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(a.1));
            let top: Vec<String> = pairs.iter().take(3)
                .map(|(a, n)| format!("{}={:.0}%", a, **n as f64 / total as f64 * 100.0))
                .collect();
            println!("       {:<24}  {}", thread, top.join("  "));
        }
    }
}
