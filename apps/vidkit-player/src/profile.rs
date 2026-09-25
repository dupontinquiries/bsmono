//! `VIDKIT_PROFILE=<file>`: one tab-separated line per second of what the
//! player spends, for long runs on real content (see tools/profile/).
//!
//! CPU is per THREAD, grouped by name, from /proc/self/task/*/stat -- so a
//! run says whether the time went to the UI thread, the decoder thread,
//! llvmpipe's rasteriser workers, whisper, or the ffmpeg children (found by
//! parent pid in /proc). Percent of one core over the second.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::Instant;

/// Thread-name groups, in the column order written.
pub const GROUPS: [&str; 7] = ["ui", "video", "audio", "gl", "transcribe", "other", "children"];

/// Which group a thread name belongs to. Pure.
pub fn group(comm: &str) -> &'static str {
    match comm {
        c if c.starts_with("vidkit-player") || c == "main" => "ui",
        c if c.starts_with("vidkit-video") => "video",
        c if c.starts_with("vidkit-audio") || c.starts_with("vidkit-cpal") || c.contains("alsa") => "audio",
        c if c.starts_with("llvmpipe") || c.starts_with("gallium") || c.contains("gl") || c.starts_with("radeon") || c.starts_with("iris") => "gl",
        c if c.starts_with("vidkit-transcr") || c.starts_with("ggml") || c.starts_with("whisper") => "transcribe",
        _ => "other",
    }
}

/// utime+stime ticks from a /proc stat line (the comm may contain spaces
/// and parentheses; fields are counted after the LAST ')'). Pure.
pub fn stat_ticks(line: &str) -> Option<(String, u64, i32)> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let comm = line[open + 1..close].to_string();
    let rest: Vec<&str> = line[close + 1..].split_whitespace().collect();
    // After ')': state(0) ppid(1) ... utime(11) stime(12)
    let ppid = rest.get(1)?.parse().ok()?;
    let ut: u64 = rest.get(11)?.parse().ok()?;
    let st: u64 = rest.get(12)?.parse().ok()?;
    Some((comm, ut + st, ppid))
}

fn thread_ticks() -> BTreeMap<&'static str, u64> {
    let mut m = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir("/proc/self/task") {
        for e in rd.flatten() {
            if let Some((comm, t, _)) = std::fs::read_to_string(e.path().join("stat")).ok().as_deref().and_then(stat_ticks) {
                *m.entry(group(&comm)).or_insert(0) += t;
            }
        }
    }
    let me = std::process::id() as i32;
    let mut kids = 0;
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let n = e.file_name();
            if !n.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            if let Some((_, t, ppid)) = std::fs::read_to_string(e.path().join("stat")).ok().as_deref().and_then(stat_ticks) {
                if ppid == me {
                    kids += t;
                }
            }
        }
    }
    m.insert("children", kids);
    m
}

fn page_size() -> u64 {
    // SAFETY: sysconf with a constant.
    let p = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if p > 0 { p as u64 } else { 4096 }
}

fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        // The real page size: Asahi kernels use 16 KiB pages, and a fixed
        // 4096 read a quarter of the true RSS there.
        .map(|pages| pages * page_size() / (1 << 20))
        .unwrap_or(0)
}

/// Counters the app bumps; the profiler turns them into per-second rates.
#[derive(Default, Clone, Debug)]
pub struct Sample {
    pub repaints: u64,
    pub shown: u64,
    pub dropped: u64,
    pub uploaded_bytes: u64,
    pub damaged_tiles: u64,
    pub leaves: u64,
    pub conversions: u64,
    pub lod_max_seen: u8,
    pub decoder: String,
    pub enhance: String,
    pub power: String,
    pub presenter: String,
}

pub struct Profiler {
    out: std::fs::File,
    started: Instant,
    last: Instant,
    last_ticks: BTreeMap<&'static str, u64>,
    last_totals: (u64, u64, u64),
    pub acc: Sample,
    hz: f64,
}

impl Profiler {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("VIDKIT_PROFILE")?;
        let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
        let mut header = vec!["t_s", "repaints", "shown", "dropped", "upload_kb", "damaged_pct", "conv_kpx", "lod_max"];
        let cpu_cols: Vec<String> = GROUPS.iter().map(|g| format!("cpu_{g}")).collect();
        header.extend(cpu_cols.iter().map(String::as_str));
        header.extend(["rss_mb", "presenter", "decoder", "enhance", "power"]);
        let _ = writeln!(out, "{}", header.join("\t"));
        // SAFETY: sysconf with a constant.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
        Some(Self { out, started: Instant::now(), last: Instant::now(), last_ticks: thread_ticks(), last_totals: (0, 0, 0), acc: Sample::default(), hz })
    }

    /// Call every UI frame with the engine's running totals (shown,
    /// dropped) and the presenter's uploaded bytes; writes a line a second.
    pub fn frame(&mut self, shown_total: u64, dropped_total: u64, uploaded_total: u64) {
        self.acc.repaints += 1;
        let dt = self.last.elapsed().as_secs_f64();
        if dt < 1.0 {
            return;
        }
        let ticks = thread_ticks();
        let cpu = |g: &str| {
            let d = ticks.get(g).copied().unwrap_or(0).saturating_sub(self.last_ticks.get(g).copied().unwrap_or(0));
            d as f64 / self.hz / dt * 100.0
        };
        let (s0, d0, u0) = self.last_totals;
        let a = &self.acc;
        let damaged = if a.leaves > 0 { a.damaged_tiles as f64 / a.leaves as f64 * 100.0 } else { f64::NAN };
        let mut cols = vec![
            format!("{:.1}", self.started.elapsed().as_secs_f64()),
            format!("{:.1}", a.repaints as f64 / dt),
            format!("{:.1}", shown_total.saturating_sub(s0) as f64 / dt),
            format!("{:.1}", dropped_total.saturating_sub(d0) as f64 / dt),
            format!("{:.0}", uploaded_total.saturating_sub(u0) as f64 / dt / 1024.0),
            if damaged.is_nan() { "-".into() } else { format!("{damaged:.1}") },
            format!("{:.0}", a.conversions as f64 / dt / 1000.0),
            a.lod_max_seen.to_string(),
        ];
        cols.extend(GROUPS.iter().map(|g| format!("{:.1}", cpu(g))));
        cols.extend([rss_mb().to_string(), a.presenter.clone(), a.decoder.clone(), a.enhance.clone(), a.power.clone()]);
        let _ = writeln!(self.out, "{}", cols.join("\t"));
        self.last = Instant::now();
        self.last_ticks = ticks;
        self.last_totals = (shown_total, dropped_total, uploaded_total);
        let keep = (a.decoder.clone(), a.enhance.clone(), a.power.clone(), a.presenter.clone());
        self.acc = Sample { decoder: keep.0, enhance: keep.1, power: keep.2, presenter: keep.3, ..Sample::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_names_fall_into_their_groups() {
        assert_eq!(group("vidkit-video"), "video");
        assert_eq!(group("vidkit-audio"), "audio");
        assert_eq!(group("llvmpipe-3"), "gl");
        assert_eq!(group("vidkit-transcri"), "transcribe", "comm is truncated to 15 chars");
        assert_eq!(group("vidkit-player"), "ui");
        assert_eq!(group("tokio-runtime-w"), "other");
    }

    #[test]
    fn stat_lines_parse_even_with_awkward_names() {
        let l = "4242 (vidkit (video) x) S 4200 4242 4242 0 -1 4194560 100 0 0 0 150 25 0 0 20 0 1 0 1 1 1";
        assert_eq!(stat_ticks(l), Some(("vidkit (video) x".into(), 175, 4200)));
        assert_eq!(stat_ticks("garbage"), None);
    }

    /// Our own process's threads are readable and the main thread is counted.
    #[test]
    fn this_process_is_measurable() {
        let t = thread_ticks();
        assert!(t.contains_key("children"));
        let total: u64 = t.values().sum();
        let spin = Instant::now();
        let mut x = 0u64;
        while spin.elapsed().as_millis() < 120 {
            x = x.wrapping_mul(31).wrapping_add(7);
        }
        std::hint::black_box(x);
        let after: u64 = thread_ticks().values().sum();
        assert!(after > total, "120 ms of spinning did not show up");
    }
}
