//! voiceterm_egui.rs — the deployable egui voiceterm app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Defaults to the Voice palette
//! (terminal orange/green) matching voiceterm's Qt/QML identity.
//!
//! Assembles the real app shell — session sidebar, terminal with its accent border, the mic FAB +
//! its action compass docked at the corner, and the bottom keycap bar — ported from actual
//! screenshots of the Qt app, not just QML reading (see shell.rs's doc comment for what was
//! deliberately NOT ported: the screenshots used a selectable "Mocha" theme, not this port's
//! baseline Voice identity, and the settings panel's full multi-theme picker is a separate,
//! bigger feature left for later).
//!
//! Run:  cargo run --bin voiceterm-egui --features app   (from apps/voiceterm/egui-frontend)

use eframe::egui;
use voiceterm_egui::action_compass::ActionCompass;
use voiceterm_egui::gesture_menus::{brace_bytes, brace_menu, brace_slide_menu, link_dispatch, link_menu, pane_dispatch, pane_menu, pipe_char, pipe_menu, pipe_slide_menu, tab_dispatch, tab_menu, tab_slide_menu, BlockAction, LinkAction, PaneAction};
use voiceterm_egui::shell::{blocks_drawer_ui, fab_rect, keycap_bar_ui, load_prefs, mic_fab_ui, overflow_counts, paint_terminal_border, save_prefs, settings_panel, tab_row, theme_menu, Session, Sidebar, SidebarAction, Workspace, TERMINAL_KEYCAPS};
use voiceterm_egui::slide_menu::SlideOutcome;
use uikit_egui::PaletteColors;
use voiceterm_egui::App;

/// What the central panel is showing.
///
/// The terminal is the default and always one tap away — voiceterm is a
/// terminal first, and a switcher that made it a peer of the mail reader would
/// be the wrong app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CentralView { Terminal, Mail, Browser, Graph, Table, Docs, Archive }

impl VoicetermApp {
    /// The connections graph over what this app actually holds.
    ///
    /// Bookmarks become Pages and mail threads become ChatThreads, because
    /// those are the two data systems with real content here. The other four
    /// node kinds are left unproduced rather than faked — a graph of invented
    /// nodes looks like a working feature and tells you nothing true.
    fn connections_nodes(&self)
        -> (Vec<voiceterm_egui::connections_graph::Node>,
            Vec<voiceterm_egui::connections_graph::Edge>) {
        use voiceterm_egui::connections_graph as cg;
        let mut nodes = Vec::new();
        for b in self.browser.bookmarks.all() {
            nodes.push(cg::Node {
                id: cg::node_id(cg::NodeKind::Page, &b.url),
                kind: cg::NodeKind::Page,
                title: b.title.clone(),
                subtitle: b.url.clone(),
            });
        }
        for t in &self.mail.threads {
            nodes.push(cg::Node {
                id: cg::node_id(cg::NodeKind::ChatThread, &t.conversation_id),
                kind: cg::NodeKind::ChatThread,
                title: t.subject.clone(),
                subtitle: t.participants.join(", "),
            });
        }

        // SameDomain is the one edge this data can honestly produce: two
        // bookmarks on one host are related, weakly. The others need a link
        // the app does not record — a recording tagged with the page it was
        // captured on, a chat that cites a URL — so producing them would be
        // asserting a connection nobody established.
        let mut edges = Vec::new();
        let pages: Vec<(&str, String)> = self.browser.bookmarks.all().iter()
            .map(|b| (b.url.as_str(), host_of(&b.url)))
            .collect();
        for (i, (ua, ha)) in pages.iter().enumerate() {
            for (ub, hb) in pages.iter().skip(i + 1) {
                if !ha.is_empty() && ha == hb {
                    edges.push(cg::Edge {
                        from: cg::node_id(cg::NodeKind::Page, ua),
                        to: cg::node_id(cg::NodeKind::Page, ub),
                        reason: cg::EdgeReason::SameDomain,
                    });
                }
            }
        }
        (nodes, cg::prune(edges))
    }
}

/// The host part of a URL, or empty when there is not one.
///
/// Deliberately not a URL parser: this only needs the authority, and pulling in
/// a crate to find one `/` would be the tail wagging the dog.
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split('/').next().unwrap_or("").to_lowercase()
}

/// How many files a snapshot of `root` would capture.
///
/// Walks with `snapshot_policy`'s rules rather than counting everything: the
/// skip list is the difference between snapshotting a source tree and
/// snapshotting node_modules, and `should_descend` refuses a directory by NAME
/// before its contents are ever read — which is what makes the walk cheap
/// enough to run at command-submit time.
fn snapshot_scope(root: &str) -> usize {
    use voiceterm_egui::snapshot_policy::{decide, should_descend, Candidate, Capture};
    fn walk(dir: &std::path::Path, base: &std::path::Path, depth: usize, taken: &mut usize) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let path = e.path();
            let Ok(rel) = path.strip_prefix(base) else { continue };
            let rel_s = rel.to_string_lossy().to_string();
            let name = e.file_name();
            let name_s = name.to_string_lossy().to_string();
            let meta = match e.metadata() { Ok(m) => m, Err(_) => continue };
            if meta.is_dir() {
                if should_descend(&name_s, depth) {
                    walk(&path, base, depth + 1, taken);
                }
                continue;
            }
            let c = Candidate {
                rel: &rel_s,
                size: meta.len(),
                depth,
                is_symlink: meta.file_type().is_symlink(),
                // to_string_lossy would have replaced invalid bytes, so a
                // round-trip mismatch is exactly the non-UTF-8 case the policy
                // rejects.
                name_is_utf8: name.to_str().is_some(),
            };
            match decide(&c, *taken) {
                Capture::Skipped(_) => {}
                _ => *taken += 1,
            }
        }
    }
    let base = std::path::Path::new(root);
    let mut taken = 0;
    walk(base, base, 0, &mut taken);
    taken
}

/// Build a `Doc` from bundled markdown.
///
/// The view's contract is "already-converted HTML — the host owns the markdown
/// and the conversion", so the conversion lives here. Deliberately minimal:
/// headings, paragraphs and code fences, because a full markdown renderer is a
/// dependency and these documents are the project's own, written to be read as
/// plain text first.
fn doc(name: &str, title: &str, md: &str) -> voiceterm_egui::manpage_view::Doc {
    let mut html = String::with_capacity(md.len() + 256);
    let mut in_code = false;
    for line in md.lines() {
        if let Some(rest) = line.strip_prefix("```") {
            // Fences toggle rather than nest: a stray closing fence in prose
            // would otherwise swallow the remainder of the document.
            let _ = rest;
            html.push_str(if in_code { "</pre>" } else { "<pre>" });
            in_code = !in_code;
            continue;
        }
        if in_code {
            html.push_str(&escape(line));
            html.push('\n');
            continue;
        }
        let t = line.trim_end();
        if let Some(h) = t.strip_prefix("### ") {
            html.push_str(&format!("<h3>{}</h3>", escape(h)));
        } else if let Some(h) = t.strip_prefix("## ") {
            html.push_str(&format!("<h2>{}</h2>", escape(h)));
        } else if let Some(h) = t.strip_prefix("# ") {
            html.push_str(&format!("<h1>{}</h1>", escape(h)));
        } else if t.is_empty() {
            html.push_str("<br>");
        } else {
            html.push_str(&format!("<p>{}</p>", escape(t)));
        }
    }
    // An unclosed fence at EOF: close it, or the reader renders the rest of the
    // document inside a <pre> that never ends.
    if in_code { html.push_str("</pre>"); }
    voiceterm_egui::manpage_view::Doc {
        name: name.to_string(), title: title.to_string(), html,
    }
}

/// Escape the three characters that would otherwise inject markup.
///
/// These documents are ours, but they contain shell snippets and XML examples —
/// an unescaped `<` in a code fence silently eats everything to the next `>`.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl CentralView {
    // The array LENGTH is part of the type: adding a variant without widening
    // this is a compile error rather than a tab that silently never renders,
    // which is the failure mode a Vec here would have had.
    const ALL: [CentralView; 7] = [CentralView::Terminal, CentralView::Mail,
                                   CentralView::Browser, CentralView::Graph,
                                   CentralView::Table, CentralView::Docs,
                                   CentralView::Archive];
    fn label(self) -> &'static str {
        match self {
            CentralView::Terminal => "Terminal", CentralView::Mail => "Mail",
            CentralView::Browser => "Web", CentralView::Graph => "Graph",
            CentralView::Table => "Table", CentralView::Docs => "Docs",
            CentralView::Archive => "Archive",
        }
    }
}

struct VoicetermApp {
    app: App,
    /// The accelerometer, if this device has one. `None` on a desktop, which is
    /// not a failure — discovery returning None is how the feature stays absent
    /// rather than broken.
    accel: Option<voiceterm_egui::iio_accel::IioAccel>,
    /// Flick detection relative to the phone's CURRENT resting attitude, so a
    /// flick means the same thing lying on a desk as held upright.
    flicks: voiceterm_egui::iio_accel::RelativeFlicks,
    /// Last orientation seen, for the rotation hint.
    orientation: voiceterm_egui::iio_accel::OrientationUp,
    /// Progress bars spotted in terminal output. The engine
    /// (`dumb_summary::detect_progress_bars`) has always been able to find
    /// them; nothing showed you that it had — which is the whole gap the
    /// detect module names.
    progress: voiceterm_egui::detect::ProgressReadout,
    /// Table view state: the op stack, the command line, the status. The
    /// ENGINE (table_ops + table_nlp) was already here and is not
    /// re-implemented — what was missing is the surface that owns a stack and
    /// lets you take one back.
    table: voiceterm_egui::table_view::TableView,
    /// The table being transformed. Demo data until a command's output is
    /// routed here; the view is generic over where the rows came from.
    table_headers: Vec<String>,
    table_rows: Vec<Vec<String>>,
    /// The bundled-document reader. The view takes documents rather than
    /// fetching them — its own header explains why the engine could not be
    /// included here — so the HOST owns which docs exist and the conversion.
    docs: voiceterm_egui::manpage_view::ManpageView,
    /// The spoken mail phrase, parsed by the selector grammar.
    mail_phrase: String,
    /// Bytes destined for a PTY: the encoded control keys plus any submitted
    /// line. The demo has no PTY, so they accumulate here rather than being
    /// dropped — which is what was happening before, and why Ctrl-C did
    /// nothing while ordinary typing worked.
    pty_out: Vec<u8>,
    sidebar: Sidebar,
    /// Touch chrome or voice chrome. On a phone the tab sidebar and the button
    /// rows spend scarce width and height on affordances a voice user has
    /// stopped touching, and a terminal converts that space directly into rows
    /// of output.
    chrome: voiceterm_egui::chrome_mode::Chrome,
    compass: ActionCompass,
    colors: PaletteColors,
    /// The toolbar gesture menus (QML `_pipePopup` / `_bracePopup`): press-drag-release on the
    /// `|⋯` / `{⋯}` chips at the left of the keycap bar.
    pipe_menu: voiceterm_egui::slide_menu::SlideMenu,
    brace_menu: voiceterm_egui::slide_menu::SlideMenu,
    /// The pane long-press marking menu (QML `paneActionPopup`, v0.1.838): hold the terminal to
    /// open it right there; mic at dead center so a plain release is push-to-talk.
    pane_menu: voiceterm_egui::slide_menu::SlideMenu,
    /// The blue-link tap menu (QML `linkMenuModel`, v0.1.538): tap a URL in the output → 2×2
    /// intent grid at the tap point. The tapped URL rides here while the menu is open (the QML
    /// context argument).
    link_menu: voiceterm_egui::slide_menu::SlideMenu,
    link_url: Option<String>,
    /// The workspace tab sidebar (QML vTabBar, v0.1.796): click activates, HOLD a tab opens the
    /// per-tab action menu (v0.1.790) with the tab's name in the title band.
    workspace: Workspace,
    /// The active fleet palette ("long-press 🎨 to show all vibes"); Voice is voiceterm's
    /// baseline identity.
    theme: snapclone_theme::PaletteId,
    theme_menu: voiceterm_egui::slide_menu::SlideMenu,
    /// Command blocks (QML blocksDrawer): tracked from what the demo feeds; `blocks_open`
    /// toggles the left-edge drawer.
    blocks: uikit_term::blocks::BlockTracker,
    blocks_open: bool,
    /// One-shot scroll target for the tab bar ("N more" pill taps apply next frame).
    tabbar_scroll_to: Option<f32>,
    /// The QML toast, ported: fire-and-forget status pill above the bottom chrome.
    toasts: uikit_egui::toast::Toasts,
    /// WHICH SURFACE the central panel shows. voiceterm had twelve ported
    /// modules — mail, browser, graph, cards, viz, model picker, setup wizard,
    /// chat panel and more — and the central panel always drew the terminal, so
    /// none of them could be reached. This is the switch that makes them exist.
    view: CentralView,
    mail: voiceterm_egui::mail_view::MailView,
    browser: voiceterm_egui::browser::Browser,
    graph: voiceterm_egui::graph_view::GraphView,
    archive: voiceterm_egui::bars::ArchiveView,
    bar_state: voiceterm_egui::bars::BarState,
    /// The graph's SOURCE, not a prebuilt topology.
    ///
    /// A previous pass drew `Topology::new(&[], &[])` — an empty graph, which
    /// the wiring checker now flags as a view fed nothing. The real path, from
    /// the Qt bin's `nifty_topology`, is `niftyplot::parse(source)`: nodes and
    /// edges come from text. Holding the SOURCE means the graph redraws when
    /// the text changes, which is the point of a live diagram.
    graph_source: String,
    /// The settings screen (QML settings page): sidebar ⚙ toggles the right panel; edits apply
    /// live (App::apply_settings + palette each frame) and persist to prefs.kv on change.
    settings_open: bool,
    /// Terminals for the panes that are NOT focused. `self.app` is always the focused pane's
    /// terminal; on a focus change the two are swapped, so each pane keeps its own scrollback
    /// and every existing `self.app` call site stays as it was.
    pane_apps: std::collections::HashMap<u64, uikit_term::App>,
    /// Which pane `self.app` currently belongs to — the swap's other half.
    pane_focus: u64,
}

impl eframe::App for VoicetermApp {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(self.theme));

        // The escape chord, checked before anything draws: if the recogniser
        // has stopped working, voice mode must not be a room with no door.
        if ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift
                         && i.key_pressed(egui::Key::M)) {
            if !self.chrome.escape() { self.chrome.toggle(); }
        }
        let show_chrome = self.chrome.mode().shows_tab_strip();
        if show_chrome {
        egui::SidePanel::left("sidebar").resizable(true).default_width(160.0).width_range(120.0..=320.0).show(ctx, |ui| {
            if let Some(action) = self.sidebar.ui(ui, &self.colors) {
                match action {
                    SidebarAction::SelectSession(_) => {} // on-device: switch the active PTY/session
                    SidebarAction::New => {} // on-device: spawn a new session
                    SidebarAction::Graph => {} // on-device: open the session graph view
                    SidebarAction::Settings => self.settings_open = !self.settings_open,
                }
            }
        });
        }

        // toast above the bottom chrome (keycap bar ~76px + margin, the QML bind-to-chrome idea)
        self.toasts.ui(ctx, 84.0);
        egui::TopBottomPanel::bottom("keycaps").show(ctx, |ui| {
            // The pipe/brace gesture chips sit on their own row above the keycaps (the QML toolbar
            // put them in modRow): hold + drag to a cell + release. The brace menu's deadband makes
            // each cell 3-zone — left of center emits the opener, right the closer, center the pair
            // with the cursor backed between (see gesture_menus::brace_bytes).
            ui.horizontal(|ui| {
                let pipe_host = ui.button("|\u{22EF}");
                if let SlideOutcome::Selected(id, _) = self.pipe_menu.show(ui, &pipe_host, &pipe_menu()) {
                    if let Some(ch) = pipe_char(&id) {
                        self.app.terminal.feed(ch);
                    }
                }
                // the 📜 chip toggles the blocks drawer (QML blocksDrawer.open())
                if ui.button("\u{1F4DC}").clicked() {
                    self.blocks_open = !self.blocks_open;
                }
                // hold the palette chip to show all vibes (the settings screenshot's theme grid)
                let theme_host = ui.button("\u{1F3A8}");
                if let voiceterm_egui::slide_menu::SlideOutcome::Selected(id, _) =
                    self.theme_menu.show(ui, &theme_host, &theme_menu())
                {
                    self.theme = snapclone_theme::PaletteId::from_str(&id);
                    self.toasts.info(format!("Theme: {}", self.theme.display_name()), ctx.input(|i| i.time));
                    // persist: the pick survives restarts (theme.palette in the kv prefs file)
                    self.app.prefs.palette = id;
                    let _ = save_prefs(&self.app.prefs);
                }
                let brace_host = ui.button("{\u{22EF}}");
                if let SlideOutcome::Selected(id, variant) = self.brace_menu.show(ui, &brace_host, &brace_menu()) {
                    if let Some(bytes) = brace_bytes(&id, variant) {
                        self.app.terminal.feed(&bytes);
                    }
                }
            });
            // Above the keycaps, below the grid: a build's progress belongs
            // where the output is, not in a status bar the user is not looking
            // at while watching a compile.
            voiceterm_egui::detect::progress_ui(ui, &self.progress, &self.colors);
            if let Some(send) = keycap_bar_ui(ui, TERMINAL_KEYCAPS, &self.colors) {
                self.app.terminal.feed(send);
            }
        });

        // the settings screen (QML settings page) as a right panel; edits persist + re-apply live
        if self.settings_open {
            egui::SidePanel::right("settings").resizable(true).default_width(300.0).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("Settings");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("\u{2715}").clicked() {
                            self.settings_open = false;
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if settings_panel(ui, &mut self.app.prefs) {
                        // palette is owned by prefs; keep the frame-applied theme in sync
                        self.theme = snapclone_theme::PaletteId::from_str(&self.app.prefs.palette);
                        let prefs = self.app.prefs.clone();
                        self.app.apply_settings(ctx, &prefs);
                        let _ = save_prefs(&self.app.prefs);
                    }
                });
            });
        }

        // the blocks drawer slides in at the left edge (QML Drawer, LeftEdge)
        if self.blocks_open {
            egui::SidePanel::left("blocks-drawer").resizable(true).default_width(340.0).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("Blocks");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("\u{2715}").clicked() {
                            self.blocks_open = false;
                        }
                    });
                });
                ui.separator();
                if let Some((id, action)) = blocks_drawer_ui(ui, &self.blocks.blocks, &self.colors) {
                    match action {
                        BlockAction::Rerun => {
                            if let Some(cmd) = self.blocks.get_block_command(id).map(str::to_string) {
                                // demo semantics: echo the command line into the grid (a PTY
                                // would receive cmd + \r) and track it as a new block
                                let echoed = format!("$ {cmd}\r\n");
                                // Observe what goes into the grid, not what
                                // comes out of the keyboard: a progress bar is
                                // OUTPUT, and watching the input side would
                                // never see one.
                                self.progress.observe(&echoed);
                                // RISK BEFORE RUNNING, not after. cmd_risk
                                // decides whether snapshot_policy's capture
                                // should run, and the asymmetry is the design:
                                // a false positive is one wasted snapshot the
                                // store dedups away, a false negative is a
                                // command that mutated the directory with no
                                // way back. So Unknown snapshots too.
                                let risk = voiceterm_egui::cmd_risk::classify(&cmd);
                                if risk.needs_snapshot() {
                                    // The policy decides WHAT the capture would
                                    // take. Counting the cwd rather than
                                    // writing a store: the store is GITLOG-2's
                                    // and lives outside this port, so producing
                                    // a real snapshot here would be a second
                                    // implementation of it. The count is the
                                    // honest half — it proves the policy runs
                                    // against real files and tells the user the
                                    // scale of what is protected.
                                    let n = snapshot_scope(".");
                                    self.toasts.info(
                                        format!("{}: would snapshot {n} file(s)",
                                                if risk == voiceterm_egui::cmd_risk::Risk::Unknown
                                                { "unknown command" } else { "mutating" }),
                                        ui.input(|i| i.time));
                                }
                                self.app.terminal.feed(&echoed);
                                self.blocks.submit_command(&cmd, 0);
                            }
                        }
                        BlockAction::CopyCommand => {
                            if let Some(cmd) = self.blocks.get_block_command(id) {
                                ctx.copy_text(cmd.to_string());
                                self.toasts.success("Copied command", ctx.input(|i| i.time));
                            }
                        }
                        BlockAction::CopyOutput => {
                            if let Some(out) = self.blocks.get_block_output(id) {
                                ctx.copy_text(out.to_string());
                                self.toasts.success("Copied output", ctx.input(|i| i.time));
                            }
                        }
                        // the summary worker is backend-side (global toggle in the reference)
                        BlockAction::Summary => {}
                    }
                }
            });
        }

        // the vertical tab sidebar sits between the session sidebar and the terminal
        egui::SidePanel::left("tabbar").resizable(false).exact_width(120.0).show(ctx, |ui| {
            ui.add_space(4.0);
            let mut apply: Vec<voiceterm_egui::gesture_menus::WsCall> = Vec::new();
            const ROW_PITCH: f32 = 33.0; // QML v0.1.803: 30px row + 3px spacing
            let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
            if let Some(target) = self.tabbar_scroll_to.take() {
                scroll = scroll.vertical_scroll_offset(target);
            }
            let out = scroll.show(ui, |ui| {
                for i in 0..self.workspace.tabs.len() {
                    let tab = self.workspace.tabs[i].clone();
                    let resp = tab_row(ui, &tab, i == self.workspace.active, &self.colors);
                    if resp.clicked() {
                        self.workspace.active = i;
                    }
                    // hold-to-open the per-tab menu on the same row host (0.35 s, QML hold
                    // feel); the held tab's name rides in the title band
                    let menu = tab_slide_menu(("vt-tab-menu", i), &tab.name).hold(0.35);
                    if let voiceterm_egui::slide_menu::SlideOutcome::Selected(id, variant) =
                        menu.show_stored(ui, egui::Id::new(("vt-tab-menu", i)), &resp, &tab_menu())
                    {
                        apply = tab_dispatch(&id, variant, i);
                    }
                }
            });
            // v0.1.803 "N more" pills, pinned to the viewport top/bottom while rows overflow;
            // tapping scrolls a page in that direction
            let (above, below) = overflow_counts(
                out.state.offset.y,
                out.inner_rect.height(),
                out.content_size.y,
                ROW_PITCH,
            );
            let page = out.inner_rect.height() * 0.8;
            if above > 0 {
                let pos = egui::pos2(out.inner_rect.center().x, out.inner_rect.top() + 10.0);
                if ui
                    .put(egui::Rect::from_center_size(pos, egui::vec2(90.0, 18.0)), egui::Button::new(format!("\u{2191} {above} more")).small())
                    .clicked()
                {
                    self.tabbar_scroll_to = Some((out.state.offset.y - page).max(0.0));
                }
            }
            if below > 0 {
                let pos = egui::pos2(out.inner_rect.center().x, out.inner_rect.bottom() - 10.0);
                if ui
                    .put(egui::Rect::from_center_size(pos, egui::vec2(90.0, 18.0)), egui::Button::new(format!("\u{2193} {below} more")).small())
                    .clicked()
                {
                    self.tabbar_scroll_to = Some(out.state.offset.y + page);
                }
            }
            let now = ctx.input(|i| i.time);
            for call in &apply {
                // Pane-level calls (split / close pane / web pane) are handled for real now —
                // shell::Workspace is backed by voiceterm's TermWorkspace — so these report
                // what happened instead of the old "needs the workspace backend" stub.
                let handled = self.workspace.apply(call);
                use voiceterm_egui::gesture_menus::WsCall;
                match call {
                    WsCall::ToggleFence(_) => self.toasts.info("Toggled fence", now), // QML's exact string
                    WsCall::TogglePin(i) => {
                        let pinned = self.workspace.tabs.get(*i).map(|t| t.pinned).unwrap_or(false);
                        self.toasts.info(if pinned { "Pinned" } else { "Unpinned" }, now);
                    }
                    WsCall::Command(c) if c == "close tab" => self.toasts.info("Closed tab", now),
                    WsCall::OpenWebPane => self.toasts.info("Web pane opened", now),
                    WsCall::Command(c) if c.starts_with("split") => {
                        self.toasts.info(format!("{} panes", self.workspace.active_pane_ids().len()), now);
                    }
                    WsCall::Command(c) if c == "close pane" => self.toasts.info("Closed pane", now),
                    WsCall::Command(c) if !handled => {
                        self.toasts.warning(format!("{c}: unrecognised"), now);
                    }
                    _ => {}
                }
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // The view switcher. Terminal stays the default and is always one
            // tap away — voiceterm is a terminal first.
            ui.horizontal(|ui| {
                for v in CentralView::ALL {
                    if ui.selectable_label(self.view == v, v.label()).clicked() { self.view = v; }
                }
            });
            ui.separator();
            if self.view != CentralView::Terminal {
                let c = self.colors;
                match self.view {
                    CentralView::Mail => {
                        // The spoken selector, above the typed query. "my last
                        // couple emails from bob" parses to a count and a
                        // sender — the grammar every mail intent except
                        // OpenDrafts is built on, and it had no surface at all.
                        ui.horizontal(|ui| {
                            ui.label("say:");
                            ui.text_edit_singleline(&mut self.mail_phrase);
                        });
                        if !self.mail_phrase.trim().is_empty() {
                            let sel = voiceterm_egui::mail_selector::parse_selector(
                                &self.mail_phrase);
                            let hits = self.mail.selected_by(&sel);
                            // Echo what the grammar UNDERSTOOD, not just the
                            // result. A selector that silently picks the wrong
                            // sender looks like missing mail; showing the parse
                            // makes the misunderstanding visible.
                            let mut parts = Vec::new();
                            if let Some(f) = &sel.from { parts.push(format!("from {f}")); }
                            if let Some(a) = &sel.about { parts.push(format!("about {a}")); }
                            parts.push(if sel.all { "all".into() }
                                       else { format!("count {}", sel.count) });
                            ui.label(egui::RichText::new(format!(
                                "{} → {} thread(s)", parts.join(", "), hits.len()))
                                .weak().size(11.0));
                        }
                        if let Some(conv) = voiceterm_egui::mail_view::list_ui(ui, &mut self.mail, &c) {
                            self.toasts.info(format!("opened {conv}"), ui.input(|i| i.time));
                        }
                    }
                    CentralView::Browser => {
                        ui.horizontal(|ui| {
                            // THE ONE-CHARACTER BUG. This used to clone
                            // `self.browser.url` into a local every frame, edit
                            // the COPY, and write back only on lost_focus() —
                            // so every keystroke was discarded on the next
                            // repaint and the field could never hold more than
                            // the character typed since the last frame.
                            //
                            // Edit the field itself; commit on Enter or on
                            // focus loss.
                            let r = ui.text_edit_singleline(&mut self.browser.url);
                            let entered = r.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if entered || r.lost_focus() {
                                let u = self.browser.url.clone();
                                self.browser.load(&u);
                            }
                        });
                        // THE SITEMAP, under the URL bar. `explore` holds the
                        // views over voiceterm's browsing cores and had no
                        // surface at all — the app could group a site's pages
                        // by what it knew about each one and never showed it.
                        //
                        // Built from the browser's OWN pages rather than a
                        // fixture: a sitemap of invented paths would look like
                        // a feature and be a decoration.
                        let pages: Vec<voiceterm_egui::explore::SitePage> = self.browser
                            .bookmarks.all().iter()
                            .map(|b| voiceterm_egui::explore::SitePage {
                                path: b.url.clone(),
                                title: b.title.clone(),
                                source: "bookmark",
                                // Saved pages are ones the user reached, so
                                // Visited is the honest status — claiming
                                // Probed would assert a check we never ran.
                                status: voiceterm_egui::explore::PageStatus::Visited,
                            })
                            .collect();
                        let groups = voiceterm_egui::explore::group_rows(&pages);
                        if let Some(path) = voiceterm_egui::explore::sitemap_ui(ui, &groups, &c) {
                            self.browser.load(&path);
                        }
                        if let Some(why) = self.browser.empty_reason() {
                            ui.colored_label(c.text_weak, why);
                        }
                        for a in self.browser.ranked() {
                            let on = a.actionable();
                            ui.add_enabled(on, egui::Button::new(a.label()));
                            if let Some(r) = a.blocked_reason() {
                                ui.colored_label(c.text_weak, r);
                            }
                        }
                    }
                    CentralView::Graph => {
                        // CONNECTIONS, above the niftyplot source. Two
                        // different graphs on one tab: this one is over the
                        // app's OWN data — "a unified Node abstraction over the
                        // existing data systems" — while the source box below
                        // draws a diagram the user wrote. Neither replaces the
                        // other.
                        let cg = self.connections_nodes();
                        ui.label(egui::RichText::new(format!(
                            "connections: {} node(s), {} edge(s)", cg.0.len(), cg.1.len()))
                            .weak().size(11.0));
                        for e in cg.1.iter().take(6) {
                            // The WEIGHT is shown, not just the link. An edge
                            // the user cannot judge is a line they must take on
                            // trust, and SameDomain is explicitly "weak but
                            // useful" — showing its weight is what lets them
                            // discount it.
                            //
                            // EdgeReason has weight() and Debug, not label():
                            // checked, because a label() method does exist on
                            // NodeKind and reaching for the same name on the
                            // sibling type is exactly the mistake to make here.
                            ui.label(egui::RichText::new(format!(
                                "  {:?} (w {:.2}) — {} → {}",
                                e.reason, e.reason.weight(), e.from, e.to))
                                .weak().size(10.0));
                        }
                        ui.separator();
                        // Parsed from the source, exactly as the Qt bin's
                        // nifty_topology does: parse, then take ids and edges.
                        ui.add(egui::TextEdit::multiline(&mut self.graph_source)
                               .desired_rows(3).hint_text("graph source"));
                        let doc = nifty_core::niftyplot::parse(&self.graph_source);
                        if doc.nodes.is_empty() {
                            // An empty graph is a real state — an unwritten
                            // diagram — but it must SAY so rather than drawing
                            // an empty ring that looks like a failed render.
                            ui.colored_label(c.text_weak, "no nodes — type a graph above");
                        } else {
                            let ids: Vec<String> = doc.nodes.iter().map(|n| n.id.clone()).collect();
                            let edges: Vec<(String, String)> = doc.edges.iter()
                                .map(|e| (e.from.clone(), e.to.clone())).collect();
                            let topo = voiceterm_egui::graph_view::nifty::Topology::new(&ids, &edges);
                            voiceterm_egui::graph_view::graph_ui(ui, &mut self.graph, &topo, &c);
                        }
                    }
                    CentralView::Table => {
                        // The view returns a submitted line; the ENGINE decides
                        // what it means. Routing it through submit_with_engine
                        // rather than parsing here is what keeps table_nlp the
                        // single place a command is understood.
                        if let Some(_line) = voiceterm_egui::table_view::table_view_ui(
                            ui, &mut self.table, &self.table_headers, &self.table_rows, &c)
                        {
                            let out = voiceterm_egui::table_view::submit_with_engine(
                                &mut self.table);
                            let _ = out;
                        }
                    }
                    CentralView::Docs => {
                        if let Some(name) = voiceterm_egui::manpage_view::manpage_ui(
                            ui, &mut self.docs, &c)
                        {
                            // The view returns a NAVIGATION target, not a
                            // command: a link inside a document names another
                            // document, and following it is the reader's job.
                            let _ = name;
                        }
                    }
                    CentralView::Archive => {
                        ui.text_edit_singleline(&mut self.archive.query);
                        let pages: Vec<voiceterm_egui::bars::PageEntry> = Vec::new();
                        for p in self.archive.results(&pages) {
                            ui.label(p.label());
                        }
                        if pages.is_empty() { ui.colored_label(c.text_weak, "nothing archived yet"); }
                    }
                    CentralView::Terminal => {}
                }
                return;
            }
            // Submitted line (Enter) and the raw BYTES are two different
            // outputs and both matter. handle_input returns the line; the
            // control keys — arrows, Ctrl-C, Tab — are encoded into
            // pending_bytes, and until now NOTHING DRAINED THEM. They were
            // computed correctly every frame and discarded, which is exactly
            // why typing worked and Ctrl-C appeared dead.
            // TERMINAL CHORDS, from the ported pty_input. uikit-term's
            // handle_input encodes bytes and knows nothing about tabs or
            // splits, so Ctrl-Shift-T and Ctrl-Shift-[/] reached the SHELL as
            // literal control bytes — the terminal received an escape sequence
            // for a keystroke the user meant for the window.
            //
            // Intercepted before handle_input for that reason: a chord that is
            // also encodable must not do both.
            // MOTION, on the same frame as the keys. Flicks switch tabs and
            // orientation drives the rotation hint — the accelerometer was
            // ported with 40 tests and read by nothing, so a phone that knows
            // which way up it is was throwing that away every frame.
            if let Some(a) = self.accel.as_mut() {
                let (reading, up, _tilt) = a.poll();
                if let voiceterm_egui::iio_accel::Reading::Ok { x, y, z, scale } = reading {
                    let t_ms = ui.input(|i| i.time) * 1000.0;
                    // Reading::Ok carries RAW i32 counts and a Scale; Sample
                    // wants f64 metres. Feeding the counts unscaled would make
                    // the flick threshold depend on the sensor's resolution —
                    // a different phone would need a different flick.
                    let sample = voiceterm_egui::iio_accel::Sample {
                        x: x as f64 * scale.x,
                        y: y as f64 * scale.y,
                        z: z as f64 * scale.z,
                        t_ms,
                    };
                    if let Some(f) = self.flicks.feed(sample) {
                        // Relative to the phone's CURRENT attitude, so a flick
                        // means the same thing on a desk as held upright — the
                        // reason RelativeFlicks exists rather than a raw axis
                        // threshold.
                        use voiceterm_egui::iio_accel::RelFlick;
                        let n = self.workspace.tabs.len();
                        if n > 1 {
                            let to = match f {
                                RelFlick::Left => (self.workspace.active + 1) % n,
                                RelFlick::Right => (self.workspace.active + n - 1) % n,
                                // Up/Down are not tab moves. Ignored rather
                                // than mapped to something plausible: a gesture
                                // that does a surprising thing is worse than
                                // one that does nothing.
                                _ => self.workspace.active,
                            };
                            if to != self.workspace.active {
                                self.workspace.apply(
                                    &voiceterm_egui::gesture_menus::WsCall::Command(
                                        format!("go to tab {}", to + 1)));
                            }
                        }
                    }
                }
                self.orientation = up;
            }

            let mut chorded = false;
            for out in voiceterm_egui::pty_input::drain_egui(
                ui, voiceterm_egui::pty_input::TermMode::default(),
                self.workspace.tabs.len() > 1)
            {
                if let Some(c) = out.chord {
                    chorded = true;
                    use voiceterm_egui::pty_input::Chord;
                    match c {
                        // Through apply(), never by writing `tabs`/`active`
                        // directly: those are a PROJECTION of `model` that
                        // `sync` rebuilds, so a direct write is undone the next
                        // time anything else changes. WsCall::Command("new
                        // tab") is the path tab_dispatch already uses, so the
                        // chord and the menu item cannot diverge.
                        Chord::NewTab => {
                            self.workspace.apply(
                                &voiceterm_egui::gesture_menus::WsCall::Command("new tab".into()));
                        }
                        Chord::NextTab => {
                            let n = self.workspace.tabs.len();
                            if n > 0 {
                                let next = (self.workspace.active + 1) % n;
                                self.workspace.apply(
                                    &voiceterm_egui::gesture_menus::WsCall::Command(
                                        format!("go to tab {}", next + 1)));
                            }
                        }
                        Chord::PrevTab => {
                            let n = self.workspace.tabs.len();
                            if n > 0 {
                                let prev = (self.workspace.active + n - 1) % n;
                                self.workspace.apply(
                                    &voiceterm_egui::gesture_menus::WsCall::Command(
                                        format!("go to tab {}", prev + 1)));
                            }
                        }
                        Chord::FocusRight => self.workspace.focus_dir(
                            voiceterm_egui::terminal_workspace::Dir::Right),
                        Chord::FocusLeft => self.workspace.focus_dir(
                            voiceterm_egui::terminal_workspace::Dir::Left),
                    }
                }
            }
            // A chord consumed the frame's keys; letting handle_input run too
            // would encode the same keystroke into the PTY behind the action.
            let submitted = if chorded { None } else { self.app.handle_input(ui) };
            let key_bytes = self.app.take_pending_bytes();
            self.pty_out.extend_from_slice(&key_bytes);
            if let Some(line) = submitted {
                // A submitted line goes to the same place, newline included: a
                // PTY does not distinguish "the user typed a command" from "the
                // user typed bytes", and having two paths is how they drift.
                self.pty_out.extend_from_slice(line.as_bytes());
                self.pty_out.push(b'\r');
            }
            // The demo has no PTY to write to. Rather than dropping the bytes
            // again, they are kept on the app where a host can drain them, and
            // capped so a long session cannot grow without bound.
            const PTY_OUT_CAP: usize = 64 * 1024;
            if self.pty_out.len() > PTY_OUT_CAP {
                let drop = self.pty_out.len() - PTY_OUT_CAP;
                self.pty_out.drain(..drop);
            }
            let area = ui.available_rect_before_wrap();
            let colors = self.colors; // PaletteColors is Copy — keeps the borrows below simple

            // PER-PANE TERMINALS. `self.app` is always the focused pane's terminal; the rest
            // are parked in `pane_apps`. When focus moves, the two are SWAPPED rather than the
            // whole app being reworked around a map — so each pane keeps its own scrollback
            // and selection, and every existing `self.app` call site is untouched.
            let focus_now = self.workspace.focused_pane();
            if focus_now != self.pane_focus {
                if let Some(next) = self.pane_apps.remove(&focus_now) {
                    let prev = std::mem::replace(&mut self.app, next);
                    self.pane_apps.insert(self.pane_focus, prev);
                }
                self.pane_focus = focus_now;
            }
            // Reconcile hosts against the model: a split wants one more terminal, a closed
            // pane's terminal is dropped (and with it its pty, on device).
            let hosted: Vec<u64> = self
                .pane_apps
                .keys()
                .copied()
                .chain(std::iter::once(self.pane_focus))
                .collect();
            let diff = self.workspace.host_diff(&hosted);
            for id in &diff.spawn {
                self.pane_apps.insert(*id, uikit_term::App::new(80, 24));
            }
            for id in &diff.retire {
                self.pane_apps.remove(id);
            }

            // THE PANE GRID. A tab can hold several panes now, so the terminal no longer owns
            // the whole region: geometry comes from the model (shell::Workspace::pane_boxes,
            // backed by TermWorkspace) so it matches what QML lays out from ws_command.
            // The focused pane hosts the live terminal; the rest draw their frame and take a
            // click to focus. Per-pane ptys are on-device work — one App is live today.
            const PANE_GUTTER: f32 = 6.0;
            let boxes = self.workspace.pane_boxes(area.width(), area.height(), PANE_GUTTER);
            let to_rect = |p: &voiceterm_egui::shell::PaneBox| {
                egui::Rect::from_min_size(egui::pos2(area.min.x + p.x, area.min.y + p.y), egui::vec2(p.w, p.h))
            };
            let term_rect = boxes.iter().find(|p| p.focused).map(|p| to_rect(p)).unwrap_or(area);

            // Painted before the terminal so the live pane and its detached menus sit on top.
            let mut focus_to: Option<u64> = None;
            for p in boxes.iter().filter(|p| !p.focused) {
                let r = to_rect(p);
                let resp = ui.interact(r, ui.id().with(("pane", p.id)), egui::Sense::click());
                ui.painter().rect_filled(r, egui::CornerRadius::same(4), colors.bg_card);
                ui.painter().rect_stroke(
                    r,
                    egui::CornerRadius::same(4),
                    egui::Stroke::new(1.0_f32, if resp.hovered() { colors.accent } else { colors.border }),
                    egui::StrokeKind::Inside,
                );
                // its own terminal, dimmed by the unfocused frame around it; web panes have no
                // pty and stay a label until the reader is wired in.
                match self.pane_apps.get_mut(&p.id) {
                    Some(a) if !p.web => {
                        a.fit_to(r.size());
                        a.render_at(ui, &colors, r.min);
                    }
                    _ => {
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            if p.web { "web pane" } else { "pane" },
                            egui::FontId::proportional(12.0),
                            colors.text_weak,
                        );
                    }
                }
                if resp.clicked() {
                    focus_to = Some(p.id);
                }
            }
            if let Some(id) = focus_to {
                self.workspace.focus_pane(id);
            }

            // DIVIDER DRAG. Registered after the pane rects so a grab near a shared edge wins
            // the hover. The maths lives in the model (Workspace::drag_seam -> TermWorkspace::
            // resize); this only turns a pointer drag into a seam grab and paints the handle.
            const SEAM_GRAB: f32 = 5.0;
            let mut dragged: Option<(voiceterm_egui::shell::PaneSeam, f32)> = None;
            for (i, s) in self.workspace.pane_seams(area.width(), area.height()).iter().enumerate() {
                let band = if s.vertical {
                    egui::Rect::from_min_max(
                        egui::pos2(area.min.x + s.pos - SEAM_GRAB, area.min.y + s.from),
                        egui::pos2(area.min.x + s.pos + SEAM_GRAB, area.min.y + s.to),
                    )
                } else {
                    egui::Rect::from_min_max(
                        egui::pos2(area.min.x + s.from, area.min.y + s.pos - SEAM_GRAB),
                        egui::pos2(area.min.x + s.to, area.min.y + s.pos + SEAM_GRAB),
                    )
                };
                let resp = ui.interact(band, ui.id().with(("seam", i)), egui::Sense::drag());
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(if s.vertical {
                        egui::CursorIcon::ResizeHorizontal
                    } else {
                        egui::CursorIcon::ResizeVertical
                    });
                    // the shared divider look — uikit-egui exports this precisely for callers
                    // that own their drag handling (SplitPane itself keeps a `fraction`, which
                    // would fight TermWorkspace's ratios, so only the visual is reused).
                    let hair = if s.vertical {
                        egui::vec2(SEAM_GRAB - 1.0, 0.0)
                    } else {
                        egui::vec2(0.0, SEAM_GRAB - 1.0)
                    };
                    uikit_egui::split_pane::paint_divider(ui, band.shrink2(hair), colors.accent);
                }
                if resp.dragged() {
                    let d = if s.vertical { resp.drag_delta().x } else { resp.drag_delta().y };
                    if d != 0.0 {
                        dragged = Some((*s, d));
                    }
                }
            }
            if let Some((s, d)) = dragged {
                self.workspace.drag_seam(&s, d);
            }

            self.app.fit_to(term_rect.size()); // the grid matches the FOCUSED pane, not the region
            if self.app.handle_gestures(ctx, term_rect.min) {
                ctx.request_repaint(); // fling momentum still gliding
            }
            // Long-press on the pane opens the 3x3 marking menu at the press point (detached host:
            // the terminal isn't a widget Response). The line selection the long-press also made is
            // exactly what the menu's "copy" cell copies.
            if let Some(p) = self.app.take_long_press() {
                self.pane_menu.open_at(ui, p, &pane_menu());
            }
            // A tap on a URL opens the link intent menu at the tap point, with the URL as context.
            if let Some((p, url)) = self.app.take_link_tap() {
                self.link_url = Some(url);
                self.link_menu.open_at(ui, p, &link_menu());
            }
            if let voiceterm_egui::slide_menu::SlideOutcome::Selected(id, _) =
                self.link_menu.show_detached(ui, &link_menu())
            {
                if let Some(url) = self.link_url.take() {
                    match link_dispatch(&id, &url) {
                        Some(LinkAction::CopyUrl(u)) => {
                            ctx.copy_text(u);
                            self.toasts.success("Copied URL", ctx.input(|i| i.time));
                        }
                        // Open/Read/NewTab drive the web pane / reader — on-device wiring, same
                        // status as the workspace actions above.
                        Some(LinkAction::Open(_)) | Some(LinkAction::Read(_)) | Some(LinkAction::NewTab(_)) => {}
                        None => {}
                    }
                }
            }
            if let voiceterm_egui::slide_menu::SlideOutcome::Selected(id, variant) =
                self.pane_menu.show_detached(ui, &pane_menu())
            {
                match pane_dispatch(&id, variant) {
                    Some(PaneAction::Clear) => self.app.terminal.clear(), // the demo has no PTY; Terminal::clear is Ctrl+L's effect
                    Some(PaneAction::CopySelection) => {
                        let text = self.app.selection.take().map(|s| s.text).unwrap_or_else(|| self.app.terminal.screen_text());
                        ctx.copy_text(text);
                        self.toasts.success("Copied", ctx.input(|i| i.time));
                    }
                    Some(PaneAction::CommandPalette) => self.app.open_search(), // nearest on-device affordance today
                    // Split/Close/NewTab drive the workspace (ws_command) and Mic starts capture —
                    // on-device wiring, same status as the sidebar's New/Graph/Settings above.
                    Some(PaneAction::Split(_)) | Some(PaneAction::Close(_)) | Some(PaneAction::NewTab) => {}
                    Some(PaneAction::Mic) => {}
                    Some(PaneAction::PageUpScrollback) => {
                        let page = self.app.terminal.grid.rows.saturating_sub(1).max(1);
                        self.app.terminal.grid.page_up(page);
                    }
                    // menu-triggered paste needs a clipboard READ; egui only delivers paste as
                    // an input event (Ctrl+V works today via handle_input's Event::Paste)
                    Some(PaneAction::Paste) => {}
                    None => {}
                }
            }
            self.app.render_at(ui, &self.colors, term_rect.min);
            paint_terminal_border(ui, term_rect, &self.colors);

            // the mic FAB docks at the terminal's bottom-right corner; press-hold opens the
            // action compass right there (matches the reference: the compass pops up ABOVE the FAB,
            // opening from the mic button itself, not a separate host button).
            // Docked against a SHRUNK rect, not the terminal's own. fab_dock
            // deliberately overlaps the corner it docks to — there is a test
            // pinning that, because overlapping the terminal BORDER is the
            // intended look. What it must not overlap is the window edge, and
            // when the terminal runs to the edge those are the same pixel.
            //
            // So the margin belongs here rather than in the shared helper: the
            // shared behaviour is right, this call site just needs to hand it a
            // rect that already has room.
            let fab = fab_rect(term_rect.shrink(10.0), 56.0);
            let resp = mic_fab_ui(ui, fab, &self.colors);
            let _ = self.compass.ui_from_host(ui, &resp, &mut self.app);
        });
    }
}

/// Must match packaging/org.voiceterm.VoiceTerm.desktop exactly. A mismatch fails
/// SILENTLY: the app runs and has no icon, no dock grouping and the wrong
/// name in the switcher.
const APP_ID: &str = "org.voiceterm.VoiceTerm";

fn main() -> eframe::Result<()> {
    let mut app = App::new(80, 24);
    // First run (no prefs file yet): voiceterm's Voice identity, NOT settings-kit's generic
    // "Dark" default — Prefs::default() can't know which app it's defaulting for.
    let first_run = !voiceterm_egui::shell::prefs_path().exists();
    app.prefs = load_prefs();
    let boot_theme = if first_run {
        app.prefs.palette = "voice".into();
        snapclone_theme::PaletteId::Voice
    } else {
        snapclone_theme::PaletteId::from_str(&app.prefs.palette)
    };
    // realistic content, not a toy script — to actually surface rendering gotchas (colors,
    // wrapping, unicode) rather than just confirming an empty prompt renders.
    // track the demo session as command blocks too, so the blocks drawer has content
    let mut blocks = uikit_term::blocks::BlockTracker::new();
    blocks.set_cwd("/home/kessler/proj");
    app.terminal.feed("$ ls --color\r\n");
    blocks.submit_command("ls --color", 0);
    app.terminal.feed("\x1b[1;34mCargo.toml\x1b[0m  \x1b[1;32mbuild.sh\x1b[0m  \x1b[1;34msrc\x1b[0m  \x1b[1;31mold.bak\x1b[0m\r\n");
    blocks.append_output("\x1b[1;34mCargo.toml\x1b[0m  \x1b[1;32mbuild.sh\x1b[0m  src  old.bak\n");
    blocks.note_exit_code(0);
    app.terminal.feed("$ echo \"a very long line that should wrap across more than one terminal row so we can see how wrapping and the cursor position interact once it spills over\"\r\n");
    app.terminal.feed("a very long line that should wrap across more than one terminal row so we can see how wrapping and the cursor position interact once it spills over\r\n");
    blocks.submit_command("echo \"a very long line ...\"", 2);
    blocks.append_output("a very long line that should wrap across more than one terminal row\n");
    blocks.finish(Some(0));
    app.terminal.feed("$ echo café 🚀 你好\r\n"); // multibyte + emoji + CJK, straight in the source
    app.terminal.feed("café 🚀 你好\r\n");
    app.terminal.feed("$ ");
    let colors = uikit_egui::palette_colors("voice");
    let sidebar = Sidebar::new(vec![
        Session { name: "kesslerd".into(), count: 5 },
        Session { name: "kesslerd".into(), count: 2 },
    ]);
    eframe::run_native(
        "voiceterm (egui)",
        eframe::NativeOptions {
        viewport: uikit_egui::desktop::viewport(APP_ID, "VoiceTerm", [900.0, 700.0]),
        ..Default::default()
    },
        Box::new(move |_cc| {
            Ok(Box::new(VoicetermApp {
                // Discovery once at startup, not per frame: scanning
                // /sys/bus/iio/devices every repaint is a directory walk at 60
                // Hz for a device that does not appear mid-session.
                accel: voiceterm_egui::iio_accel::IioAccel::discover(),
                flicks: Default::default(),
                orientation: voiceterm_egui::iio_accel::OrientationUp::Undefined,
                progress: Default::default(),
                // TableView::new(true), not Default: has_data defaults to
                // FALSE, and a view told it has no data reports "nothing to
                // transform" while sitting on four rows. The constructor takes
                // the flag precisely because it cannot be inferred from here.
                mail_phrase: String::new(),
                table: voiceterm_egui::table_view::TableView::new(true),
                docs: {
                    let mut v = voiceterm_egui::manpage_view::ManpageView::default();
                    // include_str!, not a runtime read: the docs ship WITH the
                    // binary, so a phone install cannot end up with a reader
                    // and no documents — and there is no path to get wrong at
                    // startup on a device whose layout differs from the repo's.
                    v.set_docs(vec![
                        doc("readme", "README", include_str!("../../../docs/README.md")),
                        doc("roadmap", "Roadmap", include_str!("../../../docs/ROADMAP.md")),
                        doc("pane-voice", "Pane voice",
                            include_str!("../../../docs/PANE_VOICE.md")),
                        doc("niftyplot", "niftyplot spec",
                            include_str!("../../../docs/niftyplot-spec.md")),
                    ]);
                    v
                },
                table_headers: vec!["name".into(), "size".into(), "modified".into()],
                table_rows: vec![
                    vec!["Cargo.toml".into(), "1204".into(), "10:19".into()],
                    vec!["build.sh".into(), "88".into(), "09:41".into()],
                    vec!["src".into(), "4096".into(), "10:22".into()],
                    vec!["old.bak".into(), "51200".into(), "08:03".into()],
                ],
                chrome: Default::default(),
                pty_out: Vec::new(),
                app,
                sidebar,
                compass: ActionCompass::default(),
                colors,
                pipe_menu: pipe_slide_menu("voiceterm-pipe-menu"),
                // comfort sizing (QML braceCompact defaults false): deadband 28 px, 120 px cells
                brace_menu: brace_slide_menu("voiceterm-brace-menu", false),
                pane_menu: voiceterm_egui::slide_menu::SlideMenu::new("voiceterm-pane-menu").columns(3),
                link_menu: voiceterm_egui::slide_menu::SlideMenu::new("voiceterm-link-menu").columns(2),
                link_url: None,
                workspace: Workspace::new(&["main", "logs"]),
                pane_apps: std::collections::HashMap::new(),
                pane_focus: 0, // TermWorkspace's first pane is id 0

                theme: boot_theme,
                theme_menu: voiceterm_egui::slide_menu::SlideMenu::new("voiceterm-theme-menu").columns(3).hold(0.35).item_size(64.0),
                blocks: blocks,
                blocks_open: false,
                tabbar_scroll_to: None,
                toasts: uikit_egui::toast::Toasts::new(),
                view: CentralView::Terminal,
                mail: Default::default(),
                browser: Default::default(),
                graph: Default::default(),
                archive: Default::default(),
                bar_state: Default::default(),
                graph_source: String::new(),
                settings_open: false,
            }))
        }),
    )
}
