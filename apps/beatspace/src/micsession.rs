//! micsession — write kept crops where the trainer can already read them.
//!
//! [`crate::micsample`] decides WHICH moments to keep. This puts them on disk,
//! and the whole design question is where.
//!
//! ## Not the cache directory
//!
//! `dataset.rs` caches decoded audio as `.bspc` in `cfg.cache_dir` — the same
//! format these crops are — so dropping them there looks obvious and is a
//! data-loss trap. `trim_cache` DELETES every `.bspc` in that directory that is
//! not in the run's live set, by design, because it is a cache. A day of
//! captured sound would survive until the next training run and then vanish.
//!
//! ## One file per crop, not one per session
//!
//! A session file holding N crops back-to-back would be smaller and is wrong.
//! `Dataset::random_crop` takes a window at any offset in a clip, so a window
//! straddling two concatenated crops would blend two unrelated sounds into one
//! training sample — a door slam fading into a kettle, presented to the model
//! as a thing that happens.
//!
//! Writing each crop as its own file of exactly `time_frames` frames removes
//! the possibility: `max_start` is then 0 and the only window is the whole
//! clip. Costs an inode per crop — 2 000 files of 32 KiB for a busy day, which
//! is nothing — and buys the guarantee.
//!
//! ## What the trainer needs
//!
//! Nothing new. These are ordinary `.bspc` files in a source directory, and
//! `dataset.rs` accepts a `.bspc` found among the sources as an already-built
//! clip. So `--data-dir` at a session directory trains on a day of captured
//! sound with no decode step, no new format, and no change to the training
//! loop.
//!
//! ## Dated directories, deliberately
//!
//! A session writes to `{root}/{stamp}/`, not into an existing library. The
//! extra step of pointing the trainer at a day, or moving it, is the audit
//! point — and for a microphone that ran unattended in a room with people in
//! it, an audit point is worth more than the convenience it costs.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Magic bytes of the cache format `dataset.rs` reads. Duplicated rather than
/// imported because `dataset.rs` keeps it private; the round-trip test below
/// is what actually holds the two in agreement.
const MAGIC: &[u8; 4] = b"BSPC";

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// The wall-clock limit was reached.
    Deadline,
    /// The crop budget is spent.
    Budget,
    /// Someone wrote the STOP file.
    Asked,
}

/// When an unattended microphone stops listening.
///
/// Pure, and clock-INJECTED: `should_stop` takes elapsed seconds rather than
/// reading a clock, so a twelve-hour session is a unit test. The shell passes
/// `start.elapsed().as_secs_f64()`.
///
/// ## Why a wall-clock limit exists at all
///
/// The crop budget already bounds the OUTPUT, so a duration limit is redundant
/// for disk. It is not redundant for consent. "Stops when it has collected
/// enough" and "stops when I said so" are different promises, and only the
/// second is one you can make to somebody else in the room. A budget-only
/// session in a quiet house runs until something interesting happens, which
/// might be the conversation you have when you get home.
///
/// So the default is a limit rather than none, and `None` has to be asked for.
#[derive(Debug, Clone)]
pub struct StopPolicy {
    /// Hard wall-clock limit. `None` means "until budget or STOP", which is
    /// deliberately not the default.
    pub max_seconds: Option<f64>,
    /// Stop after this many crops.
    pub budget: usize,
}

impl Default for StopPolicy {
    fn default() -> Self {
        // Eight hours: a working day out of the house, and short enough that
        // forgetting the mic is on costs one day rather than indefinitely.
        StopPolicy { max_seconds: Some(8.0 * 3600.0), budget: 4096 }
    }
}

impl StopPolicy {
    /// `Some(reason)` when the session should end now.
    ///
    /// Deadline is checked FIRST. When a session hits both at once the honest
    /// report is the time limit — that is the promise that was made to the
    /// room, and reporting "budget" would imply it would have kept going.
    pub fn should_stop(&self, elapsed_s: f64, kept: usize, asked: bool) -> Option<Stopped> {
        if let Some(max) = self.max_seconds {
            if elapsed_s >= max {
                return Some(Stopped::Deadline);
            }
        }
        if asked {
            return Some(Stopped::Asked);
        }
        if kept >= self.budget {
            return Some(Stopped::Budget);
        }
        None
    }
}

pub struct Session {
    dir: PathBuf,
    n_mels: usize,
    time_frames: usize,
    written: usize,
}

impl Session {
    /// Create `{root}/{stamp}/`. `stamp` is the caller's — usually a local
    /// timestamp, but a test passes something fixed.
    pub fn create(root: &Path, stamp: &str, n_mels: usize, time_frames: usize) -> Result<Session> {
        let dir = root.join(stamp);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating session dir {}", dir.display()))?;
        Ok(Session { dir, n_mels, time_frames, written: 0 })
    }

    pub fn dir(&self) -> &Path { &self.dir }
    pub fn written(&self) -> usize { self.written }

    /// Write one crop. `crop` is `time_frames * n_mels`, row-major, oldest
    /// frame first — exactly what [`crate::micsample::Sampler::crop`] returns.
    pub fn write_crop(&mut self, crop: &[f32]) -> Result<PathBuf> {
        anyhow::ensure!(
            crop.len() == self.time_frames * self.n_mels,
            "crop is {} values, expected {} ({} frames x {} mels)",
            crop.len(), self.time_frames * self.n_mels, self.time_frames, self.n_mels
        );
        // Zero-padded so a directory listing is in capture order, which is the
        // order someone auditing a day wants to walk it in.
        let path = self.dir.join(format!("crop-{:06}.bspc", self.written));
        let mut f = std::fs::File::create(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        f.write_all(MAGIC)?;
        f.write_all(&(self.n_mels as u32).to_le_bytes())?;
        f.write_all(&(self.time_frames as u32).to_le_bytes())?;
        let mut bytes = Vec::with_capacity(crop.len() * 4);
        for v in crop {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        f.write_all(&bytes)?;
        f.sync_all().ok(); // best effort: a killed session should keep its crops
        self.written += 1;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("beatspace-micsession-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_crop_round_trips_through_the_bspc_header() {
        let root = tmp("roundtrip");
        let (n_mels, tf) = (8usize, 4usize);
        let mut s = Session::create(&root, "20260823-0100", n_mels, tf).unwrap();
        let crop: Vec<f32> = (0..tf * n_mels).map(|i| i as f32 * 0.5).collect();
        let p = s.write_crop(&crop).unwrap();

        let raw = std::fs::read(&p).unwrap();
        assert_eq!(&raw[..4], MAGIC, "magic must match what dataset.rs reads");
        assert_eq!(u32::from_le_bytes(raw[4..8].try_into().unwrap()), n_mels as u32);
        assert_eq!(u32::from_le_bytes(raw[8..12].try_into().unwrap()), tf as u32);
        assert_eq!(raw.len(), 12 + tf * n_mels * 4, "header is 12 bytes, then f32 data");

        let back: Vec<f32> = raw[12..]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(back, crop, "values must survive the round trip in order");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The property the one-file-per-crop decision exists for.
    #[test]
    fn every_file_is_exactly_one_crop_so_no_window_can_straddle_two_sounds() {
        let root = tmp("nostraddle");
        let (n_mels, tf) = (8usize, 4usize);
        let mut s = Session::create(&root, "day", n_mels, tf).unwrap();
        for k in 0..3 {
            s.write_crop(&vec![k as f32; tf * n_mels]).unwrap();
        }
        for e in std::fs::read_dir(s.dir()).unwrap().flatten() {
            let raw = std::fs::read(e.path()).unwrap();
            let t = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as usize;
            assert_eq!(t, tf,
                "a clip longer than time_frames lets random_crop take a window \
spanning two captured sounds");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_wrong_sized_crop_is_refused_rather_than_written_short() {
        let root = tmp("badsize");
        let mut s = Session::create(&root, "day", 8, 4).unwrap();
        let err = s.write_crop(&vec![0.0; 7]).unwrap_err().to_string();
        assert!(err.contains("expected 32"), "the error must name both sizes: {err}");
        assert_eq!(s.written(), 0);
        assert_eq!(std::fs::read_dir(s.dir()).unwrap().count(), 0,
                   "a refused crop must leave no partial file");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn names_sort_in_capture_order() {
        let root = tmp("order");
        let mut s = Session::create(&root, "day", 4, 2).unwrap();
        for _ in 0..12 { s.write_crop(&vec![0.0; 8]).unwrap(); }
        let mut names: Vec<String> = std::fs::read_dir(s.dir()).unwrap().flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names.first().unwrap(), "crop-000000.bspc");
        assert_eq!(names.last().unwrap(), "crop-000011.bspc",
                   "zero-padding must keep lexical order equal to capture order");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_session_is_its_own_dated_directory() {
        let root = tmp("dated");
        let a = Session::create(&root, "20260823-0100", 4, 2).unwrap();
        let b = Session::create(&root, "20260824-0100", 4, 2).unwrap();
        assert_ne!(a.dir(), b.dir(), "two days must not share a directory");
        assert!(a.dir().starts_with(&root));
        std::fs::remove_dir_all(&root).ok();
    }

    /// The claim the whole design rests on: a captured session trains.
    ///
    /// Every other test here checks one side. This drives the real chain —
    /// `micsample::Sampler` decides, `Session` writes, and the REAL
    /// `Dataset::build_or_load` reads it back as training data — because
    /// "writes a valid header" and "the trainer can use this" are different
    /// claims, and only the second one is the feature.
    #[test]
    fn a_captured_session_is_readable_as_a_dataset() {
        use crate::config::Config;
        use crate::micsample::{Decision, Gate, Sampler};

        let root = tmp("e2e");
        let (n_mels, tf) = (32usize, 16usize);

        // Capture: a quiet room with four events in disjoint bands.
        let gate = Gate { time_frames: tf, refractory: tf, flux_window: 80, ..Gate::default() };
        let mut sampler = Sampler::new(gate, n_mels);
        let mut session = Session::create(&root, "20260823-e2e", n_mels, tf).unwrap();

        let quiet = vec![-80.0f32; n_mels];
        for _ in 0..120 { sampler.push(&quiet); }
        for (lo, hi) in [(0usize, 6usize), (8, 14), (16, 22), (24, 31)] {
            for age in 0..10 {
                let decay = (-(age as f32) * 0.4).exp();
                let f: Vec<f32> = (0..n_mels)
                    .map(|m| if m >= lo && m < hi { -80.0 + 70.0 * decay } else { -80.0 })
                    .collect();
                if sampler.push(&f) == Decision::Keep {
                    session.write_crop(&sampler.crop()).unwrap();
                }
            }
            for _ in 0..60 {
                if sampler.push(&quiet) == Decision::Keep {
                    session.write_crop(&sampler.crop()).unwrap();
                }
            }
        }
        assert!(session.written() >= 3, "capture kept only {}", session.written());

        // Train-side: point a Config at the session directory and load it.
        let cfg = Config {
            n_mels,
            time_frames: tf,
            data_dir: session.dir().to_path_buf(),
            cache_dir: root.join("cache"),
            ..Config::default()
        };
        let ds = crate::dataset::build_or_load(&cfg)
            .expect("the trainer must read a capture directory with no decode step");
        assert_eq!(ds.len(), session.written(),
                   "every written crop must become a clip the trainer can sample");

        // And a real batch comes out, the shape the trainer feeds the model.
        let dev = candle_core::Device::Cpu;
        let batch = ds.sample_batch(2, &dev).expect("a batch from captured audio");
        assert_eq!(batch.dims(), &[2, 1, n_mels, tf],
                   "a batch must be (B, 1, n_mels, time_frames)");
        let flat = batch.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert!(flat.iter().any(|v| *v > -79.0),
                "every value is floor silence — the crops did not survive the round trip");

        // The capture directory must be untouched by the cache machinery: these
        // files exist nowhere else.
        let still = std::fs::read_dir(session.dir()).unwrap().count();
        assert_eq!(still, session.written(),
                   "build_or_load removed or added files in the CAPTURE directory");

        std::fs::remove_dir_all(&root).ok();
    }


    #[test]
    fn a_session_stops_at_its_deadline_even_with_budget_left() {
        let p = StopPolicy { max_seconds: Some(100.0), budget: 1000 };
        assert_eq!(p.should_stop(99.9, 0, false), None);
        assert_eq!(p.should_stop(100.0, 0, false), Some(Stopped::Deadline));
    }

    #[test]
    fn the_budget_stops_it_before_the_deadline_when_the_room_is_busy() {
        let p = StopPolicy { max_seconds: Some(1e9), budget: 5 };
        assert_eq!(p.should_stop(10.0, 4, false), None);
        assert_eq!(p.should_stop(10.0, 5, false), Some(Stopped::Budget));
    }

    #[test]
    fn asking_stops_it_immediately() {
        let p = StopPolicy::default();
        assert_eq!(p.should_stop(1.0, 0, true), Some(Stopped::Asked));
    }

    /// When both fire, the reported reason is the DEADLINE.
    ///
    /// Not cosmetic. Reporting "budget" for a session that also hit its time
    /// limit implies it would have carried on listening, which is the opposite
    /// of the promise the limit represents.
    #[test]
    fn the_deadline_is_reported_when_both_conditions_are_met() {
        let p = StopPolicy { max_seconds: Some(60.0), budget: 5 };
        assert_eq!(p.should_stop(60.0, 500, false), Some(Stopped::Deadline));
    }

    #[test]
    fn the_default_is_bounded_rather_than_open_ended() {
        // A default of "listen until you have enough" is a different promise
        // to the people in the room. If this ever becomes None by default it
        // should be a decision someone made on purpose.
        let d = StopPolicy::default();
        assert!(d.max_seconds.is_some(), "the default session must be time-bounded");
        assert!(d.max_seconds.unwrap() <= 12.0 * 3600.0, "and bounded to a day at most");
    }

    #[test]
    fn an_unbounded_session_is_possible_but_must_be_asked_for() {
        let p = StopPolicy { max_seconds: None, budget: 2 };
        assert_eq!(p.should_stop(1e9, 0, false), None, "no deadline means no deadline");
        assert_eq!(p.should_stop(1e9, 2, false), Some(Stopped::Budget),
                   "the budget is still the backstop");
    }

}
