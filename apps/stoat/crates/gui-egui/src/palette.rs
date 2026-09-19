// palette — everything stoat can do, one search away, by keyboard, thumb or
// voice.
//
// From the kit, not re-derived: `palette_flow` (Ctrl+P opens typed search; an
// idle pause settles into voice), `command_bar` (bar vs pane by the space
// available, ghost completion, clamped cursor), `cmdstrip` (tap-or-hold on a
// button that a phone can reach, and a strip of recents), `fuzzy` (ranking).
//
// From stoat-core: `command`. A row, a typed line and an utterance all become a
// `Command`, and one function runs it.
//
// The part that is this app's own decision is `heard`: speech runs navigation
// and inspection as heard, and NEVER runs anything that resumes the program
// without a confirmation, because a misheard "continue" throws away a stop that
// cannot be got back. The heard command is put in the box, selected, and waits
// for Enter or a tap.

use crate::shell::{ShellState, PAGES};
use egui::{Align2, Color32, Key, RichText};
use stoat_core::command::{self, Command};
use stoat_core::history::Invocation;
use stoat_core::model::RunState;
use uikit_egui::cmdstrip::{Cmd, Hold, Strip};
use uikit_egui::command_bar::{self, Mode as Surface};
use uikit_egui::palette_flow::{Mode, PaletteFlow, Transition};
use uikit_egui::{text_field, PaletteColors};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Session(Command),
    Page(usize),
    Frame(usize),
    Candidate(usize),
    History(Invocation),
    ToggleStrip,
    ToggleWire,
    Rescan,
    RefreshProcs,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Stable, and for session commands re-parsable: `c:<typed form>`.
    pub id: String,
    pub label: String,
    pub detail: String,
    /// Extra words the search should hit.
    keywords: String,
    pub action: Action,
}

impl Item {
    fn new(id: impl Into<String>, label: impl Into<String>, detail: impl Into<String>, keywords: &str, action: Action) -> Item {
        Item { id: id.into(), label: label.into(), detail: detail.into(), keywords: keywords.into(), action }
    }

    fn session(cmd: Command, detail: &str, keywords: &str) -> Item {
        Item::new(format!("c:{}", cmd.typed()), cmd.label(), detail, keywords, Action::Session(cmd))
    }
}

/// What speech did. Returned so a test can assert the rule without a microphone.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    Ran(Command),
    NeedsConfirm(Command),
    Searching,
}

pub struct Palette {
    pub flow: PaletteFlow,
    pub query: String,
    pub cursor: usize,
    pub strip: Strip,
    pub hold: Hold,
    /// The last utterance, shown in the surface so a wrong hearing is visible.
    pub heard_text: Option<String>,
    wants_focus: bool,
    /// The click that opened the palette is, on that same frame, a click outside
    /// the palette. Without this it opened and closed in one frame (seen on
    /// screen: a tap on ⌘ did nothing while a hold worked).
    just_opened: bool,
    #[cfg(feature = "voice")]
    voice: uikit_egui::voice_popup::VoicePopup,
}

impl Default for Palette {
    fn default() -> Self {
        let mut strip = Strip::new(3);
        for c in [Command::Step(stoat_core::backend::StepKind::Over), Command::Continue] {
            strip.pin(Cmd::new(format!("c:{}", c.typed()), c.typed()));
        }
        Palette {
            flow: PaletteFlow::new(),
            query: String::new(),
            cursor: 0,
            strip,
            hold: Hold::new(),
            heard_text: None,
            wants_focus: false,
            just_opened: false,
            #[cfg(feature = "voice")]
            voice: Default::default(),
        }
    }
}

/// Was this build given a microphone path at all.
pub const HAS_VOICE: bool = cfg!(feature = "voice");

impl Palette {
    pub fn is_open(&self) -> bool {
        self.flow.mode() != Mode::Off
    }

    pub fn open(&mut self) {
        if !self.is_open() {
            self.flow.ctrl_p_tapped();
            self.query.clear();
            self.cursor = 0;
            self.heard_text = None;
            self.wants_focus = true;
            self.just_opened = true;
        }
    }

    pub fn close(&mut self) {
        if self.flow.force_off() == Transition::Closed(Mode::Voice) {
            self.voice_cancel();
        }
    }

    /// Open already listening (the hold on the ⌘ button). Without the `voice`
    /// feature this is just `open`.
    pub fn open_listening(&mut self) {
        self.open();
        if HAS_VOICE {
            let dwell = self.flow.dwell_ms();
            if self.flow.tick(dwell) == Transition::SwitchedToVoice && !self.voice_start() {
                self.flow.voice_refused();
            }
        }
    }

    /// Everything offered for `query`, best first, capped to what the surface shows.
    pub fn items(query: &str, st: &ShellState) -> Vec<Item> {
        let q = query.trim();
        let stopped = st.session.state == RunState::Stopped;
        let running = st.session.state == RunState::Running;
        let live = stopped || running;
        let mut pool = Vec::new();

        for (cmd, kw) in command::catalog() {
            let available = match &cmd {
                Command::Interrupt => running,
                Command::Run => live || st.session.state == RunState::Exited,
                Command::Build => st.plan.is_some() && !st.building,
                _ => stopped,
            };
            if available {
                pool.push(Item::session(cmd, "", kw));
            }
        }
        if stopped {
            for (i, f) in st.session.frames.iter().enumerate().filter(|(i, _)| *i != st.session.selected) {
                pool.push(Item::new(format!("frame:{i}"), format!("#{} {}", f.level, f.func.as_deref().unwrap_or("??")), f.location(), "frame stack", Action::Frame(i)));
            }
        }
        for (i, c) in st.candidates.iter().enumerate().take(8) {
            let name = c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            pool.push(Item::new(format!("debug:{name}"), format!("debug {name}"), format!("score {} · {}", c.score(), c.path.display()), "launch run load start", Action::Candidate(i)));
        }
        for (i, p) in PAGES.iter().enumerate().filter(|(i, _)| *i != st.page) {
            pool.push(Item::new(format!("page:{i}"), format!("go to {p}"), "", "page tab show open", Action::Page(i)));
        }
        pool.push(Item::new("toggle:strip", "recents strip on/off", "a row of what you just used, under the title", "command bar recent", Action::ToggleStrip));
        pool.push(Item::new("toggle:wire", if st.show_wire { "stop logging MI traffic" } else { "log MI traffic" }, "", "wire gdb protocol", Action::ToggleWire));
        pool.push(Item::new("rescan", "rescan project", "binaries, shell history, caches", "refresh reload", Action::Rescan));
        pool.push(Item::new("procs", "refresh process list", "for attach", "attach ps", Action::RefreshProcs));

        if q.is_empty() {
            pool.truncate(command_bar::MAX_ROWS);
            return pool;
        }

        // Only worth searching once there is something to search for.
        for p in st.procs.iter().take(60) {
            pool.push(Item::session(Command::Attach(p.pid), &p.cmdline, &format!("{} process pid", p.name)));
        }
        if q.len() >= 2 {
            for e in stoat_core::history::grep(&st.history, q).into_iter().rev().take(3) {
                if let Some(inv) = stoat_core::history::invocation(e) {
                    pool.push(Item::new(format!("hist:{}", e.cmd), format!("use args from: {}", e.cmd), "shell history", "", Action::History(inv)));
                }
            }
        }

        let mut scored: Vec<(i32, Item)> = pool
            .into_iter()
            .filter_map(|it| {
                let on_label = uikit_egui::fuzzy::score(q, &it.label);
                // A keyword or detail hit counts, but less than the label saying it.
                let elsewhere = uikit_egui::fuzzy::score(q, &format!("{} {}", it.keywords, it.detail)).map(|s| s / 2);
                on_label.max(elsewhere).map(|s| (s, it))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        let mut out: Vec<Item> = scored.into_iter().map(|(_, it)| it).collect();
        // A line that parses as a command is what the person typed it for.
        if let Some(cmd) = command::parse(q) {
            let id = format!("c:{}", cmd.typed());
            out.retain(|it| it.id != id);
            out.insert(0, Item::session(cmd, "typed command", ""));
        }
        out.truncate(command_bar::MAX_ROWS);
        out
    }

    pub fn run(&mut self, item: &Item, st: &mut ShellState) {
        // The strip shows the short typed form; a label like "up (toward the
        // caller)" does not fit a thumb-sized button.
        let short = match &item.action {
            Action::Session(c) => c.typed(),
            _ => item.label.clone(),
        };
        self.strip.used(Cmd::new(item.id.clone(), short));
        match &item.action {
            Action::Session(c) => st.execute(c),
            Action::Page(i) => st.page = *i,
            Action::Frame(i) => st.select_frame(*i),
            Action::Candidate(i) => {
                st.choose(*i);
                st.debug_chosen();
            }
            Action::History(inv) => {
                st.adopt(inv);
                st.page = crate::shell::PAGE_LAUNCH;
            }
            Action::ToggleStrip => drop(self.strip.toggle()),
            Action::ToggleWire => st.show_wire = !st.show_wire,
            Action::Rescan => st.reload_project(),
            Action::RefreshProcs => st.refresh_procs(),
        }
        self.close();
    }

    /// Re-run a strip entry by id.
    pub fn run_id(&mut self, id: &str, st: &mut ShellState) {
        let item = match id.strip_prefix("c:").and_then(command::parse) {
            Some(cmd) => Some(Item::session(cmd, "", "")),
            None => Self::items("", st).into_iter().chain(Self::items(id.split(':').next_back().unwrap_or(""), st)).find(|it| it.id == id),
        };
        match item {
            Some(it) => self.run(&it, st),
            None => st.toast("That is not available right now", uikit_egui::toast::Severity::Info),
        }
    }

    /// An utterance arrived. See the module docs for the rule.
    pub fn heard(&mut self, text: &str, st: &mut ShellState) -> Heard {
        self.heard_text = Some(text.to_string());
        let names: Vec<String> = st.session.current_frame().and_then(|f| st.session.locals.get(&f.level)).into_iter().flatten().map(|v| v.name.clone()).collect();
        match command::interpret_spoken(text, &names) {
            Some(cmd) if cmd.resumes_target() => {
                self.query = cmd.typed();
                self.cursor = 0;
                self.wants_focus = true;
                self.flow.on_type();
                Heard::NeedsConfirm(cmd)
            }
            Some(cmd) => {
                let item = Item::session(cmd.clone(), "", "");
                self.run(&item, st);
                st.toast(format!("heard: {}", cmd.label()), uikit_egui::toast::Severity::Info);
                Heard::Ran(cmd)
            }
            None => {
                // Not a command: search with it, so a wrong hearing is on screen
                // and nothing has happened. The words people wrap a request in are
                // dropped, or "show me the vitals page" would match nothing.
                const WRAPPING: [&str; 12] = ["show", "me", "the", "page", "go", "to", "open", "please", "switch", "tab", "a", "can"];
                let cleaned: String = text.chars().map(|ch| if ch.is_alphanumeric() || ch == '_' || ch == '-' { ch } else { ' ' }).collect();
                self.query = cleaned.split_whitespace().filter(|w| !WRAPPING.contains(&w.to_lowercase().as_str())).collect::<Vec<_>>().join(" ");
                self.cursor = 0;
                self.wants_focus = true;
                self.flow.on_type();
                Heard::Searching
            }
        }
    }

    #[cfg(feature = "voice")]
    fn voice_start(&mut self) -> bool {
        self.voice = Default::default();
        self.voice.start(false)
    }

    #[cfg(not(feature = "voice"))]
    fn voice_start(&mut self) -> bool {
        false
    }

    #[cfg(feature = "voice")]
    fn voice_cancel(&mut self) {
        self.voice.cancel();
    }

    #[cfg(not(feature = "voice"))]
    fn voice_cancel(&mut self) {}

    /// Poll the recogniser; `Some(text)` once, when an utterance is ready.
    #[cfg(feature = "voice")]
    fn voice_poll(&mut self, st: &mut ShellState) -> Option<String> {
        use uikit_egui::voice_popup::Stage;
        if self.voice.tick() {
            // Nothing was said. Back to typing rather than closing: the palette
            // was opened on purpose.
            self.flow.on_type();
            return None;
        }
        match std::mem::replace(&mut self.voice.stage, Stage::Listening) {
            Stage::Done { text } => Some(text),
            Stage::Error { message } => {
                st.toast(message, uikit_egui::toast::Severity::Error);
                self.flow.voice_refused();
                None
            }
            other => {
                self.voice.stage = other;
                None
            }
        }
    }

    #[cfg(not(feature = "voice"))]
    fn voice_poll(&mut self, _st: &mut ShellState) -> Option<String> {
        None
    }
}

/// Keys, timers and the surface. Call once a frame, after the page is drawn.
pub fn ui(ctx: &egui::Context, st: &mut ShellState, pal: &mut Palette, c: &PaletteColors) {
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::P)) {
        if pal.is_open() {
            pal.close();
        } else {
            pal.open();
        }
    }
    if !pal.is_open() {
        return;
    }

    // ---- time: the idle dwell toward voice ----
    let dt_ms = ctx.input(|i| i.stable_dt) * 1000.0;
    if pal.flow.tick(dt_ms) == Transition::SwitchedToVoice && !pal.voice_start() {
        // Built without a microphone path (or password mode): stay in typed
        // search and do not keep trying every dwell.
        pal.flow.voice_refused();
    }
    if pal.flow.mode() == Mode::Voice {
        if let Some(text) = pal.voice_poll(st) {
            pal.heard(&text, st);
        }
        ctx.request_repaint();
    } else if HAS_VOICE {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    if !pal.is_open() {
        return;
    }

    // ---- keys ----
    let items = Palette::items(&pal.query, st);
    pal.cursor = pal.cursor.min(items.len().saturating_sub(1));
    let (esc, up, down, tab, enter) = ctx.input_mut(|i| {
        (i.consume_key(egui::Modifiers::NONE, Key::Escape), i.consume_key(egui::Modifiers::NONE, Key::ArrowUp), i.consume_key(egui::Modifiers::NONE, Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, Key::Tab), i.key_pressed(Key::Enter))
    });
    if esc {
        return pal.close();
    }
    if up || down {
        pal.cursor = command_bar::move_cursor(pal.cursor, items.len(), if down { 1 } else { -1 });
        pal.flow.on_navigate();
    }
    if tab {
        if let Some(best) = items.first() {
            pal.query = command_bar::accept(&pal.query, &best.label);
            pal.flow.on_type();
        }
    }
    let mut chosen: Option<usize> = (enter && !items.is_empty()).then_some(pal.cursor);

    // ---- surface ----
    let screen = ctx.content_rect();
    let surface = command_bar::mode_for(screen.height(), items.len());
    let width = (screen.width() - 24.0).min(520.0);
    if surface == Surface::Pane {
        let scrim = Color32::from_black_alpha((command_bar::SCRIM_PANE * 255.0) as u8);
        ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("stoat-palette-scrim"))).rect_filled(screen, 0.0, scrim);
    }
    let (anchor, offset) = match surface {
        Surface::Pane => (Align2::CENTER_TOP, egui::vec2(0.0, 48.0)),
        Surface::Bar => (Align2::CENTER_BOTTOM, egui::vec2(0.0, -10.0)),
    };
    let area = egui::Area::new(egui::Id::new("stoat-palette")).order(egui::Order::Foreground).anchor(anchor, offset).show(ctx, |ui| {
        egui::Frame::new().fill(c.bg_raised).stroke(egui::Stroke::new(1.0, c.border)).corner_radius(10.0).inner_margin(10.0).show(ui, |ui| {
            ui.set_width(width);
            let cursor = pal.cursor;
            let rows = |ui: &mut egui::Ui, chosen: &mut Option<usize>| {
                for (i, it) in items.iter().enumerate() {
                    let mut job = egui::text::LayoutJob::default();
                    // One line, elided. A frame's detail is a path as long as
                    // /rustc/<40 hex>/library/std/src/panicking.rs; wrapped, each row
                    // grew to three lines and a "bar" sized for 34pt rows covered
                    // the whole phone (seen on screen, not in any test).
                    // (`.truncate()` on the Button below is what does it: the widget
                    // overrides a LayoutJob's own wrap settings with its wrap mode,
                    // so setting max_rows here changed nothing.)
                    job.append(&it.label, 0.0, egui::TextFormat { font_id: egui::FontId::proportional(14.0), color: c.text, ..Default::default() });
                    if !it.detail.is_empty() {
                        job.append(&it.detail, 10.0, egui::TextFormat { font_id: egui::FontId::proportional(11.0), color: c.text_weak, ..Default::default() });
                    }
                    if ui.add_sized([width, command_bar::ROW_H], egui::Button::selectable(i == cursor, job).truncate()).clicked() {
                        *chosen = Some(i);
                    }
                }
            };
            let status = |ui: &mut egui::Ui, pal: &Palette| {
                if let Some(h) = &pal.heard_text {
                    ui.label(RichText::new(format!("heard: “{h}” — Enter or tap to run it")).small().color(c.warning));
                }
                match pal.flow.mode() {
                    Mode::Voice => drop(ui.label(RichText::new("listening… type to cancel").small().color(c.accent))),
                    _ if HAS_VOICE => {
                        let p = pal.flow.dwell_progress();
                        let (r, _) = ui.allocate_exact_size(egui::vec2(width, 3.0), egui::Sense::hover());
                        ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(r.width() * p, 3.0)), 1.5, c.accent);
                    }
                    _ => {}
                }
            };
            let input = |ui: &mut egui::Ui, pal: &mut Palette| {
                let hint = if HAS_VOICE { "command, frame, binary, pid… or pause to speak" } else { "command, frame, binary, pid…  (p expr · b loc · ! gdb)" };
                let r = ui.add(text_field::edit("palette-query", &mut pal.query).hint_text(hint).desired_width(width));
                if std::mem::take(&mut pal.wants_focus) {
                    r.request_focus();
                }
                if r.changed() {
                    pal.cursor = 0;
                    pal.heard_text = None;
                    if pal.flow.on_type() == Transition::SwitchedToCommand {
                        pal.voice_cancel();
                    }
                }
                if let Some(rest) = items.first().and_then(|b| command_bar::ghost(&pal.query, &b.label)) {
                    ui.label(RichText::new(format!("tab → {}{rest}", pal.query)).small().color(c.text_weak));
                }
            };
            #[cfg(feature = "voice")]
            if pal.flow.mode() == Mode::Voice {
                pal.voice.draw(ui, c);
            }
            // The bar sits on the bottom edge, so its input goes last: that is
            // where the thumb and the on-screen keyboard are.
            match surface {
                Surface::Pane => {
                    input(ui, pal);
                    status(ui, pal);
                    rows(ui, &mut chosen);
                }
                Surface::Bar => {
                    rows(ui, &mut chosen);
                    status(ui, pal);
                    input(ui, pal);
                }
            }
        });
    });

    let fresh = std::mem::take(&mut pal.just_opened);
    if let Some(i) = chosen {
        if let Some(it) = items.get(i).cloned() {
            pal.run(&it, st);
        }
    } else if !fresh && ctx.input(|i| i.pointer.any_click()) && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| !area.response.rect.contains(p)) {
        pal.close();
    }
}
