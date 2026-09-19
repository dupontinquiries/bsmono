// stoat-egui shell state — everything the window knows, and nothing that draws.
//
// Threading, same school as drivemind-egui: egui is immediate-mode and must not
// block, so
//   * gdb's events are handled on the thread that receives them, by
//     stoat-core's Driver (session + backend + vitals glue) behind a mutex. The
//     UI never handles an event: a frame-marker stop has to be resumed NOW, not
//     at the next painted frame, or the pacing graph measures this window's
//     repaint rate. The thread forwards the resulting `Change`s and asks for a
//     repaint only when a batch is worth one.
//   * the UI draws from `Snapshot`, a copy of the session's public state taken
//     when the driver's version moves. Commands lock the driver for the length
//     of one pipe write.
//   * everything that touches disk or runs a process (scan, build, cache
//     measure/plan/apply) is a `Task` on one worker thread, answered by `Msg`s.
//   * vitals are sampled by stoat-core's own thread into a shared ring the
//     Vitals page reads.
// No async runtime, no shared mutable state beyond that ring.
//
// What the GTK shell needed and this one does not: any synchronisation between
// "the session's selected frame" and "the widget showing a frame". There it was
// a carousel with its own position, echoes of programmatic scrolls, and a race
// with the first layout. Here the page is drawn from `session.selected` every
// frame, so there is no widget-side copy to disagree. (The Snapshot is a copy,
// but a one-way one: nothing ever writes it back.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use stoat_core::backend::{Backend, Channel, Dir, Event, Request, StepKind};
use stoat_core::cache::{self, CacheKind, PrunePlan, Usage};
use stoat_core::diag::{Diagnostic, Severity};
use stoat_core::discover::{self, Candidate};
use stoat_core::gdb::{Debuginfod, GdbBackend, GdbConfig};
use stoat_core::history::{self, Entry, Invocation};
use stoat_core::driver::{self, Driver};
use stoat_core::model::{Frame, RunState, Thread, Variable};
use stoat_core::session::{Change, Session};
use stoat_core::vitals::{self, Vitals};
use stoat_core::{build, build::BuildPlan};
use uikit_egui::log_view::{LogEntry, LogLevel};

pub const PAGE_LAUNCH: usize = 0;
pub const PAGE_STACK: usize = 1;
pub const PAGE_VITALS: usize = 2;
pub const PAGE_WIRE: usize = 3;
pub const PAGE_CACHES: usize = 4;
/// Order matters on a wide window: the dashboard shows the active page and its
/// followers, so Stack is followed by the two things you read beside a stop.
pub const PAGES: [&str; 5] = ["Launch", "Stack", "Vitals", "Wire", "Caches"];

pub enum Task {
    Scan { root: PathBuf, artifacts: Vec<PathBuf> },
    Build(BuildPlan),
    MeasureCaches { root: PathBuf },
    PlanPrune { root: PathBuf, unused_for: Duration },
    ApplyPrune(Vec<(PathBuf, PrunePlan)>),
}

pub enum Msg {
    Scanned { history: Vec<Entry>, candidates: Vec<Candidate> },
    BuildLine(String),
    BuildDone(Option<build::BuildOutcome>),
    CacheUsage(Vec<(CacheKind, PathBuf, Usage)>),
    PrunePlans(Vec<(PathBuf, PrunePlan)>),
    Pruned { freed: u64, failed: usize },
}

pub type Repaint = Arc<dyn Fn() + Send + Sync>;

/// Runs until the task channel closes (app exit).
pub fn worker(tasks: Receiver<Task>, out: Sender<Msg>, repaint: Repaint) {
    for task in tasks {
        let send = |m: Msg| {
            let _ = out.send(m);
            repaint();
        };
        match task {
            Task::Scan { root, artifacts } => {
                let history = history::load_default();
                let candidates = discover::scan(&root, &history, &artifacts);
                send(Msg::Scanned { history, candidates });
            }
            Task::Build(plan) => {
                let r = build::run(&plan, |l| {
                    let (build::BuildLine::Out(s) | build::BuildLine::Err(s)) = l;
                    send(Msg::BuildLine(s));
                });
                send(Msg::BuildDone(r.ok()));
            }
            Task::MeasureCaches { root } => {
                let u = cache::locate(Some(&root)).into_iter().map(|(k, p)| (k, p.clone(), cache::measure(&p))).collect();
                send(Msg::CacheUsage(u));
            }
            Task::PlanPrune { root, unused_for } => {
                let p = cache::locate(Some(&root)).into_iter().map(|(_, p)| (p.clone(), cache::plan_prune(&p, unused_for))).collect();
                send(Msg::PrunePlans(p));
            }
            Task::ApplyPrune(plans) => {
                let (freed, failed) = plans.iter().map(|(_, p)| cache::apply(p)).fold((0, 0), |(b, f), (freed, bad)| (b + freed, f + bad.len()));
                send(Msg::Pruned { freed, failed });
            }
        }
    }
}

/// Source files read once per debug run, not once per frame.
#[derive(Default)]
pub struct SourceCache(HashMap<PathBuf, Option<Arc<Vec<String>>>>);

impl SourceCache {
    pub fn lines(&mut self, file: &Path) -> Option<Arc<Vec<String>>> {
        self.0
            .entry(file.to_path_buf())
            .or_insert_with(|| std::fs::read_to_string(file).ok().map(|s| Arc::new(s.lines().map(|l| l.replace('\t', "    ")).collect())))
            .clone()
    }
}

/// What the pages draw from. Same field names as `Session`, on purpose.
#[derive(Default, Clone)]
pub struct Snapshot {
    pub state: RunState,
    pub pid: Option<u32>,
    pub threads: Vec<Thread>,
    pub thread: Option<u32>,
    pub frames: Vec<Frame>,
    pub selected: usize,
    pub locals: HashMap<u32, Vec<Variable>>,
}

impl Snapshot {
    fn of(s: &Session) -> Self {
        Snapshot { state: s.state, pid: s.pid, threads: s.threads.clone(), thread: s.thread, frames: s.frames.clone(), selected: s.selected, locals: s.locals.clone() }
    }

    pub fn current_frame(&self) -> Option<&Frame> {
        self.frames.get(self.selected)
    }
}

pub struct ShellState {
    tasks: Sender<Task>,
    from_worker: Receiver<Msg>,
    repaint: Repaint,
    pub busy: u32,

    pub session: Snapshot,
    driver: Option<Arc<Mutex<Driver>>>,
    seen_version: u64,
    changes: Option<Receiver<(u64, Vec<Change>)>>,
    generation: u64,
    pub vitals: vitals::Shared,
    pub sources: SourceCache,

    pub page: usize,
    pub program_name: String,

    pub root: PathBuf,
    pub root_text: String,
    pub plan: Option<BuildPlan>,
    pub building: bool,
    pub build_log: Vec<String>,
    artifacts: Vec<PathBuf>,
    pub history: Vec<Entry>,
    pub candidates: Vec<Candidate>,
    pub chosen: usize,
    pub args_text: String,
    pub env_text: String,
    pub marker_text: String,
    pub history_query: String,

    /// Why we are stopped, plus the vitals summary captured at that moment.
    pub diagnostic: Option<(Diagnostic, Vec<String>)>,
    /// Aggregates the user expanded: (frame level, expression) -> value.
    pub values: HashMap<(u32, String), String>,

    pub debuginfod: bool,
    pub usage: Vec<(CacheKind, PathBuf, Usage)>,
    pub prune_days: u32,
    pub plans: Vec<(PathBuf, PrunePlan)>,
    /// Request slot: the Caches page wants the destructive step confirmed. The
    /// host asks (it owns the clock the toast needs) and calls `apply_prune`.
    pub confirm_prune: bool,

    pub log: Vec<LogEntry>,
    log_id: u64,
    pub show_wire: bool,
    pub raw_cmd: String,

    pub toasts: uikit_egui::toast::Toasts,
    /// Set by state code, which has no clock; shown by `draw`, which does.
    pub pending_toast: Option<(String, uikit_egui::toast::Severity)>,
}

impl ShellState {
    pub fn new(root: PathBuf, tasks: Sender<Task>, from_worker: Receiver<Msg>, repaint: Repaint) -> Self {
        let mut s = ShellState {
            tasks,
            from_worker,
            repaint,
            busy: 0,
            session: Snapshot::default(),
            driver: None,
            seen_version: 0,
            changes: None,
            generation: 0,
            vitals: Vitals::shared(1200, 1200),
            sources: SourceCache::default(),
            page: PAGE_LAUNCH,
            program_name: String::new(),
            root_text: root.display().to_string(),
            plan: None,
            root,
            building: false,
            build_log: Vec::new(),
            artifacts: Vec::new(),
            history: Vec::new(),
            candidates: Vec::new(),
            chosen: 0,
            args_text: String::new(),
            env_text: String::new(),
            marker_text: String::new(),
            history_query: String::new(),
            diagnostic: None,
            values: HashMap::new(),
            debuginfod: false,
            usage: Vec::new(),
            prune_days: 30,
            plans: Vec::new(),
            confirm_prune: false,
            log: Vec::new(),
            log_id: 0,
            show_wire: true,
            raw_cmd: String::new(),
            toasts: uikit_egui::toast::Toasts::new(),
            pending_toast: None,
        };
        s.reload_project();
        s
    }

    /// A state with a dead worker, for tests that only draw.
    pub fn detached(root: PathBuf) -> Self {
        let (tx, _rx) = mpsc::channel();
        let (_tx2, rx2) = mpsc::channel();
        Self::new(root, tx, rx2, Arc::new(|| {}))
    }

    fn send(&mut self, t: Task) {
        if self.tasks.send(t).is_ok() {
            self.busy += 1;
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, sev: uikit_egui::toast::Severity) {
        self.pending_toast = Some((text.into(), sev));
    }

    fn push_log(&mut self, level: LogLevel, prefix: &str, text: &str) {
        for line in text.trim_end_matches('\n').split('\n') {
            self.log_id += 1;
            self.log.push(LogEntry::new(level, format!("{prefix}{line}"), self.log_id));
        }
        // log_view lays out every row it is given, every frame; an MI session is
        // thousands of lines an hour. Keep a window.
        if self.log.len() > 600 {
            self.log.drain(..self.log.len() - 600);
        }
    }

    // ---- launch ----

    pub fn set_root(&mut self) {
        let p = PathBuf::from(self.root_text.trim());
        self.root = p.canonicalize().unwrap_or(p);
        self.artifacts.clear();
        self.reload_project();
    }

    pub fn reload_project(&mut self) {
        self.plan = build::plan(&self.root);
        self.send(Task::Scan { root: self.root.clone(), artifacts: self.artifacts.clone() });
        self.send(Task::MeasureCaches { root: self.root.clone() });
    }

    pub fn run_build(&mut self) {
        if let (Some(plan), false) = (self.plan.clone(), self.building) {
            self.building = true;
            self.build_log = vec![format!("$ {}", plan.command_line())];
            self.send(Task::Build(plan));
        }
    }

    pub fn choose(&mut self, i: usize) {
        self.chosen = i;
        if let Some(inv) = self.candidates.get(i).and_then(|c| c.invocations.first().cloned()) {
            self.adopt(&inv);
        }
    }

    pub fn adopt(&mut self, inv: &Invocation) {
        self.args_text = inv.args.iter().map(|a| if a.contains(' ') { format!("'{a}'") } else { a.clone() }).collect::<Vec<_>>().join(" ");
        self.env_text = inv.env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ");
    }

    pub fn history_hits(&self) -> Vec<(String, Option<Invocation>)> {
        if self.history_query.trim().len() < 2 {
            return Vec::new();
        }
        let mut v: Vec<_> = history::grep(&self.history, &self.history_query).into_iter().rev().take(6).map(|e| (e.cmd.clone(), history::invocation(e))).collect();
        v.dedup_by(|a, b| a.0 == b.0);
        v
    }

    pub fn debug_chosen(&mut self) {
        let Some(c) = self.candidates.get(self.chosen).cloned() else {
            return self.toast("Nothing to debug yet", uikit_egui::toast::Severity::Warning);
        };
        let args = history::words(&self.args_text);
        let env = history::words(&self.env_text).into_iter().filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect();
        let marker = Some(self.marker_text.trim().to_string()).filter(|m| !m.is_empty());
        self.start_debug(c.path, args, env, marker, c.is_rust);
    }

    pub fn start_debug(&mut self, program: PathBuf, args: Vec<String>, env: Vec<(String, String)>, marker: Option<String>, is_rust: bool) {
        let cfg = GdbConfig { debuginfod: if self.debuginfod { Debuginfod::On } else { Debuginfod::Off }, ..Default::default() };
        let (gdb, events) = match GdbBackend::spawn(&cfg) {
            Ok(x) => x,
            Err(e) => return self.toast(format!("Could not start gdb: {e}"), uikit_egui::toast::Severity::Error),
        };
        // Replacing the driver drops the old backend, which kills its gdb; its
        // reader threads then close the old channel and the old event thread
        // ends. Anything it already queued carries the old generation and is
        // dropped in pump().
        self.begin_run(Box::new(gdb), &program);
        let generation = self.generation;
        let driver = self.driver.clone().expect("just set");
        let (tx, rx) = mpsc::channel();
        let repaint = self.repaint.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = events.recv_blocking() {
                let changes = driver.lock().unwrap().handle(ev);
                let wake = driver::wants_repaint(&changes);
                if tx.send((generation, changes)).is_err() {
                    break;
                }
                if wake {
                    repaint();
                }
            }
        });
        self.changes = Some(rx);

        let root = self.root.clone();
        self.with_session(|session, b| {
            b.submit(Request::Load { program, args, env, cwd: Some(root) });
            if is_rust {
                session.break_on_rust_panic(b);
            }
            if let Some(m) = &marker {
                session.insert_frame_marker(m, b);
            }
            b.submit(Request::Run);
            Vec::new()
        });
    }

    /// Everything `start_debug` does that is not gdb. Public so a test can run a
    /// session against a recording backend and `feed` it a transcript.
    pub fn begin_run(&mut self, backend: Box<dyn Backend + Send>, program: &Path) {
        self.vitals.lock().unwrap().reset();
        self.driver = Some(Arc::new(Mutex::new(Driver::new(Session::new(Some(self.root.clone())), backend, self.vitals.clone()))));
        self.changes = None;
        self.generation += 1;
        self.seen_version = 0;
        self.session = Snapshot::default();
        self.sources = SourceCache::default();
        self.diagnostic = None;
        self.values.clear();
        self.program_name = program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.page = PAGE_STACK;
    }

    // ---- session ----

    fn with_session(&mut self, f: impl FnOnce(&mut Session, &mut dyn Backend) -> Vec<Change>) {
        let Some(d) = self.driver.clone() else {
            return self.toast("No debug session. Start one from Launch.", uikit_egui::toast::Severity::Info);
        };
        let changes = d.lock().unwrap().with(f);
        self.apply(changes);
        self.refresh();
    }

    /// Handle an event synchronously. The running app never calls this (the
    /// event thread does the equivalent); it is how a test replays a transcript.
    pub fn feed(&mut self, ev: Event) {
        if let Some(d) = self.driver.clone() {
            let changes = d.lock().unwrap().handle(ev);
            self.apply(changes);
            self.refresh();
        }
    }

    fn refresh(&mut self) {
        if let Some(d) = &self.driver {
            let d = d.lock().unwrap();
            if d.version != self.seen_version {
                self.seen_version = d.version;
                self.session = Snapshot::of(&d.session);
            }
        }
    }

    pub fn submit(&mut self, req: Request) {
        self.with_session(|_, b| {
            b.submit(req);
            Vec::new()
        });
    }

    pub fn toward_caller(&mut self) {
        self.with_session(|s, b| s.up(b));
    }

    pub fn toward_callee(&mut self) {
        self.with_session(|s, b| s.down(b));
    }

    pub fn select_frame(&mut self, idx: usize) {
        self.with_session(|s, b| s.select_frame(idx, b));
    }

    pub fn select_thread(&mut self, id: u32) {
        self.with_session(|s, b| {
            s.select_thread(id, b);
            Vec::new()
        });
    }

    pub fn evaluate(&mut self, expr: &str) {
        let expr = expr.to_string();
        self.with_session(|s, b| {
            s.evaluate(&expr, b);
            Vec::new()
        });
    }

    pub fn go_or_interrupt(&mut self) {
        let running = self.session.state == RunState::Running;
        self.submit(if running { Request::Interrupt } else { Request::Continue });
    }

    pub fn step(&mut self, kind: StepKind) {
        let thread = self.session.thread;
        self.submit(Request::Step { kind, thread });
    }

    pub fn send_raw(&mut self) {
        let line = std::mem::take(&mut self.raw_cmd);
        if !line.trim().is_empty() {
            self.submit(Request::Raw(line));
        }
    }

    // ---- caches ----

    pub fn plan_prune(&mut self) {
        self.send(Task::PlanPrune { root: self.root.clone(), unused_for: Duration::from_secs(u64::from(self.prune_days) * 86_400) });
    }

    pub fn planned(&self) -> (usize, u64) {
        self.plans.iter().fold((0, 0), |(n, b), (_, p)| (n + p.remove.len(), b + p.bytes))
    }

    pub fn apply_prune(&mut self) {
        let plans = std::mem::take(&mut self.plans);
        if !plans.is_empty() {
            self.send(Task::ApplyPrune(plans));
        }
    }

    // ---- per frame ----

    /// Drain both channels. Returns whether the target is running, so the host
    /// knows to keep repainting for the vitals graphs.
    pub fn pump(&mut self) -> bool {
        use uikit_egui::toast::Severity as T;
        while let Ok(msg) = self.from_worker.try_recv() {
            // Every task ends in exactly one message; BuildLine is the only
            // message that is not an ending.
            if !matches!(msg, Msg::BuildLine(_)) {
                self.busy = self.busy.saturating_sub(1);
            }
            match msg {
                Msg::BuildLine(l) => self.build_log.push(l),
                Msg::Scanned { history, candidates } => {
                    self.history = history;
                    self.candidates = candidates;
                    self.chosen = 0;
                    if self.args_text.is_empty() && self.env_text.is_empty() {
                        self.choose(0);
                    }
                }
                Msg::BuildDone(outcome) => {
                    self.building = false;
                    match outcome {
                        Some(o) if o.success => {
                            self.artifacts = o.executables;
                            self.reload_project();
                        }
                        Some(_) => self.toast("Build failed; see the log", T::Error),
                        None => self.toast("Could not start the build tool", T::Error),
                    }
                }
                Msg::CacheUsage(u) => self.usage = u,
                Msg::PrunePlans(p) => self.plans = p,
                Msg::Pruned { freed, failed } => {
                    let tail = if failed > 0 { format!(", {failed} files could not be removed") } else { String::new() };
                    self.toast(format!("Freed {}{tail}", cache::human_bytes(freed)), if failed > 0 { T::Warning } else { T::Success });
                    self.send(Task::MeasureCaches { root: self.root.clone() });
                }
            }
        }

        let mut batches = Vec::new();
        if let Some(rx) = &self.changes {
            while let Ok((generation, changes)) = rx.try_recv() {
                if generation == self.generation {
                    batches.push(changes);
                }
            }
        }
        for changes in batches {
            self.apply(changes);
        }
        self.refresh();
        self.session.state == RunState::Running
    }

    fn apply(&mut self, changes: Vec<Change>) {
        use uikit_egui::toast::Severity as T;
        for c in changes {
            match c {
                // Sampler, freeze/thaw and frame marks are the Driver's, done on
                // the event thread. What is left here is what only a UI cares about.
                Change::State(RunState::Running) => {
                    self.diagnostic = None;
                    self.values.clear();
                }
                Change::Started { .. } | Change::State(_) | Change::FrameMark(_) => {}
                // Drawn straight from the session every frame; nothing to mirror.
                Change::Threads | Change::Frames | Change::Selection(_) | Change::Locals { .. } => {}
                Change::Value { expr, value } => {
                    self.refresh();
                    let level = self.session.current_frame().map(|f| f.level);
                    let is_local = level.and_then(|l| self.session.locals.get(&l)).is_some_and(|vs| vs.iter().any(|v| v.name == expr));
                    match (level, is_local) {
                        (Some(l), true) => drop(self.values.insert((l, expr), value)),
                        _ => self.push_log(LogLevel::Info, "    ", &format!("{expr} = {value}")),
                    }
                }
                Change::Diagnostic(d) => {
                    self.push_log(if d.severity == Severity::Error { LogLevel::Error } else { LogLevel::Info }, "  ! ", &d.render(0));
                    // An error from a command (bad expression, not running) must not
                    // replace the explanation of why we are stopped.
                    let about_command = matches!(d.code.as_str(), "no-symbol" | "gdb" | "bad-address" | "optimized-out" | "not-running" | "no-debuginfo" | "ptrace-denied");
                    if about_command {
                        let help = d.help.first().map(|h| format!(" — {h}")).unwrap_or_default();
                        self.toast(format!("{}{help}", d.title), T::Error);
                    } else {
                        let notes = vitals::summarize(&self.vitals.lock().unwrap());
                        self.diagnostic = Some((d, notes));
                    }
                }
                Change::Output { channel, text } => {
                    let (level, prefix) = match channel {
                        Channel::Target => (LogLevel::Info, "out "),
                        Channel::Console => (LogLevel::Info, "gdb "),
                        Channel::Log => (LogLevel::Warn, "log "),
                    };
                    self.push_log(level, prefix, &text);
                }
                Change::Wire { dir, line } => {
                    if self.show_wire {
                        self.push_log(LogLevel::Debug, if dir == Dir::ToBackend { " -> " } else { " <- " }, &line);
                    }
                }
                Change::Gone(why) => self.push_log(LogLevel::Warn, "    ", &why),
            }
        }
    }

    pub fn state_label(&self) -> &'static str {
        match (self.driver.is_some(), self.session.state) {
            (false, _) => "no program",
            (_, RunState::NotStarted) => "starting",
            (_, RunState::Running) => "running",
            (_, RunState::Stopped) => "stopped",
            (_, RunState::Exited) => "exited",
        }
    }
}
