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
