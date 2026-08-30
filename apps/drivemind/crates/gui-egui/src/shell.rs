// drivemind-egui shell — a thin, honest window onto drivemind-core.
//
// Threading model: egui is immediate-mode, the engine's operations block
// (hashing a terabyte, walking a tree), so ALL engine work runs on one
// worker thread. The UI sends `Task`s down a channel; the worker runs them
// against HostRunner and forwards every core `Event` (plus results) back
// as `Msg`s. The UI drains the channel each frame. No shared mutable
// state, no async runtime — two mpsc channels and a thread.
//
// The bottom pane renders the raw Event stream through uikit's log_view.
// That is deliberate product, not debug garnish: the CLI prints the same
// events to stderr, so both frontends show the identical loop — which
// commands ran, which ladder rungs failed, which files were skipped. When
// a probe does something surprising, the user sees it instead of
// trusting a summary.

use drivemind_core::{
    advisor, cleanup, device, human_bytes, integrity, smart, video, Event, HostRunner,
};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

/// Work the UI can ask for.
pub enum Task {
    Enumerate,
    Smart(String),
    VerifyCreate { root: PathBuf, manifest: PathBuf },
    VerifyCheck { root: PathBuf, manifest: PathBuf },
    Cleanup(PathBuf),
    Videos(PathBuf),
}

/// Everything that flows back to the UI.
pub enum Msg {
    Ev(Event),
    Devices(Vec<device::BlockDevice>),
    Smart(smart::SmartReport),
    Verify(integrity::VerifyReport),
    Cleanup(Vec<cleanup::Suggestion>),
    Videos(video::VideoScan),
    Failed(String),
    Idle,
}

/// The worker loop. Runs until the task channel closes (app exit).
pub fn worker(tasks: Receiver<Task>, out: Sender<Msg>, repaint: impl Fn() + Send + 'static) {
    let sys = HostRunner;
    for task in tasks {
        let tx = out.clone();
        let mut sink = |e: Event| {
            let _ = tx.send(Msg::Ev(e));
            repaint();
        };
        let result = match task {
            Task::Enumerate => device::enumerate(&sys).map(Msg::Devices),
            Task::Smart(dev) => smart::probe(&sys, &dev, &mut sink).map(Msg::Smart),
            Task::VerifyCreate { root, manifest } => integrity::create(&root, &manifest, &mut sink)
                .map(|n| {
                    Msg::Ev(Event::Note(format!("manifest written: {n} file(s)")))
                }),
            Task::VerifyCheck { root, manifest } => {
                integrity::verify(&root, &manifest, &mut sink).map(Msg::Verify)
            }
            Task::Cleanup(root) => {
                Ok(Msg::Cleanup(cleanup::scan(&root, &cleanup::CleanupConfig::default(), &mut sink)))
            }
            Task::Videos(root) => Ok(Msg::Videos(video::scan(&sys, &root, &mut sink))),
        };
        let _ = out.send(result.unwrap_or_else(Msg::Failed));
        let _ = out.send(Msg::Idle);
        repaint();
    }
}

/// UI-side state. Pure data + a drain method; rendering lives in app().
pub struct ShellState {
    pub tasks: Sender<Task>,
    pub from_worker: Receiver<Msg>,
    pub busy: bool,

    pub devices: Vec<device::BlockDevice>,
    pub selected: Option<usize>,
    pub smart: Option<smart::SmartReport>,
    pub verify: Option<integrity::VerifyReport>,
    pub cleanup: Vec<cleanup::Suggestion>,
    pub videos: Option<video::VideoScan>,

    pub tab: usize,
    pub scan_root: String,
    pub manifest_path: String,
    pub log: Vec<uikit_egui::log_view::LogEntry>,
    log_id: u64,
    pub progress: Option<(u64, Option<u64>)>,
}

impl ShellState {
    pub fn new(tasks: Sender<Task>, from_worker: Receiver<Msg>) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let s = Self {
            tasks,
            from_worker,
            busy: false,
            devices: Vec::new(),
            selected: None,
            smart: None,
            verify: None,
            cleanup: Vec::new(),
            videos: None,
            tab: 0,
            scan_root: home.clone(),
            manifest_path: format!("{home}/drivemind-manifest.jsonl"),
            log: Vec::new(),
            log_id: 0,
            progress: None,
        };
        let _ = s.tasks.send(Task::Enumerate);
        s
    }

    pub fn send(&mut self, t: Task) {
        self.busy = true;
        let _ = self.tasks.send(t);
    }

    fn push_log(&mut self, level: uikit_egui::log_view::LogLevel, text: String) {
        self.log_id += 1;
        self.log.push(uikit_egui::log_view::LogEntry::new(level, text, self.log_id));
        // Bounded: the log is a window, not an archive.
        if self.log.len() > 500 {
            self.log.drain(..self.log.len() - 500);
        }
    }

    /// Drain worker messages. Called once per frame before rendering.
    pub fn pump(&mut self) {
        use uikit_egui::log_view::LogLevel;
        while let Ok(msg) = self.from_worker.try_recv() {
            match msg {
                Msg::Ev(Event::Exec(c)) => self.push_log(LogLevel::Info, format!("$ {c}")),
                Msg::Ev(Event::Note(n)) => self.push_log(LogLevel::Warn, n),
                Msg::Ev(Event::Phase { name, subject }) => {
                    self.progress = None;
                    self.push_log(LogLevel::Info, format!("== {name}: {subject}"));
                }
                Msg::Ev(Event::Progress { done, total }) => self.progress = Some((done, total)),
                Msg::Devices(d) => {
                    self.devices = d;
                    if self.selected.map(|i| i >= self.devices.len()).unwrap_or(true) {
                        self.selected = if self.devices.is_empty() { None } else { Some(0) };
                    }
                }
                Msg::Smart(rep) => {
                    self.push_log(
                        if rep.worst() >= smart::Severity::Warning {
                            LogLevel::Error
                        } else {
                            LogLevel::Info
                        },
                        format!("{}: {:?}", rep.device, rep.worst()),
                    );
                    self.smart = Some(rep);
                }
                Msg::Verify(rep) => {
                    self.push_log(
                        if rep.clean() { LogLevel::Info } else { LogLevel::Error },
                        format!(
                            "verify: ok {} changed {} missing {} read-errors {}",
                            rep.ok,
                            rep.changed.len(),
                            rep.missing.len(),
                            rep.read_errors.len()
                        ),
                    );
                    self.verify = Some(rep);
                }
                Msg::Cleanup(s) => {
                    let total: u64 = s.iter().map(|x| x.reclaim_bytes).sum();
                    self.push_log(
                        LogLevel::Info,
                        format!("cleanup: {} suggestion(s), {}", s.len(), human_bytes(total)),
                    );
                    self.cleanup = s;
                }
                Msg::Videos(v) => {
                    self.push_log(
                        LogLevel::Info,
                        format!(
                            "videos: {} candidate(s), est. {} recoverable",
                            v.candidates.len(),
                            human_bytes(v.est_savings_bytes())
                        ),
                    );
                    self.videos = Some(v);
                }
                Msg::Failed(e) => self.push_log(LogLevel::Error, e),
                Msg::Idle => {
                    self.busy = false;
                    self.progress = None;
                }
            }
        }
    }

    /// The advisor over whatever this session has collected so far.
    pub fn recommendations(&self) -> Vec<advisor::Recommendation> {
        let smart_slice: Vec<smart::SmartReport> = self.smart.iter().cloned().collect();
        advisor::advise(&advisor::Inputs {
            devices: &self.devices,
            smart: &smart_slice,
            verify: self.verify.as_ref(),
            cleanup: &self.cleanup,
            migration: None,
            videos: self.videos.as_ref(),
            video_batch_path: None,
        })
    }
}
