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
enum CentralView { Terminal, Mail, Browser, Graph, Table, Docs, Archive, Models, Chat, Notes, Build }

impl VoicetermApp {
    /// Feed PTY bytes to the screen.
    ///
    /// term_grid deliberately takes SEMANTIC operations — print, newline, sgr —
    /// and leaves escape parsing to the caller, so it can be tested without a
    /// terminal. This is the smallest parser that keeps that contract honest:
    /// the printable characters, the four C0 controls that move the cursor, and
    /// CSI sequences for the cases a shell prompt actually emits.
    ///
    /// Unknown escapes are SWALLOWED, not printed. A terminal that renders the
    /// bytes of a sequence it does not understand fills the screen with
    /// `[?2004h` — which looks like the shell is broken rather than the
    /// emulator being incomplete.
    fn feed_pty(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let mut it = text.chars().peekable();
        while let Some(c) = it.next() {
            match c {
                '\x1b' => {
                    if it.peek() == Some(&'[') {
                        it.next();
                        let mut params = String::new();
                        let mut final_byte = None;
                        for c2 in it.by_ref() {
                            if c2.is_ascii_alphabetic() {
                                final_byte = Some(c2);
                                break;
                            }
                            params.push(c2);
                        }
                        let n = |d: usize| params.split(';').next()
                            .and_then(|x| x.parse::<usize>().ok()).unwrap_or(d);
                        match final_byte {
                            Some('H') => {
                                // 1-based on the wire, 0-based in the grid.
                                let mut p = params.split(';');
                                let row = p.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(1);
                                let col = p.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(1);
                                self.grid.move_to(col.saturating_sub(1), row.saturating_sub(1));
                            }
                            Some('J') => self.grid.clear(),
                            Some('K') => self.grid.erase_to_eol(),
                            Some('m') => {
                                for part in params.split(';') {
                                    if let Ok(v) = part.parse::<u16>() { self.grid.sgr(v); }
                                }
                                if params.is_empty() { self.grid.sgr(0); }
                            }
                            // Cursor moves. Clamped by the grid, not here.
                            Some('A') | Some('B') | Some('C') | Some('D') => {
                                let (x, y) = self.grid.cursor();
                                let d = n(1);
                                let (nx, ny) = match final_byte {
                                    Some('A') => (x, y.saturating_sub(d)),
                                    Some('B') => (x, y + d),
                                    Some('C') => (x + d, y),
                                    _ => (x.saturating_sub(d), y),
                                };
                                self.grid.move_to(nx, ny);
                            }
                            _ => {}
                        }
                    }
                    // A bare ESC or an unrecognised introducer: dropped.
                }
                '\r' => self.grid.carriage_return(),
                '\n' => self.grid.newline(),
                '\t' => {
                    // Next 8-column stop. Printing spaces rather than moving,
                    // so erase_to_eol still clears what a tab covered.
                    let (x, _) = self.grid.cursor();
                    for _ in 0..(8 - (x % 8)) { self.grid.print(' '); }
                }
                '\x08' => {
                    let (x, y) = self.grid.cursor();
                    self.grid.move_to(x.saturating_sub(1), y);
                }
                c if (c as u32) < 0x20 => {}   // other C0: ignored, not drawn
                c => self.grid.print(c),
            }
        }
    }

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
                // preview() truncates on a word boundary, so a long title does
                // not end mid-word in the graph. Built from the title because a
                // bookmark is all the app stored — an empty preview would be
                // honest but leaves the node unreadable at a glance.
                preview: cg::preview(&b.title),
                // No tags: the app records none for a bookmark, and inventing
                // one ("web", "saved") would be a facet nobody can filter on
                // meaningfully.
                tags: Vec::new(),
            });
        }
        for t in &self.mail.threads {
            nodes.push(cg::Node {
                id: cg::node_id(cg::NodeKind::ChatThread, &t.conversation_id),
                kind: cg::NodeKind::ChatThread,
                title: t.subject.clone(),
                subtitle: t.participants.join(", "),
                preview: cg::preview(&t.subject),
                tags: Vec::new(),
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
    const ALL: [CentralView; 11] = [CentralView::Terminal, CentralView::Mail,
                                   CentralView::Browser, CentralView::Graph,
                                   CentralView::Table, CentralView::Docs,
                                   CentralView::Archive, CentralView::Models,
                                   CentralView::Chat, CentralView::Notes,
                                   CentralView::Build];
    fn label(self) -> &'static str {
        match self {
            CentralView::Terminal => "Terminal", CentralView::Mail => "Mail",
            CentralView::Browser => "Web", CentralView::Graph => "Graph",
            CentralView::Table => "Table", CentralView::Docs => "Docs",
            CentralView::Archive => "Archive",
            CentralView::Models => "Models",
            CentralView::Chat => "Chat",
            CentralView::Notes => "Notes",
            CentralView::Build => "Build",
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
    /// The ENGINE's table: base data plus the op stack. table_view tracks
    /// only the stack DEPTH (it is generic over op types by design), so the
    /// ops themselves live here — and the rendered rows come from
    /// `apply()`, which is what makes a sort or filter actually do something.
    table_engine: voiceterm_egui::table_ops::TableView,
    table_headers: Vec<String>,
    table_rows: Vec<Vec<String>>,
    /// The bundled-document reader. The view takes documents rather than
    /// fetching them — its own header explains why the engine could not be
    /// included here — so the HOST owns which docs exist and the conversion.
    docs: voiceterm_egui::manpage_view::ManpageView,
    /// The spoken mail phrase, parsed by the selector grammar.
    mail_phrase: String,
    /// A spoken date question — "how many days until christmas". Answered
    /// locally rather than shelled out to `date`, because the arithmetic is
    /// where naive implementations go quietly wrong and date_phrase already
    /// got it right.
    date_phrase: String,
    /// Which summary paths may run. ONE value with two consumers, not two
    /// booleans: the original is explicit that a third state — terminal on,
    /// web on, but "summaries off" somewhere else — is the bug this shape
    /// prevents.
    summary_scope: voiceterm_egui::summary_scope::Scope,
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
    /// A spoken editing phrase, e.g. "delete line" or "save as notes.txt".
    ///
    /// Typed here because dictation lands as text either way — the point of
    /// editor_keys is the TRANSLATION, and it is the same whether the words
    /// arrived by microphone or by keyboard. Wiring it to the mic first would
    /// have made the feature untestable without one.
    editor_phrase: String,
    /// Which modal editor is on the other end. Helix and vim diverge exactly
    /// where their keymaps do, so this cannot be guessed per phrase.
    editor_kind: voiceterm_egui::editor_keys::Editor,
    /// A translated action awaiting confirmation, when it would lose work.
    editor_pending: Option<voiceterm_egui::editor_keys::EditorAction>,
    /// GGUF search box, and the parameter count to size against.
    model_query: String,
    model_params_b: f32,
    /// Memory budget in GiB. Defaults low because this targets phones — a
    /// picker that assumes a desktop recommends a model the device cannot load,
    /// and the failure is an OOM kill rather than a message.
    model_budget_gib: f32,
    /// Token events, for the accounting readout.
    tokens: Vec<voiceterm_egui::token_log::TokenEvent>,
    /// tldr-style pages the command search scores against.
    ///
    /// A small built-in set rather than a disk index: command_search says the
    /// indexing and the cache "belong to the shell" and the SCORING is the
    /// judgement it ports. Shipping a handful of real pages exercises the
    /// scoring honestly; shipping a fake index would exercise nothing.
    cmd_docs: Vec<voiceterm_egui::command_search::CommandDoc>,
    /// Candidates from the last phrase that matched no editor action.
    cmd_hits: Vec<voiceterm_egui::command_search::CommandMatch>,
    /// The terminal screen the PTY's output paints onto.
    ///
    /// The app has always had pty_out — bytes going TO the shell — and no
    /// return path, so nothing rendered what came back. uikit_term::blocks
    /// models a command/output LOG, which is a different thing: it cannot show
    /// a cursor, a redraw, or anything that repaints in place.
    grid: voiceterm_egui::term_grid::TermGrid,
    /// How much of `pty_out` has already been echoed to the screen.
    pty_echoed: usize,
    /// Which grid rows changed, fed from the grid's own dirty range.
    row_dirty: voiceterm_egui::row_cache::RowCache,
    /// Shaped galleys, one per row, reused while the row is unchanged.
    ///
    /// This caches TEXT LAYOUT, not painting. Immediate mode repaints every
    /// visible row every frame by definition — skipping the paint is not
    /// available and row_render's docs say so. Shaping a row into a galley is
    /// what costs, and it is what this keeps.
    row_galleys: voiceterm_egui::row_render::RowLayoutCache<String>,
    /// The reading-mode corpus tape. Fed by an audio callback on a device with
    /// a microphone; here only its POLICY runs — start, bound, keep-or-discard.
    tape: voiceterm_egui::corpus_tape::Tape,
    /// The paragraph being read aloud, which the recording is saved beside.
    tape_reference: String,
    /// What the last stop decided, so a discarded take says why.
    tape_note: String,
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
    // The real archive surface (archived_view), replacing a bars::ArchiveView
    // that was only ever fed an empty vec — a connected placeholder the
    // wiring checker counted as done.
    archive: voiceterm_egui::archived_view::ArchiveView,
    /// The raw messages beside the MailView: threads hold message IDS, and
    /// the inbox list draws Message rows, so both live here and are set
    /// together.
    mail_messages: Vec<voiceterm_egui::mail_view::Message>,
    /// The open message id on the Mail view, if any.
    mail_open: Option<String>,
    /// The command audit chain: every submitted line hashed into a tamper-
    /// evident sequence. Held on the app because the chain's whole value is
    /// continuity — a per-frame chain would verify nothing.
    audit: voiceterm_egui::audit_chain::Chain,
    /// The rendered audit records, newest last, shown on the Docs tab.
    audit_lines: Vec<String>,
    /// True while the foreground looks like it is asking for a secret.
    /// Suppresses echo AND keeps the line out of the audit log.
    password_mode: bool,
    /// Promotion thresholds for the acoustic cache — the runtime knobs the
    /// original exposes, here on the Docs tab beside the corpus tape that
    /// feeds it.
    acoustic: voiceterm_egui::acoustic_cache::Thresholds,
    /// The distance of the nearest fingerprint, as a what-if for the sliders.
    acoustic_distance: f32,
    acoustic_confirms: u32,
    acoustic_same_speaker: bool,
    /// The first-run model wizard (kit widget, re-exported by our
    /// setup_wizard). Some() until it finishes; the Models tab hosts it.
    wizard: Option<voiceterm_egui::setup_wizard::SetupWizard>,
    /// The chat panel (kit widget via chat_panel) — the local model's
    /// conversation, on its own view.
    chat: voiceterm_egui::chat_panel::ChatPanel,
    /// The voice halo (repl_voice_glow): drawn over everything while the mic
    /// is armed, so the state is visible from across the room.
    glow: voiceterm_egui::repl_voice_glow::ReplVoiceGlow,
    /// A destructive action awaiting confirmation (confirm_dialog).
    pending_confirm: Option<(String, voiceterm_egui::confirm_dialog::ConfirmDialog)>,
    /// The note-card deck: captured text split into atomic cards.
    deck: voiceterm_egui::cards_view::Deck,
    /// What the user has captured so far, re-split on change. Held because
    /// the splitter is the source of truth — cards are derived, not edited
    /// in place.
    captured: String,
    /// "What links here", over the notes the deck has produced.
    backlinks: voiceterm_egui::backlinks_view::BacklinksView,
    /// Compiler output, parsed into navigable diagnostics.
    build: voiceterm_egui::build_view::BuildView,
    /// Pasted cargo output — this build shells out to nothing, so the input
    /// is explicit rather than a fake invocation.
    build_input: String,
    /// The workspaces page: saved workspaces + the live session.
    workspaces: voiceterm_egui::workspaces_view::WorkspacesView,
    /// Document frequencies over the captured notes, for tag suggestion.
    /// Rebuilt with the deck: TF-IDF against a corpus that no longer matches
    /// the notes would suggest tags for text that is gone.
    doc_freq: voiceterm_egui::nlp::DocFreq,
    /// Suggested tags for the selected card.
    tag_suggestions: Vec<String>,
    /// Tags the user accepted, excluded from future suggestions.
    accepted_tags: Vec<String>,
    /// Block-summary scheduling: debounce, backoff, and yielding to voice.
    summaries: voiceterm_egui::summary_queue::SummaryQueue,
    /// Summaries the queue has produced, by block id.
    summary_text: std::collections::HashMap<u64, String>,
    /// Is the mic live? One flag, read by both the halo and the summary
    /// queue's yield rule, so they cannot disagree.
    /// The ambient light sensor, when the device has one. Discovered once:
    /// a sensor does not appear mid-session, and re-scanning sysfs every
    /// frame to learn the same answer is a syscall storm on a phone.
    light: Option<voiceterm_egui::iio_light::IioSensor>,
    /// The last usable reading, so a transient read error shows the previous
    /// value rather than blanking the row.
    light_value: Option<f64>,
    /// Learned voice fingerprints — the LEARNING half of the same path
    /// acoustic_cache decides with. The cache asks "is this close enough to
    /// promote"; this is what it is close TO, and it has the caps and the
    /// eviction rule that keep the index from growing without bound.
    phonetics: voiceterm_egui::phonetic_store::Store,
    glow_active: bool,
    workspaces_open: bool,
    workspaces_filter: String,
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

        // CONTEXT-LEVEL WIDGETS, drawn above every panel because that is what
        // they are for: a modal that a panel can cover is not modal, and a
        // voice halo that only shows on one tab is a status light behind a
        // door.
        if let Some((cmd, dlg)) = &mut self.pending_confirm {
            use voiceterm_egui::confirm_dialog::Outcome;
            match dlg.show(ctx) {
                Outcome::Confirmed => {
                    let cmd = cmd.clone();
                    self.pending_confirm = None;
                    self.pty_out.extend_from_slice(cmd.as_bytes());
                    self.pty_out.push(b'\r');
                }
                // Cancelled and timed-out both drop the command. The line is
                // NOT restored to the input: it was already audited as
                // submitted, and silently re-typing it would invite a second
                // Enter to run what was just refused.
                // Cancelled and Discarded both drop it. They stay distinct in
                // the enum because a caller that PERSISTS a draft needs the
                // difference — this one does not, and conflating them here is
                // a decision about this call site, not about the kit.
                Outcome::Cancelled | Outcome::Discarded => self.pending_confirm = None,
                Outcome::Pending => {}
            }
        }
        // The mic halo follows the router's own state rather than a separate
        // flag, so the glow cannot disagree with what is actually listening.
        self.glow.show(ctx);

        // THE WORKSPACES PAGE, as a side panel. Rows come from the sidebar's
        // real session list and the live tabs from the real workspace — a
        // page fed demo rows would sort and filter convincingly over nothing.
        if self.workspaces_open {
            use voiceterm_egui::workspaces_view as wv;
            let now_ms = (ctx.input(|i| i.time) * 1000.0) as u64;
            let rows: Vec<wv::WorkspaceRow> = self.sidebar.sessions.iter()
                .map(|s| wv::WorkspaceRow {
                    name: s.name.clone(),
                    speaker: String::new(),
                    browser_tabs: self.browser.bookmarks.all().len(),
                    terminal_sessions: s.count as usize,
                    updated_ms: now_ms,
                }).collect();
            self.workspaces.set_rows(rows);
            self.workspaces.set_filter(&self.workspaces_filter);
            self.workspaces.live = Some(wv::LiveSession {
                speaker_display: String::new(),
                tabs: self.workspace.tabs.iter()
                    .map(|t| (t.name.clone(), now_ms)).collect(),
                active_tab: self.workspace.active,
            });
            let colors = self.colors.clone();
            egui::SidePanel::right("workspaces").show(ctx, |ui| {
                ui.heading("Workspaces");
                ui.text_edit_singleline(&mut self.workspaces_filter);
                // The live session only shows at two or more tabs, matching
                // the tab strip's own rule — a page that showed a one-tab
                // session while the strip hid it would look like one of the
                // two was broken.
                if let Some(live) = &self.workspaces.live {
                    if live.is_showable() {
                        ui.colored_label(colors.text_weak, "current session");
                        for (i, (url, _)) in live.tabs.iter().enumerate() {
                            let label = wv::tab_label(url);
                            ui.colored_label(
                                if i == live.active_tab { colors.accent } else { colors.text },
                                label);
                        }
                        ui.separator();
                    }
                }
                if self.workspaces.is_empty() {
                    ui.colored_label(colors.text_weak, "no saved workspaces");
                }
                for r in self.workspaces.ordered() {
                    ui.horizontal(|ui| {
                        ui.colored_label(colors.text, &r.name);
                        ui.colored_label(colors.text_weak, egui::RichText::new(format!(
                            "{} tab(s) · {} session(s) · {}",
                            r.browser_tabs, r.terminal_sessions,
                            wv::age_label(now_ms, r.updated_ms))).size(11.0));
                    });
                }
                if ui.small_button("close").clicked() { self.workspaces_open = false; }
            });
        }

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
                    SidebarAction::Graph => {
                        // The workspaces page. The sidebar's Graph action was
                        // a no-op with a TODO; this is the page the Qt app
                        // puts behind it.
                        self.workspaces_open = !self.workspaces_open;
                    }
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
            // The scope control. A segmented row rather than two checkboxes,
            // because two checkboxes can express a state this deliberately
            // cannot — and the pair would drift the first time one was saved
            // and the other was not.
            ui.horizontal(|ui| {
                ui.label("summaries:");
                // AMBIENT LIGHT. Reported as a capability, not a dead
                // control: the module mirrors hover_kit::capability so a
                // settings page can EXPLAIN a missing sensor instead of
                // showing a slider that does nothing.
                {
                    use voiceterm_egui::iio_light::{Sample, Unit};
                    match self.light.as_ref() {
                        Some(s) => {
                            if let Sample::Value(v) = s.read() { self.light_value = Some(v); }
                            let unit = match s.unit {
                                // Counts are RELATIVE — labelling them lux
                                // would invite comparing two phones' numbers.
                                Unit::Illuminance => "lux",
                                Unit::Counts => "counts (relative)",
                            };
                            ui.label(match self.light_value {
                                Some(v) => format!("ambient light: {v:.0} {unit}  [{}]", s.channel),
                                None => format!("ambient light: sensor found, no reading  [{}]", s.channel),
                            });
                        }
                        None => { ui.label("ambient light: no sensor on this device"); }
                    }
                }
                for sc in voiceterm_egui::summary_scope::Scope::ALL {
                    if ui.selectable_label(self.summary_scope == sc, sc.label()).clicked() {
                        self.summary_scope = sc;
                    }
                }
            });

            // The date question, beside the terminal it answers into. A voice
            // terminal is exactly where "how long until christmas" gets asked,
            // and the answer is a line of text — the same shape as any other
            // command's output.
            ui.horizontal(|ui| {
                ui.label("date:");
                ui.text_edit_singleline(&mut self.date_phrase);
                let q = self.date_phrase.trim().to_string();
                if !q.is_empty() && ui.small_button("ask").clicked() {
                    // TODAY is passed in, not read here: date_phrase is pure so
                    // it can be tested across a leap day and a year roll, and a
                    // function that reads the clock cannot be.
                    let today = (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs()).unwrap_or(0) / 86_400) as i64;
                    let line = match voiceterm_egui::date_phrase::resolve(&q, today) {
                        Some(target) => voiceterm_egui::date_phrase::phrase_days(
                            q.trim(), target - today),
                        // A phrase it does not know says so rather than
                        // guessing a date: a wrong answer to "when is diwali"
                        // is worse than "I do not know that one", because the
                        // user acts on it.
                        None => format!("I do not know the date of {q}"),
                    };
                    self.app.terminal.feed(&format!("{line}\r\n"));
                    self.date_phrase.clear();
                }
            });

            // SPOKEN EDITING. editor_keys turns "delete line" into the keys a
            // modal editor wants — the mobility thesis of this app, "edit files
            // on the move, one-handed, on a phone" — and had no surface at all.
            ui.horizontal(|ui| {
                ui.label("say:");
                let r = ui.text_edit_singleline(&mut self.editor_phrase);
                // The editor is a CHOICE, not a guess: helix and vim diverge
                // exactly where their keymaps do, and translating a phrase for
                // the wrong one sends valid keystrokes that mean something else.
                for (k, name) in [(voiceterm_egui::editor_keys::Editor::Helix, "helix"),
                                  (voiceterm_egui::editor_keys::Editor::Vim, "vim")] {
                    if ui.selectable_label(self.editor_kind == k, name).clicked() {
                        self.editor_kind = k;
                    }
                }
                let go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if go && !self.editor_phrase.trim().is_empty() {
                    // POLITENESS COMES OFF FIRST. Speech is not typed input:
                    // "could you please just delete this line" and "delete
                    // line" are the same command, and editor_keys' rules are
                    // written against the second form.
                    //
                    // voice_prefix::normalise also GATES — it returns None for
                    // something that reads as prose rather than a command, so
                    // dictation that was meant as speech does not become
                    // keystrokes. That is the more important half: a modal
                    // editor turns stray words into commands.
                    //
                    // The two normalisers are not duplicates and I checked
                    // before stacking them: editor_keys strips ANCHORED
                    // multi-word filler ("could you" at the start only, because
                    // "the file you saved" contains "you" as content), while
                    // this strips politeness bounded against a transcript of
                    // nothing but "please please please".
                    let spoken = voiceterm_egui::voice_prefix::normalise(&self.editor_phrase);
                    // The early return exits this ui.horizontal closure only —
                    // the rest of the frame still draws. Worth stating because
                    // a `return` inside an egui closure looks like it abandons
                    // the panel and does not.
                    let Some(phrase) = spoken else {
                        self.toasts.info(
                            format!("that reads as prose, not a command: {:?}",
                                    self.editor_phrase),
                            ui.input(|i| i.time));
                        self.editor_phrase.clear();
                        return;
                    };
                    match voiceterm_egui::editor_keys::translate(
                        self.editor_kind, &phrase)
                    {
                        Some(a) if a.confirm => {
                            // Destructive actions WAIT. `confirm` is set only
                            // for the ones that lose work, and a misheard word
                            // that quits without writing is the failure this
                            // whole feature would be blamed for.
                            self.editor_pending = Some(a);
                        }
                        Some(a) => {
                            self.pty_out.extend_from_slice(a.keys.as_bytes());
                            self.editor_phrase.clear();
                        }
                        // Unrecognised phrases are NOT sent as literal text. A
                        // modal editor in normal mode reads stray letters as
                        // commands, so "umm delete that" would run d, e, l...
                        //
                        // Instead they fall through to command_search, which is
                        // what it was written for: "when spoken text matches
                        // none of the structured intents, it is scored against
                        // an index of tldr pages". A phrase that is not an
                        // editor action may well be a shell command.
                        None => {
                            self.cmd_hits = voiceterm_egui::command_search::search(
                                &phrase, &self.cmd_docs, 3);
                            if self.cmd_hits.is_empty() {
                                self.toasts.info(
                                    format!("nothing matches {phrase:?}"),
                                    ui.input(|i| i.time));
                            }
                        }
                    }
                }
            });
            // CANDIDATES, WITH THEIR REASONS. CommandMatch carries why it
            // matched, and showing that is what makes a suggestion checkable —
            // the module's own worry is whether "delete the old logs" surfaces
            // `rm` or something alarming, and a bare command name gives the
            // user no way to tell which they got.
            //
            // Nothing is run from here. A voice phrase that becomes a shell
            // command without a deliberate second act is the failure this whole
            // path is careful about; these are offered, and the user types or
            // taps.
            if !self.cmd_hits.is_empty() {
                ui.colored_label(self.colors.text_weak, "did you mean:");
                let mut chosen: Option<String> = None;
                for m in &self.cmd_hits {
                    ui.horizontal(|ui| {
                        if ui.small_button(&m.name).clicked() {
                            chosen = Some(m.name.clone());
                        }
                        ui.colored_label(self.colors.text_weak,
                            format!("{:.2}  {}", m.score, m.reasons.join(", ")));
                    });
                }
                if let Some(name) = chosen {
                    // Placed in the phrase box, not the PTY. One more
                    // deliberate act before anything executes.
                    self.editor_phrase = name;
                    self.cmd_hits.clear();
                }
                if ui.small_button("dismiss").clicked() { self.cmd_hits.clear(); }
            }
            if let Some(a) = self.editor_pending.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(self.colors.warning, "this discards changes:");
                    if ui.button("send").clicked() {
                        self.pty_out.extend_from_slice(a.keys.as_bytes());
                        self.editor_pending = None;
                        self.editor_phrase.clear();
                    }
                    if ui.button("cancel").clicked() { self.editor_pending = None; }
                });
            }
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
                // BLOCK SUMMARIES, scheduled rather than fired. The queue
                // debounces (a block that is still producing output is not
                // worth summarising yet), backs off on failure, and YIELDS TO
                // VOICE — the mic and the summariser want the same model, and
                // the user talking wins. Gated by summary_scope, so turning
                // summaries off means the queue is never fed rather than fed
                // and ignored.
                {
                    use voiceterm_egui::summary_queue::{Action, Wait};
                    let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;
                    if self.summary_scope.terminal_enabled() {
                        for b in &self.blocks.blocks {
                            let id = b.id as u64;
                            if !self.summary_text.contains_key(&id) {
                                self.summaries.request(id, now_ms);
                            }
                        }
                    }
                    // Voice wins: the router's own state decides, so the queue
                    // cannot disagree with what is actually listening.
                    let voice_active = self.glow_active;
                    match self.summaries.next_action(now_ms, voice_active) {
                        Action::Run { block_id } => {
                            // No summariser attached in this build. Completing
                            // with an explicit notice keeps the queue draining
                            // — leaving it Running forever would make the
                            // backoff logic untestable in the app it serves.
                            self.summary_text.insert(block_id,
                                "(no summariser attached in this build)".into());
                            self.summaries.complete(block_id);
                        }
                        Action::Wait(w) => {
                            // The three waits are three different situations
                            // and the drawer says which: Idle is nothing to
                            // do, NotYet is patience, YieldToVoice is a
                            // deliberate concession the user should see.
                            if w != Wait::Idle {
                                ui.colored_label(self.colors.text_weak, match w {
                                    Wait::NotYet => format!(
                                        "summaries: {} queued", self.summaries.len()),
                                    Wait::YieldToVoice => "summaries paused — voice has the model".into(),
                                    Wait::Idle => String::new(),
                                });
                            }
                        }
                    }
                }
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
                                // Gated: with summaries off, the detector does
                                // not run at all rather than running and being
                                // ignored. On a phone that is the difference
                                // between a setting and a lie.
                                if self.summary_scope.terminal_enabled() {
                                    self.progress.observe(&echoed);
                                }
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
                        ui.separator();
                        // The inbox proper (mail_panes, ported): rows over the
                        // real Message model, message pane below on open.
                        use voiceterm_egui::mail as mp;
                        if let Some(act) = mp::inbox_ui(ui, &self.mail_messages, false, &c) {
                            match act {
                                mp::MailAction::Open(id) => self.mail_open = Some(id),
                                mp::MailAction::Search(q) => self.mail.query = q,
                                _ => {}
                            }
                        }
                        if let Some(id) = self.mail_open.clone() {
                            if let Some(m) = self.mail_messages.iter().find(|m| m.id == id) {
                                let body = self.mail.bodies.get(&id)
                                    .map(|b| b.text().to_string()).unwrap_or_default();
                                let view = mp::MessageView {
                                    id: m.id.clone(),
                                    from_name: mp::sender_of(m).to_string(),
                                    from_addr: String::new(),
                                    to: String::new(),
                                    subject: mp::subject_of(m).to_string(),
                                    date: m.time.to_string(),
                                    body_text: body,
                                };
                                mp::message_ui(ui, &view, &c);
                                if ui.small_button("close").clicked() { self.mail_open = None; }
                            }
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
                        // The page path is gated on the SAME value, and says so
                        // when it is off. The sitemap itself is navigation
                        // rather than summarisation, so it stays — what the
                        // scope withholds is the derived text, and hiding the
                        // navigation too would look like a broken tab rather
                        // than an honoured setting.
                        if !self.summary_scope.web_enabled() {
                            ui.label(egui::RichText::new(
                                "page summaries are off for this scope")
                                .weak().small());
                        }
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
                        // THE DRAWN GRAPH: layered_graph positions the nodes
                        // (Sugiyama — back edges cut, layers assigned, two
                        // crossing-reduction sweeps), viz renders the Scene.
                        // "Lay out once into a Scene; render any Scene each
                        // frame" is that module's whole design.
                        if !cg.0.is_empty() {
                            use voiceterm_egui::{layered_graph as lg, viz};
                            let idx: std::collections::HashMap<&str, usize> = cg.0.iter()
                                .enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
                            let n = cg.0.len();
                            let mut succ = vec![Vec::new(); n];
                            let mut pred = vec![Vec::new(); n];
                            for e in &cg.1 {
                                if let (Some(&a), Some(&b)) = (idx.get(e.from.as_str()),
                                                               idx.get(e.to.as_str())) {
                                    if a != b { succ[a].push(b); pred[b].push(a); }
                                }
                            }
                            let back = lg::back_edges(&succ);
                            let layer = lg::assign_layers(&succ, &back);
                            let mut order = lg::layers_of(&layer);
                            lg::sweep(&mut order, &succ, &pred, true);
                            lg::sweep(&mut order, &succ, &pred, false);
                            let placed = lg::place(&order, n, lg::FlowDirection::Down);
                            let gnodes: Vec<viz::GNode> = cg.0.iter().zip(&placed)
                                .map(|(node, p)| viz::GNode {
                                    id: node.id.clone(),
                                    label: voiceterm_egui::archived_view::safe_trunc(&node.title, 18).to_string(),
                                    pos: (p.x, p.y),
                                    size: (110.0, 26.0),
                                }).collect();
                            let gedges: Vec<viz::GEdge> = cg.1.iter()
                                .map(|e| viz::GEdge { from: e.from.clone(), to: e.to.clone() })
                                .collect();
                            let scene = viz::graph_scene(&gnodes, &gedges);
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 220.0), egui::Sense::hover());
                            viz::render_scene(ui, &scene, rect, &c);
                        }
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
                        // The base data lives in the engine; seed it once from
                        // the demo rows so a fresh session has something to
                        // transform.
                        if self.table_engine.base.rows.is_empty() && !self.table_rows.is_empty() {
                            self.table_engine.base = voiceterm_egui::table_ops::TableData::new(
                                self.table_headers.clone(), self.table_rows.clone());
                        }
                        // RENDER THE RESULT, not the base. apply() walks the op
                        // stack; showing base rows under a "sorted by size"
                        // status is the exact lie this tab used to tell.
                        let shown = self.table_engine.apply();
                        let line = self.table.input.clone();
                        if voiceterm_egui::table_view::table_view_ui(
                            ui, &mut self.table, &shown.headers, &shown.rows, &c).is_some()
                        {
                            use voiceterm_egui::table_view::Submitted;
                            match voiceterm_egui::table_view::submit_with_engine(&mut self.table) {
                                // submit() is generic over op types and throws
                                // the parsed op away, so the engine stack is
                                // fed by re-parsing the same line. One extra
                                // parse of a short command is the price of
                                // that genericity — and keeping BOTH stacks in
                                // step here is what stops the depth counter
                                // from disagreeing with the data.
                                Submitted::Pushed => {
                                    if let Ok(op) = voiceterm_egui::table_nlp::parse_command(&line) {
                                        self.table_engine.push_op(op);
                                    }
                                }
                                Submitted::Undone => { self.table_engine.pop_op(); }
                                Submitted::Reset => self.table_engine.clear_ops(),
                                Submitted::NotUnderstood(_) => {}
                            }
                        }
                    }
                    CentralView::Docs => {
                        // READING MODE. corpus_tape records a paragraph read
                        // aloud, and the corpus it fills is what acoustic_cache
                        // later learns from — "a bad recording here becomes a
                        // bad fingerprint there, and a bad fingerprint promotes
                        // wrong text". So the quality gate is the feature.
                        use voiceterm_egui::corpus_tape as ct;
                        ui.horizontal(|ui| {
                            ui.label("read aloud:");
                            ui.text_edit_singleline(&mut self.tape_reference);
                            if !self.tape.is_active() {
                                // Refused without a reference rather than
                                // recorded and discarded later: worth_keeping
                                // rejects an empty reference, so starting one
                                // is asking the user to read for nothing.
                                let ok = !self.tape_reference.trim().is_empty();
                                if ui.add_enabled(ok, egui::Button::new("record")).clicked() {
                                    self.tape.start(&self.tape_reference);
                                    self.tape_note.clear();
                                }
                            } else if ui.button("stop").clicked() {
                                match self.tape.stop() {
                                    Some(r) if ct::worth_keeping(&r) => {
                                        self.tape_note = format!(
                                            "kept {:.1}s for {:?}",
                                            r.duration_s(), ct::sanitize_id(&r.reference));
                                        // A KEPT TAKE IS A LEARNED FINGERPRINT.
                                        // The store's caps and eviction rule
                                        // apply here rather than at some later
                                        // import step — that is what stops the
                                        // index growing without bound, and it
                                        // is why worth_keeping gates this and
                                        // not the other way round.
                                        use voiceterm_egui::phonetic_store as ps;
                                        let today = (ui.input(|i| i.time) / 86_400.0) as u64;
                                        self.phonetics.add(ps::Entry {
                                            label: ct::sanitize_id(&r.reference),
                                            style: ps::Style::Normal,
                                            // No pitch estimator attached, so
                                            // the sentinel bin — NOT bin 0,
                                            // which would file every unvoiced
                                            // capture with the deepest voices.
                                            pitch_bin: ps::pitch_bin(None),
                                            match_count: 0,
                                            created_day: today,
                                            last_matched_day: today,
                                        });
                                    }
                                    // A take that is too short is DISCARDED and
                                    // said so. Silently keeping it would put a
                                    // fingerprint in the corpus built from a
                                    // fraction of a word.
                                    Some(r) => {
                                        self.tape_note = format!(
                                            "discarded: {:.2}s is under the {:.1}s minimum",
                                            r.duration_s(), ct::MIN_KEEP_S);
                                    }
                                    // No samples at all — which is this build,
                                    // with no microphone attached.
                                    None => self.tape_note =
                                        "no audio captured (no microphone on this build)".into(),
                                }
                            }
                        });
                        if self.tape.is_active() {
                            // The 60s cap is shown while recording, not
                            // discovered at it: a tape that stops on its own
                            // with no warning reads as a crash.
                            let secs = self.tape.len() as f32
                                / self.tape.sample_rate().max(1) as f32;
                            ui.colored_label(c.warning,
                                format!("● {secs:.1}s / {}s max",
                                        ct::MAX_SAMPLES / 48_000));
                        }
                        if !self.tape_note.is_empty() {
                            ui.colored_label(c.text_weak, &self.tape_note);
                        }
                        // WHAT THE CORPUS IS FOR: the promotion policy the
                        // tape's recordings feed. Shown here rather than
                        // buried in settings because the tape's doc names
                        // this as its consumer — a bad recording becomes a
                        // bad fingerprint becomes wrongly promoted text.
                        ui.collapsing("acoustic promotion", |ui| {
                            use voiceterm_egui::acoustic_cache as ac;
                            let (mut same, mut cross) = (self.acoustic.for_match(true),
                                                         self.acoustic.for_match(false));
                            ui.horizontal(|ui| {
                                ui.label("same-speaker max distance");
                                if ui.add(egui::Slider::new(&mut same, 0.0..=1.0)).changed() {
                                    self.acoustic = ac::Thresholds::set(same, cross);
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("cross-speaker max distance");
                                if ui.add(egui::Slider::new(&mut cross, 0.0..=1.0)).changed() {
                                    self.acoustic = ac::Thresholds::set(same, cross);
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("nearest match");
                                ui.add(egui::Slider::new(&mut self.acoustic_distance, 0.0..=1.0));
                                ui.add(egui::DragValue::new(&mut self.acoustic_confirms)
                                       .prefix("confirms "));
                                ui.checkbox(&mut self.acoustic_same_speaker, "same speaker");
                            });
                            let cand = ac::Candidate {
                                transcript: "…".into(),
                                distance: self.acoustic_distance,
                                same_speaker: self.acoustic_same_speaker,
                                confirms: self.acoustic_confirms,
                            };
                            // The VERDICT, not a boolean: TooFar, Unconfirmed
                            // and Disabled are three different reasons a
                            // transcript was not reused, and collapsing them
                            // is how a tuning session becomes guesswork.
                            let v = ac::decide(Some(&cand), &self.acoustic);
                            let col = if v == ac::Verdict::Promote { c.success } else { c.text_weak };
                            ui.colored_label(col, format!("{v:?}"));
                        });
                        // THE OTHER HALF: what the cache matches AGAINST. A
                        // kept take learns a fingerprint here; the promotion
                        // panel above decides whether a later capture is close
                        // enough to reuse it.
                        ui.collapsing(format!("voice fingerprints ({})", self.phonetics.len()), |ui| {
                            use voiceterm_egui::phonetic_store as ps;
                            if self.phonetics.is_empty() {
                                ui.colored_label(c.text_weak,
                                    "nothing learned yet — a kept take adds one");
                            }
                            for e in self.phonetics.entries() {
                                ui.colored_label(c.text_weak, egui::RichText::new(format!(
                                    "{} · {:?} · bin {} · {} match(es)",
                                    e.label, e.style, e.pitch_bin, e.match_count)).size(11.0));
                            }
                            // Pruning is offered rather than automatic: an
                            // index that silently forgot a phrase between two
                            // uses would look like the recogniser regressing.
                            let today = (ui.input(|i| i.time) / 86_400.0) as u64;
                            if !self.phonetics.is_empty()
                                && ui.small_button("prune stale").clicked()
                            {
                                let n = self.phonetics.prune_stale(today);
                                self.tape_note = format!("pruned {n} stale fingerprint(s)");
                            }
                            let _ = ps::pitch_bin(None);
                        });
                        // THE AUDIT CHAIN. Every command hashed into the
                        // previous one's hash, so an edited history fails
                        // verification at the record that was changed.
                        ui.collapsing(format!("command audit ({})", self.audit.len()), |ui| {
                            if self.audit_lines.is_empty() {
                                ui.colored_label(c.text_weak,
                                    "no commands recorded yet (password prompts are never recorded)");
                            }
                            egui::ScrollArea::vertical().id_salt("audit").max_height(160.0)
                                .stick_to_bottom(true).show(ui, |ui| {
                                for line in &self.audit_lines {
                                    ui.label(egui::RichText::new(line).monospace().size(10.0));
                                }
                            });
                            ui.colored_label(c.text_weak,
                                format!("chain tip {}", &self.audit.tail()
                                        .chars().take(16).collect::<String>()));
                        });
                        ui.separator();
                        if let Some(name) = voiceterm_egui::manpage_view::manpage_ui(
                            ui, &mut self.docs, &c)
                        {
                            // The view returns a NAVIGATION target, not a
                            // command: a link inside a document names another
                            // document, and following it is the reader's job.
                            let _ = name;
                        }
                    }
                    CentralView::Models => {
                        // FIRST RUN: the wizard, until it finishes. It owns
                        // the phase machine (Intro → Browse → Downloading →
                        // Done) and reports events; the manual search below
                        // stays available throughout, because a wizard that
                        // hides the tools is a wall when it guesses wrong.
                        if let Some(w) = &mut self.wizard {
                            use voiceterm_egui::setup_wizard::WizardEvent;
                            match w.ui(ui) {
                                Some(WizardEvent::Finish) => self.wizard = None,
                                Some(WizardEvent::Download(action)) => {
                                    // No HTTP client in this crate — the same
                                    // contract the Models search follows.
                                    self.toasts.info(format!("{action:?}"),
                                                     ui.input(|i| i.time));
                                }
                                _ => {}
                            }
                            ui.separator();
                        }
                        use voiceterm_egui::model_picker as mp;
                        ui.horizontal(|ui| {
                            ui.label("search:");
                            ui.text_edit_singleline(&mut self.model_query);
                            if ui.button("open").clicked() && !self.model_query.trim().is_empty() {
                                // URL construction, not fetching: this crate has
                                // no HTTP client and the module was written to
                                // be testable without one.
                                self.toasts.info(mp::search_url(&self.model_query, 20),
                                                 ui.input(|i| i.time));
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("params (B)");
                            ui.add(egui::Slider::new(&mut self.model_params_b, 0.5..=70.0));
                        });
                        ui.horizontal(|ui| {
                            ui.label("budget (GiB)");
                            ui.add(egui::Slider::new(&mut self.model_budget_gib, 1.0..=32.0));
                        });

                        // WHICH QUANT FITS. best_fitting takes the LARGEST that
                        // fits, because quality is the thing being traded away
                        // — take as little of it as the machine forces.
                        match mp::best_fitting(self.model_params_b, self.model_budget_gib) {
                            Some(t) => {
                                ui.colored_label(self.colors.text,
                                    format!("best fit: {}  (~{:.1} GiB)",
                                            t.label(), t.approx_gib(self.model_params_b)));
                            }
                            None => {
                                // Said plainly rather than showing the smallest
                                // anyway: a model that cannot load is not a
                                // recommendation.
                                ui.colored_label(self.colors.warning,
                                    "no quant fits this budget — smaller model or more RAM");
                            }
                        }
                        ui.separator();
                        // Every tier, so the trade-off is visible rather than
                        // decided silently.
                        for t in mp::QuantTier::ALL {
                            let ok = mp::fits(t, self.model_params_b, self.model_budget_gib);
                            let c = if ok { self.colors.text_weak } else { self.colors.warning };
                            ui.colored_label(c, format!("{:<8} {:>6.1} GiB  {}",
                                t.label(), t.approx_gib(self.model_params_b),
                                if ok { "fits" } else { "too big" }));
                        }

                        ui.separator();
                        // WHAT WOULD BE SENT. context_redact strips sensitive
                        // lines out of terminal context BEFORE it becomes a
                        // prompt — its threat model is "don't let sensitive
                        // data escape the process", and an LLM call is the
                        // largest hole in that.
                        //
                        // Shown before any call exists, deliberately: the point
                        // is to see what would leave, and a preview that only
                        // appears once the feature ships is a preview nobody
                        // checks.
                        let ctx: String = (0..self.grid.rows)
                            .map(|y| self.grid.row_text(y))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let total = ctx.lines().count();
                        let red = voiceterm_egui::context_redact::context_strip(&ctx);
                        ui.colored_label(self.colors.text_weak,
                            format!("context: {total} lines, {} redacted", red.lines_redacted));
                        // MARKERS, NEVER THE LINES. The module is explicit that
                        // markers_hit holds which rule fired and "never the
                        // lines themselves" — printing the matched text here
                        // would leak exactly what the redaction removed.
                        if !red.markers_hit.is_empty() {
                            ui.colored_label(self.colors.warning,
                                format!("held back: {}", red.markers_hit.join(", ")));
                        }
                        if total > 0 && !voiceterm_egui::context_redact::worth_sending(&red, total) {
                            // More than half redacted. Sending the remainder
                            // costs tokens for context that has had its
                            // substance removed, and the answer would be built
                            // on the half that survived.
                            ui.colored_label(self.colors.warning,
                                "most of this screen is sensitive — not worth sending");
                        }

                        ui.separator();
                        // TOKEN ACCOUNTING. Empty until something logs a call —
                        // shown as a count rather than hidden, so "no LLM calls
                        // yet" is distinguishable from "the readout is broken".
                        let now = (ui.input(|i| i.time) * 1000.0) as u64;
                        let tot = voiceterm_egui::token_log::totals_by_op(
                            &self.tokens, voiceterm_egui::token_log::TimeWindow::Session,
                            now, now);
                        if tot.is_empty() {
                            ui.colored_label(self.colors.text_weak, "no LLM calls this session");
                        } else {
                            for (op, t) in tot {
                                ui.colored_label(self.colors.text_weak,
                                    format!("{op}: {} calls, {} in / {} out, {} cached",
                                            t.calls, t.input, t.output, t.cached));
                            }
                        }
                    }
                    CentralView::Archive => {
                        // Built from the browser's OWN saved pages, the same
                        // source the explore grouping uses — an archive of
                        // nothing would be the placeholder this replaces.
                        use voiceterm_egui::archived_view as av;
                        let pages: Vec<av::ArchivedPage> = self.browser.bookmarks.all()
                            .iter().map(|b| av::ArchivedPage {
                                url: b.url.clone(),
                                title: b.title.clone(),
                                domain: b.url.split('/').nth(2).unwrap_or("").to_string(),
                                archived_at: String::new(),
                                key_points: Vec::new(),
                                text_snippet: String::new(),
                                html_size: 0,
                            }).collect();
                        if pages.is_empty() {
                            ui.colored_label(c.text_weak, "nothing archived yet — save pages from the Web tab");
                        } else if let Some(url) = av::archive_ui(ui, &mut self.archive, &pages, &c) {
                            // Same contract as the Models tab's open button:
                            // this crate has no HTTP client, so opening means
                            // surfacing the URL.
                            self.toasts.info(url, ui.input(|i| i.time));
                        }
                    }
                    CentralView::Chat => {
                        // The local model's conversation. The panel owns the
                        // transcript and returns what the user sent; there is
                        // no llama.cpp in this crate, so the reply is an
                        // explicit notice rather than a fabricated answer —
                        // an invented assistant turn is worse than none.
                        if let Some(sent) = self.chat.ui(ui) {
                            self.chat.reload(&format!(
                                "user: {sent}\nassistant: (no local model attached in this build)"));
                        }
                    }
                    CentralView::Notes => {
                        // CAPTURE → CARDS → BACKLINKS, in that order, because
                        // that is the dependency: the splitter produces the
                        // notes the index links. Editing the capture re-splits
                        // (the deck keeps its selection BY IDENTITY, which is
                        // why a re-split does not silently move it).
                        use voiceterm_egui::{backlinks_view as bl, cards_view as cv};
                        ui.label(egui::RichText::new("captured text").weak().size(11.0));
                        let changed = ui.add(
                            egui::TextEdit::multiline(&mut self.captured)
                                .desired_rows(4).desired_width(f32::INFINITY)).changed();
                        if changed {
                            let src = cv::CardSource {
                                url: self.browser.url.clone(),
                                paragraph_id: String::new(),
                            };
                            self.deck.set_cards(cv::note_cards::split_into_cards(
                                &self.captured, &src));
                            // The index is rebuilt from the SAME cards the
                            // deck shows, so "what links here" can never
                            // describe a note the deck does not have.
                            let notes: Vec<(String, String)> = self.deck.cards.iter()
                                .map(|c| (c.title.clone(), c.body.clone())).collect();
                            self.backlinks.rebuild(&notes);
                            // The DF table is rebuilt WITH the deck, not
                            // accumulated: TF-IDF against a corpus that no
                            // longer matches the notes suggests tags for text
                            // that is gone.
                            let mut df = voiceterm_egui::nlp::DocFreq::new();
                            for (_, body) in &notes { df.add_document(body); }
                            self.doc_freq = df;
                        }
                        ui.separator();
                        if let Some(title) = cv::deck_ui(ui, &mut self.deck, &c) {
                            // Opening a card sets the backlink context: the
                            // panel below answers about THAT note.
                            self.backlinks.set_current(&title);
                            // …and re-suggests tags for it. Suggestions are
                            // per-card, so they must be recomputed on
                            // selection rather than left showing the previous
                            // card's terms.
                            if let Some(card) = self.deck.selected() {
                                self.tag_suggestions = voiceterm_egui::nlp::suggest_tags(
                                    &card.body, &self.doc_freq, &self.accepted_tags, 6);
                            }
                        }
                        if !self.tag_suggestions.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(c.text_weak, "tags:");
                                let mut taken = None;
                                for tag in &self.tag_suggestions {
                                    if ui.small_button(tag).clicked() { taken = Some(tag.clone()); }
                                }
                                if let Some(tag) = taken {
                                    // Accepting a tag REMOVES it from future
                                    // suggestions (the exclude list is the
                                    // module's own parameter) — re-offering a
                                    // tag the user already applied is noise.
                                    self.tag_suggestions.retain(|t| t != &tag);
                                    self.accepted_tags.push(tag);
                                }
                            });
                        }
                        if !self.accepted_tags.is_empty() {
                            ui.colored_label(c.text_weak,
                                format!("applied: {}", self.accepted_tags.join(", ")));
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!(
                                "backlinks — {} ({})",
                                if self.backlinks.current.is_empty() { "no note selected" }
                                else { &self.backlinks.current },
                                self.backlinks.count_here())).weak().size(11.0));
                            if ui.selectable_label(
                                self.backlinks.listing == bl::Listing::Orphans, "orphans").clicked()
                            {
                                self.backlinks.listing = match self.backlinks.listing {
                                    bl::Listing::Orphans => bl::Listing::Here,
                                    bl::Listing::Here => bl::Listing::Orphans,
                                };
                            }
                            ui.text_edit_singleline(&mut self.backlinks.query);
                        });
                        // The empty reason is SHOWN, not collapsed into "no
                        // results": not-indexed, no-match and nothing-links
                        // are three different problems with three different
                        // fixes.
                        match self.backlinks.empty_reason() {
                            Some(why) => { ui.colored_label(c.text_weak, why); }
                            None => {
                                for (path, ctx) in self.backlinks.rows() {
                                    ui.horizontal(|ui| {
                                        if ui.small_button(&path).clicked() {
                                            self.backlinks.set_current(&path);
                                        }
                                        ui.colored_label(c.text_weak,
                                            egui::RichText::new(ctx).size(11.0));
                                    });
                                }
                            }
                        }
                    }
                    CentralView::Build => {
                        // Compiler output as a navigable list. No cargo is
                        // invoked here — the output is pasted, which is the
                        // same no-subprocess contract the Models tab keeps.
                        ui.horizontal(|ui| {
                            ui.label("paste cargo output:");
                            if ui.button("parse").clicked() {
                                self.build.set_output(&self.build_input);
                            }
                        });
                        ui.add(egui::TextEdit::multiline(&mut self.build_input)
                               .desired_rows(3).desired_width(f32::INFINITY));
                        ui.colored_label(c.text, self.build.status());
                        ui.horizontal(|ui| {
                            use voiceterm_egui::build_view::Show;
                            for (label, show) in [("errors", Show::Errors), ("all", Show::All)] {
                                if ui.selectable_label(self.build.show == show, label).clicked() {
                                    self.build.show = show;
                                }
                            }
                            // Category chips carry their COUNT: "borrow
                            // checker × 12" is a different situation from
                            // "borrow checker × 1" and the chip should say so.
                            for (cat, n) in self.build.categories() {
                                if ui.selectable_label(
                                    self.build.show == Show::Category(cat),
                                    format!("{cat:?} × {n}")).clicked()
                                {
                                    self.build.show = Show::Category(cat);
                                }
                            }
                            ui.text_edit_singleline(&mut self.build.query);
                        });
                        // rows() borrows the view; jump() mutates it (it
                        // records the selection). Collect the labels first so
                        // the click can act — the alternative is a clone of
                        // every diagnostic per frame.
                        let labels: Vec<(usize, String)> = self.build.rows().into_iter()
                            .map(|(o, d)| (o, d.one_line())).collect();
                        let mut jumped = None;
                        egui::ScrollArea::vertical().id_salt("build").show(ui, |ui| {
                            for (ordinal, line) in &labels {
                                if ui.small_button(format!("{ordinal}. {line}")).clicked() {
                                    jumped = Some(*ordinal);
                                }
                            }
                        });
                        if let Some(o) = jumped {
                            // jump() returns the diagnostic to open; there is
                            // no editor attached here, so the target is
                            // surfaced rather than followed.
                            if let Some(hit) = self.build.jump(o) {
                                self.toasts.info(hit.one_line(), ui.input(|i| i.time));
                            }
                        }
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
            // IS THE SHELL ASKING FOR A SECRET? Checked before the line is
            // read, because the answer changes what happens to it: a password
            // is not echoed into the block log and never enters the audit
            // chain. The visible text is the last grid row; there is no PTY
            // here to name a foreground process, so the heuristic runs on the
            // prompt text alone — which is the case the module was written for
            // (`foreground` is None rather than guessed).
            self.password_mode = voiceterm_egui::password_prompt::is_password_prompt(
                &voiceterm_egui::password_prompt::TermState {
                    alt_screen: false,
                    foreground: None,
                    visible: Some(&self.grid.row_text(self.grid.cursor().1)),
                });
            if self.password_mode {
                // SAID OUT LOUD. A terminal that silently stops echoing looks
                // broken; one that says why looks careful — and this is also
                // the only signal that the line will not be audited.
                ui.colored_label(self.colors.warning, "password prompt — input hidden, not recorded");
            }
            let submitted = if chorded { None } else { self.app.handle_input(ui) };
            let key_bytes = self.app.take_pending_bytes();
            self.pty_out.extend_from_slice(&key_bytes);
            // THE SCREEN. Drawn only when there is something on it, so a fresh
            // session shows the block log rather than 24 blank rows.
            if self.grid.is_dirty() || self.grid.cursor() != (0, 0) {
                let (cx, cy) = self.grid.cursor();
                // The grid reports WHICH rows changed; RowCache turns that into
                // the minimal set to re-shape. take_dirty rather than
                // dirty_rows, so the range is consumed once — reading it
                // without clearing would mark the same rows every frame and
                // the cache would never hit.
                if let Some((lo, hi)) = self.grid.take_dirty() {
                    self.row_dirty.mark_range(lo, hi);
                }
                let width = ui.available_width();
                // frame_plan flushes the dirty set and INVALIDATES those
                // slots, returning which rows need rebuilding. The return value
                // is not iterated here — get_or_build below does the work,
                // building exactly the invalidated rows and reusing the rest.
                // Calling it for its effect is the point.
                let _plan = voiceterm_egui::row_render::frame_plan(
                    &mut self.row_dirty, &mut self.row_galleys, width);
                egui::Frame::NONE.show(ui, |ui| {
                    for y in 0..24 {
                        // A hit returns the cached string; a miss shapes it
                        // once. row_text is the expensive part on a wide grid,
                        // and the closure means an unchanged row never calls it.
                        let Some(row) = self.row_galleys
                            .get_or_build(y, || self.grid.row_text(y)) else { continue };
                        if row.trim().is_empty() && y > cy { continue; }
                        // The cursor row is marked rather than drawn as a
                        // block: this is a monospace label, not a canvas, and a
                        // fake caret that sits in the wrong column is worse
                        // than none.
                        let txt = if y == cy { format!("{row}  <{cx}") } else { row };
                        ui.label(egui::RichText::new(txt).monospace().small());
                    }
                });
            }
            if let Some(line) = submitted {
                // AUDIT, before the bytes leave. Every command joins the hash
                // chain — except one typed at a password prompt, which is the
                // one line that must not be recorded anywhere. Skipping the
                // append (rather than logging a redacted placeholder) keeps
                // the chain's meaning exact: it records commands, and that
                // was not one.
                if !self.password_mode && !line.trim().is_empty() {
                    let e = self.audit.append(voiceterm_egui::audit_chain::Entry {
                        ts_ms: (ui.input(|i| i.time) * 1000.0) as u64,
                        command: line.clone(),
                        speaker: None,
                        prev_hash: String::new(),
                    });
                    self.audit_lines.push(e.render());
                    const AUDIT_CAP: usize = 2000;
                    if self.audit_lines.len() > AUDIT_CAP {
                        let cut = self.audit_lines.len() - AUDIT_CAP;
                        self.audit_lines.drain(..cut);
                    }
                }
                // A MUTATING COMMAND ASKS FIRST. cmd_risk already classified
                // every line for the snapshot policy; until now that verdict
                // never stopped anything. The dialog's timeout action is
                // Cancel — a confirmation that runs `rm -rf` because nobody
                // was looking is not a confirmation.
                use voiceterm_egui::cmd_risk::Risk;
                use voiceterm_egui::confirm_dialog::{ConfirmDialog, TimeoutAction};
                if voiceterm_egui::cmd_risk::classify(&line) == Risk::Mutating
                    && self.pending_confirm.is_none()
                {
                    self.pending_confirm = Some((
                        line.clone(),
                        ConfirmDialog::new(
                            "Run this command?",
                            format!("`{line}` can modify or delete data."),
                            "Run",
                        )
                        .with_timeout(20.0)
                        .with_timeout_action(TimeoutAction::Cancel),
                    ));
                } else {
                    // A PTY does not distinguish "the user typed a command"
                    // from "the user typed bytes", and having two paths is how
                    // they drift.
                    self.pty_out.extend_from_slice(line.as_bytes());
                    self.pty_out.push(b'\r');
                }
            }
            // LOCAL ECHO INTO THE SCREEN, since there is no PTY here.
            //
            // The grid is fed from what was SENT rather than from a reply,
            // which is exactly what a terminal does before the shell answers —
            // and it is honest: no output is invented, only the keystrokes that
            // really left. A host that attaches a real PTY calls feed_pty with
            // what comes back and this line becomes redundant rather than
            // wrong.
            //
            // ONLY THE NEW BYTES. pty_out is a buffer a host drains, not a
            // queue this consumes, so it still holds everything sent — feeding
            // all of it each frame would reprint the whole session sixty times
            // a second. The high-water mark is what makes this idempotent.
            if self.pty_out.len() > self.pty_echoed {
                let fresh = self.pty_out[self.pty_echoed..].to_vec();
                self.feed_pty(&fresh);
                self.pty_echoed = self.pty_out.len();
            }
            // The demo has no PTY to write to. Rather than dropping the bytes
            // again, they are kept on the app where a host can drain them, and
            // capped so a long session cannot grow without bound.
            const PTY_OUT_CAP: usize = 64 * 1024;
            if self.pty_out.len() > PTY_OUT_CAP {
                let drop = self.pty_out.len() - PTY_OUT_CAP;
                self.pty_out.drain(..drop);
                // The cap drains from the FRONT, so the mark has to move with
                // it or the next frame re-echoes everything the drain shifted
                // down. Saturating because a large drain can pass the mark.
                self.pty_echoed = self.pty_echoed.saturating_sub(drop);
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
                    // Only a terminal pane draws a grid. The check is on the
                    // pane's VIEW rather than a web flag, so a mail or docs
                    // pane cannot fall through to a terminal renderer.
                    Some(a) if p.view.is_terminal() => {
                        a.fit_to(r.size());
                        a.render_at(ui, &colors, r.min);
                    }
                    _ => {
                        // The pane says what it is. Until each view's renderer
                        // is called from here, naming it is honest and a blank
                        // box is not — an unlabelled pane reads as broken.
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            p.view.label(),
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
                light: voiceterm_egui::iio_light::ambient_light(),
                light_value: None,
                flicks: Default::default(),
                orientation: voiceterm_egui::iio_accel::OrientationUp::Undefined,
                progress: Default::default(),
                // TableView::new(true), not Default: has_data defaults to
                // FALSE, and a view told it has no data reports "nothing to
                // transform" while sitting on four rows. The constructor takes
                // the flag precisely because it cannot be inferred from here.
                mail_phrase: String::new(),
                date_phrase: String::new(),
                // Terminal-only by default: summarising what you typed is
                // local, summarising a page means reading it. Full is a choice
                // the user makes, not one they inherit.
                summary_scope: voiceterm_egui::summary_scope::Scope::Terminal,
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
                table_engine: voiceterm_egui::table_ops::TableView::new(
                    voiceterm_egui::table_ops::TableData::new(
                        vec!["name".into(), "size".into(), "modified".into()],
                        Vec::new())),
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
                editor_phrase: String::new(),
                editor_kind: voiceterm_egui::editor_keys::Editor::Helix,
                editor_pending: None,
                model_query: String::new(),
                model_params_b: 7.0,
                model_budget_gib: 4.0,
                tokens: Vec::new(),
                cmd_docs: default_command_docs(),
                cmd_hits: Vec::new(),
                // 80x24 until a resize: the classic default, and every program
                // that queries COLUMNS gets a plausible answer rather than 0.
                grid: voiceterm_egui::term_grid::TermGrid::new(80, 24),
                pty_echoed: 0,
                row_dirty: voiceterm_egui::row_cache::RowCache::new(24),
                row_galleys: voiceterm_egui::row_render::RowLayoutCache::new(),
                tape: Default::default(),
                tape_reference: String::new(),
                tape_note: String::new(),
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
                mail_messages: Vec::new(),
                mail_open: None,
                audit: Default::default(),
                audit_lines: Vec::new(),
                password_mode: false,
                acoustic: Default::default(),
                acoustic_distance: 0.20,
                acoustic_confirms: 2,
                acoustic_same_speaker: true,
                // has_active_model = false: this build ships no .gguf, and
                // claiming one would send the wizard to the wrong phase.
                wizard: Some(voiceterm_egui::setup_wizard::SetupWizard::new(false)),
                chat: Default::default(),
                glow: Default::default(),
                pending_confirm: None,
                deck: Default::default(),
                captured: String::new(),
                backlinks: Default::default(),
                build: Default::default(),
                build_input: String::new(),
                workspaces: Default::default(),
                doc_freq: voiceterm_egui::nlp::DocFreq::new(),
                tag_suggestions: Vec::new(),
                accepted_tags: Vec::new(),
                summaries: Default::default(),
                summary_text: std::collections::HashMap::new(),
                phonetics: Default::default(),
                glow_active: false,
                workspaces_open: false,
                workspaces_filter: String::new(),
                bar_state: Default::default(),
                graph_source: String::new(),
                settings_open: false,
            }))
        }),
    )
}

/// A small set of real tldr-style pages for the command search.
///
/// command_search deliberately leaves indexing and caching to the shell — the
/// scoring is what it ports. So this ships a handful of genuine summaries
/// rather than a fake index: enough for the scorer to be exercised against real
/// wording, and honest about being a subset.
///
/// `on_path` is CHECKED, not assumed. The field exists so an uninstalled
/// command ranks below an installed one, and hardcoding `true` would make the
/// search confidently recommend something the machine cannot run — which is
/// worse than not suggesting it, because the user tries it first.
fn default_command_docs() -> Vec<voiceterm_egui::command_search::CommandDoc> {
    fn on_path(name: &str) -> bool {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
            .unwrap_or(false)
    }
    let mk = |name: &str, summary: &str, examples: &[&str]| {
        voiceterm_egui::command_search::CommandDoc {
            name: name.to_string(),
            summary: summary.to_string(),
            examples: examples.iter().map(|s| s.to_string()).collect(),
            on_path: on_path(name),
        }
    };
    vec![
        mk("rm", "remove files or directories",
           &["rm file.txt", "rm -r old_logs", "rm -f stale"]),
        mk("find", "search for files in a directory hierarchy",
           &["find . -name '*.log'", "find . -mtime +30 -delete"]),
        mk("grep", "search text for a pattern",
           &["grep -r pattern .", "grep -n error log.txt"]),
        mk("ls", "list directory contents",
           &["ls -la", "ls -lh /var/log"]),
        mk("du", "estimate file space usage",
           &["du -sh *", "du -h --max-depth=1"]),
        mk("df", "report file system disk space usage", &["df -h"]),
        mk("tar", "archive files",
           &["tar czf out.tar.gz dir", "tar xzf in.tar.gz"]),
        mk("git", "the stupid content tracker",
           &["git status", "git log --oneline", "git diff"]),
        mk("ps", "report running processes", &["ps aux", "ps -ef"]),
        mk("kill", "send a signal to a process", &["kill -9 1234"]),
        mk("cp", "copy files and directories", &["cp a b", "cp -r dir dest"]),
        mk("mv", "move or rename files", &["mv old new"]),
        mk("cat", "concatenate and print files", &["cat file.txt"]),
        mk("less", "page through a file", &["less log.txt"]),
        mk("chmod", "change file mode bits", &["chmod +x script.sh"]),
    ]
}

