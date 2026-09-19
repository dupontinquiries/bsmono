//! What the process looked like in the seconds before it stopped: memory, CPU,
//! thread count, and frame pacing, kept in fixed-size rings.
//!
//! Sources, and how far to trust them:
//!   * Memory/CPU/threads are sampled from /proc/<pid> by a thread in this
//!     process. No ptrace involvement, negligible perturbation. Local targets
//!     only; for a gdbserver target the sampler would have to run on the device.
//!   * Frame pacing comes from frame-marker breakpoints (`Session::
//!     insert_frame_marker`). Each mark is a ptrace stop plus an MI round trip,
//!     so intervals are inflated by a roughly constant amount. Relative shape
//!     (hitches, a slowdown before the crash) is meaningful; absolute frame
//!     times are not a benchmark. A uprobe-based source would remove the
//!     overhead but needs CAP_PERFMON or perf_event_paranoid <= 1.
//!
//! Rings freeze while the target is stopped. Otherwise the sampler would spend
//! the time you are reading the backtrace overwriting the history you want.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub at: Instant,
    pub rss_kb: u64,
    pub anon_kb: u64,
    pub swap_kb: u64,
    pub threads: u32,
    /// Percent of one core; above 100 with several busy threads.
    pub cpu_pct: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameInterval {
    pub at: Instant,
    pub ms: f32,
}

#[derive(Debug)]
pub struct Vitals {
    samples: VecDeque<Sample>,
    frames: VecDeque<FrameInterval>,
    last_mark: Option<Instant>,
    frozen: bool,
    cap_samples: usize,
    cap_frames: usize,
}

pub type Shared = Arc<Mutex<Vitals>>;

impl Vitals {
    pub fn new(cap_samples: usize, cap_frames: usize) -> Self {
        Vitals { samples: VecDeque::new(), frames: VecDeque::new(), last_mark: None, frozen: false, cap_samples, cap_frames }
    }

    pub fn shared(cap_samples: usize, cap_frames: usize) -> Shared {
        Arc::new(Mutex::new(Self::new(cap_samples, cap_frames)))
    }

    pub fn push_sample(&mut self, s: Sample) {
        if self.frozen {
            return;
        }
        if self.samples.len() == self.cap_samples {
            self.samples.pop_front();
        }
        self.samples.push_back(s);
    }

    pub fn mark_frame(&mut self, at: Instant) {
        if let Some(prev) = self.last_mark.replace(at) {
            if self.frames.len() == self.cap_frames {
                self.frames.pop_front();
            }
            self.frames.push_back(FrameInterval { at, ms: at.duration_since(prev).as_secs_f32() * 1000.0 });
        }
    }

    /// Call with true on a user-visible stop and false on resume. Unfreezing
    /// forgets the last mark so the pause does not show up as one huge frame.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
        if !frozen {
            self.last_mark = None;
        }
    }

    pub fn reset(&mut self) {
        self.samples.clear();
        self.frames.clear();
        self.last_mark = None;
        self.frozen = false;
    }

    pub fn samples(&self) -> Vec<Sample> {
        self.samples.iter().copied().collect()
    }

    pub fn frames(&self) -> Vec<FrameInterval> {
        self.frames.iter().copied().collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameStats {
    pub count: usize,
    pub p50_ms: f32,
    pub p95_ms: f32,
    pub p99_ms: f32,
    pub worst_ms: f32,
    /// Intervals longer than 1.5x the median: a missed vsync at any refresh rate,
    /// without having to know the refresh rate.
    pub hitches: usize,
    /// How long before the last mark the most recent hitch happened.
    pub last_hitch_before_end: Option<Duration>,
}

pub fn frame_stats(frames: &[FrameInterval]) -> Option<FrameStats> {
    if frames.len() < 2 {
        return None;
    }
    let mut sorted: Vec<f32> = frames.iter().map(|f| f.ms).collect();
    sorted.sort_by(f32::total_cmp);
    let pct = |p: f32| sorted[(((sorted.len() - 1) as f32) * p).round() as usize];
    let p50 = pct(0.50);
    let end = frames.last()?.at;
    let hitch = |f: &&FrameInterval| f.ms > p50 * 1.5;
    Some(FrameStats {
        count: frames.len(),
        p50_ms: p50,
        p95_ms: pct(0.95),
        p99_ms: pct(0.99),
        worst_ms: *sorted.last()?,
        hitches: frames.iter().filter(hitch).count(),
        last_hitch_before_end: frames.iter().rev().find(hitch).map(|f| end.duration_since(f.at)),
    })
}

/// kB per second over the window, by least squares; a leak shows as a steady
/// positive slope where an allocation burst does not.
pub fn rss_slope_kb_per_s(samples: &[Sample]) -> Option<f32> {
    let first = samples.first()?.at;
    if samples.len() < 3 {
        return None;
    }
    let pts: Vec<(f32, f32)> = samples.iter().map(|s| (s.at.duration_since(first).as_secs_f32(), s.rss_kb as f32)).collect();
    let n = pts.len() as f32;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let (mx, my) = (sx / n, sy / n);
    let (num, den) = pts.iter().fold((0.0, 0.0), |(nu, de), (x, y)| (nu + (x - mx) * (y - my), de + (x - mx) * (x - mx)));
    (den > 0.0).then(|| num / den)
}

/// Lines suitable for the notes of a stop diagnostic.
pub fn summarize(v: &Vitals) -> Vec<String> {
    let mut out = Vec::new();
    let samples = v.samples();
    if let (Some(first), Some(last)) = (samples.first(), samples.last()) {
        let span = last.at.duration_since(first.at).as_secs_f32();
        let mut line = format!("before the stop: rss {:.1} MiB", last.rss_kb as f32 / 1024.0);
        // Under a second of samples is too little to call a trend.
        if let Some(slope) = rss_slope_kb_per_s(&samples).filter(|s| s.abs() > 64.0 && span >= 1.0) {
            line += &format!(" ({:+.1} MiB/s over the last {span:.1} s)", slope / 1024.0);
        }
        if last.swap_kb > 0 {
            line += &format!(", {:.1} MiB swapped", last.swap_kb as f32 / 1024.0);
        }
        let cpu = samples.iter().rev().take(10).map(|s| s.cpu_pct).sum::<f32>() / samples.len().min(10) as f32;
        line += &format!(", cpu {cpu:.0}%, {} thread{}", last.threads, if last.threads == 1 { "" } else { "s" });
        out.push(line);
    }
    if let Some(f) = frame_stats(&v.frames()) {
        let mut line = format!("frame pacing over {} frames: p50 {:.1} ms, p99 {:.1} ms, worst {:.1} ms, {} hitches", f.count, f.p50_ms, f.p99_ms, f.worst_ms, f.hitches);
        if let Some(d) = f.last_hitch_before_end {
            line += &format!(" (last one {:.2} s before the stop)", d.as_secs_f32());
        }
        out.push(line);
    }
    out
}

/// One reading. `prev` is the (cpu ticks, time) of the previous reading.
pub fn read_sample(proc_root: &Path, pid: u32, prev: Option<(u64, Instant)>) -> Option<(Sample, u64)> {
    let dir = proc_root.join(pid.to_string());
    let at = Instant::now();
    // /proc/<pid>/status rather than statm: status reports kB, statm reports
    // pages, and page size is 16K on Asahi and 4K almost everywhere else.
    let status = std::fs::read_to_string(dir.join("status")).ok()?;
    let field = |k: &str| status.lines().find_map(|l| l.strip_prefix(k)).and_then(|r| r.split_whitespace().next()?.parse::<u64>().ok());
    // comm may contain spaces and parentheses; fields are only reliable after the last ')'.
    let stat = std::fs::read_to_string(dir.join("stat")).ok()?;
    let rest: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let ticks = rest.get(11)?.parse::<u64>().ok()? + rest.get(12)?.parse::<u64>().ok()?;
    // USER_HZ is 100 on every Linux architecture; it is ABI, unlike the kernel's HZ.
    let cpu_pct = match prev {
        Some((pt, pa)) if at > pa => (ticks.saturating_sub(pt)) as f32 / at.duration_since(pa).as_secs_f32(),
        _ => 0.0,
    };
    let s = Sample {
        at,
        rss_kb: field("VmRSS:")?,
        anon_kb: field("RssAnon:").unwrap_or(0),
        swap_kb: field("VmSwap:").unwrap_or(0),
        threads: field("Threads:").unwrap_or(1) as u32,
        cpu_pct,
    };
    Some((s, ticks))
}

/// One extra reading taken at the moment of a stop, so the last point in the
/// ring is the state at the crash rather than up to one interval earlier. The
/// tracee is ptrace-stopped but /proc still answers. CPU is carried over from
/// the previous sample since there is no interval to compute it from.
pub fn sample_now(pid: u32, into: &Shared) {
    if let Some((mut s, _)) = read_sample(Path::new("/proc"), pid, None) {
        let mut v = into.lock().unwrap();
        s.cpu_pct = v.samples.back().map_or(0.0, |p| p.cpu_pct);
        v.push_sample(s);
    }
}

pub struct Sampler {
    stop: Arc<AtomicBool>,
}

impl Sampler {
    pub fn start(pid: u32, every: Duration, into: Shared) -> Sampler {
        Self::start_at(PathBuf::from("/proc"), pid, every, into)
    }

    pub fn start_at(proc_root: PathBuf, pid: u32, every: Duration, into: Shared) -> Sampler {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let _ = std::thread::Builder::new().name("stoat-vitals".into()).spawn(move || {
            let mut prev = None;
            while !flag.load(Ordering::Relaxed) {
                match read_sample(&proc_root, pid, prev) {
                    Some((s, ticks)) => {
                        prev = Some((ticks, s.at));
                        into.lock().unwrap().push_sample(s);
                    }
                    // Process is gone. Keep what we have.
                    None => break,
                }
                std::thread::sleep(every);
            }
        });
        Sampler { stop }
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
