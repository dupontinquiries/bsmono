use crate::pages::{caches::CachePage, launch::LaunchPage, stack::StackPage, vitals::VitalsPage, wire::WirePage};
use crate::Cli;
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use stoat_core::backend::*;
use stoat_core::diag::Severity;
use stoat_core::gdb::{Debuginfod, GdbBackend, GdbConfig};
use stoat_core::model::RunState;
use stoat_core::session::{Change, Session};
use stoat_core::vitals::{self, Sampler, Vitals};
use stoat_core::{build, cache, discover, history};

const CSS: &str = "
.stoat-source { font-size: 0.85em; }
.stoat-diag { padding: 10px 12px; }
";

pub struct App {
    window: adw::ApplicationWindow,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    views: adw::ViewStack,
    launch: LaunchPage,
    stack: Rc<StackPage>,
    vitals_page: VitalsPage,
    caches: CachePage,
    wire: WirePage,

    session: RefCell<Session>,
    backend: RefCell<Option<GdbBackend>>,
    /// Bumped per debug run; events from an older gdb are dropped on arrival.
    generation: Cell<u64>,
    vitals: vitals::Shared,
    sampler: RefCell<Option<Sampler>>,
    root: RefCell<PathBuf>,
    history: RefCell<Vec<history::Entry>>,
    candidates: RefCell<Vec<discover::Candidate>>,
    chosen: Cell<usize>,
    artifacts: RefCell<Vec<PathBuf>>,
    plans: RefCell<Vec<(PathBuf, cache::PrunePlan)>>,
}

impl App {
    pub fn build(application: &adw::Application, cli: Rc<Cli>) {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }

        let vitals = Vitals::shared(1200, 1200);
        let launch = LaunchPage::new();
        let stack = Rc::new(StackPage::new());
        let vitals_page = VitalsPage::new(vitals.clone());
        let caches = CachePage::new();
        let wire = WirePage::new();

        let views = adw::ViewStack::new();
        views.add_titled_with_icon(&launch.root, Some("launch"), "Launch", "system-run-symbolic");
        views.add_titled_with_icon(&stack.root, Some("stack"), "Stack", "view-list-symbolic");
        views.add_titled_with_icon(&vitals_page.root, Some("vitals"), "Vitals", "power-profile-performance-symbolic");
        views.add_titled_with_icon(&caches.root, Some("caches"), "Caches", "drive-harddisk-symbolic");
        views.add_titled_with_icon(&wire.root, Some("wire"), "Wire", "utilities-terminal-symbolic");

        let title = adw::WindowTitle::new("Stoat", "no program");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        // Bottom switcher always: thumbs reach the bottom edge of a phone, and the
        // header has no room for five entries at 360 px anyway.
        let switcher = adw::ViewSwitcherBar::builder().stack(&views).reveal(true).build();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&views));
        let shell = adw::ToolbarView::new();
        shell.add_top_bar(&header);
        shell.add_bottom_bar(&switcher);
        shell.set_content(Some(&toasts));

        let window = adw::ApplicationWindow::builder().application(application).content(&shell).default_width(400).default_height(760).title("Stoat").build();
        window.set_size_request(320, 400);

        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let app = Rc::new(App {
            window,
            title,
            toasts,
            views,
            launch,
            stack,
            vitals_page,
            caches,
            wire,
            session: RefCell::new(Session::new(Some(root.clone()))),
            backend: RefCell::new(None),
            generation: Cell::new(0),
            vitals,
            sampler: RefCell::new(None),
            root: RefCell::new(root),
            history: RefCell::new(Vec::new()),
            candidates: RefCell::default(),
            chosen: Cell::new(0),
            artifacts: RefCell::default(),
            plans: RefCell::default(),
        });
        app.stack.set_state(RunState::NotStarted);
        app.wire_signals();
        app.launch.dir.set_text(&app.root.borrow().display().to_string());
        if let Some(m) = &cli.marker {
            app.launch.marker.set_text(m);
        }
        app.reload_project();
        app.refresh_caches();
        if let Some((program, args)) = &cli.program {
            let is_rust = discover::inspect(program).is_some_and(|f| f.is_rust);
            app.start_debug(program.clone(), args.clone(), Vec::new(), cli.marker.clone(), is_rust);
        }
        if let Some(page) = &cli.page {
            app.views.set_visible_child_name(page);
        }
        app.window.present();
    }

    /// Every closure holds a Weak; the Rc<App> is owned by nothing but these
    /// upgrades plus the one strong ref parked on the window below.
    fn wire_signals(self: &Rc<Self>) {
        macro_rules! on {
            ($widget:expr, $connect:ident, |$app:ident $(, $arg:ident)*| $body:block) => {{
                let weak = Rc::downgrade(self);
                $widget.$connect(move |$($arg),*| {
                    let Some($app) = weak.upgrade() else { return };
                    $body
                });
            }};
        }

        on!(self.launch.dir, connect_apply, |app, row| {
            let p = PathBuf::from(row.text().as_str());
            *app.root.borrow_mut() = p.canonicalize().unwrap_or(p);
            app.artifacts.borrow_mut().clear();
            app.reload_project();
            app.refresh_caches();
        });
        on!(self.launch.build_button, connect_clicked, |app, _b| { app.run_build(); });
        on!(self.launch.debug, connect_clicked, |app, _b| { app.debug_chosen(); });
        on!(self.launch.history_query, connect_changed, |app, row| { app.search_history(row.text().as_str()); });

        on!(self.stack.toward_caller, connect_clicked, |app, _b| { app.with_session(|s, b| s.up(b)); });
        on!(self.stack.toward_callee, connect_clicked, |app, _b| { app.with_session(|s, b| s.down(b)); });
        on!(self.stack.go, connect_clicked, |app, _b| {
            let running = app.session.borrow().state == RunState::Running;
            app.submit(if running { Request::Interrupt } else { Request::Continue });
        });
        for (button, kind) in [(&self.stack.step_over, StepKind::Over), (&self.stack.step_into, StepKind::Into), (&self.stack.step_out, StepKind::Out)] {
            on!(button, connect_clicked, |app, _b| {
                let thread = app.session.borrow().thread;
                app.submit(Request::Step { kind, thread });
            });
        }
        on!(self.stack.carousel, connect_page_changed, |app, _c, index| {
            // Equal to the selection means this is the echo of our own scroll_to.
            if app.stack.programmatic.get() || app.stack.pending.get().is_some() || app.session.borrow().selected == index as usize {
                return;
            }
            app.with_session(|s, b| s.select_frame(index as usize, b));
        });
        on!(self.stack.threads, connect_selected_notify, |app, dd| {
            if app.stack.programmatic.get() {
                return;
            }
            let id = app.session.borrow().threads.get(dd.selected() as usize).map(|t| t.id);
            if let Some(id) = id {
                app.with_session(|s, b| {
                    s.select_thread(id, b);
                    Vec::new()
                });
            }
        });

        on!(self.wire.entry, connect_activate, |app, e| {
            let line = e.text().to_string();
            if !line.trim().is_empty() {
                app.submit(Request::Raw(line));
                e.set_text("");
            }
        });

        on!(self.caches.plan, connect_clicked, |app, _b| { app.plan_prune(); });
        on!(self.caches.apply, connect_clicked, |app, _b| { app.apply_prune(); });

        // Graphs repaint on a timer while visible; cheap, and it keeps the sampler
        // thread free of any GTK knowledge.
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(250), move || {
            let Some(app) = weak.upgrade() else { return glib::ControlFlow::Break };
            if app.views.visible_child_name().as_deref() == Some("vitals") {
                app.vitals_page.refresh(&app.vitals);
            }
            glib::ControlFlow::Continue
        });

        // Keep the App alive as long as the window; drop gdb with it.
        let keep = RefCell::new(Some(self.clone()));
        self.window.connect_close_request(move |_| {
            if let Some(app) = keep.borrow_mut().take() {
                app.backend.borrow_mut().take();
                app.sampler.borrow_mut().take();
            }
            glib::Propagation::Proceed
        });
    }

    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn submit(&self, req: Request) {
        match self.backend.borrow_mut().as_mut() {
            Some(b) => drop(b.submit(req)),
            None => self.toast("No debug session. Start one from Launch."),
        }
    }

    /// Run `f` with the session and backend borrowed, then apply its changes with
    /// the borrows released: applying touches widgets, widgets emit signals, and
    /// signal handlers borrow the session again.
    fn with_session(self: &Rc<Self>, f: impl FnOnce(&mut Session, &mut dyn Backend) -> Vec<Change>) {
        let changes = {
            let mut backend = self.backend.borrow_mut();
            let Some(b) = backend.as_mut() else { return };
            f(&mut self.session.borrow_mut(), b)
        };
        self.apply(changes);
    }

    // ---- launch -----------------------------------------------------------

    fn reload_project(self: &Rc<Self>) {
        let root = self.root.borrow().clone();
        match build::plan(&root) {
            Some(p) => {
                self.launch.build_row.set_subtitle(&glib::markup_escape_text(&p.command_line()));
                self.launch.build_button.set_sensitive(true);
            }
            None => {
                self.launch.build_row.set_subtitle("no Cargo.toml, configured meson/cmake build dir, or Makefile here");
                self.launch.build_button.set_sensitive(false);
            }
        }
        let artifacts = self.artifacts.borrow().clone();
        let weak = Rc::downgrade(self);
        // Scanning stats and opens every executable in the build tree; on eMMC
        // that is long enough to stutter a swipe, so keep it off the main loop.
        glib::spawn_future_local(async move {
            let scanned = gio::spawn_blocking(move || {
                let h = history::load_default();
                let c = discover::scan(&root, &h, &artifacts);
                (h, c)
            })
            .await;
            let (Some(app), Ok((h, c))) = (weak.upgrade(), scanned) else { return };
            *app.history.borrow_mut() = h;
            *app.candidates.borrow_mut() = c;
            let w = Rc::downgrade(&app);
            app.launch.set_candidates(&app.candidates.borrow(), move |i| {
                let Some(app) = w.upgrade() else { return };
                app.chosen.set(i);
                // try_borrow: the first radio activates while set_candidates still
                // holds the list borrowed.
                let inv = app.candidates.try_borrow().ok().and_then(|c| c.get(i)?.invocations.first().cloned());
                if let Some(inv) = inv {
                    app.launch.set_invocation(&inv);
                }
            });
            let inv = app.candidates.borrow().first().and_then(|c| c.invocations.first().cloned());
            if let Some(inv) = inv {
                app.launch.set_invocation(&inv);
            }
        });
    }

    fn run_build(self: &Rc<Self>) {
        let Some(plan) = build::plan(&self.root.borrow()) else { return };
        self.launch.build_button.set_sensitive(false);
        self.launch.build_log.set_visible(true);
        self.launch.build_log.set_text(&format!("$ {}", plan.command_line()));
        let (tx, rx) = async_channel::unbounded::<Result<String, Option<build::BuildOutcome>>>();
        std::thread::spawn(move || {
            let line_tx = tx.clone();
            let out = build::run(&plan, move |l| {
                let (build::BuildLine::Out(s) | build::BuildLine::Err(s)) = l;
                let _ = line_tx.send_blocking(Ok(s));
            });
            let _ = tx.send_blocking(Err(out.ok()));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let mut tail: Vec<String> = Vec::new();
            while let Ok(msg) = rx.recv().await {
                let Some(app) = weak.upgrade() else { return };
                match msg {
                    Ok(line) => {
                        tail.push(line);
                        let from = tail.len().saturating_sub(14);
                        app.launch.build_log.set_text(&tail[from..].join("\n"));
                    }
                    Err(outcome) => {
                        app.launch.build_button.set_sensitive(true);
                        match outcome {
                            Some(o) if o.success => {
                                *app.artifacts.borrow_mut() = o.executables;
                                app.reload_project();
                            }
                            Some(_) => app.toast("Build failed; see the log above."),
                            None => app.toast("Could not start the build tool."),
                        }
                    }
                }
            }
        });
    }

    fn search_history(self: &Rc<Self>, query: &str) {
        let hits: Vec<_> = if query.trim().len() < 2 {
            Vec::new()
        } else {
            let h = self.history.borrow();
            let mut v: Vec<_> = history::grep(&h, query).into_iter().rev().take(6).map(|e| (e.cmd.clone(), history::invocation(e))).collect();
            v.dedup_by(|a, b| a.0 == b.0);
            v
        };
        let weak = Rc::downgrade(self);
        self.launch.set_history_hits(&hits, move |inv| {
            if let Some(app) = weak.upgrade() {
                app.launch.set_invocation(&inv);
                app.toast("Arguments taken from history");
            }
        });
    }

    fn debug_chosen(self: &Rc<Self>) {
        let Some(c) = self.candidates.borrow().get(self.chosen.get()).cloned() else {
            self.toast("Nothing to debug yet");
            return;
        };
        let args = history::words(self.launch.args.text().as_str());
        let env = history::words(self.launch.env.text().as_str()).into_iter().filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect();
        let marker = Some(self.launch.marker.text().to_string()).filter(|m| !m.trim().is_empty());
        self.start_debug(c.path, args, env, marker, c.is_rust);
    }

    fn start_debug(self: &Rc<Self>, program: PathBuf, args: Vec<String>, env: Vec<(String, String)>, marker: Option<String>, is_rust: bool) {
        // Dropping the old backend kills its gdb; its reader threads then close
        // the old channel, which ends the old pump below.
        self.backend.borrow_mut().take();
        self.sampler.borrow_mut().take();
        self.vitals.lock().unwrap().reset();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);

        let cfg = GdbConfig { debuginfod: if self.caches.debuginfod.is_active() { Debuginfod::On } else { Debuginfod::Off }, ..Default::default() };
        let (mut gdb, events) = match GdbBackend::spawn(&cfg) {
            Ok(x) => x,
            Err(e) => return self.toast(&format!("Could not start gdb: {e}")),
        };
        let mut session = Session::new(Some(self.root.borrow().clone()));
        self.title.set_title(&program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        gdb.submit(Request::Load { program, args, env, cwd: Some(self.root.borrow().clone()) });
        if is_rust {
            session.break_on_rust_panic(&mut gdb);
        }
        if let Some(m) = &marker {
            session.insert_frame_marker(m, &mut gdb);
        }
        gdb.submit(Request::Run);
        *self.session.borrow_mut() = session;
        *self.backend.borrow_mut() = Some(gdb);
        self.stack.set_frames(&[]);
        self.views.set_visible_child_name("stack");

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(ev) = events.recv().await {
                let Some(app) = weak.upgrade() else { return };
                if app.generation.get() != generation {
                    return;
                }
                app.with_session(|s, b| s.handle(ev, b));
            }
        });
    }

    // ---- session -> widgets ----------------------------------------------

    fn apply(self: &Rc<Self>, changes: Vec<Change>) {
        for c in changes {
            match c {
                Change::Started { pid } => {
                    if let Some(pid) = pid {
                        *self.sampler.borrow_mut() = Some(Sampler::start(pid, Duration::from_millis(100), self.vitals.clone()));
                    }
                }
                Change::State(state) => {
                    match state {
                        RunState::Stopped => {
                            if let Some(pid) = self.session.borrow().pid {
                                vitals::sample_now(pid, &self.vitals);
                            }
                            self.vitals.lock().unwrap().set_frozen(true);
                        }
                        RunState::Running => self.vitals.lock().unwrap().set_frozen(false),
                        _ => {}
                    }
                    self.title.set_subtitle(match state {
                        RunState::NotStarted => "not started",
                        RunState::Running => "running",
                        RunState::Stopped => "stopped",
                        RunState::Exited => "exited",
                    });
                    self.stack.set_state(state);
                    self.vitals_page.refresh(&self.vitals);
                }
                Change::Threads => {
                    let s = self.session.borrow();
                    let labels: Vec<String> = s.threads.iter().map(|t| format!("{} {}", t.id, t.name.as_deref().unwrap_or(""))).collect();
                    let sel = s.threads.iter().position(|t| Some(t.id) == s.thread).unwrap_or(0) as u32;
                    drop(s);
                    self.stack.set_threads(&labels, sel);
                }
                Change::Frames => {
                    let frames = self.session.borrow().frames.clone();
                    self.stack.set_frames(&frames);
                }
                Change::Selection(idx) => self.stack.select(idx),
                Change::Locals { frame } => {
                    let s = self.session.borrow();
                    let idx = s.frames.iter().position(|f| f.level == frame);
                    let vars = s.locals.get(&frame).cloned().unwrap_or_default();
                    drop(s);
                    if let Some(idx) = idx {
                        let weak = Rc::downgrade(self);
                        self.stack.set_locals(idx, &vars, move |name| {
                            if let Some(app) = weak.upgrade() {
                                app.with_session(|s, b| {
                                    s.evaluate(&name, b);
                                    Vec::new()
                                });
                            }
                        });
                    }
                }
                Change::Value { expr, value } => {
                    let idx = self.session.borrow().selected;
                    if !self.stack.set_value(idx, &expr, &value) {
                        self.wire.push("   ", &format!("{expr} = {value}"));
                    }
                }
                Change::Diagnostic(d) => {
                    let stopped = self.session.borrow().state != RunState::Running;
                    if d.code == "gdb" || (d.severity == Severity::Error && !stopped) {
                        self.toast(&d.title);
                    }
                    let extra = if stopped { vitals::summarize(&self.vitals.lock().unwrap()) } else { Vec::new() };
                    // Errors from commands (bad expression etc.) must not replace
                    // the explanation of why we are stopped.
                    let about_stop = self.session.borrow().last_stop.is_some() && !matches!(d.code.as_str(), "no-symbol" | "gdb" | "bad-address" | "optimized-out" | "not-running");
                    if about_stop {
                        self.stack.set_diagnostic(&d, &extra);
                    } else {
                        self.toast(&format!("{}{}", d.title, d.help.first().map(|h| format!(" — {h}")).unwrap_or_default()));
                    }
                    self.wire.push(" ! ", &d.render(0));
                }
                Change::FrameMark(at) => self.vitals.lock().unwrap().mark_frame(at),
                Change::Output { channel, text } => self.wire.push(
                    match channel {
                        Channel::Target => "out ",
                        Channel::Console => "gdb ",
                        Channel::Log => "log ",
                    },
                    &text,
                ),
                Change::Wire { dir, line } => self.wire.push(if dir == Dir::ToBackend { " -> " } else { " <- " }, &line),
                Change::Gone(why) => {
                    self.title.set_subtitle(&why);
                    self.stack.set_state(RunState::Exited);
                    self.sampler.borrow_mut().take();
                }
            }
        }
    }

    // ---- caches -----------------------------------------------------------

    fn refresh_caches(self: &Rc<Self>) {
        let root = self.root.borrow().clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let usage = gio::spawn_blocking(move || cache::locate(Some(&root)).into_iter().map(|(k, p)| { let u = cache::measure(&p); (k, p, u) }).collect::<Vec<_>>()).await;
            if let (Some(app), Ok(u)) = (weak.upgrade(), usage) {
                app.caches.set_usage(&u);
            }
        });
    }

    fn plan_prune(self: &Rc<Self>) {
        let root = self.root.borrow().clone();
        let age = Duration::from_secs(self.caches.days.value() as u64 * 86_400);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let plans = gio::spawn_blocking(move || cache::locate(Some(&root)).into_iter().map(|(_, p)| { let plan = cache::plan_prune(&p, age); (p, plan) }).collect::<Vec<_>>()).await;
            if let (Some(app), Ok(plans)) = (weak.upgrade(), plans) {
                app.caches.show_plans(&plans);
                *app.plans.borrow_mut() = plans;
            }
        });
    }

    fn apply_prune(self: &Rc<Self>) {
        let plans = std::mem::take(&mut *self.plans.borrow_mut());
        self.caches.apply.set_sensitive(false);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let done = gio::spawn_blocking(move || plans.iter().map(|(_, p)| cache::apply(p)).fold((0u64, 0usize), |(b, f), (freed, failed)| (b + freed, f + failed.len()))).await;
            if let (Some(app), Ok((freed, failed))) = (weak.upgrade(), done) {
                app.toast(&format!("Freed {}{}", cache::human_bytes(freed), if failed > 0 { format!(", {failed} files could not be removed") } else { String::new() }));
                app.refresh_caches();
            }
        });
    }
}
