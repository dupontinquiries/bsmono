//! trainpage — watching a training run.
//!
//! Training is a long loop whose interesting output is a handful of numbers
//! that change slowly and a lot of text that scrolls past. This is the model
//! behind a page for that: what the run is doing, the recent history of the
//! loss, the log, and a summary the operator can paste into a message.
//!
//! Kept apart from the widget for the reason every model in this tree is: the
//! decisions worth getting right — when a run counts as stalled, what a
//! summary should contain, how much log to keep — are testable without a
//! screen, and a page that owns them is a page nobody can check.

use std::collections::VecDeque;

/// How much log to keep in memory.
///
/// A 20,000-epoch run prints continuously and the interesting part is always
/// the end. Keeping everything grows without bound on a machine that is
/// already holding a model and a mel cache; keeping the tail is what a reader
/// actually scrolls to.
pub const LOG_LINES: usize = 2000;

/// A point on the loss curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub epoch: u64,
    pub loss: f32,
    /// Seconds since the run started.
    pub at_secs: f64,
}

/// What the run is doing.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Idle,
    /// Decoding and caching audio before the first epoch. Reported separately
    /// because on a cold cache it is most of the wall time and a progress bar
    /// stuck at "epoch 0" looks like a hang.
    Preparing { done: usize, total: usize },
    Training,
    /// Finished normally.
    Done { epochs: u64 },
    /// Stopped by the operator or by an error.
    Stopped { reason: String },
}

/// The page's state.
#[derive(Debug, Clone)]
pub struct TrainPage {
    pub phase: Phase,
    pub epochs_target: u64,
    pub batch_size: usize,
    pub device: String,
    pub data_dir: String,
    /// Loss history, one point per reported epoch.
    pub curve: Vec<Point>,
    log: VecDeque<String>,
    /// Set when the operator asks to stop; the trainer observes it.
    pub stop_requested: bool,
    /// The mel cache, re-read periodically rather than remembered.
    pub cache: CacheView,
    /// GPU numbers, when a run reports them.
    pub gpu: GpuView,
}

impl Default for TrainPage {
    fn default() -> Self {
        TrainPage {
            phase: Phase::Idle,
            epochs_target: 0,
            batch_size: 0,
            device: "unknown".into(),
            data_dir: String::new(),
            curve: Vec::new(),
            log: VecDeque::new(),
            stop_requested: false,
            cache: Default::default(),
            gpu: Default::default(),
        }
    }
}

impl TrainPage {
    pub fn start(&mut self, epochs: u64, batch_size: usize, device: &str, data_dir: &str) {
        self.phase = Phase::Preparing { done: 0, total: 0 };
        self.epochs_target = epochs;
        self.batch_size = batch_size;
        self.device = device.to_string();
        self.data_dir = data_dir.to_string();
        self.curve.clear();
        self.log.clear();
        self.stop_requested = false;
    }

    /// Take a line of trainer output.
    ///
    /// Parses the shapes the trainer already prints rather than requiring it
    /// to emit a new structured format — the CLI and the GUI then show the
    /// same run, and a change to one cannot silently diverge from the other.
    pub fn feed(&mut self, line: &str) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line.to_string());

        if let Some(rest) = line.strip_prefix("[dataset] (") {
            if let Some((frac, _)) = rest.split_once(')') {
                if let Some((d, t)) = frac.split_once('/') {
                    if let (Ok(d), Ok(t)) = (d.trim().parse(), t.trim().parse()) {
                        self.phase = Phase::Preparing { done: d, total: t };
                    }
                }
            }
        }
        if let Some(p) = parse_epoch(line) {
            self.phase = Phase::Training;
            self.curve.push(p);
        }
        if line.starts_with("[train] device:") {
            if let Some(d) = line.split(':').nth(1) {
                self.device = d.trim().to_string();
            }
        }
    }

    pub fn finish(&mut self, epochs: u64) {
        self.phase = Phase::Done { epochs };
    }

    pub fn stop(&mut self, reason: impl Into<String>) {
        self.phase = Phase::Stopped { reason: reason.into() };
    }

    pub fn log_lines(&self) -> impl Iterator<Item = &str> {
        self.log.iter().map(String::as_str)
    }

    pub fn latest(&self) -> Option<Point> {
        self.curve.last().copied()
    }

    /// Loss improvement over the last `n` points, as a fraction.
    ///
    /// Positive means falling. Used for the stall check rather than a raw
    /// comparison of first and last, because a single noisy epoch either end
    /// would decide it.
    pub fn trend(&self, n: usize) -> Option<f32> {
        if self.curve.len() < n.max(2) {
            return None;
        }
        let tail = &self.curve[self.curve.len() - n..];
        let half = tail.len() / 2;
        let older: f32 = tail[..half].iter().map(|p| p.loss).sum::<f32>() / half as f32;
        let newer: f32 = tail[half..].iter().map(|p| p.loss).sum::<f32>()
            / (tail.len() - half) as f32;
        if older.abs() < f32::EPSILON {
            return None;
        }
        Some((older - newer) / older.abs())
    }

    /// Has the loss stopped moving?
    ///
    /// Reported, never acted on: a plateau is sometimes convergence and
    /// sometimes a dead run, and only the operator knows which. Stopping
    /// automatically would throw away a run that was about to escape a
    /// plateau, which is a thing training does.
    pub fn stalled(&self) -> bool {
        matches!(self.trend(40), Some(t) if t.abs() < 0.001)
    }

    /// Epochs per second, from the curve's own timestamps.
    pub fn rate(&self) -> Option<f64> {
        let (first, last) = (self.curve.first()?, self.curve.last()?);
        let dt = last.at_secs - first.at_secs;
        let de = last.epoch.saturating_sub(first.epoch);
        (dt > 0.0 && de > 0).then(|| de as f64 / dt)
    }

    /// Seconds until the target epoch at the current rate.
    pub fn eta_secs(&self) -> Option<f64> {
        let rate = self.rate()?;
        let done = self.latest()?.epoch;
        (self.epochs_target > done).then(|| (self.epochs_target - done) as f64 / rate)
    }

    /// The line under the progress bar.
    pub fn status_line(&self) -> String {
        match &self.phase {
            Phase::Idle => "not running".into(),
            Phase::Preparing { done, total } if *total > 0 => {
                format!("preparing clips {done}/{total}")
            }
            Phase::Preparing { .. } => "preparing clips…".into(),
            Phase::Training => match (self.latest(), self.eta_secs()) {
                (Some(p), Some(eta)) => format!(
                    "epoch {}/{} · loss {:.4} · {}",
                    p.epoch, self.epochs_target, p.loss, human_secs(eta)
                ),
                (Some(p), None) => {
                    format!("epoch {}/{} · loss {:.4}", p.epoch, self.epochs_target, p.loss)
                }
                _ => "training…".into(),
            },
            Phase::Done { epochs } => format!("done after {epochs} epochs"),
            Phase::Stopped { reason } => format!("stopped: {reason}"),
        }
    }

    /// A summary to paste into a message.
    ///
    /// Written to be read by someone who was not watching: what ran, on what,
    /// how far it got, and whether the loss was still moving. The last is the
    /// part a bare "epoch 12000, loss 0.031" leaves out, and it is the only
    /// number that says whether continuing is worth anything.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        s.push_str("beatspace training\n");
        s.push_str(&format!("  data:    {}\n", self.data_dir));
        s.push_str(&format!("  device:  {}\n", self.device));
        s.push_str(&format!("  batch:   {}\n", self.batch_size));
        s.push_str(&format!("  status:  {}\n", self.status_line()));
        if let Some(p) = self.latest() {
            s.push_str(&format!("  loss:    {:.5} at epoch {}\n", p.loss, p.epoch));
        }
        if let Some(first) = self.curve.first() {
            s.push_str(&format!("  started: loss {:.5} at epoch {}\n", first.loss, first.epoch));
        }
        match self.trend(40) {
            Some(t) if t.abs() < 0.001 => s.push_str(
                "  trend:   flat over the last 40 points — plateau or converged\n"),
            Some(t) if t > 0.0 => s.push_str(&format!(
                "  trend:   falling {:.1}% over the last 40 points\n", t * 100.0)),
            Some(t) => s.push_str(&format!(
                "  trend:   RISING {:.1}% over the last 40 points\n", -t * 100.0)),
            // Said explicitly rather than omitted: a summary missing a line is
            // read as a tool that forgot, and "too early to say" is an answer.
            None => s.push_str("  trend:   too few points to say\n"),
        }
        if let Some(r) = self.rate() {
            s.push_str(&format!("  rate:    {r:.2} epochs/sec\n"));
        }
        s
    }
}

/// Parse `[train] epoch 120/20000 loss 0.0412 (12.5s)` and near variants.
///
/// Tolerant on purpose: the trainer's line has changed shape before and a
/// dashboard that silently stops plotting is worse than one that misses a
/// field. Anything with an epoch and a loss is a point.
pub fn parse_epoch(line: &str) -> Option<Point> {
    let epoch_at = line.find("epoch ")? + 6;
    let rest = &line[epoch_at..];
    let epoch: u64 = rest
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())?
        .parse()
        .ok()?;
    let loss_at = line.find("loss ")? + 5;
    let loss: f32 = line[loss_at..]
        .split_whitespace()
        .next()?
        .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.')
        .parse()
        .ok()?;
    // The timestamp is optional — an older line has none and the point is
    // still worth plotting; only the rate and ETA need it.
    let at_secs = line
        .rfind('(')
        .and_then(|i| line[i + 1..].split(|c| c == 's' || c == ')').next())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    Some(Point { epoch, loss, at_secs })
}

/// `1h 04m`, `12m 30s`, `45s` — whichever two units matter.
pub fn human_secs(s: f64) -> String {
    let s = s.max(0.0) as u64;
    match s {
        0..=59 => format!("{s}s left"),
        60..=3599 => format!("{}m {:02}s left", s / 60, s % 60),
        _ => format!("{}h {:02}m left", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_is_reachable_and_its_buttons_do_something() {
        // The failure this session kept finding: a model built, tested, and
        // driven by nothing. Checked here rather than left for an audit.
        let gui = include_str!("gui.rs");
        assert!(gui.contains("Page::Train"), "no way to reach the page");
        assert!(gui.contains("self.train_body(ui)"), "the dashboard is never drawn");
        assert!(gui.contains("self.start_training()"), "start goes nowhere");
        assert!(gui.contains("self.train.summary()"), "copy summary goes nowhere");
        assert!(gui.contains("self.train.feed(&l)"), "trainer output reaches nothing");
    }

    #[test]
    fn stopping_asks_rather_than_kills() {
        // The trainer checks between epochs and writes a checkpoint on the way
        // out. Tearing the thread down loses everything since the last save,
        // which on a CPU run is the work of an hour.
        let gui = include_str!("gui.rs");
        assert!(gui.contains("self.train.stop_requested = true"));
        assert!(!gui.contains("child.kill()"), "the run is killed rather than asked");
    }

    fn page_with(losses: &[f32]) -> TrainPage {
        let mut p = TrainPage::default();
        p.start(20000, 32, "Cpu", "/music");
        for (i, l) in losses.iter().enumerate() {
            p.curve.push(Point { epoch: i as u64, loss: *l, at_secs: i as f64 });
        }
        p.phase = Phase::Training;
        p
    }

    #[test]
    fn a_real_trainer_line_becomes_a_point() {
        let p = parse_epoch("[train] epoch 120/20000 loss 0.0412 (12.5s)").unwrap();
        assert_eq!(p.epoch, 120);
        assert!((p.loss - 0.0412).abs() < 1e-6);
        assert!((p.at_secs - 12.5).abs() < 1e-6);
    }

    #[test]
    fn a_line_without_a_timestamp_still_plots() {
        // The trainer's format has changed before. A dashboard that silently
        // stops plotting is worse than one that misses a field — only the rate
        // and the ETA need the clock.
        let p = parse_epoch("epoch 7 loss 1.25").unwrap();
        assert_eq!(p.epoch, 7);
        assert_eq!(p.at_secs, 0.0);
        assert!(parse_epoch("[dataset] (3/900) cached \"x.mp3\"").is_none());
        assert!(parse_epoch("").is_none());
    }

    #[test]
    fn preparing_is_its_own_phase_so_epoch_zero_does_not_look_hung() {
        // On a cold cache, decoding 5,630 files is most of the wall time. A
        // bar stuck at "epoch 0" through all of it reads as a hang.
        let mut p = TrainPage::default();
        p.start(20000, 32, "Cpu", "/music");
        p.feed("[dataset] (417/5630) cached \"track.mp3\"");
        assert_eq!(p.phase, Phase::Preparing { done: 417, total: 5630 });
        assert!(p.status_line().contains("417/5630"));
        p.feed("[train] epoch 1/20000 loss 2.5 (30.0s)");
        assert_eq!(p.phase, Phase::Training);
    }

    #[test]
    fn the_log_keeps_the_tail_rather_than_everything() {
        // 20,000 epochs of output on a machine already holding a model and a
        // mel cache. The end is the part anyone scrolls to.
        let mut p = TrainPage::default();
        p.start(1, 1, "Cpu", "/m");
        for i in 0..LOG_LINES + 500 {
            p.feed(&format!("line {i}"));
        }
        let lines: Vec<&str> = p.log_lines().collect();
        assert_eq!(lines.len(), LOG_LINES);
        assert_eq!(*lines.last().unwrap(), format!("line {}", LOG_LINES + 499));
        assert!(lines[0].starts_with("line 500"), "the oldest line was not dropped");
    }

    #[test]
    fn a_flat_curve_is_reported_and_not_acted_on() {
        // A plateau is sometimes convergence and sometimes a dead run, and
        // only the operator knows which. Stopping automatically would throw
        // away a run about to escape a plateau, which training does.
        let p = page_with(&[0.5f32; 60]);
        assert!(p.stalled());
        assert!(!p.stop_requested, "the page must not stop a run by itself");
        assert!(p.summary().contains("plateau"));
    }

    #[test]
    fn a_falling_curve_reads_as_falling_and_a_rising_one_says_so() {
        let falling: Vec<f32> = (0..60).map(|i| 1.0 - i as f32 * 0.01).collect();
        let p = page_with(&falling);
        assert!(!p.stalled());
        assert!(p.trend(40).unwrap() > 0.0);
        assert!(p.summary().contains("falling"));

        // Rising is the case worth shouting about: it is the shape of a
        // diverging run, and a summary that called it "changing" would let it
        // go unnoticed.
        let rising: Vec<f32> = (0..60).map(|i| 0.1 + i as f32 * 0.01).collect();
        let q = page_with(&rising);
        assert!(q.trend(40).unwrap() < 0.0);
        assert!(q.summary().contains("RISING"), "{}", q.summary());
    }

    #[test]
    fn a_short_run_says_it_is_too_early_rather_than_omitting_the_line() {
        // A summary missing a line reads as a tool that forgot. "Too few
        // points" is an answer.
        let p = page_with(&[1.0, 0.9]);
        assert!(p.trend(40).is_none());
        assert!(p.summary().contains("too few points"));
    }

    #[test]
    fn the_eta_comes_from_the_runs_own_pace() {
        let mut p = TrainPage::default();
        p.start(1000, 32, "Cpu", "/m");
        p.curve.push(Point { epoch: 0, loss: 1.0, at_secs: 0.0 });
        p.curve.push(Point { epoch: 100, loss: 0.5, at_secs: 50.0 });
        assert!((p.rate().unwrap() - 2.0).abs() < 1e-9, "100 epochs in 50s");
        let eta = p.eta_secs().unwrap();
        assert!((eta - 450.0).abs() < 1e-6, "900 epochs left at 2/sec");
        assert!(human_secs(eta).contains("7m"), "{}", human_secs(eta));
    }

    #[test]
    fn the_summary_names_what_ran_and_where() {
        // Written for someone who was not watching: the loss alone does not
        // say which library or which device produced it.
        let p = page_with(&[1.0, 0.9, 0.8]);
        let s = p.summary();
        for want in ["/music", "Cpu", "batch:   32", "loss:"] {
            assert!(s.contains(want), "summary missing {want:?}:\n{s}");
        }
    }

    #[test]
    fn a_device_line_from_the_trainer_updates_the_page() {
        // The trainer decides the device, not the page — mirroring it here
        // rather than guessing keeps one answer.
        let mut p = TrainPage::default();
        p.start(10, 4, "unknown", "/m");
        p.feed("[train] device: Cpu");
        assert_eq!(p.device, "Cpu");
        assert!(p.summary().contains("device:  Cpu"));
    }
}

// ── what the page shows besides the loss ────────────────────────────────────

/// The mel cache on disk, as the page needs to draw it.
///
/// Read rather than inferred: the trainer's cache is a directory of `.bspc`
/// files that a previous run may have half-filled, another run may be evicting,
/// and a user may have deleted by hand. A page that displayed a remembered
/// count would be wrong the first time any of those happened.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CacheView {
    pub files: usize,
    pub bytes: u64,
    /// `cache_batches * batch_size`, or None when unbounded.
    pub budget: Option<usize>,
    /// Sources found by the recursive scan, when a run has reported it.
    pub sources: usize,
}

impl CacheView {
    /// How full the cache is against its budget, 0..1.
    ///
    /// None when unbounded, because a progress bar with no maximum is a bar
    /// that means nothing — better to show the count and no bar.
    pub fn fullness(&self) -> Option<f32> {
        let b = self.budget?;
        if b == 0 {
            return None;
        }
        Some((self.files as f32 / b as f32).min(1.0))
    }

    /// One line for the page.
    pub fn summary(&self) -> String {
        let mib = self.bytes as f64 / (1024.0 * 1024.0);
        match self.budget {
            Some(b) => format!(
                "{} of {} clips cached · {:.0} MiB{}",
                self.files, b, mib,
                if self.files >= b { " · at budget, evicting oldest" } else { "" }
            ),
            None => format!("{} clips cached · {:.0} MiB · unbounded", self.files, mib),
        }
    }

    /// Is the cache the reason a run has not started producing epochs?
    ///
    /// Answered here because it is the question an operator actually has while
    /// watching a bar not move, and the honest answer needs both numbers: a
    /// cold cache with many sources is a wait, and a warm cache is not.
    pub fn explains_a_slow_start(&self, phase: &Phase) -> bool {
        matches!(phase, Phase::Preparing { .. }) && self.sources > self.files
    }

    /// Will this library re-decode as it goes?
    ///
    /// True when the source count exceeds the budget: the cache then holds a
    /// moving window, and a random sampler reaching outside it decodes again.
    /// Worth saying BEFORE a run rather than leaving the operator to infer it
    /// from a slow epoch — the fix is a larger `cache_batches`, and it is free
    /// on a machine with disk to spare.
    ///
    /// An unbounded cache never thrashes, and neither does a library that fits.
    pub fn will_thrash(&self) -> bool {
        match self.budget {
            Some(b) => self.sources > b,
            None => false,
        }
    }

    /// One line for a panel. An alias for `summary` so the page and any other
    /// caller cannot drift into two phrasings of the same fact.
    pub fn line(&self) -> String {
        self.summary()
    }
}

/// Read the cache directory.
pub fn read_cache(dir: &std::path::Path, budget: Option<usize>) -> CacheView {
    let mut v = CacheView { budget, ..Default::default() };
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().extension().and_then(|x| x.to_str()) == Some("bspc") {
                v.files += 1;
                v.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    v
}

/// GPU numbers, when the run reports them.
///
/// All optional. This is the field where a dashboard invents a plausible figure
/// because a blank looks unfinished — and a made-up utilisation is worse than
/// no utilisation, because someone will tune against it. Nothing here is
/// derived; every value arrives from a log line or stays None.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuView {
    pub adapter: Option<String>,
    pub backend: Option<String>,
    /// Bytes of GPU buffers the run has reserved.
    pub buffer_bytes: Option<u64>,
    /// The concurrent-buffer ceiling this build assumes.
    pub buffer_ceiling: Option<u64>,
    /// Dispatches in the last step.
    pub dispatches: Option<u64>,
    /// Milliseconds the CPU spent blocked on the GPU in the last step.
    pub sync_ms: Option<f64>,
    /// Milliseconds of the last step in total.
    pub step_ms: Option<f64>,
}

impl GpuView {
    /// The share of a step spent waiting on the GPU.
    /// Take a line of trainer output and keep anything about the GPU.
    ///
    /// Parsed from the log rather than passed through a channel for the reason
    /// the whole page works this way: the CLI and the GUI then show the same
    /// run. A second path would let the panel disagree with the terminal, and
    /// the terminal is what gets pasted into a bug report.
    ///
    /// Fields stay `None` until a line supplies them, so a CPU run shows no GPU
    /// panel at all — an empty panel reads as "no GPU" and a panel of zeros
    /// reads as a broken one.
    pub fn feed_gpu(&mut self, line: &str) {
        if let Some(rest) = line.split("adapter: ").nth(1) {
            // "adapter: Apple M2 Ultra (G14D B1) (Vulkan)" — the backend is the
            // last parenthesised group, and the name itself contains
            // parentheses, so it is taken from the right.
            let rest = rest.trim();
            if let Some(open) = rest.rfind('(') {
                self.adapter = Some(rest[..open].trim().to_string());
                self.backend = Some(rest[open + 1..].trim_end_matches(')').to_string());
            } else {
                self.adapter = Some(rest.to_string());
            }
        }
        if let Some(rest) = line.split("dispatches ").nth(1) {
            if let Some(n) = rest.split_whitespace().next().and_then(|s| s.parse().ok()) {
                self.dispatches = Some(n);
            }
        }
        if let Some(rest) = line.split("sync ").nth(1) {
            // "sync 60519ms" — trimmed of the unit rather than parsed with it,
            // because the engine writes both "60519ms" and "60519 ms".
            let tok = rest.split_whitespace().next().unwrap_or("");
            if let Ok(ms) = tok.trim_end_matches("ms").trim().parse::<f64>() {
                self.sync_ms = Some(ms);
            }
        }
        if let Some(rest) = line.split("buffer budget ").nth(1) {
            let mut it = rest.split_whitespace();
            if let (Some(a), Some(b)) = (it.next(), it.nth(1)) {
                if let (Ok(u), Ok(c)) = (a.parse::<f64>(), b.parse::<f64>()) {
                    // Reported in MiB by the engine; stored in bytes so the
                    // fullness maths does not carry a unit.
                    self.buffer_bytes = Some((u * 1024.0 * 1024.0) as u64);
                    self.buffer_ceiling = Some((c * 1024.0 * 1024.0) as u64);
                }
            }
        }
    }

    pub fn sync_share(&self) -> Option<f32> {
        let (s, t) = (self.sync_ms?, self.step_ms?);
        (t > 0.0).then(|| (s / t) as f32)
    }

    /// What the numbers say, in the terms the engine's own diagnostics use.
    ///
    /// The wording is deliberate: a high sync share is NOT automatically
    /// overhead. logscope's engine records the same signature and reads it
    /// correctly — at few submits carrying real work, the wait IS the compute.
    /// A dashboard that shouted "stalled" at 90% sync would send the operator
    /// optimising a queue that was already full.
    pub fn verdict(&self) -> Option<String> {
        let share = self.sync_share()?;
        let d = self.dispatches.unwrap_or(0);
        Some(if share > 0.8 && d > 200 {
            format!("{:.0}% waiting on the GPU across {d} dispatches — many small dispatches, \
                     so the wait is probably per-dispatch overhead", share * 100.0)
        } else if share > 0.8 {
            format!("{:.0}% waiting on the GPU across {d} dispatches — few dispatches carrying \
                     real work, so the wait is the compute", share * 100.0)
        } else {
            format!("{:.0}% waiting on the GPU", share * 100.0)
        })
    }

    /// Buffer use against the ceiling, 0..1.
    pub fn buffer_fullness(&self) -> Option<f32> {
        let (used, cap) = (self.buffer_bytes?, self.buffer_ceiling?);
        (cap > 0).then(|| (used as f64 / cap as f64) as f32)
    }
}

impl TrainPage {
    /// Parse the GPU lines the engine and the trainer print.
    ///
    /// Tolerant, and it fills nothing it did not read. A parser that defaulted a
    /// missing field to zero would show 0% utilisation on a working run.
    pub fn feed_gpu(&mut self, line: &str) {
        if let Some(rest) = line.strip_prefix("adapter: ") {
            // "Apple M2 Ultra (G14D B1) (Vulkan)" — the backend is the last
            // parenthesised group, and the name itself contains parentheses,
            // which is why this splits from the right.
            if let Some(open) = rest.rfind('(') {
                self.gpu.adapter = Some(rest[..open].trim().to_string());
                self.gpu.backend =
                    Some(rest[open + 1..].trim_end_matches(')').trim().to_string());
            } else {
                self.gpu.adapter = Some(rest.trim().to_string());
            }
        }
        if let Some(v) = after_key(line, "dispatches ") {
            self.gpu.dispatches = v.parse().ok();
        }
        if let Some(v) = after_key(line, "sync ") {
            self.gpu.sync_ms = v.trim_end_matches("ms").parse().ok();
        }
        if let Some(v) = after_key(line, "step ") {
            self.gpu.step_ms = v.trim_end_matches("ms").parse().ok();
        }
    }
}

fn after_key(line: &str, key: &str) -> Option<String> {
    let i = line.find(key)? + key.len();
    Some(line[i..].split_whitespace().next()?.to_string())
}

#[cfg(test)]
mod view_tests {
    use super::*;

    #[test]
    fn an_unbounded_cache_shows_no_bar() {
        // A progress bar with no maximum means nothing. Better to show the
        // count and omit the bar than to draw one against an invented ceiling.
        let v = CacheView { files: 40, bytes: 1 << 20, budget: None, sources: 0 };
        assert!(v.fullness().is_none());
        assert!(v.summary().contains("unbounded"));
    }

    #[test]
    fn a_full_cache_says_it_is_evicting() {
        // The operator seeing files stop growing needs to know that is the
        // policy working rather than the run stalling.
        let v = CacheView { files: 128, bytes: 0, budget: Some(128), sources: 0 };
        assert_eq!(v.fullness(), Some(1.0));
        assert!(v.summary().contains("evicting"));
        // And it cannot exceed 1: a cache the user over-filled by hand should
        // not draw a bar past its own end.
        let over = CacheView { files: 500, budget: Some(128), ..Default::default() };
        assert_eq!(over.fullness(), Some(1.0));
    }

    #[test]
    fn a_cold_cache_explains_a_slow_start_and_a_warm_one_does_not() {
        // The question an operator has while watching a bar not move.
        let cold = CacheView { files: 10, sources: 5630, budget: Some(128), bytes: 0 };
        assert!(cold.explains_a_slow_start(&Phase::Preparing { done: 10, total: 5630 }));
        let warm = CacheView { files: 5630, sources: 5630, budget: None, bytes: 0 };
        assert!(!warm.explains_a_slow_start(&Phase::Preparing { done: 0, total: 0 }));
        // And it never explains a slow TRAINING phase — that is a different
        // problem and pointing at the cache would misdirect.
        assert!(!cold.explains_a_slow_start(&Phase::Training));
    }

    #[test]
    fn an_adapter_name_with_parentheses_still_parses() {
        // "Apple M2 Ultra (G14D B1) (Vulkan)" — the name itself is
        // parenthesised, so splitting on the FIRST bracket gives the backend as
        // "G14D B1) (Vulkan" and the name as "Apple M2 Ultra".
        let mut p = TrainPage::default();
        p.feed_gpu("adapter: Apple M2 Ultra (G14D B1) (Vulkan)");
        assert_eq!(p.gpu.adapter.as_deref(), Some("Apple M2 Ultra (G14D B1)"));
        assert_eq!(p.gpu.backend.as_deref(), Some("Vulkan"));
    }

    #[test]
    fn nothing_unread_is_invented() {
        // The field where a dashboard shows a plausible zero because a blank
        // looks unfinished. A made-up utilisation is worse than none, because
        // someone tunes against it.
        let p = TrainPage::default();
        assert!(p.gpu.sync_share().is_none());
        assert!(p.gpu.verdict().is_none());
        assert!(p.gpu.buffer_fullness().is_none());
        assert!(p.gpu.dispatches.is_none());
    }

    #[test]
    fn a_high_sync_share_is_not_called_a_stall_without_the_dispatch_count() {
        // logscope's engine reads this signature correctly: at few submits
        // carrying real work, the wait IS the compute. Shouting "stalled" at
        // 90% sync sends the operator optimising a queue that is already full.
        let busy = GpuView {
            sync_ms: Some(900.0), step_ms: Some(1000.0), dispatches: Some(12),
            ..Default::default()
        };
        let v = busy.verdict().unwrap();
        assert!(v.contains("the wait is the compute"), "{v}");

        let chatty = GpuView {
            sync_ms: Some(900.0), step_ms: Some(1000.0), dispatches: Some(30000),
            ..Default::default()
        };
        let v = chatty.verdict().unwrap();
        assert!(v.contains("per-dispatch overhead"), "{v}");
    }

    #[test]
    fn the_cache_is_read_from_disk_rather_than_remembered() {
        // A previous run may have half-filled it, another may be evicting, and
        // a user may have deleted it by hand. A remembered count is wrong the
        // first time any of those happens.
        let dir = std::env::temp_dir().join(format!("cv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..3 {
            std::fs::write(dir.join(format!("c{i}.bspc")), vec![0u8; 100]).unwrap();
        }
        std::fs::write(dir.join("notes.txt"), b"ignored").unwrap();
        let v = read_cache(&dir, Some(8));
        assert_eq!(v.files, 3, "non-cache files were counted");
        assert_eq!(v.bytes, 300);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
