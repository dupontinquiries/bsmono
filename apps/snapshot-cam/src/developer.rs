//! developer — the preview's develop pass, off the UI thread.
//!
//! `develop::render` at preview size is ~15–20 ms on a desktop in release
//! and several times that on a phone's little cores: on the UI thread that
//! is the frame budget gone. So it runs here, one worker, LATEST WINS — the
//! same rule as capture-kit's slot: a job submitted while one is running
//! replaces the waiting one, never queues behind it, so a slow develop
//! drops frames instead of making the preview lag further and further
//! behind the camera.
//!
//! The shutter does NOT come through here: it renders at native size on the
//! UI thread, once, because a photo must be exactly the frame the shutter
//! was pressed on.

use crate::develop::{self, Analysis, Params};
use capture_kit::RawFrame;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

/// One preview frame to develop.
pub struct Job {
    pub frame: RawFrame,
    pub params: Params,
    pub zebra: bool,
    pub peaking: bool,
    /// Also render this (small, neutral) for autocrop / auto-WB.
    pub measure: Option<Params>,
}

/// A developed preview frame.
pub struct Done {
    pub seq: u64,
    pub params: Params,
    pub rgba: Vec<u8>,
    pub analysis: Analysis,
    /// The measurement render, `(pixels, w, h)`.
    pub measured: Option<(Vec<u8>, u32, u32)>,
}

struct Slot { job: Option<Job>, stop: bool }

pub struct Developer {
    slot: Arc<(Mutex<Slot>, Condvar)>,
    rx: Receiver<Done>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Developer {
    pub fn start() -> Developer {
        let slot = Arc::new((Mutex::new(Slot { job: None, stop: false }), Condvar::new()));
        let (tx, rx): (Sender<Done>, Receiver<Done>) = channel();
        let s = slot.clone();
        let thread = std::thread::Builder::new().name("develop".into()).spawn(move || loop {
            let job = {
                let (m, cv) = &*s;
                let mut g = m.lock().unwrap();
                while g.job.is_none() && !g.stop { g = cv.wait(g).unwrap(); }
                if g.stop { return; }
                g.job.take().unwrap()
            };
            let f = &job.frame;
            let rgba = develop::render(&f.rgba, f.width, f.height, &job.params);
            let analysis = develop::analyse(&rgba, job.params.out_w, job.params.out_h, job.zebra, job.peaking);
            let measured = job.measure.map(|m| (develop::render(&f.rgba, f.width, f.height, &m), m.out_w, m.out_h));
            if tx.send(Done { seq: f.seq, params: job.params, rgba, analysis, measured }).is_err() { return; }
        }).expect("spawn the develop thread");
        Developer { slot, rx, thread: Some(thread) }
    }

    /// Hand over a frame; a job still waiting is replaced.
    pub fn submit(&self, job: Job) {
        let (m, cv) = &*self.slot;
        m.lock().unwrap().job = Some(job);
        cv.notify_one();
    }

    /// The newest finished frame, if any (older ones are dropped).
    pub fn latest(&self) -> Option<Done> { self.rx.try_iter().last() }
}

impl Drop for Developer {
    fn drop(&mut self) {
        { let (m, cv) = &*self.slot; m.lock().unwrap().stop = true; cv.notify_all(); }
        if let Some(t) = self.thread.take() { let _ = t.join(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn frame(seq: u64, v: u8) -> RawFrame {
        RawFrame { width: 32, height: 18, captured_ms: 0, seq, rgba: vec![v; 32 * 18 * 4].into() }
    }

    #[test]
    fn a_job_comes_back_developed_and_measured() {
        let d = Developer::start();
        let p = Params { gains: [2.0; 3], ..Params::identity(16, 9) };
        d.submit(Job { frame: frame(7, 100), params: p, zebra: true, peaking: false, measure: Some(Params::identity(8, 4)) });
        let deadline = Instant::now() + Duration::from_secs(3);
        let done = loop {
            if let Some(x) = d.latest() { break x; }
            assert!(Instant::now() < deadline, "nothing came back");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!((done.seq, done.rgba.len()), (7, 16 * 9 * 4));
        assert!(done.rgba[0] > 100, "the gain was applied");
        assert!(done.analysis.overlay.is_some());
        assert_eq!(done.measured.as_ref().map(|m| (m.1, m.2)), Some((8, 4)));
    }

    #[test]
    fn latest_wins_and_nothing_queues_behind_a_slow_frame() {
        let d = Developer::start();
        for seq in 0..50 { d.submit(Job { frame: frame(seq, 50), params: Params::identity(32, 18), zebra: false, peaking: false, measure: None }); }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut last = None;
        while Instant::now() < deadline {
            if let Some(x) = d.latest() { last = Some(x.seq); if x.seq == 49 { break; } }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(last, Some(49), "the newest frame was developed");
        drop(d); // joins without hanging
    }
}
//! developer — the preview's develop pass, off the UI thread.
//!
//! `develop::render` at preview size is ~15–20 ms on a desktop in release
//! and several times that on a phone's little cores: on the UI thread that
//! is the frame budget gone. So it runs here, one worker, LATEST WINS — the
//! same rule as capture-kit's slot: a job submitted while one is running
//! replaces the waiting one, never queues behind it, so a slow develop
//! drops frames instead of making the preview lag further and further
//! behind the camera.
//!
//! The shutter does NOT come through here: it renders at native size on the
//! UI thread, once, because a photo must be exactly the frame the shutter
//! was pressed on.

use crate::develop::{self, Analysis, Params};
use capture_kit::RawFrame;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

/// One preview frame to develop.
pub struct Job {
    pub frame: RawFrame,
    pub params: Params,
    pub zebra: bool,
    pub peaking: bool,
    /// Also render this (small, neutral) for autocrop / auto-WB.
    pub measure: Option<Params>,
}

/// A developed preview frame.
pub struct Done {
    pub seq: u64,
    pub params: Params,
    pub rgba: Vec<u8>,
    pub analysis: Analysis,
    /// The measurement render, `(pixels, w, h)`.
    pub measured: Option<(Vec<u8>, u32, u32)>,
}

struct Slot { job: Option<Job>, stop: bool }

pub struct Developer {
    slot: Arc<(Mutex<Slot>, Condvar)>,
    rx: Receiver<Done>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Developer {
    pub fn start() -> Developer {
        let slot = Arc::new((Mutex::new(Slot { job: None, stop: false }), Condvar::new()));
        let (tx, rx): (Sender<Done>, Receiver<Done>) = channel();
        let s = slot.clone();
        let thread = std::thread::Builder::new().name("develop".into()).spawn(move || loop {
            let job = {
                let (m, cv) = &*s;
                let mut g = m.lock().unwrap();
                while g.job.is_none() && !g.stop { g = cv.wait(g).unwrap(); }
                if g.stop { return; }
                g.job.take().unwrap()
            };
            let f = &job.frame;
            let rgba = develop::render(&f.rgba, f.width, f.height, &job.params);
            let analysis = develop::analyse(&rgba, job.params.out_w, job.params.out_h, job.zebra, job.peaking);
            let measured = job.measure.map(|m| (develop::render(&f.rgba, f.width, f.height, &m), m.out_w, m.out_h));
            if tx.send(Done { seq: f.seq, params: job.params, rgba, analysis, measured }).is_err() { return; }
        }).expect("spawn the develop thread");
        Developer { slot, rx, thread: Some(thread) }
    }

    /// Hand over a frame; a job still waiting is replaced.
    pub fn submit(&self, job: Job) {
        let (m, cv) = &*self.slot;
        m.lock().unwrap().job = Some(job);
        cv.notify_one();
    }

    /// The newest finished frame, if any (older ones are dropped).
    pub fn latest(&self) -> Option<Done> { self.rx.try_iter().last() }
}

impl Drop for Developer {
    fn drop(&mut self) {
        { let (m, cv) = &*self.slot; m.lock().unwrap().stop = true; cv.notify_all(); }
        if let Some(t) = self.thread.take() { let _ = t.join(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn frame(seq: u64, v: u8) -> RawFrame {
        RawFrame { width: 32, height: 18, captured_ms: 0, seq, rgba: vec![v; 32 * 18 * 4].into() }
    }

    #[test]
    fn a_job_comes_back_developed_and_measured() {
        let d = Developer::start();
        let p = Params { gains: [2.0; 3], ..Params::identity(16, 9) };
        d.submit(Job { frame: frame(7, 100), params: p, zebra: true, peaking: false, measure: Some(Params::identity(8, 4)) });
        let deadline = Instant::now() + Duration::from_secs(3);
        let done = loop {
            if let Some(x) = d.latest() { break x; }
            assert!(Instant::now() < deadline, "nothing came back");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!((done.seq, done.rgba.len()), (7, 16 * 9 * 4));
        assert!(done.rgba[0] > 100, "the gain was applied");
        assert!(done.analysis.overlay.is_some());
        assert_eq!(done.measured.as_ref().map(|m| (m.1, m.2)), Some((8, 4)));
    }

    #[test]
    fn latest_wins_and_nothing_queues_behind_a_slow_frame() {
        let d = Developer::start();
        for seq in 0..50 { d.submit(Job { frame: frame(seq, 50), params: Params::identity(32, 18), zebra: false, peaking: false, measure: None }); }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut last = None;
        while Instant::now() < deadline {
            if let Some(x) = d.latest() { last = Some(x.seq); if x.seq == 49 { break; } }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(last, Some(49), "the newest frame was developed");
        drop(d); // joins without hanging
    }
}
