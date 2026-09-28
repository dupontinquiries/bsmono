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
use voiceterm_egui::surface::{Surface, Surfaces};
use voiceterm_egui::overlay;
use voiceterm_egui::shell::{blocks_drawer_ui, fab_rect, keycap_bar_ui, load_prefs, mic_fab_ui, overflow_counts, paint_terminal_border, save_prefs, settings_panel, summary_menu, tab_row, theme_menu, PaneView, Session, Sidebar, SidebarAction, Workspace, TERMINAL_KEYCAPS};
use voiceterm_egui::slide_menu::SlideOutcome;
use uikit_egui::PaletteColors;
use voiceterm_egui::App;

/// What the central panel is showing.
///
/// The terminal is the default and always one tap away — voiceterm is a
/// terminal first, and a switcher that made it a peer of the mail reader would
/// be the wrong app.
///
/// Uses `voiceterm_egui::shell::PaneView` (the tiling-workspace model's own
/// enum) rather than a second, locally-duplicated one: this file used to
/// declare its own `CentralView` with the same 11 variants, set only by the
/// quick-launcher's two doors (Models/Chat). The palette's `Command::OpenView`
/// (typed or spoken — "mail", "open the graph") already called
/// `workspace.open_view(..)` and that write landed correctly and was tested,
/// but nothing here ever READ it back — the render dispatch below matched on
/// the separate `CentralView` field, so a palette-driven view change updated
/// the model and changed nothing on screen. One enum, read from the
/// workspace's focused pane, is what makes "the pure core already works" also
/// mean "and you can see it."

/// A small zettelkasten for `VOICETERM_DEMO`: three sections with notes under them, so the
/// notes panes and the QR share have something real to show. `[[wiki-links]]` included,
/// because the backlinks index is one of the things that had nothing to index.
/// The bottom bar's height, from `qml/main.qml`: topBar carries
/// `Layout.preferredHeight: (urlBarVisible && tabBar.currentIndex !== 1) ? 50 : 0`
/// and is reparented into `bottomMountSlot`, whose height tracks it.
///
/// A constant rather than a literal at the call site because it is a ported
/// value with a source, and the next person to think the bar looks wrong
/// should find the QML line rather than a bare 50.
const BOTTOM_BAR_H: f32 = 50.0;

const DEMO_NOTES: &str = "\
# Gardens

Compost turns fastest when it is turned weekly; a fortnight is already too long.

Mulch with straw over the beds before the first frost. See [[Frost dates]].

# Frost dates

Last frost here is the second week of May, give or take ten days.

# Kitchen

The sourdough starter wants feeding at the same hour each day more than it wants
a particular flour.

Salt goes in after the autolyse, not before. See [[Gardens]] for the wheat.
";

impl VoicetermApp {
    /// The document ingest surface: diagnose, choose, confirm.
    ///
    /// Every level is a button, recommended first, because the checker guesses
    /// and the operator knows — a capture kept verbatim is the right answer
    /// for a quotation and no score can tell that from a bad one.
    fn ingest_ui(&mut self, ui: &mut egui::Ui, c: &uikit_egui::PaletteColors) {
        use voiceterm_egui::docingest::{self, Level, Phase};
        let now = ui.input(|i| i.time);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("ingest").weak().size(11.0));
            if ui.small_button("load capture").clicked() {
                // The captured text is already in the app; ingest measures it.
                let n = self.captured.len();
                self.ingest.load(self.captured.clone(), 0);
                if n == 0 {
                    self.toasts.warning("nothing captured to ingest", now);
                }
            }
        });

        match self.ingest.phase.clone() {
            Phase::Idle => {}
            Phase::Diagnosed { damage, rec } => {
                ui.label(egui::RichText::new(&rec.because).color(
                    if damage.is_clean() { c.text_weak } else { c.warning }).small());
                ui.horizontal_wrapped(|ui| {
                    for (i, lvl) in self.ingest.options().into_iter().enumerate() {
                        // The recommendation is FIRST and marked, not
                        // preselected — the difference between advice and a
                        // decision made on the operator's behalf.
                        let label = if i == 0 {
                            format!("★ {}", lvl.label())
                        } else {
                            lvl.label().to_string()
                        };
                        if ui.small_button(label).clicked() {
                            self.run_ingest_level(lvl, now);
                        }
                    }
                });
                if rec.level != Level::None {
                    ui.label(egui::RichText::new(
                        "the AI pass can add words that were not captured")
                        .color(c.text_weak).size(10.0));
                }
            }
            Phase::Running(l) => {
                ui.label(egui::RichText::new(format!("running {}…", l.label())).small());
            }
            Phase::Confirming(_) => {
                // The question lives in the toast, which is sticky, so the
                // panel says nothing here — two copies of one question is one
                // too many.
            }
        }
        let _ = docingest::Level::ALL;
    }

    /// Run a level and raise the confirm question.
    fn run_ingest_level(&mut self, lvl: voiceterm_egui::docingest::Level, now: f64) {
        use voiceterm_egui::docingest::{self, Level, Tone, Verdict};
        // WHAT to run and WHAT to say is `docingest::plan` — it was a match
        // in here tangled with toast calls, and the decision worth checking
        // is that an assisted pass without a model says so rather than
        // quietly doing something lesser under the same name.
        let chunks = docingest::chunk_for_model(&self.ingest.original, 4000).len();
        let p = docingest::plan(lvl, chunks);
        if let Some((tone, msg)) = p.note {
            match tone {
                Tone::Info => self.toasts.info(msg, now),
                Tone::Warning => self.toasts.warning(msg, now),
            }
        }
        if p.revert {
            self.ingest.revert();
            return;
        }
        let Some(run) = p.run else { return };
        let verdict = match run {
            Level::Tidy => self.ingest.run(run, |s| docingest::tidy(s)),
            _ => self.ingest.run(Level::Structural, |s| docingest::structural(s)),
        };
        match verdict {
            Verdict::Failed => {
                // No question: there is nothing to keep. The toast reports
                // and expires, because a failure the operator cannot act
                // on should not need dismissing.
                if let Some(msg) = self.ingest.confirm_prompt() {
                    self.toasts.error(msg, now);
                }
                self.ingest.revert();
            }
            _ => {
                if let Some(msg) = self.ingest.confirm_prompt() {
                    self.toasts.ask(msg, &["keep", "revert"], now);
                }
            }
        }
    }

    /// Feed terminal output into the focused pane's grid.
    ///
    /// The parsing moved to `term_grid::Vt`, where it can be tested and
    /// where the crate header already claimed it was. What is left here is
    /// the part that is genuinely the app's: which grid, and which
    /// parser state goes with it.
    fn feed_pty(&mut self, bytes: &[u8]) {
        // BOTH GRIDS, AND THE REASON IS EMBARRASSING.
        //
        // `self.app.render_at` is what actually draws the terminal, so
        // `app.terminal.grid` is the DISPLAYED grid. `session.grid` is
        // the one feed_pty filled. I had it backwards for several
        // commits, reasoning from a `session.grid` row-drawing block
        // that is not the one on screen, and only found out by taking a
        // screenshot of the running app: commands executed and the
        // display never changed.
        //
        // So the shell's output has to reach the displayed grid or
        // nothing appears — that is the actual "text does not go into
        // the terminal" bug, underneath all the input-path ones.
        //
        // ONE grid now. Everything that read `session.grid` — the key
        // mode, the cursor row, the resize dimensions, the LLM screen
        // context, scroll-to-bottom — reads `app.terminal.grid`, which
        // has all of it natively: cols/rows, cur_row, scrollback, and a
        // parser that tracks DECCKM. Feeding two was the temporary state
        // while I worked out which one reaches the screen.
        self.app.terminal.feed(&String::from_utf8_lossy(bytes));

        // ANSWER THE PROGRAM'S QUERIES, back down the same PTY, immediately.
        //
        // This is the helix start-up delay. crossterm programs -- helix, gitui
        // -- send `CSI c` at launch and BLOCK ~2-3 s until the terminal answers.
        // The Qt terminal fixed it in `pty_term.rs` v0.1.784; the egui terminal
        // parsed the query and had nowhere to put an answer. It now queues one,
        // and this is the half that sends it: a reply that is queued and not
        // written is exactly as slow as no reply at all.
        //
        // Right after the feed, not batched to the next frame: the program is
        // already blocked waiting, and every frame of delay is visible as lag.
        let replies = self.app.terminal.take_replies();
        if !replies.is_empty() {
            if let Some(host) = &self.pty {
                let _ = host.write(&replies);
            }
        }

        // THE SHELL'S WORKING DIRECTORY, from the stream it is already sending.
        //
        // `uikit_term::blocks` records a cwd per block and its doc says the value comes from
        // "OSC 7 or a PROMPT_COMMAND export, already resolved by the caller". Nobody was that
        // caller: the only `set_cwd` here is a hardcoded demo path, so every block claimed
        // `/home/kessler/proj` and `session_name`'s repo half was that string forever.
        // `TODO_session_autoname.md` records it as step 1's open half.
        //
        // Read from the BYTES, before the lossy conversion loses a split multi-byte
        // sequence, and taking the LAST announcement in the chunk: a script that cds several
        // times before the terminal is read has moved each time, and where it ended is where
        // it is.
        if let Some(cwd) = self.osc7.feed(bytes).pop() {
            self.blocks.set_cwd(&cwd);
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
        // The mapping and every decision in it — which node kinds are
        // produced, which are deliberately not, which edges the data can
        // honestly support — moved to `connections_graph::build`, where
        // they can be tested. Seventy lines of reasoning in a binary is
        // seventy lines nobody can check.
        use voiceterm_egui::connections_graph as cg;
        let pages: Vec<cg::PageIn> = self.browser.bookmarks.all().iter()
            .map(|b| cg::PageIn { url: b.url.clone(), title: b.title.clone() })
            .collect();
        let threads: Vec<cg::ThreadIn> = self.mail.threads.iter()
            .map(|t| cg::ThreadIn {
                conversation_id: t.conversation_id.clone(),
                subject: t.subject.clone(),
                participants: t.participants.clone(),
            })
            .collect();
        cg::build(&pages, &threads)
    }
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
            // INLINE MARKDOWN. This used to be `escape(t)` — every bold span
            // and every link in a bundled doc came through as literal
            // asterisks and brackets (`**[vision.md](vision.md)**`, verbatim)
            // because nothing between the block-level cases above (headers,
            // fences, blank lines) ever looked at `**`, `` ` `` or `[...](...)`
            // inside a plain paragraph line. README.md and ROADMAP.md — real
            // files, `include_str!`'d below — both use all three.
            html.push_str(&format!("<p>{}</p>", inline_md(t)));
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

/// Inline markdown within one paragraph line: `` `code` ``, `**bold**`, and
/// `[text](url)`. Plain characters are escaped individually so the two never
/// fight over the same pass — a naive "escape first, then look for markup"
/// order would search for `**` after `<`/`>` had already been substituted in,
/// silently missing anything adjacent to an escaped character.
///
/// Single-pass, character-indexed rather than regex: this crate has no regex
/// dependency, and three fixed delimiters don't need one.
fn inline_md(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            if let Some(end) = find_str(&chars, i + 2, "**") {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str("<b>");
                // RECURSE, don't just escape: `**[vision.md](vision.md)**` is
                // a bold LINK, and README.md writes its file list exactly
                // that way. A plain `escape(&inner)` here left the link
                // syntax literal inside the bold tag — passed the first
                // version of this fix, failed the one test that nested two
                // spans instead of one.
                out.push_str(&inline_md(&inner));
                out.push_str("</b>");
                i = end + 2;
                continue;
            }
        } else if chars[i] == '`' {
            if let Some(end) = find_char(&chars, i + 1, '`') {
                let inner: String = chars[i + 1..end].iter().collect();
                // NOT recursed: a code span is the one place markdown
                // convention says formatting inside is suppressed, so
                // `` `**not bold**` `` must stay literal asterisks.
                out.push_str("<code>");
                out.push_str(&escape(&inner));
                out.push_str("</code>");
                i = end + 1;
                continue;
            }
        } else if chars[i] == '[' {
            if let Some(close_bracket) = find_char(&chars, i + 1, ']') {
                if chars.get(close_bracket + 1) == Some(&'(') {
                    if let Some(close_paren) = find_char(&chars, close_bracket + 2, ')') {
                        let text: String = chars[i + 1..close_bracket].iter().collect();
                        let url: String = chars[close_bracket + 2..close_paren].iter().collect();
                        out.push_str(&format!(
                            "<a href=\"{}\">{}</a>", escape(&url), inline_md(&text)));
                        i = close_paren + 1;
                        continue;
                    }
                }
            }
        }
        // No markup matched at `i` (or an opening delimiter with no closing
        // one — a truncated `**bold` at end of line, say): fall through to
        // one escaped plain character, same as the unmatched-delimiter case
        // above via `continue` not firing.
        let mut buf = [0u8; 4];
        out.push_str(&escape(chars[i].encode_utf8(&mut buf)));
        i += 1;
    }
    out
}

/// First index at or after `from` where `chars` contains `needle` verbatim.
fn find_str(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() || from > chars.len() { return None; }
    (from..=chars.len().saturating_sub(n.len())).find(|&i| chars[i..i + n.len()] == n[..])
}

/// First index at or after `from` where `chars` contains `needle`.
fn find_char(chars: &[char], from: usize, needle: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == needle)
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
    /// The session walk player — the half of session_walk that the QML kept
    /// in main.qml and the port left undrawn.
    walk: voiceterm_egui::explore::Walk,
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
    /// `uikit_web::doc::render_page`'s form state for the demo section below
    /// the manual — see the comment at the call site for why this exists at
    /// all. Real page state (per open web pane) belongs to whatever fix the
    /// Web-pane wiring gap gets; this is deliberately not that.
    page_demo_form: uikit_web::doc::FormState,
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
    // grid/pty_echoed/row_dirty/row_galleys/pty_out moved INTO `session`:
    // the typed history must FOLLOW ITS PANE (C11-6a), so everything it is
    // travels as one PaneSession, swapped with focus exactly like pane_apps.
    session: voiceterm_egui::pane_host::PaneSession,
    /// Parked sessions for the panes not holding focus.
    pane_sessions: std::collections::HashMap<u64, voiceterm_egui::pane_host::PaneSession>,
    /// Shaped galleys, one per row, reused while the row is unchanged.
    ///
    /// This caches TEXT LAYOUT, not painting. Immediate mode repaints every
    /// visible row every frame by definition — skipping the paint is not
    /// available and row_render's docs say so. Shaping a row into a galley is
    /// what costs, and it is what this keeps.
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
    /// The Off/Terminal/Web/Full picker, moved off the always-visible bar.
    summary_menu: voiceterm_egui::slide_menu::SlideMenu,
    /// Command blocks (QML blocksDrawer): tracked from what the demo feeds; the
    /// left-edge drawer shows when `surfaces` says `Surface::Blocks`.
    /// Reads the shell's cwd out of the pty stream. See `osc7`.
    osc7: voiceterm_egui::osc7::Scanner,
    blocks: uikit_term::blocks::BlockTracker,
    /// Bytes bound for the shell, split into commands as lines complete.
    line_buf: uikit_term::blocks::CommandLines,
    /// One-shot scroll target for the tab bar ("N more" pill taps apply next frame).
    tabbar_scroll_to: Option<f32>,
    /// The QML toast, ported: fire-and-forget status pill above the bottom chrome.
    toasts: uikit_egui::toast::Toasts,
    /// The shell, when one could be started.
    ///
    /// `Option` rather than required: a spawn failure should degrade to
    /// the local-echo behaviour this app has always had, not stop it
    /// launching. A terminal that will not open because it could not
    /// find a shell is worse than one that opens and says so.
    pty: Option<voiceterm_egui::pty_host::PtyHost>,
    /// One shell per pane, keyed the same way `pane_sessions` is.
    ///
    /// `pty` above is the FOCUSED pane's, kept for the single-pane path
    /// that predates panes; this map is every other pane's. They are
    /// swapped together on a focus change, exactly like the session and
    /// the terminal app, so every existing `self.pty` call site keeps
    /// meaning "the pane you are looking at".
    /// `Option` inside the map, not `PtyHost`, so `swap_focus` can move
    /// the same type in and out of `self.pty` — and so a pane whose
    /// shell failed to spawn is a REMEMBERED None rather than a missing
    /// key that the next swap would try to spawn again.
    pane_ptys: std::collections::HashMap<u64, Option<voiceterm_egui::pty_host::PtyHost>>,
    /// Panes whose shell spawn already failed. Without this a machine
    /// with no usable shell is asked for one sixty times a second.
    pty_spawn_failed: std::collections::HashSet<u64>,
    /// One-shot latch so "shell exited" is said once, not every frame.
    pty_ended_noted: bool,
    /// The QML settingsPopup's launcher half — four doors plus voice
    /// activation and theme. Preferences live in `app.prefs`.
    launcher: uikit_egui::quick_launcher::LauncherState,
    /// The accessibility scene the settings toggles register with.
    a11y: uikit_egui::a11y::A11yScene,
    /// Document ingest: diagnose a captured page, repair it, confirm or revert.
    ingest: voiceterm_egui::docingest::Ingest,
    mail: voiceterm_egui::mail_view::MailView,
    /// Global bookmark store, across every page ever opened — not per pane.
    /// See `browser::Browser`'s doc for why it's this small now.
    browser: voiceterm_egui::browser::Browser,
    /// Per-open-web-pane transient state (URL, title, actions, filter
    /// stats), keyed by the pane's id in the tiling model. Used to be
    /// fields on `Browser` itself, which is why this app could only ever
    /// show one web page at a time regardless of how many web panes
    /// existed — every pane read and wrote the same shared state.
    web_panes: std::collections::HashMap<u64, voiceterm_egui::browser::WebPane>,
    graph: voiceterm_egui::graph_view::GraphView,
    // The real archive surface (archived_view), replacing a bars::ArchiveView
    // that was only ever fed an empty vec — a connected placeholder the
    // wiring checker counted as done.
    archive: voiceterm_egui::archived_view::ArchiveView,
    /// The raw messages beside the MailView: threads hold message IDS, and
    /// the inbox list draws Message rows, so both live here and are set
    /// together.
    mail_messages: Vec<voiceterm_egui::mail_view::Message>,
    /// The MERGED (disk + harvested, deduplicated) list `rebuild_mail`
    /// computes — what `inbox_ui` actually renders. NOT the same thing as
    /// `mail_messages` above, whatever that field's own comment says:
    /// `mail_messages` is harvested-from-terminal-blocks ONLY (an INPUT to
    /// the merge), and disk-scanned mail from `rescan_mail_store` never
    /// touched it. Before this field existed, `inbox_ui` read
    /// `mail_messages` directly, so scanning a real Maildir populated
    /// `self.mail` (which the merge DOES reach, via `set_messages`) while
    /// the actual on-screen inbox list kept reading the un-merged,
    /// terminal-only source and stayed empty — confirmed live: created a
    /// real Maildir message, clicked "rescan mail", the header correctly
    /// said "1 message(s)" while "Inbox" right below still said "No
    /// messages."
    mail_all: Vec<voiceterm_egui::mail_view::Message>,
    /// Proton sign-in: the form, the worker, and the inbox it fetched.
    mail_login: voiceterm_egui::mail_login_view::Form,
    mail_signin: voiceterm_egui::mail_signin::SignIn,
    mail_proton: Vec<voiceterm_egui::mail_view::Message>,
    /// A TOTP code a 2FA account asked for, after the password.
    mail_totp_code: String,
    /// The Proton section collapsed: the pane is also the local mail
    /// reader, and a sign-in form on top of a working inbox is noise.
    mail_login_open: bool,
    /// How many blocks had been scanned for mail last time. The scan walks
    /// closed blocks and parses the email-shaped ones; re-parsing every
    /// block every frame would re-read the whole session's output at frame
    /// rate for a list that changes when a command finishes.
    mail_scanned_blocks: usize,
    /// Mail read off disk, kept apart from the terminal-harvested set so a
    /// rescan of one does not discard the other.
    mail_from_disk: Vec<voiceterm_egui::mail_view::Message>,
    /// Where that came from, for the status line and the rescan button.
    mail_store_path: Option<std::path::PathBuf>,
    /// Body text, keyed by message id, for the lazy-fetch a reader does on
    /// open — kept apart per source for the same reason `mail_from_disk`/
    /// `mail_messages` are: a rescan of one must not discard the other.
    /// `self.mail.bodies` (the cache `message_ui` reads) starts every
    /// message at `Body::NotLoaded`; these are what a message-open now
    /// looks up to fill it in, rather than leaving it permanently empty —
    /// see the commit this field was added in for what "permanently empty"
    /// looked like before.
    mail_bodies_disk: std::collections::HashMap<String, String>,
    mail_bodies_terminal: std::collections::HashMap<String, String>,
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
    /// Ctrl+P's idle-dwell state machine (`palette_flow.rs`): tap opens
    /// the command palette; pausing without typing settles into voice on
    /// its own after a dwell (750ms first time, 1500ms after typing has
    /// pulled it back once this session); Ctrl+P from either open state
    /// closes. Replaced an earlier hold-duration design (`ctrl_p_gesture`
    /// below is still real infrastructure, just no longer what drives
    /// this) after direct design feedback that holding a key felt worse
    /// than a self-adjusting dwell.
    palette_flow: uikit_egui::palette_flow::PaletteFlow,
    /// Kept for other potential hold-gesture bindings -- not currently
    /// called from anywhere in this binary after the palette_flow
    /// redesign above. Real, tested infrastructure (a keyboard adapter
    /// over the shared `cmdstrip::Hold` touch-hold primitive), left in
    /// place rather than removed on the chance something else wants it.
    #[allow(dead_code)]
    ctrl_p_gesture: uikit_egui::ctrl_p_gesture::GestureTracker,
    /// State for the voice popup Ctrl+P opens (via palette_flow's
    /// SwitchedToVoice transition).
    voice_popup: voiceterm_egui::voice_popup::VoicePopup,
    /// The "about to switch on" / "listening" badge drawn beside the
    /// palette's own query box, with its own hold-to-cancel timer.
    mic_indicator: uikit_egui::mic_indicator::MicIndicator,
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
    /// Sharing the zettelkasten as a huge QR (`zk_share`): the picker, the
    /// gates, and the codes once shown. `None` when nothing is being shared.
    zk_share: Option<ZkShareUi>,
    /// This installation's identity, loaded once at startup: the keypair the peer
    /// handshake signs with and the fingerprint it shows. `Unavailable` on a system
    /// with no randomness, and the panel then refuses to open rather than showing a
    /// fabricated one — see `identity`.
    identity: voiceterm_egui::identity::Loaded,
    /// The notebook pane's per-cell state: classifications cached by block id and
    /// output length, and which cells are folded.
    notebook_cells: voiceterm_egui::notebook_cells::NotebookCells,
    /// What this session is about (`session_name`), and the block count it was derived
    /// at — the derivation walks every block and the locator draws every frame.
    session_name: String,
    session_name_at: usize,
    /// What kind of work this session mostly is, from `cmd_cluster`. `None` until there is
    /// enough to say.
    session_cluster: Option<String>,
    /// What `session_auto` suggests doing next, if anything. Surfaced in the locator, never
    /// applied — the module's own doc calls these "surfaced (never auto-applied)", and a
    /// terminal that opened tabs on its own guesses would be worse than one that never
    /// suggested anything.
    session_proposal: Option<voiceterm_egui::session_auto::Proposal>,
    /// The find bar's FILES mode. `None` until a file search is started, so
    /// the panel costs no terminal rows in the common case.
    fs_search: voiceterm_egui::fs_results::Search,
    /// The touch command history. Ranked from the block tracker on open, not
    /// held live — the ranking is recency-weighted and a sheet built once at
    /// startup would show the wrong "most recent" an hour later.
    cmd_sheet: voiceterm_egui::cmd_sheet::CmdSheet,

    /// When the memory footprint was last sampled, in seconds since start.
    /// Only moves when WEB2QML_MEMLOG is set; see voiceterm_egui::memlog.
    last_memlog_s: f64,
    /// The acoustic peer handshake, when one is running. Held as the ENGINE's
    /// state rather than a mirror of it — the QML kept a hand-copied string
    /// and a hand-copied enum is one that drifts.
    peer: Option<(voiceterm_egui::peer_panel::PeerPanel, peer_handshake::HsState)>,
    /// Set while `handshake_audio::capture_modem_samples` is running on its
    /// own thread after `HsEvent::StartListen` — polled each frame with
    /// `try_recv()`; a result feeds `HsEvent::SymbolsCaptured` back into
    /// `peer_handshake::step`. `None` when no capture is in flight.
    peer_capture: Option<std::sync::mpsc::Receiver<Vec<u8>>>,
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
    /// The left rail's width state. Collapsed to an icon strip rather than
    /// hidden: a rail that vanishes takes the tab list with it, and the tabs
    /// are how you get back. Vertical because it costs width once instead of
    /// height per row — on a phone the terminal wants every scanline.
    /// Is the vertical tab column expanded?
    ///
    /// The Qt original could not do this — `vTabBar` is drag-resizable between
    /// 110 and 340 and has no closed state, so on a phone it always costs at
    /// least 110 points of width. This is an improvement on the port's source
    /// rather than a fidelity fix, and worth saying plainly because everything
    /// else in this file is measured against the QML.
    tabbar_open: bool,
    /// The tab column's width when open. egui's `SidePanel` is CONTENT-driven
    /// (chrome_mode.rs's headless measurement: `default_width` only widens
    /// the permitted range), so this is what pins it: the column's content
    /// asks for exactly this width every frame, and the number moves only
    /// when the user drags the panel edge. Before this the column was as
    /// wide as its widest row — one letter, most of the time — and re-laid
    /// itself every time a row's text changed, which is the "things moving
    /// around in the tabs" report.
    tabbar_width: f32,
    /// The keycaps/gesture-chip row. Its own toggle because on a phone it is
    /// a THIRD permanent bottom bar stacked under the bar and the session
    /// strip — see `apply_compact_defaults`.
    keycaps_open: bool,
    /// The session's running note — TODO_notes_clip.md's "active Note"
    /// that clips accumulate into. One note for the whole session is the
    /// smallest useful shape; splitting into multiple named notes is real
    /// future work the TODO itself defers past its first slice.
    clip_note: voiceterm_egui::note_clip::Note,
    /// Where this session's note was first written, so later clips rewrite it rather than
    /// creating a new file each time.
    clip_note_path: Option<std::path::PathBuf>,
    /// Jot mode: spoken text goes into the note rather than to the command parser. Ended by
    /// saying one of `jot::EXIT_PHRASES`.
    jotting: bool,
    /// The last word tapped in a notebook cell and what the notes said about it. Held until
    /// dismissed or acted on — see the note at the tap site.
    word_ask: Option<(String, Vec<voiceterm_egui::note_search::Hit>)>,
    /// The line the tapped word sat on, with the command that produced it — the body and the
    /// provenance of the card "make a card" would create.
    word_ask_line: Option<(String, String, u64)>,
    /// Vim-style `{N}b` / `{N}B` tab cycling. `count_prefix` and `tab_cycle` have both been
    /// pure, tested and wired in the Qt build since v0.1.719, and neither was mounted here.
    counts: voiceterm_egui::count_prefix::CountPrefix,
    /// Whether the one-time narrow-surface default has been applied. Applied
    /// once rather than every frame, so collapsing is a starting position the
    /// user can override, not a cage that re-shuts whatever they open.
    compact_defaults_applied: bool,
    /// Swipe-to-switch-tab, from the kit rather than hand-rolled.
    ///
    /// The rail keeps the screen space; this is how you move between tabs
    /// without reaching for it, which is the half a vertical rail on a phone
    /// otherwise loses. GestureLayer's own header promises "zero per-app
    /// gesture code" — the app supplies a rect each frame and reads back what
    /// the shared model decided, so voiceterm does not grow its own
    /// drag-delta arithmetic and gets the same swipe feel as every other app.
    tab_swipe: uikit_egui::GestureLayer,
    /// Which transient surface is up — blocks drawer, missing packages,
    /// workspaces, settings, favorites, command sheet, or the palette. At most
    /// one, like the QML's modal popups; see `voiceterm_egui::surface`. Every
    /// open/close route (chip, sidebar action, voice command, Ctrl+P, close
    /// button) goes through this — no per-surface flags.
    surfaces: Surfaces,
    /// The command palette's query state. Everything not on the rail is
    /// reachable here or by voice, which is what replaces eleven tabs.
    palette_query: String,
    palette_sel: usize,
    workspaces_filter: String,
    /// The last thing the bottom bar's Launch button was used to run, shown
    /// in the terminal's status line. A field rather than a local because
    /// the bar is drawn in its own panel, before the terminal that reports
    /// it.
    bar_note: String,
    /// Global and per-site web rendering settings. Loaded once; written
    /// when something changes, not per frame.
    web_prefs: voiceterm_egui::web_prefs::WebPrefs,
    /// The graph's SOURCE, not a prebuilt topology.
    ///
    /// A previous pass drew `Topology::new(&[], &[])` — an empty graph, which
    /// the wiring checker now flags as a view fed nothing. The real path, from
    /// the Qt bin's `nifty_topology`, is `niftyplot::parse(source)`: nodes and
    /// edges come from text. Holding the SOURCE means the graph redraws when
    /// the text changes, which is the point of a live diagram.
    graph_source: String,
    /// Which graph the pane draws: the deck's notes (the real zettelkasten,
    /// linked by `[[wikilink]]`) or the typed niftyplot scratch document.
    /// Defaults to the notes — the scratch buffer is a diagramming tool that
    /// happens to share this view.
    graph_from_notes: bool,
    // The settings screen (QML settings page) renders when `surfaces` says
    // `Surface::Settings`; edits apply live (App::apply_settings + palette
    // each frame) and persist to prefs.kv on change.
    /// Terminals for the panes that are NOT focused. `self.app` is always the focused pane's
    /// terminal; on a focus change the two are swapped, so each pane keeps its own scrollback
    /// and every existing `self.app` call site stays as it was.
    pane_apps: std::collections::HashMap<u64, uikit_term::App>,
    /// Which pane `self.app` currently belongs to — the swap's other half.
    pane_focus: u64,
}

impl VoicetermApp {
    /// Carry out a palette/voice command.
    ///
    /// One place, so a command means the same thing however it arrived. The
    /// placement half runs FIRST — a new pane or tab has to exist before the
    /// view can be assigned to it, and assigning then splitting would leave
    /// the content on the pane the user was looking at.
    /// Pick sane starting chrome for the surface, ONCE.
    ///
    /// `main.qml` does not keep this material on screen: the Qt surface puts
    /// the file picker in a `Drawer`, and settings / favourites / voice / the
    /// gesture menus in `Popup`s that open on demand. The egui port turned
    /// several of those into permanently-mounted panels, so on a phone the
    /// bottom bar, the session strip and the keycaps row stack into three
    /// bands under a 168pt tab column, and the terminal — the thing the app
    /// is for — gets what is left. That is the reported "too many panels
    /// visible at once, and so big compared to the Qt ones".
    ///
    /// This is the cheap half of the fix: on a narrow surface, start with the
    /// tab column collapsed to its rail and the keycaps row closed. Both stay
    /// user-toggleable and the choice is applied once, so opening one does
    /// not fight a rule that re-shuts it next frame. The real fix — moving
    /// the on-demand surfaces into overlays the way the QML has them — is
    /// larger and is written up in C13_TODO.
    fn apply_compact_defaults(&mut self, ctx: &egui::Context) {
        if self.compact_defaults_applied {
            return;
        }
        // content_rect is empty before the first layout; waiting costs one
        // frame and avoids concluding "narrow" from an unmeasured surface.
        let w = ctx.content_rect().width();
        if w <= 1.0 {
            return;
        }
        self.compact_defaults_applied = true;
        let compact = w < 600.0;
        if compact {
            self.tabbar_open = false;
            self.keycaps_open = false;
        } else {
            // MINIMAL BY DEFAULT ON DESKTOP TOO. This used to open the
            // keycaps footer (pipe/brace chips, terminal keycap row,
            // summaries scope, a date field, the say field — five things
            // stacked, permanently) on anything wide enough to be a
            // desktop. Compact/mobile already defaulted this closed and
            // relied on gesture/shortcut access instead (bar_ui below);
            // desktop had no equivalent restraint. A terminal emulator
            // that always shows five rows of chrome under the terminal
            // is not what "minimal" means anywhere else in this app.
            self.keycaps_open = false;
        }
    }

    /// The URL of whichever web pane is FOCUSED right now, or empty.
    ///
    /// "Ad filter for this site", "web settings", and note capture's source
    /// URL all mean "whichever page I'm looking at" — and now that web
    /// state is per-pane (`web_panes`, keyed by pane id) rather than one
    /// shared `Browser`, that has to mean the focused pane specifically,
    /// not just any web pane that happens to be open elsewhere in the tab.
    /// A key from the chrome (not the terminal): `{N}b` cycles N tabs forward, `{N}B` back.
    ///
    /// Returns whether the key was CONSUMED. The distinction is the whole point of
    /// `count_prefix`: a digit that is building a count must not also be typed, and a key
    /// that is not ours must pass through untouched or every text field in the app would
    /// start eating `b`.
    ///
    /// `cycle_target` does the arithmetic, including the case a hand-rolled modulo gets
    /// wrong: `5b` on 3 tabs is 2 steps, not 5, and a stale index is clamped into range
    /// rather than panicking.
    fn feed_tab_count(&mut self, ch: char) -> bool {
        use voiceterm_egui::count_prefix::CountAction;
        match self.counts.feed(ch) {
            CountAction::Absorbed(n) => {
                // Shown, because a modeless count with no feedback is a mode you cannot see
                // you are in — the reason vim puts it in the corner.
                self.toasts.info(&format!("{n}"), 0.0);
                true
            }
            CountAction::Fire { cmd, count } => {
                let n = self.workspace.tabs.len();
                let target = voiceterm_egui::tab_cycle::cycle_target(
                    self.workspace.active,
                    n,
                    count as usize,
                    cmd == 'b',
                );
                // Through the workspace's own call, like every other tab move here: a second
                // route would be a second place for the two to disagree.
                while self.workspace.active != target && n > 1 {
                    let before = self.workspace.active;
                    self.workspace.apply(&voiceterm_egui::gesture_menus::WsCall::Command(
                        if cmd == 'b' { "next tab".into() } else { "previous tab".into() },
                    ));
                    if self.workspace.active == before {
                        break; // the workspace refused; do not spin
                    }
                }
                true
            }
            CountAction::PassThrough => false,
        }
    }

    /// Clip one line into the note, with the command that produced it.
    ///
    /// The card path and the clip path are the SAME path: cards in this app are derived from
    /// the note by `split_into_cards`, and the deck is rebuilt from it. Pushing a card
    /// straight into the deck would put it somewhere the next rebuild drops.
    ///
    /// Shares the classifier and the save rule with `ClipSelection` rather than repeating
    /// them — a line clipped this way must look identical to the same line clipped by
    /// selecting it, or the note would carry two kinds of clip that differ by which button
    /// made them.
    fn clip_line(&mut self, line: &str, command: &str, at: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let kind = voiceterm_egui::clip_kind::detect(
            line,
            voiceterm_egui::clip_kind::Context::from_command(command),
        );
        self.clip_note.push_clip(voiceterm_egui::note_clip::Clip {
            kind,
            content: line.to_string(),
            source: voiceterm_egui::note_clip::Source::Terminal {
                command: command.to_string(),
                at,
            },
            captured_at: now,
        });
        let saved = match &self.clip_note_path {
            Some(p) => voiceterm_egui::note_store::save_to(p, &self.clip_note),
            None => voiceterm_egui::note_store::save(&self.clip_note).map(|p| {
                self.clip_note_path = Some(p);
            }),
        };
        match saved {
            Ok(()) => self.toasts.success("card added to the note", 0.0),
            Err(e) => self.toasts.warning(&format!("clipped, but not saved: {e}"), 0.0),
        }
    }

    /// Put the saved notes into the docs reader, if the set has changed.
    ///
    /// Called before the docs pane draws AND before anything opens a note by name: `open`
    /// refuses a document the reader does not hold, so a hit clicked from the notebook would
    /// silently do nothing while the same click from the docs pane worked. That is the kind
    /// of difference nobody reports as a bug, they just stop using the feature.
    ///
    /// Only when the count has changed. `set_docs` prunes the reader's navigation stack
    /// against the new list, so replacing it every frame would drop the page being read the
    /// moment a note was saved.
    fn refresh_note_docs(&mut self) {
        let notes = voiceterm_egui::note_store::documents();
        let have = self.docs.docs().iter().filter(|d| d.name.starts_with("note:")).count();
        if notes.len() == have {
            return;
        }
        let mut all: Vec<voiceterm_egui::manpage_view::Doc> = self
            .docs
            .docs()
            .iter()
            .filter(|d| !d.name.starts_with("note:"))
            .cloned()
            .collect();
        all.extend(notes.iter().map(|(n, t, md)| doc(n, t, md)));
        self.docs.set_docs(all);
    }

    /// What the focused pane can say about where a clip came from.
    ///
    /// `note_clip::Source` has had four variants since v0.1.641 and this app produced ONE:
    /// every clip was `Terminal { command, at }` with whatever had run last, so a paragraph
    /// clipped from a documentation page cited itself as `cargo build`. A citation naming the
    /// wrong source is worse than none — the note looks sourced, and the provenance is the
    /// reason to clip rather than retype.
    fn clip_focus(&self) -> voiceterm_egui::clip_source::Focus {
        use voiceterm_egui::clip_source::Focus;
        let id = self.workspace.focused_pane();
        if self.workspace.pane_view(id) == PaneView::Web {
            if let Some(p) = self.web_panes.get(&id) {
                return Focus::Web { url: p.url.clone(), title: p.title.clone() };
            }
        }
        // The terminal is the fallback rather than the default: a pane with no provenance to
        // offer becomes `Manual`, which cites as "(typed)" — true — instead of an empty
        // command, which cites as an empty backtick pair and reads as a bug.
        match self.blocks.blocks.last() {
            Some(b) => Focus::Terminal { command: b.command.clone(), at: b.started_at },
            None => Focus::Other,
        }
    }

    fn focused_web_url(&self) -> String {
        let id = self.workspace.focused_pane();
        if self.workspace.pane_view(id) != PaneView::Web { return String::new(); }
        self.web_panes.get(&id).map(|p| p.url.clone()).unwrap_or_default()
    }

    /// Rank the session's command blocks against `query` — `recall`'s real scoring, which
    /// until the query field existed only ever ran with an empty string.
    fn rank_recall(&self, query: &str) -> Vec<voiceterm_egui::cmd_sheet::RecallHit> {
        use voiceterm_egui::cmd_sheet::recall;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let items: Vec<recall::RecallItem> = self
            .blocks
            .blocks
            .iter()
            .filter(|b| !b.command.trim().is_empty())
            .map(|b| recall::RecallItem {
                command: b.command.clone(),
                // Truncated by the caller, as recall's own doc says: the snippet is for
                // ranking context, and a whole build log would dwarf every command.
                output_snippet: b.output.chars().take(200).collect(),
                cwd: b.cwd.clone(),
                started_at: b.started_at,
                exit_code: b.exit_code,
            })
            .collect();

        // ACROSS SESSIONS, not just this one. `recall.rs` has had
        // serialize/parse/merge_history since v0.1.660 and the Qt build has used them; this
        // frontend ranked the LIVE blocks only, so "how did I fix that" after a reboot found
        // nothing — which is the exact pain the feature was written for.
        //
        // Lazy, like the Qt path: load, merge, rank, write back, on each query. No
        // per-block finalize hook, and live blocks are folded in at query time so history
        // accrues even if recall is asked rarely. A block typed and never queried before a
        // crash is lost, which is the trade `TODO_recall.md` already accepted and named.
        let path = voiceterm_egui::shell::recall_history_path();
        let persisted = std::fs::read_to_string(&path)
            .ok()
            .map(|t| recall::parse_history(&t))
            .unwrap_or_default();
        let merged = recall::merge_history(persisted, &items);

        // Written back BEFORE ranking, so a query that panics or a process killed while the
        // sheet is open still leaves the history it had just learned.
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Best effort: a read-only config dir must not stop recall from answering. It is
        // the difference between "no history across reboots" and "no recall at all".
        let _ = std::fs::write(&path, recall::serialize_history(&merged));

        recall::recall(query, &merged, now, 40)
    }

    /// Do what a session proposal suggested.
    ///
    /// A new TAB in every case, not a new window or a silent cd: the proposals are about
    /// keeping separate work separate, and a tab is the unit this app already has for that.
    /// The proposal is consumed by the caller before this runs, so a second click cannot
    /// open two tabs for one suggestion.
    fn accept_proposal(&mut self, p: voiceterm_egui::session_auto::Proposal) {
        use voiceterm_egui::gesture_menus::WsCall;
        use voiceterm_egui::session_auto::Proposal;
        self.workspace.apply(&WsCall::Command("new tab".into()));
        match p {
            // The repo is a DIRECTORY the user is already in — the proposal fired on a `cd`
            // — so the new tab needs to go there itself. Typed rather than executed: the
            // command lands on the line and the user presses Enter, which is the same
            // contract every other suggestion in this app keeps.
            Proposal::NewSessionForRepo { repo } => self.stage_command(&format!("cd {repo}")),
            Proposal::NewTabForFile { path, editor } => {
                self.stage_command(&format!("{editor} {path}"))
            }
            // Not produced here: a reboot is an event this frontend does not yet observe
            // (see `session_proposals`). Left as a name rather than a silent fallthrough.
            Proposal::ReentryAfterReboot { .. } => {}
        }
    }

    /// Push-to-talk: open the voice popup listening, from the FAB, the
    /// pane menu or Ctrl+Space. Before this the only way in was the
    /// Ctrl+P dwell — hold the palette open and say nothing for a while —
    /// and a tap on the mic button cleared the terminal.
    fn start_voice(&mut self) {
        if self.surfaces.is(Surface::Voice) { return; }
        if self.voice_popup.start(self.password_mode) {
            self.surfaces.close_if(Surface::Palette);
            self.surfaces.open(Surface::Voice);
        } else {
            self.toasts.info("the microphone is off while a password is being typed", 0.0);
        }
    }

    /// A transcript arrived: through the one router, then the effects.
    ///
    /// Same chain as the typed `say:` box — a phrase means the same thing
    /// however it got here. Shell lines are STAGED on the input line and
    /// never run; the person presses Enter, or erases it.
    fn on_transcript(&mut self, said: &str) {
        use voiceterm_egui::voice_router::{route, Ctx, Routed};
        let ctx = Ctx { jotting: self.jotting, editor: self.editor_kind };
        let routed = route(said, &ctx, &self.cmd_docs);
        let summary = routed.summary();
        match routed {
            Routed::Jot { prose, exit } => {
                if let Some(t) = prose {
                    self.clip_note.push_prose(&t);
                    let saved = match &self.clip_note_path {
                        Some(p) => voiceterm_egui::note_store::save_to(p, &self.clip_note),
                        None => voiceterm_egui::note_store::save(&self.clip_note)
                            .map(|p| self.clip_note_path = Some(p)),
                    };
                    if let Err(e) = saved {
                        self.toasts.warning(&format!("jotted, but not saved: {e}"), 0.0);
                    }
                }
                if exit { self.jotting = false; }
                self.toasts.info(&summary, 0.0);
            }
            Routed::Prose(_) => self.toasts.info(&summary, 0.0),
            Routed::Editor(a) if a.confirm => {
                // Destructive actions WAIT for the existing confirm card.
                self.editor_pending = Some(a);
            }
            Routed::Editor(a) => {
                self.session.pty_out.extend_from_slice(a.keys.as_bytes());
            }
            Routed::Command(cmd) => {
                self.run_command(cmd);
                self.cmd_hits.clear();
            }
            Routed::Stage(line) => {
                self.stage_command(&line);
                self.bar_note = format!("staged: {line}");
                self.cmd_hits.clear();
            }
            Routed::Described(_) => {
                self.toasts.info(&summary, 0.0);
                self.cmd_hits.clear();
            }
            Routed::Candidates(hits) => {
                if hits.is_empty() { self.toasts.info(&summary, 0.0); }
                self.cmd_hits = hits;
            }
        }
        eprintln!("voiceterm: heard {said:?} -> {summary}");
    }

    fn run_command(&mut self, cmd: voiceterm_egui::palette::Command) {
        use voiceterm_egui::gesture_menus::WsCall;
        use voiceterm_egui::palette::{Command, Target};
        match cmd {
            Command::OpenView { view, target } => {
                match target {
                    Target::Focused => {}
                    Target::NewPane => { self.workspace.apply(&WsCall::Command("split right".into())); }
                    Target::NewTab => { self.workspace.apply(&WsCall::Command("new tab".into())); }
                }
                self.workspace.open_view(view);
            }
            Command::Split { vertical } => {
                self.workspace.apply(&WsCall::Command(
                    if vertical { "split down".into() } else { "split right".to_string() }));
            }
            Command::ClosePane => { self.workspace.apply(&WsCall::Command("close pane".into())); }
            Command::NewTab => { self.workspace.apply(&WsCall::Command("new tab".into())); }
            Command::NextTab => { self.workspace.apply(&WsCall::Command("next tab".into())); }
            Command::PrevTab => { self.workspace.apply(&WsCall::Command("previous tab".into())); }
            Command::ShowPalette => {
                self.surfaces.open(Surface::Palette);
                self.palette_query.clear();
                self.palette_sel = 0;
            }
            // Repointed from `rail_open` to `tabbar_open`: the palette entry
            // ("Toggle sidebar") and the voice phrase ("hide the sidebar") keep
            // working and now move the column that actually holds the tabs.
            Command::ToggleRail => { self.tabbar_open = !self.tabbar_open; }
            Command::ToggleSettings => { self.surfaces.toggle(Surface::Settings); }
            Command::ToggleWebSettings => { self.surfaces.toggle(Surface::WebSettings); }
            // The deliberate switch `Chrome::toggle` was documented as
            // needing — see the comment on the command itself. The chord
            // (Ctrl+Shift+M) is now escape/resume only, not entry.
            Command::ToggleChrome => { self.chrome.toggle(); }
            Command::ToggleKeycaps => { self.keycaps_open = !self.keycaps_open; }
            // TODO_notes_clip.md's smallest slice: "Terminal: selection
            // already works... 'clip this' over a terminal selection".
            // Attribution is best-effort — there is no link from a
            // Selection back to the CommandBlock it was made within, so
            // the most recent block is the closest approximation. True
            // for the common case (clipping something just seen), not
            // guaranteed for a selection made further back in scrollback.
            Command::Recall(q) => {
                let hits = self.rank_recall(&q);
                self.cmd_sheet.open_asking(&q, hits);
                self.surfaces.open(Surface::CmdSheet);
            }
            Command::PeerHandshake => {
                // REAL VALUES OR NONE. The panel is a trust decision; opening it with
                // placeholder identity strings would look like proof while being
                // fabricated (TODO_multi_device_share.md's own reason for leaving it
                // unreachable). Without an identity it says so instead.
                match self.identity.identity() {
                    Some(id) => {
                        let digest = voiceterm_egui::identity::workspace_digest(
                            self.deck.cards.iter().map(|c| (c.title.as_str(), c.body.as_str())));
                        self.peer = Some((
                            voiceterm_egui::peer_panel::PeerPanel {
                                my_id: id.handle.clone(),
                                my_key_fp: id.vt_id.clone(),
                                workspace_digest: digest,
                                // No voice profile is enrolled in this build, so the
                                // panel offers no voice step — which it SAYS rather
                                // than hiding, and which is honest: claiming a
                                // voiceprint we cannot produce would make the peer
                                // wait for a step that never comes.
                                my_voiceprint_ref: String::new(),
                                verify_msg: String::new(),
                            },
                            peer_handshake::HsState::Idle,
                        ));
                    }
                    None => {
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs_f64()).unwrap_or(0.0);
                        self.toasts.warning(self.identity.note(), now);
                    }
                }
            }
            Command::ShareNotesQr => {
                let share = voiceterm_egui::zk_share::Share::from_cards(&self.deck.cards);
                self.zk_share = Some(ZkShareUi { share, dialog: None, frames: Vec::new(), shown: None, note: String::new() });
            }
            Command::DocsForLastCommand => {
                use voiceterm_egui::doc_resolver::{self as dr, DocSource};
                // The query is the last command's SUBJECT, not the whole line.
                // `extract_query` strips env assignments, wrapper commands and the pipeline
                // tail, and keeps the subcommand for tools that dispatch on one — 11 tests,
                // three real bugs caught by its own probes, and no caller in this frontend
                // until now.
                let line = self.blocks.blocks.last().map(|b| b.command.clone()).unwrap_or_default();
                let Some(query) = dr::extract_query(&line) else {
                    self.toasts.info("nothing to look up yet", 0.0);
                    return;
                };
                let facts = voiceterm_egui::doc_avail::Facts { docs: &self.cmd_docs };
                let source = dr::resolve(
                    &query,
                    &dr::Availability {
                        bundled_man: &|k| facts.bundled_man(k),
                        has_tldr: &|n| facts.has_tldr(n),
                        on_path: &|n| facts.on_path(n),
                    },
                );
                match source {
                    // The two that RUN something are staged, not executed — the contract
                    // every other suggestion here keeps, and `man` on the wrong page is a
                    // keystroke from being fixed.
                    DocSource::SystemMan(arg) => self.stage_command(&format!("man {arg}")),
                    DocSource::HelpFlag(cmd) => self.stage_command(&format!("{cmd} --help")),
                    // tldr is a command here too: this build indexes tldr pages for search
                    // but does not render one, so staging it is honest where opening a
                    // renderer that does not exist would not be.
                    DocSource::Tldr(name) => self.stage_command(&format!("tldr {name}")),
                    // Not reachable in this build — see `doc_avail::bundled_man` — but
                    // matched by name so the day the corpus is mountable this is the one
                    // line that changes.
                    DocSource::BundledManPage(key) => {
                        self.toasts.info(&format!("no bundled page for {key} in this build"), 0.0)
                    }
                    DocSource::WebSearch(q) => {
                        self.toasts.info(&format!("no local docs for {q:?} — try the web pane"), 0.0)
                    }
                }
            }
            Command::OpenInEditor(phrase) => {
                use voiceterm_egui::editor_open as eo;
                match eo::parse_open_request(&phrase) {
                    Some(req) => {
                        // The host, against the real ssh config plus whatever this session
                        // has been talking to. Config first — see `known_hosts`.
                        let hosts = voiceterm_egui::editor_open_env::known_hosts(
                            &voiceterm_egui::editor_open_env::ssh_config_hosts(),
                            &[],
                        );
                        let host = req
                            .host
                            .as_deref()
                            .and_then(|h| eo::resolve_host(h, &hosts))
                            .map(|(h, _)| h);
                        // A host that was ASKED FOR and did not resolve is refused rather
                        // than silently opened locally: "edit the config on the build box"
                        // run against this machine edits the wrong file, and the command
                        // would look like it worked.
                        if req.host.is_some() && host.is_none() {
                            self.toasts.warning(
                                &format!("no host matching {:?}", req.host.unwrap_or_default()),
                                0.0,
                            );
                            return;
                        }
                        let spec = eo::OpenSpec {
                            editor: req.editor.unwrap_or_else(
                                voiceterm_egui::editor_open_env::default_editor),
                            path: req.thing.clone(),
                            line: req.line,
                            host,
                        };
                        // STAGED, never run — the Qt build's contract, and the one every
                        // other suggestion here keeps. A fuzzy path resolved to the wrong
                        // file is a keystroke away from being fixed, and a keystroke past
                        // being opened.
                        self.stage_command(&eo::build_open_argv(&spec).join(" "));
                    }
                    None => self.toasts.warning("nothing to open in that", 0.0),
                }
            }
            Command::ToggleJotMode => {
                self.jotting = !self.jotting;
                // Said out loud both ways. A mode that silently swallows your next sentence
                // into a note is worse than one that never existed, and the exit phrase is
                // no use to someone who does not know they are in it.
                if self.jotting {
                    self.toasts.info("jotting — say \"done jotting\" to stop", 0.0);
                } else {
                    self.toasts.info("done jotting", 0.0);
                }
            }
            Command::ClipSelection => {
                let content = self.app.selection.take().map(|s| s.text);
                match content {
                    Some(text) if !text.trim().is_empty() => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs()).unwrap_or(0);
                        // WHERE IT CAME FROM, decided rather than assumed — see
                        // `clip_focus`. This was always `Source::Terminal` with the last
                        // command, whatever pane was focused.
                        let focus = self.clip_focus();
                        let source = voiceterm_egui::clip_source::of(&focus);
                        // The classifier still wants a command for its context guess; a web
                        // page has none, and `Context::from_command("")` is the neutral
                        // answer rather than a wrong one.
                        let command = match &focus {
                            voiceterm_egui::clip_source::Focus::Terminal { command, .. } => command.clone(),
                            _ => String::new(),
                        };
                        // CODE OR PROSE, decided rather than assumed. This was
                        // `ClipKind::Text` always, so a clipped diff or function became
                        // a blockquote — markdown reflows it, eats the leading spaces
                        // and italicises whatever sits between asterisks.
                        let kind = voiceterm_egui::clip_kind::detect(
                            &text, voiceterm_egui::clip_kind::Context::from_command(&command));
                        // Kept for the related-notes query below: `text` moves into the
                        // clip, and searching the rendered markdown afterwards would search
                        // the citation too.
                        let clipped_text = text.clone();
                        let clip = voiceterm_egui::note_clip::Clip {
                            kind,
                            content: text,
                            source,
                            captured_at: now,
                        };
                        self.clip_note.push_clip(clip);
                        // AND ON TO DISK. `Note::to_markdown` has been pure and tested since
                        // v0.1.641 and nothing wrote its output anywhere, so every clip died
                        // with the process — a clipping tool that loses the clips is the one
                        // failure it cannot have.
                        //
                        // Saved on every clip rather than on quit: a terminal is closed by
                        // closing its window, and a note that only survives a graceful exit
                        // does not survive the way this app is actually ended.
                        // The note GROWS: each clip appends a block and the whole thing is
                        // re-rendered, so the path is chosen once and rewritten after.
                        // Saving afresh each time would scatter one note across
                        // `build-notes.md`, `-2`, `-3` — one file per clip, each a prefix of
                        // the next.
                        let saved = match &self.clip_note_path {
                            Some(p) => voiceterm_egui::note_store::save_to(p, &self.clip_note),
                            None => voiceterm_egui::note_store::save(&self.clip_note).map(|p| {
                                self.clip_note_path = Some(p);
                            }),
                        };
                        // WHAT THIS BELONGS WITH. `TODO_notes_clip.md` step 4 wants related
                        // clips from a word; this is the moment the question answers itself,
                        // because the text was just selected and the corpus is on disk.
                        //
                        // The note being written is excluded: it now CONTAINS the clip, so it
                        // would always be the top hit and always be useless.
                        let related = {
                            let corpus = voiceterm_egui::note_store::documents();
                            let mine = self
                                .clip_note_path
                                .as_ref()
                                .and_then(|p| p.file_stem())
                                .map(|s| format!("note:{}", s.to_string_lossy()));
                            let corpus: Vec<_> = corpus
                                .into_iter()
                                .filter(|(n, _, _)| Some(n) != mine.as_ref())
                                .collect();
                            voiceterm_egui::note_search::search(&clipped_text, &corpus, 1)
                        };
                        match saved {
                            Ok(_) => {
                                // Named, not counted. "3 related notes" is a number you
                                // cannot act on; a title is one you can go and open.
                                match related.first() {
                                    Some(h) => self.toasts.success(
                                        &format!("Clipped \u{2014} related: {}", h.title), 0.0),
                                    None => self.toasts.success("Clipped to notes", 0.0),
                                }
                            }
                            // Said out loud. A silent write failure leaves someone clipping
                            // into a void for an afternoon.
                            Err(e) => self
                                .toasts
                                .warning(&format!("clipped, but not saved: {e}"), 0.0),
                        }
                    }
                    _ => { self.toasts.warning("nothing selected to clip", 0.0); }
                }
            }
            // A site rule from the palette, without opening a panel: this
            // is what people actually want when a page has eaten its own
            // sidebar or is unreadable through the ads.
            Command::SiteFilter(level) => {
                use voiceterm_egui::browser::FilterMode;
                use voiceterm_egui::palette::FilterLevel;
                let mode = match level {
                    FilterLevel::Off => FilterMode::Off,
                    FilterLevel::Standard => FilterMode::Standard,
                    FilterLevel::Aggressive => FilterMode::Aggressive,
                };
                let __focused_web_url = self.focused_web_url();
                match voiceterm_egui::web_prefs::host_of(&__focused_web_url) {
                    Some(h) => {
                        self.web_prefs.site_mut(&h).filter = Some(mode);
                        self.save_web_prefs();
                        self.toasts.info(format!("{h}: ads {}", level.label()), 0.0);
                    }
                    // Refused rather than applied globally. "For this site"
                    // with no site is a command whose target the person
                    // cannot see, and silently making it global is the
                    // worst reading of it.
                    None => self.toasts.warning("no site open", 0.0),
                }
            }
            Command::SiteReset => {
                let __focused_web_url = self.focused_web_url();
                match voiceterm_egui::web_prefs::host_of(&__focused_web_url) {
                    Some(h) => {
                        self.web_prefs.sites.retain(|(p, _)| *p != h);
                        self.save_web_prefs();
                        self.toasts.info(format!("{h}: following the defaults"), 0.0);
                    }
                    None => self.toasts.warning("no site open", 0.0),
                }
            }
        }
    }

    /// The palette overlay: type to filter, Enter to run, Escape to dismiss.
    fn palette_ui(&mut self, ctx: &egui::Context, open_t: f32) {
        let entries = voiceterm_egui::palette::vocabulary();
        // TYPED QUESTIONS REACH RECALL TOO, and NOT through the fuzzy matcher. "how did
        // I fix the nginx config" is not a subsequence of any title — the 'w' of "how"
        // has nothing after "What did I do" to match — so `search` drops it entirely and
        // the question fell through to whatever else scored. The same phrase parser the
        // voice path uses runs over the typed query, and a question is prepended to the
        // HITS as its own row carrying the stripped words. One grammar, both doors.
        let asked = voiceterm_egui::palette::recall_query(&self.palette_query)
            .filter(|q| !q.is_empty())
            .map(|q| voiceterm_egui::palette::Entry {
                command: voiceterm_egui::palette::Command::Recall(q.clone()),
                title: format!("What did I do \u{2014} \u{201c}{q}\u{201d}"),
                keywords: &[],
            });
        let mut chosen = None;
        let mut close = false;
        // The QML puts the palette in a right-edge drawer (paletteDrawer,
        // main.qml:31310), not a floating window; the form table agrees.
        let out = overlay::show(ctx, Surface::Palette, open_t, |ui| {
                let resp = ui.add(egui::TextEdit::singleline(&mut self.palette_query).id_salt("voiceterm_egui-self_palette_query"));
                // Focused on open, because a palette you have to click into
                // costs the keystroke it exists to save.
                resp.request_focus();
                let mut hits = voiceterm_egui::palette::search(&entries, &self.palette_query);
                if let Some(a) = asked.as_ref() {
                    hits.insert(0, a);
                }
                if self.palette_sel >= hits.len() { self.palette_sel = 0; }
                if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    self.palette_sel = (self.palette_sel + 1).min(hits.len().saturating_sub(1));
                }
                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    self.palette_sel = self.palette_sel.saturating_sub(1);
                }
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for (i, e) in hits.iter().enumerate() {
                        if ui.selectable_label(i == self.palette_sel, &e.title).clicked() {
                            chosen = Some(e.command.clone());
                        }
                    }
                });
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Some(e) = hits.get(self.palette_sel) {
                        chosen = Some(e.command.clone());
                    }
                }
            });
        if out.closed { close = true; } // backdrop click or Esc, via Modal
        if let Some(cmd) = chosen {
            self.run_command(cmd);
            close = true;
        }
        if close {
            // `close_if`, not `close`: running ToggleSettings from here just
            // replaced Palette with Settings, and a blind close would shut the
            // panel the command opened. surface.rs has the test.
            self.surfaces.close_if(Surface::Palette);
            self.palette_query.clear();
            self.palette_sel = 0;
        }
    }
}

impl VoicetermApp {
    /// Record any commands in bytes bound for the shell.
    ///
    /// The line rules live in `uikit_term::blocks::CommandLines` — they are
    /// not app policy, they are how a byte stream becomes commands, and
    /// they were three fiddly cases (backspace before the control-byte
    /// check, escape sequences rejected, `\r` and `\n` both ending a line)
    /// sitting inline in a frame loop where nothing could test them.
    /// Type a command into the shell WITHOUT running it, and keep the
    /// widget's line buffer in step.
    ///
    /// ## The hole this closes
    ///
    /// Several surfaces "type, not run" a command — the favourites
    /// pane's `cd`, the command sheet's drop, a clicked search hit, a
    /// staged card. Each appended to `pty_out` only. The shell's line
    /// editor then held a command that `app.terminal.input` knew nothing
    /// about.
    ///
    /// That matters because `cmd_risk::withhold_enter` reads
    /// `app.terminal.input` to decide whether to hold the Enter byte. So
    /// dropping `rm -rf build` from the command sheet and pressing Enter
    /// classified the EMPTY string, found it not Mutating, and ran the
    /// command with no confirmation at all. The gate was not bypassed by
    /// a clever input; it simply never saw the line.
    ///
    /// One funnel, so a surface cannot add a command path and forget the
    /// mirror. `stage` replaces the buffer rather than appending,
    /// matching what these surfaces do to the shell's line.
    /// Send bytes to the SHELL, as if typed.
    ///
    /// `Terminal::feed` is the OUTPUT path — it parses bytes as terminal
    /// output into a grid. Several input affordances called it: the
    /// keycap bar, the pipe menu, the brace menu. Pressing a keycap
    /// therefore parsed the character into `app.terminal`'s grid, which
    /// this view does not draw, and sent the shell nothing at all. Wrong
    /// grid and wrong direction at once.
    ///
    /// Printable bytes are mirrored into the widget's line buffer for
    /// the same reason `stage_command` mirrors: `cmd_risk::withhold_enter`
    /// reads that buffer, and a `|` or a `{` inserted by a menu is part
    /// of the line it will classify.
    fn send_input(&mut self, bytes: &[u8]) {
        // Reading history and then typing means you want to see what you
        // typed. Every other terminal does this and its absence reads as
        // the keyboard being broken.
        self.app.terminal.grid.scroll_to_bottom();
        self.session.pty_out.extend_from_slice(bytes);
        if let Ok(t) = std::str::from_utf8(bytes) {
            for ch in t.chars().filter(|c| !c.is_control()) {
                self.app.terminal.input.push(ch);
            }
        }
    }

    /// Show a line on the terminal screen WITHOUT sending it to the
    /// shell — an answer this app produced itself.
    ///
    /// Goes through the session grid, the one that is drawn. Feeding
    /// `app.terminal` put these lines on a grid nothing renders, so the
    /// feature looked like it did nothing.
    fn show_line(&mut self, text: &str) {
        self.feed_pty(text.as_bytes());
    }

    fn stage_command(&mut self, cmd: &str) {
        // Kill whatever is on the shell's line first. `Terminal::stage`
        // REPLACES the widget's buffer, but appending to the shell only
        // adds — so staging on top of a half-typed line left the shell
        // holding `ls rm -rf build` while the mirror said `rm -rf build`.
        // The mirror has to be exact, not approximate, because a safety
        // check reads it.
        self.session.pty_out.push(0x15); // Ctrl-U, unix-line-discard
        self.session.pty_out.extend_from_slice(cmd.as_bytes());
        self.app.terminal.stage(cmd);
    }

    fn record_typed(&mut self, bytes: &[u8]) {
        for cmd in self.line_buf.feed(bytes) {
            // `start_row`: where the block begins in the scrollback. The
            // kit's tracker stamps its own wall clock.
            let row = self.app.terminal.grid.cur_row;
            self.blocks.submit_command(&cmd, row);
        }
    }

    /// Harvest email-shaped command output into the mail view.
    ///
    /// `MailView::set_messages` had no caller and `mail_messages` was
    /// `Vec::new()` for the life of the process, so the inbox, the thread
    /// list and the message pane were correct renderings of nothing. The
    /// Proton bridge is the intended source and is not reachable from a
    /// widget crate — `mail_proton` drags the HTTP and scraper stack, which
    /// the frontend's own header already records as the reason it is not
    /// mounted.
    ///
    /// The terminal is the source that is reachable. `BlockKind::Email`
    /// has classified `mail`, `mutt -e display` and `cat ~/Maildir/…`
    /// output since it was written, with nothing downstream of it.
    ///
    /// CLOSED blocks only: an open one is still receiving output, and
    /// parsing a half-arrived message shows a mail with no body that never
    /// updates.
    /// Read the machine's mail store, if it has one.
    ///
    /// Called on demand, not per frame: a Maildir scan is thousands of
    /// `stat`s and file reads, and doing that at frame rate would stall the
    /// UI for as long as the app is open. `discover` picks the first of
    /// `$MAIL`, `~/Maildir`, `~/mail`, `/var/mail/$USER` that holds
    /// anything.
    fn rescan_mail_store(&mut self) {
        match voiceterm_egui::mail_store::discover() {
            Some((path, _kind)) => {
                self.mail_from_disk = voiceterm_egui::mail_store::load(&path);
                self.mail_bodies_disk = voiceterm_egui::mail_store::load_bodies(&path);
                self.mail_store_path = Some(path);
            }
            None => {
                // Cleared, not left: a store that has gone away must not
                // keep showing its last contents as though they were live.
                self.mail_from_disk.clear();
                self.mail_bodies_disk.clear();
                self.mail_store_path = None;
            }
        }
        self.rebuild_mail();
    }

    /// Merge the two sources into the view.
    ///
    /// Disk first, then anything harvested from the terminal that is not
    /// already there. De-duplicated by message id, because `cat`-ing a file
    /// that is also in the Maildir is the ordinary way to read mail in a
    /// terminal — and showing it twice would make the inbox wrong precisely
    /// for the person using both halves of this app.
    fn rebuild_mail(&mut self) {
        // The rule lives in `mail_store::merge`. It was here, and the test
        // for it copied the loop instead of calling it — proving that a
        // reimplementation worked, which is the one thing a test cannot
        // usefully prove.
        let mut all = voiceterm_egui::mail_store::merge(
            &self.mail_from_disk, &self.mail_messages);
        // The account's inbox, ahead of what was found on disk and in the
        // terminal: it is the live source, and its ids are Proton's own,
        // so nothing on disk shares them. Same de-dup rule as the merge
        // — by id — so a re-fetch does not double the list.
        let mut merged: Vec<voiceterm_egui::mail_view::Message> = self.mail_proton.clone();
        for m in all.drain(..) {
            if !merged.iter().any(|p| p.id == m.id) { merged.push(m); }
        }
        self.mail.set_messages(&merged);
        self.mail_all = merged;
    }

    /// Drain the sign-in worker and fold what it fetched into the view.
    fn pump_mail_signin(&mut self) {
        for ev in self.mail_signin.poll() {
            use voiceterm_egui::mail_signin::Event;
            match ev {
                Event::Inbox { messages, .. } => {
                    self.mail_login.busy = false;
                    self.mail_proton = messages;
                    self.rebuild_mail();
                    self.mail_login_open = false;
                }
                Event::Failed { message, retryable } => {
                    self.mail_login.busy = false;
                    self.mail_login.error = Some(message);
                    if !retryable { self.mail_login_open = false; }
                }
                Event::Unavailable => {
                    self.mail_login.busy = false;
                    self.mail_login.error = Some(self.mail_signin.status.clone());
                }
                Event::NeedsTotp | Event::SignedIn { .. } => {}
            }
        }
    }

    fn harvest_mail_from_blocks(&mut self) {
        let blocks = &self.blocks.blocks;
        if blocks.len() == self.mail_scanned_blocks {
            return;
        }
        self.mail_scanned_blocks = blocks.len();
        let mut found = Vec::new();
        // Keyed the SAME way `load_bodies` keys the disk side: per split
        // part, with `messages_from_text`'s own `"{origin}#{i}"` fallback-id
        // convention, not a bare per-block origin. A block whose output is
        // itself mbox-shaped (piped through `cat` on an mbox file, say) can
        // still contain more than one message with no Message-ID each —
        // the same collision this session already caught and fixed on the
        // disk side would reappear here with a bare origin.
        let mut bodies = std::collections::HashMap::new();
        for b in blocks.iter() {
            // `duration_ms` is the kit's "this block has ended" — `finish`
            // is the only thing that sets it, and it is None while the
            // command is still running. Checked rather than assumed,
            // because a half-arrived message shows as a mail with no body
            // that never fills in.
            if b.duration_ms.is_none() {
                continue;
            }
            // The live block engine now carries kind detection itself; this was the
            // one call keeping a second 415-line copy of it alive.
            if !uikit_term::blocks::CommandBlock::looks_like_email(b.output.trim_start()) {
                continue;
            }
            let origin = format!("block:{}", b.id);
            found.extend(voiceterm_egui::mail_rfc822::messages_from_text(&origin, &b.output));
            for (i, raw) in voiceterm_egui::mail_rfc822::split_mbox(&b.output).iter().enumerate() {
                let part_origin = format!("{origin}#{i}");
                let id = voiceterm_egui::mail_rfc822::message_from_text(&part_origin, raw).id;
                bodies.insert(id, voiceterm_egui::mail_rfc822::body_from_text(raw));
            }
        }
        self.mail_bodies_terminal = bodies;
        // Rebuilt wholesale rather than appended to: a block's output can
        // still change after it closes (a late write), and appending would
        // leave the earlier parse of the same block beside the new one.
        // Cheap — the scan only runs when the block count moved.
        if found != self.mail_messages {
            self.mail_messages = found;
            self.rebuild_mail();
        }
    }

    /// Persist the web settings, reporting a failure rather than losing it.
    ///
    /// Silently, a write failure means a setting that works until restart —
    /// which is the shape of bug people spend an evening on.
    fn save_web_prefs(&mut self) {
        if let Err(e) = voiceterm_egui::web_prefs::store(&self.web_prefs) {
            self.toasts.warning(format!("web settings not saved: {e}"), 0.0);
        }
    }

    /// Draw the phone bottom bar and act on what it produces.
    ///
    /// The BUTTONS are built fresh each frame from live state rather than
    /// held as a field. A bar cached beside the tab list is a second copy
    /// of it, and the failure is the one this fleet keeps finding: the
    /// fan-out offers a tab that was closed two minutes ago, and picking it
    /// does nothing. `default_bar` is cheap — three vecs of short strings —
    /// and correct by construction.
    fn draw_bottom_bar(&mut self, ctx: &egui::Context) {
        use voiceterm_egui::bars::{bar_ui, BarEvent};

        // Tab ids are INDICES as strings, because `WsTab` has no id and the
        // dispatch below is "go to tab N" — inventing a uuid here would mean
        // mapping it straight back to the index at the other end.
        let tabs: Vec<(String, String)> = self.workspace.tabs.iter().enumerate()
            .map(|(i, t)| (i.to_string(), t.name.clone()))
            .collect();
        let workspaces: Vec<(String, String)> = self.sidebar.sessions.iter().enumerate()
            .map(|(i, s)| (i.to_string(), s.name.clone()))
            .collect();
        // Recent commands, newest first and de-duplicated: a launcher whose
        // top three entries are the same command run three times is a
        // launcher with one entry.
        let mut quick: Vec<(String, String)> = Vec::new();
        for b in self.blocks.blocks.iter().rev() {
            if quick.len() >= 8 { break; }
            let c = b.command.trim();
            if c.is_empty() || quick.iter().any(|(_, l)| l == c) { continue; }
            quick.push((c.to_string(), c.to_string()));
        }

        // WHICH PAGE — the piece `default_bar` never had. Three reads, each
        // already used elsewhere in this file for something else, composed
        // through `bar_pages::current_page` rather than re-derived here: see
        // that function's doc for why a second derivation is the failure
        // mode being avoided.
        let foreground_cmd = self.pty.as_ref().and_then(|h| h.foreground_command());
        let all_lines = self.app.terminal.grid.all_lines();
        let on_terminal_tab = {
            use voiceterm_egui::terminal_workspace::TermPanes as _;
            !matches!(
                self.workspace.model().pane_kind(self.workspace.focused_pane()),
                Some(voiceterm_egui::terminal_workspace::PaneKind::Web { .. })
            )
        };
        // NOT WIRED: this crate has no dictation-session state at all yet
        // (grepped the whole binary — the word appears only in two unrelated
        // comments). Hardcoding `false` rather than inventing a flag means
        // the Dictation page is simply unreachable here today, which is
        // honest; a fabricated always-false-except-when-I-guess flag would
        // look wired and not be.
        let dictating = false;
        let page = voiceterm_egui::bar_pages::current_page(
            foreground_cmd.as_deref(), &all_lines, on_terminal_tab, dictating,
        );
        let live = voiceterm_egui::bar_pages::Live { tabs: tabs.clone(), workspaces: workspaces.clone(), quick: quick.clone() };
        let bar = voiceterm_egui::bar_pages::bar_for(&page, &live);
        let mut event = None;
        // 50 pt EXACTLY, which is what the QML gives it: main.qml's topBar
        // (reparented into the bottom slot) is
        // `Layout.preferredHeight: urlBarVisible ? 50 : 0`.
        //
        // This panel set no height at all, so it took egui's default button
        // height plus the panel's own margins -- reported as "the bottom bar
        // is way too thick unlike the -qt version". Nothing was wrong with
        // the bar's CONTENT; it simply had no size and grew to whatever the
        // widgets wanted.
        //
        // `exact_height` rather than `min_height`: the QML value is a fixed
        // slot height, and a minimum would let one taller button push the bar
        // back out again -- the same latch, one badge away from returning.
        egui::TopBottomPanel::bottom("voiceterm_bottom_bar")
            .exact_height(BOTTOM_BAR_H)
            .show(ctx, |ui| {
            event = bar_ui(ui, &bar);
        });

        let Some(ev) = event else { return };
        match ev {
            // A tap on Tabs means "the other tab", which is what a single
            // tap on a tab button means everywhere: the thing you were just
            // looking at. Not tab 0 — that would make the button useless
            // once you are on tab 0, which is where you start.
            BarEvent::Primary(_) => {
                let n = self.workspace.tabs.len();
                if n > 1 {
                    let prev = if self.workspace.active == 0 { 1 } else { self.workspace.active - 1 };
                    self.workspace.apply(&voiceterm_egui::gesture_menus::WsCall::Command(
                        format!("go to tab {}", prev + 1)));
                }
            }
            BarEvent::Option { button, option } => {
                match bar.buttons().get(button).map(|b| b.kind) {
                    Some(voiceterm_egui::bars::BarButtonKind::Tabs) => {
                        // Through `apply` rather than by assigning `active`:
                        // the projection and the model are kept in step by
                        // `sync`, and writing the field directly is how the
                        // two drift apart.
                        self.workspace.apply(&voiceterm_egui::gesture_menus::WsCall::Command(
                            format!("go to tab {}", option + 1)));
                    }
                    Some(voiceterm_egui::bars::BarButtonKind::Workspaces) => {
                        if option < self.sidebar.sessions.len() {
                            self.sidebar.active = option;
                        }
                    }
                    Some(voiceterm_egui::bars::BarButtonKind::QuickLaunch) => {
                        // STAGED, not run. A command that executes on a
                        // thumb-slide is one nobody can take back, and the
                        // bar is exactly the surface a pocket press reaches.
                        if let Some((cmd, _)) = quick.get(option) {
                            // `stage` puts it on the input line WITHOUT a
                            // newline — that distinction is the module's
                            // own, and it is the whole reason this is safe
                            // to put under a thumb.
                            self.app.terminal.stage(cmd);
                            self.bar_note = format!("staged: {cmd}");
                        }
                    }
                    _ => {}
                }
            }
            // Slid off the fan and let go. Nothing happens, which is the
            // point — but the note clears, so a stale "staged:" line does
            // not sit there implying something is armed.
            BarEvent::Cancelled => self.bar_note.clear(),
        }
    }
}

impl eframe::App for VoicetermApp {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}


    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // WORKSPACE TAB CYCLING. Ctrl+Right / Ctrl+Left. Checked at the very
        // top of `update()`, before anything else reads `ctx.input()`.
        //
        // History worth keeping: this was tried twice before in this same
        // session and reverted both times as apparently non-functional —
        // first as Ctrl+Tab (egui/winit reserve plain Tab for widget-focus
        // internally, so that guess was at least plausible), then as
        // Ctrl+Right/Left moved to this exact position (which SHOULD have
        // worked and, it turns out, DID: a diagnostic print confirmed the
        // key arrives with `mods.command == true` and `repeat == false`,
        // exactly what `key_pressed` requires). The actual blocker was a
        // SEPARATE bug fixed one commit prior: `Workspace::command()` had
        // no match arm for "next tab"/"previous tab" at all, so
        // `apply(&WsCall::Command("next tab"))` silently returned `false`
        // and did nothing — regardless of whether the key was ever
        // detected correctly. It was. This is the same code reverted
        // last time, restored now that the thing actually blocking it is
        // fixed.
        //
        // Ctrl+Right/Left rather than Ctrl+Tab: the ordinary browser/
        // editor convention, and NOT the vim-style modeless `{N}b`/`{N}B`
        // this session found built and tested (count_prefix.rs,
        // TODO_tab_cycle_shortcut.md) but left unwired — that scheme needs
        // a reliable "am I in a text field right now" exclusion this
        // frontend has no established pattern for, and bare 'b'/digit keys
        // are typed constantly, especially in the terminal. A modifier-
        // gated arrow combination sidesteps that whole question: Ctrl+
        // Right is not something typing, in a shell or anywhere else,
        // ever produces.
        ctx.input(|i| {
            if i.modifiers.command && i.key_pressed(egui::Key::ArrowRight) {
                self.workspace.apply(&voiceterm_egui::gesture_menus::WsCall::Command("next tab".into()));
            }
            if i.modifiers.command && i.key_pressed(egui::Key::ArrowLeft) {
                self.workspace.apply(&voiceterm_egui::gesture_menus::WsCall::Command("previous tab".into()));
            }
        });

        uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(self.theme));
        // ONCE PER FRAME, shared by every panel that needs it. detect reads
        // the context and asks whether any touch input exists this frame;
        // calling it per panel is the syscall-storm shape a comment elsewhere
        // in this file already warns about.
        let adaptive = uikit_egui::adaptive::Adaptive::detect(ctx);

        // CONTEXT-LEVEL WIDGETS, drawn above every panel because that is what
        // they are for: a modal that a panel can cover is not modal, and a
        // voice halo that only shows on one tab is a status light behind a
        // door.
        // THE MEMORY POLL. The QML runs this on a 15s Timer; here it rides the
        // frame loop, which is equivalent because the gate is checked first and
        // caches in a OnceLock — a build nobody enabled it on pays an atomic
        // load per frame and nothing else.
        {
            use voiceterm_egui::memlog;
            let now = ctx.input(|i| i.time);
            if memlog::due(now, self.last_memlog_s) {
                self.last_memlog_s = now;
                let h = memlog::Holders {
                    terminals: 1,
                    scrollback_rows: self.app.terminal.grid.rows,
                    // self.workspace (singular) holds the tabs; self.workspaces
                    // is the VIEW over saved ones and has no count.
                    workspaces: self.workspace.tabs.len(),
                    cards: self.deck.cards.len(),
                    blocks: self.blocks.blocks.len(),
                    audio_kb: 0,
                };
                // Printed HERE rather than in memlog, so the diagnostic can be
                // routed — the QML pairs it with WEB2QML_LOG=mem.
                if let Some(l) = memlog::line(h) { eprintln!("{l}"); }
            }
        }

        // FAVORITES. Read fresh each draw rather than on open: the QML
        // refreshes on open for the same reason, and "each draw" is strictly
        // fresher for the cost of parsing one small XML file.
        // WEB SETTINGS. No permanent control opens this — the palette,
        // the manual and voice are the three doors, which is what "out of
        // the way until you need it" means here.
        let web_t = overlay::openness(ctx, Surface::WebSettings,
                                      self.surfaces.is(Surface::WebSettings));
        if web_t > 0.0 {
            let colors = self.colors;
            let url = self.focused_web_url();
            let mut dirty = false;
            overlay::show(ctx, Surface::WebSettings, web_t, |ui| {
                dirty = voiceterm_egui::web_prefs::web_settings_ui(
                    ui, &mut self.web_prefs, &url, &colors);
            });
            // Written on CHANGE, not per frame: the panel is open for as
            // long as someone is reading it, and rewriting the file sixty
            // times a second to record nothing is how a config file gets
            // truncated by a crash at the wrong moment.
            if dirty { self.save_web_prefs(); }
        }

        let open_t = overlay::openness(ctx, Surface::Favorites, self.surfaces.is(Surface::Favorites));
        if open_t > 0.0 {
            let colors = self.colors;
            let out = overlay::show(ctx, Surface::Favorites, open_t, |ui| {
                    use voiceterm_egui::favorites as fv;
                    let home = std::env::var("HOME").unwrap_or_default();
                    let rows = fv::entries(&home, fv::smart_cd::dolphin_favorites());
                    if let Some(a) = fv::favorites_ui(ui, &rows, &colors) {
                        match a {
                            // Typed, not run — consistent with every other
                            // surface in this app that produces a command.
                            fv::Action::Cd(path) => {
                                self.stage_command(&format!("cd {path:?}"));
                                self.surfaces.close();
                            }
                            fv::Action::Close => self.surfaces.close(),
                        }
                    }
            });
            if out.closed { self.surfaces.close(); }
        }

        // MISSING PACKAGES, right edge as in the QML. A side panel rather
        // than a modal: the list is something to read WHILE looking at the
        // command output that produced it. The list itself lives in the
        // detector, which owns dismissal so a package reappearing in a NEW
        // block comes back.
        let open_t = overlay::openness(ctx, Surface::MissingPkg, self.surfaces.is(Surface::MissingPkg));
        if open_t > 0.0 {
            let colors = self.colors;
            let out = overlay::show(ctx, Surface::MissingPkg, open_t, |ui| {
                use voiceterm_egui::missing_pkg as mp;
                // Asked fresh each frame. The detector scans blocks as output
                // arrives, so a list cached on open goes stale while the
                // drawer is still on screen.
                let all = mp::missing_packages::list_active();
                if let Some(a) = mp::missing_pkg_ui(ui, &all, &colors) {
                    match a {
                        // COPIED, never run. The detector calls its own hint a
                        // best guess the user should verify.
                        mp::Action::Copy(cmd) => ctx.copy_text(cmd),
                        mp::Action::Dismiss { ecosystem, name } => {
                            mp::missing_packages::dismiss(&ecosystem, &name);
                        }
                        mp::Action::Close => self.surfaces.close(),
                    }
                }
            });
            if out.closed { self.surfaces.close(); }
        }

        // THE TOUCH COMMAND HISTORY, as the QML has it: a bottom sheet over a
        // full-screen tap-to-close scrim (cmdHistorySheet's MouseArea,
        // main.qml:1533). The terminal stays visible through the dim, which
        // is what "the context for choosing" needs — the old bottom-PANEL
        // form got that by not being modal at all, and lost exclusivity.
        let open_t = overlay::openness(ctx, Surface::CmdSheet, self.surfaces.is(Surface::CmdSheet));
        if open_t > 0.0 {
            let colors = self.colors;
            let out = overlay::show(ctx, Surface::CmdSheet, open_t, |ui| {
                if let Some(a) = voiceterm_egui::cmd_sheet::cmd_sheet_ui(
                    ui, &mut self.cmd_sheet, &colors)
                {
                    // (the re-rank happens after the closure — `rank_recall`
                    // borrows self, and the sheet is already borrowed here)
                    use voiceterm_egui::cmd_sheet::SheetAction as A;
                    match a {
                        // Typed, not run — and here the rule matters most:
                        // these are commands you already ran once, often the
                        // one just before something went wrong.
                        A::Drop(cmd) => {
                            self.stage_command(&cmd);
                            self.surfaces.close();
                            self.cmd_sheet.close(); // clears the in-flight drag
                        }
                        A::Save(cmd) => {
                            // Marked in the sheet so a second drag on the same
                            // card does not offer to star it again.
                            self.cmd_sheet.mark_saved(&cmd);
                        }
                        A::Dismiss => {
                            self.surfaces.close();
                            self.cmd_sheet.close(); // clears the in-flight drag
                        }
                    }
                }
            });
            if out.closed {
                self.surfaces.close();
                self.cmd_sheet.close(); // scrim/Esc dismissal also clears the drag
            }
        }

        // THE FIND BAR'S FILE RESULTS. Drawn before the modal below, because
        // it is not modal — it sits under the find bar and the terminal keeps
        // working around it.
        if self.fs_search.occupies_space() {
            let colors = self.colors;
            egui::TopBottomPanel::bottom("fs_results").show(ctx, |ui| {
                if let Some(hit) = voiceterm_egui::fs_results::fs_results_ui(
                    ui, &self.fs_search, &colors, 220.0)
                {
                    // TYPED, NOT RUN. The command lands on the input line
                    // without a trailing return, so the operator sees what is
                    // about to happen and presses Enter themselves.
                    //
                    // The confirm-dialog path above appends b'\r' because the
                    // command was already reviewed in a modal. This was not:
                    // clicking a search result is a navigation gesture, and
                    // spawning an editor from one is a surprise.
                    let cmd = format!("${{EDITOR:-vi}} +{} {}", hit.line, hit.path);
                    self.stage_command(&cmd);
                    self.fs_search.cancel();
                }
            });
        }

        // THE QUERY RE-RANK, outside the draw closure: `rank_recall` reads the
        // blocks and the closure already holds `self.cmd_sheet`. One frame of
        // lag on a keystroke, which is invisible and cheaper than cloning the
        // corpus into the sheet.
        if self.cmd_sheet.query_dirty {
            let q = self.cmd_sheet.query.clone();
            let hits = self.rank_recall(&q);
            self.cmd_sheet.set_hits(hits);
        }

        // THE PEER HANDSHAKE. Modal: it is an identity decision and letting
        // the terminal run underneath it invites approving a peer by reflex
        // while reading something else.
        if let Some((panel, state)) = &mut self.peer {
            let colors = self.colors;
            let mut act = None;
            egui::Window::new("peer")
                .collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    act = voiceterm_egui::peer_panel::peer_panel_ui(ui, panel, state, &colors);
                });
            match act {
                Some(voiceterm_egui::peer_panel::Action::Event(ev)) => {
                    // The ENGINE advances the state. The panel only says which
                    // event a press means — see the test in peer_panel that
                    // asserts the view never calls step itself. Side effects
                    // (actually playing/capturing sound) happen HERE, driven
                    // by which transition just occurred — not inside step()
                    // itself, which stays a pure (state, event) -> state
                    // function with no I/O of its own.
                    if matches!(ev, peer_handshake::HsEvent::StartDial) {
                        let payload = peer_handshake::HandshakePayload {
                            peer_id: panel.my_id.clone(),
                            key_fp: panel.my_key_fp.clone(),
                            workspace_hash: panel.workspace_digest.clone(),
                            voiceprint_ref: panel.my_voiceprint_ref.clone(),
                            // No Bluetooth pairing/address-selection flow exists
                            // yet (bt-kit and bt_send.rs can send once THIS
                            // exists, but nothing produces a real address to
                            // offer here) — empty is the documented "no
                            // transport offered" value, not a placeholder
                            // standing in for a real one.
                            bt_addr: String::new(),
                        };
                        let samples = peer_handshake::audio_dialup::symbols_to_samples(&payload.to_symbols());
                        voiceterm_egui::handshake_audio::play_modem(samples, peer_handshake::audio_dialup::SAMPLE_RATE);
                    }
                    if matches!(ev, peer_handshake::HsEvent::StartListen) {
                        let (tx, rx) = std::sync::mpsc::channel();
                        self.peer_capture = Some(rx);
                        std::thread::spawn(move || {
                            // A modem frame is short; 6s covers the actual
                            // frame plus room for the two devices' "press
                            // listen" moments not being perfectly aligned.
                            let samples = voiceterm_egui::handshake_audio::capture_modem_samples(6_000)
                                .unwrap_or_default();
                            let symbols = peer_handshake::audio_dialup::samples_to_symbols(&samples);
                            // An empty/failed capture still sends an (empty)
                            // symbol stream rather than nothing: `step` already
                            // handles a frame that fails to decode by moving to
                            // `Rejected("could not decode peer handshake")` —
                            // reusing that existing, tested path rather than
                            // inventing a second "capture failed" state here.
                            let _ = tx.send(symbols);
                        });
                    }
                    *state = peer_handshake::step(state.clone(), ev);
                }
                Some(voiceterm_egui::peer_panel::Action::Dismiss) => {
                    self.peer = None;
                    self.peer_capture = None;
                }
                None => {}
            }
            // A capture in flight is polled once per frame — try_recv is a
            // non-blocking check, matching this app's immediate-mode redraw
            // model rather than blocking the render thread on the capture
            // thread finishing.
            if let Some(rx) = &self.peer_capture {
                if let Ok(symbols) = rx.try_recv() {
                    self.peer_capture = None;
                    if let Some((_, state)) = &mut self.peer {
                        *state = peer_handshake::step(state.clone(), peer_handshake::HsEvent::SymbolsCaptured(symbols));
                    }
                    ctx.request_repaint();
                }
            }
            // A trusted handshake is NOT auto-dismissed. The panel shows the
            // peer's fingerprint and a Close button, and closing it the moment
            // trust is established would hide the identity just verified —
            // which is the one thing the operator is there to read.
        }

        if let Some((cmd, dlg)) = &mut self.pending_confirm {
            use voiceterm_egui::confirm_dialog::Outcome;
            match dlg.show(ctx) {
                Outcome::Confirmed => {
                    let cmd = cmd.clone();
                    self.pending_confirm = None;
                    self.session.pty_out.extend_from_slice(cmd.as_bytes());
                    self.session.pty_out.push(b'\r');
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
        // SHARING THE ZETTELKASTEN AS A QR: picker → gate one → gate two → the
        // huge view, each its own screen, none skippable.
        if self.zk_share.is_some() {
            let colors = self.colors;
            let cards = self.deck.cards.clone();
            let mut done = false;
            if let Some(z) = self.zk_share.as_mut() {
                done = z.show(ctx, &cards, &colors);
            }
            if done {
                self.zk_share = None;
            }
        }
        // The mic halo follows the router's own state rather than a separate
        // flag, so the glow cannot disagree with what is actually listening.
        self.glow.show(ctx);

        // THE WORKSPACES PAGE, as a side panel. Rows come from the sidebar's
        // real session list and the live tabs from the real workspace — a
        // page fed demo rows would sort and filter convincingly over nothing.
        let open_t = overlay::openness(ctx, Surface::Workspaces, self.surfaces.is(Surface::Workspaces));
        if open_t > 0.0 {
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
            let out = overlay::show(ctx, Surface::Workspaces, open_t, |ui| {
                ui.heading("Workspaces");
                ui.add(egui::TextEdit::singleline(&mut self.workspaces_filter).id_salt("voiceterm_egui-self_workspaces_filter"));
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
                if ui.small_button("close").clicked() { self.surfaces.close(); }
            });
            if out.closed { self.surfaces.close(); }
        }

        // The escape chord, checked before anything draws: if the recogniser
        // has stopped working, voice mode must not be a room with no door.
        //
        // Escape-or-resume ONLY, not entry. It used to fall through to
        // `toggle()` when there was nothing to escape from, which made this
        // chord the sole way to ever reach voice mode in the first place —
        // a panic hatch standing in for a feature nobody could discover.
        // `Command::ToggleChrome` is the deliberate switch now; a chord that
        // also chose to enter the mode it exists to escape was the wrong
        // control wearing two hats.
        if ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift
                         && i.key_pressed(egui::Key::M)) {
            if !self.chrome.escape() { self.chrome.resume(); }
        }
        self.apply_compact_defaults(ctx);
        let show_chrome = self.chrome.mode().shows_tab_strip();
        // The session list used to be its own `SidePanel::left("sidebar")` at 160
        // points, beside the 168-point tab column. It now lives INSIDE that
        // column — see the tab column below — because `main.qml`'s `vTabBar`
        // hosts "work tabs (scrollable) + a default fence + system tabs" in one
        // bar, and two bars for one job is the whole of why this surface read as
        // heavier than the Qt one.

        // toast above the bottom chrome (keycap bar ~76px + margin, the QML bind-to-chrome idea)
        // ui_actions rather than ui: the ingest asks "keep this version?" and
        // the answer has to come back. A question nothing listens for is worse
        // than no question — the operator presses keep and the document
        // silently stays reverted.
        match self.toasts.ui_actions(ctx, 84.0).as_deref() {
            Some("keep") => {
                self.ingest.accept();
                // The accepted document becomes the capture the cards split
                // from, which is the point of the whole flow.
                self.captured = self.ingest.original.clone();
            }
            Some("revert") => self.ingest.revert(),
            _ => {}
        }
        // RE-GATED. Removing the standalone `sidebar` panel deleted the
        // `if show_chrome {` that also wrapped this, so the keycap and gesture
        // rows stopped hiding in Voice mode — silently, because nothing counts
        // panels. `chrome_mode`'s accounting still claimed BUTTONS_H back.
        // Caught by maintenance/tools/egui_chrome_audit.py, not by eye.
        if show_chrome && self.keycaps_open {
        egui::TopBottomPanel::bottom("keycaps").show(ctx, |ui| {
            // The pipe/brace gesture chips sit on their own row above the keycaps (the QML toolbar
            // put them in modRow): hold + drag to a cell + release. The brace menu's deadband makes
            // each cell 3-zone — left of center emits the opener, right the closer, center the pair
            // with the cursor backed between (see gesture_menus::brace_bytes).
            ui.horizontal(|ui| {
                let pipe_host = ui.button("|\u{22EF}");
                // MOUSE-ACCESSIBLE NOW: `show_hybrid` instead of `show` — a
                // held press still opens the fan-out exactly as before
                // (release commits); a plain click ALSO opens it now
                // (tap-select: a second click confirms), for the mouse
                // users the hold gesture serves worse than a touchscreen
                // does. Safe here specifically because a bare click on this
                // button was already a complete no-op — `show` only ever
                // reacted to a HELD press, so there was no existing
                // "quick tap means X" behavior this could silently replace.
                if let SlideOutcome::Selected(id, _) = self.pipe_menu.show_hybrid(ui, &pipe_host, &pipe_menu()) {
                    if let Some(ch) = pipe_char(&id) {
                        self.send_input(ch.as_bytes());
                    }
                }
                // the 🏠 chip opens favorites (QML: home icon in the toolbar)
                if ui.button("\u{1F3E0}").clicked() {
                    self.surfaces.toggle(Surface::Favorites);
                }
                // the 📦 chip opens the missing-package sidebar
                if ui.button("\u{1F4E6}").clicked() {
                    self.surfaces.toggle(Surface::MissingPkg);
                }
                // the ⏱ chip opens the touch command history (QML cmdSheet)
                if ui.button("\u{23F1}").clicked() {
                    if self.surfaces.is(Surface::CmdSheet) {
                        self.surfaces.close();
                        self.cmd_sheet.close(); // clears the in-flight drag
                    } else {
                        // RANKED AT OPEN, not held live: recall scores recency
                        // against frequency, so a list built once at startup
                        // shows the wrong "most recent" an hour later.
                        // Empty query: no filter, pure recency/frequency rank,
                        // which is what a history sheet opens on. Through the
                        // same ranker the query field and "how did I…" use, so
                        // the sheet cannot rank two ways.
                        let hits = self.rank_recall("");
                        self.cmd_sheet.open_with(hits);
                        self.surfaces.open(Surface::CmdSheet);
                    }
                }
                // the 📜 chip toggles the blocks drawer (QML blocksDrawer.open())
                if ui.button("\u{1F4DC}").clicked() {
                    self.surfaces.toggle(Surface::Blocks);
                }
                // hold the palette chip to show all vibes (the settings screenshot's theme grid)
                let theme_host = ui.button("\u{1F3A8}");
                if let voiceterm_egui::slide_menu::SlideOutcome::Selected(id, _) =
                    self.theme_menu.show_hybrid(ui, &theme_host, &theme_menu())
                {
                    self.theme = snapclone_theme::PaletteId::from_str(&id);
                    self.toasts.info(format!("Theme: {}", self.theme.display_name()), ctx.input(|i| i.time));
                    // persist: the pick survives restarts (theme.palette in the kv prefs file)
                    self.app.prefs.palette = id;
                    let _ = save_prefs(&self.app.prefs);
                }
                let brace_host = ui.button("{\u{22EF}}");
                if let SlideOutcome::Selected(id, variant) = self.brace_menu.show_hybrid(ui, &brace_host, &brace_menu()) {
                    if let Some(bytes) = brace_bytes(&id, variant) {
                        self.send_input(bytes.as_bytes());
                    }
                }
            });
            // The scope control. A segmented row rather than two checkboxes,
            // because two checkboxes can express a state this deliberately
            // cannot — and the pair would drift the first time one was saved
            // and the other was not.
            ui.horizontal(|ui| {
                ui.label("summaries:");
                // SENSORS, BEHIND A DISCLOSURE. This was fifty-five lines
                // of capability reporting inline in the bar — two of them
                // permanent labels saying what the device CANNOT do. A bar
                // that is always on screen should carry what the operator
                // acts on; "no sensor features available on this device" is
                // a settings answer, and I put it here myself two commits
                // ago while the bar was already too crowded.
                //
                // Collapsed by default: the common case is a device whose
                // sensors do not change, and the answer is worth one tap
                // when it is worth anything.
                ui.collapsing("sensors", |ui| {
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

                        // WHAT THE HARDWARE ALLOWS, from Sensors — which answers
                        // all three questions and was called by nothing. The block
                        // above hand-rolls the light half and never asks about
                        // proximity at all, so half the capability report the
                        // module exists to give was missing.
                        //
                        // can_auto_brightness needs LUX, not counts: light-kit's
                        // curve is calibrated in lux and raw counts give a
                        // differently-shaped response, which looks like bad tuning
                        // rather than bad units. can_detect_cover only needs
                        // consistency, so counts are fine. Saying which is
                        // available is the difference between a missing feature
                        // and a dead control.
                        let caps = voiceterm_egui::iio_light::Sensors::detect();
                        let mut have: Vec<&str> = Vec::new();
                        if caps.can_auto_brightness() { have.push("auto brightness"); }
                        if caps.can_detect_cover() { have.push("cover detection"); }
                        if caps.has_proximity { have.push("proximity"); }
                        ui.label(egui::RichText::new(if have.is_empty() {
                            "no sensor features available on this device".to_string()
                        } else {
                            format!("available: {}", have.join(", "))
                        }).weak().small());
                        if caps.has_light && caps.light_is_relative {
                            // Named rather than silent: the sensor IS there and
                            // auto brightness still cannot run, which reads as a
                            // bug unless the reason is given.
                            ui.label(egui::RichText::new(
                                "auto brightness needs lux; this sensor reports relative counts")
                                .weak().small());
                        }
                    }
                });
                // Was a permanent row of 4 selectable_label buttons -- one
                // of the always-on rows crowding a mobile bar that also has
                // to share the screen with a software keyboard on a 7"
                // device. Made rarely, not every frame it was costing
                // space, so it moves behind a toolbar icon, matching the
                // theme picker's own show_hybrid pattern exactly (tap or
                // hold, same host response).
                let summary_host = ui.button(format!("\u{1F4CB} {}", self.summary_scope.label()));
                if let voiceterm_egui::slide_menu::SlideOutcome::Selected(id, _) =
                    self.summary_menu.show_hybrid(ui, &summary_host, &summary_menu())
                {
                    if let Some(sc) = voiceterm_egui::summary_scope::Scope::from_label(&id) {
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
                ui.add(egui::TextEdit::singleline(&mut self.date_phrase).id_salt("voiceterm_egui-self_date_phrase"));
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
                    self.show_line(&format!("{line}\r\n"));
                    self.date_phrase.clear();
                }
            });

            // SPOKEN EDITING. editor_keys turns "delete line" into the keys a
            // modal editor wants — the mobility thesis of this app, "edit files
            // on the move, one-handed, on a phone" — and had no surface at all.
            ui.horizontal(|ui| {
                ui.label("say:");
                let r = ui.add(egui::TextEdit::singleline(&mut self.editor_phrase).id_salt("voiceterm_egui-self_editor_phrase"));
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
                    // JOT MODE takes the transcript before the command parser sees it —
                    // that is the whole feature: "route transcription straight into the
                    // active Note as Block::Prose, instead of through the command intent
                    // parser… this just changes the SINK".
                    if self.jotting {
                        use voiceterm_egui::jot::Jotted;
                        let said = std::mem::take(&mut self.editor_phrase);
                        let (prose, exit) = match voiceterm_egui::jot::route(&said) {
                            Jotted::Append(t) => (Some(t), false),
                            Jotted::AppendAndExit(t) => (t, true),
                        };
                        if let Some(t) = prose {
                            self.clip_note.push_prose(&t);
                            // Saved with the same path rule as a clip: the note grows and
                            // rewrites its own file. A jotted thought lost to a window
                            // close is the same failure a clip would be.
                            let saved = match &self.clip_note_path {
                                Some(p) => voiceterm_egui::note_store::save_to(p, &self.clip_note),
                                None => voiceterm_egui::note_store::save(&self.clip_note)
                                    .map(|p| self.clip_note_path = Some(p)),
                            };
                            if let Err(e) = saved {
                                self.toasts.warning(&format!("jotted, but not saved: {e}"), 0.0);
                            }
                        }
                        if exit {
                            self.jotting = false;
                            self.toasts.info("done jotting", 0.0);
                        }
                        return;
                    }
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
                            self.session.pty_out.extend_from_slice(a.keys.as_bytes());
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
                            // SHELL COMMANDS FIRST, before the tldr index.
                            // "show me my mail in a new pane" is a request the
                            // app can carry out exactly, and scoring it against
                            // man pages instead would offer `mail(1)` — a
                            // plausible wrong answer, which is the worst kind.
                            // The grammar declines anything it does not fully
                            // understand, so the fall-through below still gets
                            // everything it used to.
                            if let Some(cmd) = voiceterm_egui::palette::parse_phrase(&phrase) {
                                self.run_command(cmd);
                                self.cmd_hits.clear();
                                return;
                            }
                            // THEN THE FULL INTENT PARSER, before the tldr index. The Qt
                            // build's `voice_intent` — 5,243 lines, ~174 intents and the
                            // recipes behind them — was never mounted here, so this frontend
                            // understood the politeness normaliser it had borrowed from that
                            // file and nothing else. Everything the grammar above does not
                            // cover went straight to a man-page similarity score.
                            //
                            // AFTER `palette::parse_phrase`, not before: that grammar maps to
                            // commands this app actually has, and the intent parser knows the
                            // Qt build's vocabulary — where the two overlap, the one wired to
                            // real actions should win.
                            //
                            // Only high-confidence matches act. `is_high_confidence` exists
                            // because a parser with 174 variants will find SOMETHING in any
                            // sentence, and acting on a weak match is how a voice UI earns
                            // its reputation for doing the wrong thing.
                            let intent = voiceterm_egui::voice_intent::parse(&phrase);
                            if !matches!(intent, voiceterm_egui::voice_intent::Intent::Unknown)
                            {
                                // Described, not executed. The intents map onto the Qt
                                // build's actions, and half of them have no counterpart here
                                // yet — running whatever happened to match would be worse
                                // than saying what was understood.
                                self.toasts.info(&format!("heard: {}", intent.describe()), 0.0);
                                self.cmd_hits.clear();
                                return;
                            }
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
                        self.session.pty_out.extend_from_slice(a.keys.as_bytes());
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
            // KEYCAPS ONLY WHERE THERE IS NO KEYBOARD. Esc, Tab and the arrows
            // are what a phone cannot otherwise send to a terminal; on a
            // desktop they duplicate keys already under the operator's hands
            // and cost a row of the screen to do it.
            //
            // adaptive.touch, not width: a small window on a laptop still has
            // a keyboard, and a large tablet still does not.
            if adaptive.touch {
                if let Some(send) = keycap_bar_ui(ui, TERMINAL_KEYCAPS, &self.colors) {
                    self.send_input(send.as_bytes());
                }
            }
            // A discoverable way back to minimal: this panel is now closed
            // by default (see compact_defaults_applied), so once it's open
            // by request, closing it again needs its own control rather
            // than becoming permanent again by accident.
            if ui.small_button("\u{2715} hide").clicked() {
                self.keycaps_open = false;
            }
        });
        }  // if show_chrome — the keycap and gesture rows

        // THE REVEAL HANDLE. Closed by default now (desktop used to default
        // this OPEN — see compact_defaults_applied), so there has to be a
        // way back that isn't "resize the window past 600px and reopen the
        // app." A palette command covers the keyboard/voice path
        // (Command::ToggleKeycaps); this is the pointer path, and
        // deliberately NOT just a plain toggle button.
        //
        // The pipe/brace toolbar and bar_ui both ask for a press-HOLD then
        // drag — natural on a touchscreen, awkward with a mouse, which has
        // no equivalent of "rest a finger and wait". `SlideMenu::show_tap`
        // (already built for the link menu, unused as a second general
        // input mode until now) opens on a plain click instead and commits
        // on a second click — the same fan-out, reachable without ever
        // holding a button down. One cell today; the shape holds if this
        // handle grows options later without changing how it is driven.
        if show_chrome && !self.keycaps_open {
            egui::TopBottomPanel::bottom("footer_handle").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let handle = ui.small_button("\u{22EF}");
                    let menu = voiceterm_egui::slide_menu::SlideMenu::new("footer_reveal_menu").columns(1);
                    let opts = [voiceterm_egui::slide_menu::SlideOption::new("show", "Show toolbar")];
                    if let SlideOutcome::Selected(id, _) =
                        menu.show_tap(ui, egui::Id::new("footer_reveal"), &handle, &opts)
                    {
                        if id == "show" { self.keycaps_open = true; }
                    }
                });
            });
        }

        // the settings screen as the QML has it: settingsPopup (main.qml:27729),
        // a centered modal popup. Resizability went with the panel form; the
        // QML popup is fixed at 320 wide, and the form table is where to
        // change that on device.
        let open_t = overlay::openness(ctx, Surface::Settings, self.surfaces.is(Surface::Settings));
        if open_t > 0.0 {
            // Deferred because the surface switch happens INSIDE the
            // Settings overlay's own closure — opening another surface
            // mid-draw would tear down the panel being drawn.
            let mut next_view: Option<PaneView> = None;
            let out = overlay::show(ctx, Surface::Settings, open_t, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("Settings");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("\u{2715}").clicked() {
                            self.surfaces.close();
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // THE LAUNCHER, above the preferences. The QML's
                    // settingsPopup is mostly four doors — model setup,
                    // chat, recipes, sessions — and until now none of
                    // them had a route from this port: the panel below
                    // is preferences, and preferences do not open
                    // screens. Doors first because that is what someone
                    // opening this menu is usually after; the sliders
                    // are for a visit you made on purpose.
                    use uikit_egui::quick_launcher::{quick_launcher_ui, LauncherEvent};
                    if let Some(ev) = quick_launcher_ui(ui, &mut self.launcher) {
                        // NONE of the four doors has a Surface in this
                        // port yet — `Surface` has Blocks, MissingPkg,
                        // Workspaces, Settings, Favorites, CmdSheet and
                        // Palette, and no Setup, Chat, Recipes or
                        // Sessions. So every door reports honestly rather
                        // than opening nothing: a button that silently
                        // does nothing is read as a bug in the app, while
                        // one that says "not ported yet" is read as the
                        // truth, which it is.
                        //
                        // The widgets for two of them EXIST in the kit
                        // (setup_wizard, chat_panel) and are unhosted —
                        // adding the Surface variants is the wiring that
                        // turns these into real doors, and it is a
                        // separate change from putting the launcher on
                        // screen.
                        // BOTH DESTINATIONS ALREADY EXIST — as CentralView
                        // tabs (Models hosts the setup wizard, Chat hosts
                        // the chat panel), not as overlay surfaces. I
                        // first added Surface::Setup/Chat and duplicate
                        // widget fields for them; the compiler rejected
                        // the duplicates and that was the right answer.
                        // The launcher's job is to NAVIGATE to what
                        // exists, not to grow a second copy of it.
                        match ev {
                            LauncherEvent::OpenSetup => {
                                next_view = Some(PaneView::Models);
                            }
                            LauncherEvent::OpenChat => {
                                next_view = Some(PaneView::Chat);
                            }
                            // These two genuinely have no destination, so
                            // they say so rather than opening a blank one.
                            LauncherEvent::OpenRecipes => {
                                self.toasts.info("recipes: not ported yet", ui.input(|i| i.time));
                            }
                            LauncherEvent::OpenSessions => {
                                self.toasts.info("sessions: not ported yet", ui.input(|i| i.time));
                            }
                            LauncherEvent::VoiceActivationChanged
                            | LauncherEvent::ThemeChanged => {}
                        }
                    }
                    ui.add_space(10.0);
                    ui.separator();
                    if settings_panel(ui, &mut self.app.prefs, &mut self.a11y) {
                        // palette is owned by prefs; keep the frame-applied theme in sync
                        self.theme = snapclone_theme::PaletteId::from_str(&self.app.prefs.palette);
                        let prefs = self.app.prefs.clone();
                        self.app.apply_settings(ctx, &prefs);
                        let _ = save_prefs(&self.app.prefs);
                    }
                });
            });
            if out.closed { self.surfaces.close(); }
            if let Some(v) = next_view {
                // Close the launcher on the way: leaving it over the
                // destination would hide what the door just opened.
                self.surfaces.close();
                // Through the same model the palette uses (`workspace.open_view`),
                // not a separate field — that split is exactly what let a
                // palette-driven "mail"/"graph"/etc. update the model with zero
                // visible effect (see this file's `PaneView` doc comment above).
                self.workspace.open_view(v);
            }
        }

        // the blocks drawer slides in at the left edge (QML blocksDrawer,
        // LeftEdge, main.qml:30637) — width now from the form table, not a
        // resizable panel.
        let open_t = overlay::openness(ctx, Surface::Blocks, self.surfaces.is(Surface::Blocks));
        if open_t > 0.0 {
            let out = overlay::show(ctx, Surface::Blocks, open_t, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("Blocks");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("\u{2715}").clicked() {
                            self.surfaces.close();
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
                                self.show_line(&echoed);
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
            if out.closed { self.surfaces.close(); }
        }

        // THE VERTICAL TAB COLUMN, matched to the QML rather than approximated.
        //
        // Three differences from `main.qml`'s `vTabBar`, all of which made this
        // surface heavier than the one it ports:
        //
        // 1. It ignored ChromeMode. `sidebar` and `keycaps` hide in Voice mode
        //    and this did not, so "voice hides the chrome" left a 120px column
        //    standing. chrome_mode.rs's own accounting only counts HEIGHT
        //    (STRIP_H + BUTTONS_H), so the reclaim figure never noticed.
        // 2. Fixed at 120px. The QML is `width: root.termSidebarWidth`,
        //    default 168, dragged between 110 and 340 with snap detents
        //    (main.qml:4015, 4685). Fixed-width here meant the one dimension a
        //    phone user most wants to trade was not tradeable.
        // 3. Qt hosts ONE left bar. `vTabBar` holds "work tabs (scrollable) + a
        //    default fence + system tabs, same model as the old top strip (now
        //    hidden)" — while this surface stacks `sidebar` (160) beside
        //    `tabbar` (120) for 280px against Qt's 168. Merging them is the
        //    real fidelity fix and is not this change; gating and sizing are.
        //
        // ALL THREE ARE DONE, re-verified 2026-09-19 against main.qml after a
        // report that "the left sidebar is not working yet like the -qt qml":
        // the gate is `show_chrome` below, the width is `tabbar_width` (168.0,
        // main.qml:4015) with the 110..=340 drag clamp (main.qml:4685), and
        // the session list is merged in rather than a second panel — see the
        // note at the top of `draw_sessions_strip`, "used to be its own
        // SidePanel::left(\"sidebar\") at 160".
        //
        // Left here rather than deleted because the three-point comparison is
        // the map someone checks the next time this surface looks wrong, and
        // a resolved item with a date is more useful than a missing one.
        if show_chrome {
        // COLLAPSED, the column is one touch target wide — the same closed
        // width `rail` uses, so the two behave alike rather than each having
        // its own idea of "narrow".
        //
        // Resizable only when open: a drag handle on a chevron-width strip is a
        // target the user hits by accident while reaching for the chevron.
        // The frame is painted EXPLICITLY, in the palette's card colour. With
        // the style's default panel fill this column was drawn with whatever
        // `apply_palette` left in `visuals.panel_fill`, and when that was
        // transparent the collapsed strip showed the window's clear colour —
        // the black bar the chevron "revealed". A panel that names its own
        // fill cannot show through to anything.
        let tabbar_frame = egui::Frame::side_top_panel(&ctx.style()).fill(self.colors.bg_card);
        let tabbar_id = egui::Id::new("tabbar"); // SidePanel::left("tabbar")'s id, for the resize probe
        // While the edge is being dragged the content must NOT pin the
        // width, or a drag narrower than the pinned width is undone by
        // layout on the same frame. egui names the resize widget
        // `id.with("__resize")` (panel.rs); dragging it is the only time the
        // column is allowed to be narrower than `tabbar_width`.
        let resizing = ctx.is_being_dragged(tabbar_id.with("__resize"));
        let tabbar_panel = if self.tabbar_open {
            egui::SidePanel::left("tabbar")
                .resizable(true)
                .frame(tabbar_frame)
                .default_width(self.tabbar_width)  // main.qml:4015 termSidebarWidth
                .width_range(110.0..=340.0)        // main.qml:4685 drag clamp
        } else {
            egui::SidePanel::left("tabbar")
                .resizable(false)
                .frame(tabbar_frame)
                .exact_width(adaptive.min_touch_target)
        };
        // (No `pinned` capture: the content fills the panel instead -- see below.)
        let tabbar_out = tabbar_panel.show(ctx, |ui| {
            if self.tabbar_open && !resizing {
                // FILL THE PANEL -- NEVER EXCEED IT. This used to pin the
                // content to `pinned`, the REMEMBERED width, and that is the
                // thin black strip that appeared on shrink and swallowed
                // terminal scrolling. From egui's own panel.rs (0.34):
                //
                //   * the panel's rect comes from its stored width, and its
                //     content is CLIPPED to that rect ("If we overflow, don't
                //     do so visibly (#4475)");
                //   * but the width the panel ALLOCATES is taken from the
                //     content's response, and the central panel starts at
                //     that edge (`cursor.min.x = rect.max.x`).
                //
                // On the frame a drag ends, `pinned` is still the OLD wider
                // width (it is only read back after `show`). The content
                // demanded that width, the panel allocated it, the fill was
                // clipped short of it -- and the gap between was painted by
                // nothing, so the window's black showed. Because it belonged to
                // the sidebar's allocation, the terminal was not under the
                // pointer there, which is why scrolling over it went nowhere.
                //
                // `ui.max_rect()` is the panel's own inner rect, so this is the
                // idiom egui itself uses for horizontal panels: content fills
                // exactly the space the panel has, and cannot push its
                // allocation past its paint.
                ui.set_min_width(ui.max_rect().width());
            }
            // The chevron points the way the panel will move, which is the
            // convention `rail` already set two panels over.
            if ui.small_button(if self.tabbar_open { "‹" } else { "›" })
                .on_hover_text(if self.tabbar_open { "Collapse tabs" } else { "Expand tabs" })
                .clicked()
            {
                self.tabbar_open = !self.tabbar_open;
            }
            if !self.tabbar_open {
                // COLLAPSED: BOTH LISTS, reduced to initials. A unified bar that
                // drops half its contents when narrow is not unified — it is two
                // bars with one of them hidden, which is the arrangement this
                // column was merged to get rid of.
                //
                // Sessions stay for the same reason the tabs do: the point of a
                // collapsed column is to say which thing is live at a glance,
                // and "which session" is the question a terminal user asks more
                // often than "which tab". Dropping it also stranded `+ New`,
                // `Graph` and `Settings`, which is why `ToggleSettings` had to
                // be added when this branch first went in.
                //
                // Separator between them, not a heading: at one touch target
                // wide there is no room for a word, and two ungrouped columns of
                // single letters read as one list.
                ui.separator();
                let tabs: Vec<(String, bool)> = self.workspace.tabs.iter().enumerate()
                    .map(|(i, t)| (t.name.clone(), i == self.workspace.active)).collect();
                // Rows at the OPEN column's pitch (33 = QML's 30 + 3), not 26:
                // a toggle that keeps every row's y is a column folding, one
                // that shuffles them is two different lists.
                for (i, (name, active)) in tabs.iter().enumerate() {
                    let initial = name.chars().next().unwrap_or('\u{2022}').to_string();
                    if uikit_egui::dock::list_row(ui, &initial, *active, 30.0, &self.colors)
                        .on_hover_text(name)
                        .clicked()
                    {
                        self.workspace.active = i;
                    }
                }
                let sessions: Vec<(String, bool)> = self.sidebar.sessions.iter().enumerate()
                    .map(|(i, s)| (s.name.clone(), i == self.sidebar.active)).collect();
                if !sessions.is_empty() {
                    ui.separator();
                    for (i, (name, active)) in sessions.iter().enumerate() {
                        let initial = name.chars().next().unwrap_or('\u{2022}').to_string();
                        if uikit_egui::dock::list_row(ui, &initial, *active, 30.0, &self.colors)
                            .on_hover_text(name)
                            .clicked()
                        {
                            self.sidebar.active = i;
                        }
                    }
                }
                return;
            }
            // SESSIONS AT THE BOTTOM, work tabs above — the QML's own order
            // ("work tabs (scrollable) + a default fence + system tabs").
            //
            // `show_inside` with a bottom panel rather than appending after the
            // scroll area: the tab list uses `auto_shrink([false, false])` and
            // takes the whole column, so anything written after it would be
            // pushed off the bottom. Reserving the space first is the only
            // order that leaves both visible when there are many tabs.
            egui::TopBottomPanel::bottom("tab-sessions").show_inside(ui, |ui| {
                if let Some(action) = self.sidebar.ui(ui, &self.colors) {
                    match action {
                        SidebarAction::SelectSession(_) => {} // on-device: switch the active PTY/session
                        SidebarAction::New => {} // on-device: spawn a new session
                        SidebarAction::Graph => {
                            // The workspaces page. The sidebar's Graph action was
                            // a no-op with a TODO; this is the page the Qt app
                            // puts behind it.
                            self.surfaces.toggle(Surface::Workspaces);
                        }
                        SidebarAction::Settings => self.surfaces.toggle(Surface::Settings),
                    }
                }
            });
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
                    // hold-to-open the per-tab menu on the same row host. 0.38 s, not
                    // 0.35: `main.qml:4494` is `Timer { id: _holdTimer; interval: 380 }`,
                    // and its comment explains why it is a MANUAL timer rather than
                    // Qt's `onPressAndHold` — "cancelled when a touch point drifts past
                    // the drag threshold, so on a touchscreen a finger-hold almost never
                    // fired the menu". A port that shortens it re-introduces a race the
                    // original lengthened the interval to avoid. (QML hold
                    // feel); the held tab's name rides in the title band
                    let menu = tab_slide_menu(("vt-tab-menu", i), &tab.name).hold(0.38);
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
        // Read the width back: after a drag the panel's rect is the new
        // width, and it becomes the pinned one. Not during the drag itself
        // (the rect is then wherever the pointer is), and never when
        // collapsed (the strip's width is the touch target, not a choice).
        if self.tabbar_open && !resizing {
            let w = tabbar_out.response.rect.width();
            if (w - self.tabbar_width).abs() > 0.5 && (110.0..=340.0).contains(&w) {
                self.tabbar_width = w;
            }
        }
        }  // if show_chrome — the tab column follows the sidebar and keycaps

        // THE COMMAND PALETTE, above everything. Ctrl+P or "open command
        // palette" — the eleven-tab switcher's replacement, and the reason the
        // rail only needs to carry tabs.
        //
        // Redesigned twice this session. First: a bare `i.key_pressed
        // (Key::P)` toggle, reported as "spamming the command palette
        // on/off" while the key is held (OS-level key-repeat sends the
        // key down again and again while physically held, and
        // `key_pressed` toggled on every one). Second: a hold-duration
        // gesture (hold Ctrl+P to trigger voice) -- replaced again after
        // direct design feedback, in favor of the idle-dwell flow below:
        // tap once to open (always the palette), then just PAUSE without
        // typing and it settles into listening on its own. No key to
        // hold, no timing to get right with a hand on the keyboard.
        //
        // The repeat-spam bug's fix still applies here, explicitly:
        // `ctrl_p_tapped()` must fire once per PHYSICAL press, not once
        // per repeat event, so the raw event is checked for `repeat:
        // false` directly rather than using `key_pressed` (whose
        // repeat-inclusion was never fully certain) or `key_down` (a
        // continuous state, wrong primitive for a discrete "was this
        // just pressed" question).
        let ctrl_p_pressed = ctx.input(|i| {
            i.events.iter().any(|e| matches!(
                e,
                egui::Event::Key { key: egui::Key::P, pressed: true, repeat: false, modifiers, .. }
                    if modifiers.command && !modifiers.shift
            ))
        });
        // Ctrl+Space is push-to-talk, as in the Qt build (main.qml's
        // `Ctrl+Space` shortcut); Ctrl+Shift+P opens the palette straight
        // to typing, the Qt v0.1.880 binding, so the two ways in match.
        let (ctrl_space, ctrl_shift_p) = ctx.input(|i| (
            i.events.iter().any(|e| matches!(e,
                egui::Event::Key { key: egui::Key::Space, pressed: true, repeat: false, modifiers, .. }
                    if modifiers.command)),
            i.events.iter().any(|e| matches!(e,
                egui::Event::Key { key: egui::Key::P, pressed: true, repeat: false, modifiers, .. }
                    if modifiers.command && modifiers.shift)),
        ));
        if ctrl_space { self.start_voice(); }
        if ctrl_shift_p && !self.surfaces.is(Surface::Palette) {
            self.surfaces.open(Surface::Palette);
            self.palette_query.clear();
            self.palette_sel = 0;
        }
        if ctrl_p_pressed {
            match self.palette_flow.ctrl_p_tapped() {
                uikit_egui::palette_flow::Transition::Opened => {
                    self.surfaces.open(Surface::Palette);
                    self.palette_query.clear();
                    self.palette_sel = 0;
                }
                uikit_egui::palette_flow::Transition::Closed(uikit_egui::palette_flow::Mode::Command) => {
                    self.surfaces.close_if(Surface::Palette);
                }
                uikit_egui::palette_flow::Transition::Closed(uikit_egui::palette_flow::Mode::Voice) => {
                    // Closing OUT of Voice mode via Ctrl+P (not by typing
                    // -- that path is handled below) drops whatever was
                    // being captured, the same way Escape/backdrop-click
                    // already do for every other surface.
                    self.surfaces.close_if(Surface::Voice);
                    self.voice_popup.cancel();
                }
                _ => {}
            }
        }

        // Typing while the palette is open in Voice mode means "I
        // changed my mind about speaking" -- switch back to Command and
        // drop the in-flight capture. Checked via the raw Text events
        // rather than diffing `palette_query`'s length, since a paste or
        // an IME commit should count as "typing" too, the same events
        // `palette_ui`'s own TextEdit already consumes without these
        // being removed from `i.events` first (egui does not consume
        // Text events on read the way a pointer click's Response can).
        let typed_this_frame = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Text(_)))
        });
        if typed_this_frame
            && self.palette_flow.on_type() == uikit_egui::palette_flow::Transition::SwitchedToCommand
        {
            self.surfaces.close_if(Surface::Voice);
            self.surfaces.open(Surface::Palette);
            self.voice_popup.cancel();
        }

        // Advance the idle-dwell timer. Only has an effect while the
        // flow is in Command mode (see palette_flow's own doc); a no-op
        // otherwise, safe to call unconditionally every frame.
        let dt_ms = ctx.input(|i| i.stable_dt) * 1000.0;
        if self.palette_flow.tick(dt_ms) == uikit_egui::palette_flow::Transition::SwitchedToVoice {
            if self.voice_popup.start(self.password_mode) {
                self.surfaces.close_if(Surface::Palette);
                self.surfaces.open(Surface::Voice);
            } else {
                // Password mode refused the start -- the flow already
                // moved to Voice, but nothing is actually listening.
                // `voice_refused()` steps it straight back to Command
                // (named for this reason, sharing on_type's real effect
                // rather than the flow staying stuck showing a voice
                // popup that will never produce a result). A visible
                // refusal toast is a real gap, not solved here (matches
                // the same gap noted when this trigger was still
                // hold-based).
                self.palette_flow.voice_refused();
            }
        }

        let open_t = overlay::openness(ctx, Surface::Palette, self.surfaces.is(Surface::Palette));
        if open_t > 0.0 {
            self.palette_ui(ctx, open_t);
        }
        let voice_open_t = overlay::openness(ctx, Surface::Voice, self.surfaces.is(Surface::Voice));
        if voice_open_t > 0.0 {
            self.voice_popup.tick();
            // THE TRANSCRIPT REACHES THE ROUTER. `Stage::Done` was drawn
            // and read by nothing: the voice terminal listened and did
            // not act. Once per utterance — `take_result` hands it over
            // a single time.
            if let Some(said) = self.voice_popup.take_result() {
                self.on_transcript(&said);
                // The popup has said its piece; close it the way the Qt
                // build's voicePopup closes after an intent is handled,
                // so the next tap starts a fresh listen.
                self.surfaces.close_if(Surface::Voice);
                if self.palette_flow.mode() == uikit_egui::palette_flow::Mode::Voice {
                    self.palette_flow.ctrl_p_tapped(); // Voice -> Off
                }
            }
            let close_now = overlay::show(ctx, Surface::Voice, voice_open_t, |ui| {
                self.voice_popup.draw(ui, &self.colors);
            })
            .closed;
            // `tick()` returning true (a cancelled recording) closes
            // immediately, matching the Qt build's own voice_cancel
            // behavior; a backdrop click or Esc via the Modal (`.closed`
            // from `overlay::show`) closes it the same way every other
            // surface already does. Either path routes back through
            // palette_flow so its own state stays in sync with what is
            // actually on screen.
            if close_now {
                self.surfaces.close_if(Surface::Voice);
                if self.palette_flow.mode() == uikit_egui::palette_flow::Mode::Voice {
                    self.palette_flow.ctrl_p_tapped(); // Voice -> Off
                }
                self.voice_popup.cancel();
            }
        }

        // The "about to switch on"/"listening" badge, drawn OUTSIDE both
        // surfaces above -- it has to stay visible across BOTH `Command`
        // (dwelling) and `Voice` (listening), and `palette_ui`/the voice
        // popup's own overlay are only ever drawn one at a time (whichever
        // surface is actually open), so nesting the indicator inside
        // either one would make it disappear exactly when the OTHER mode
        // is showing -- caught before wiring it in nested inside
        // `palette_ui`, where the `listening` state could never actually
        // be reached.
        if self.palette_flow.mode() != uikit_egui::palette_flow::Mode::Off {
            let dt = ctx.input(|i| i.stable_dt);
            let warming = self.palette_flow.dwell_progress();
            let listening = self.palette_flow.mode() == uikit_egui::palette_flow::Mode::Voice;
            let action = egui::Area::new(egui::Id::new("voiceterm_mic_indicator"))
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    self.mic_indicator.draw(ui, &self.colors, warming, listening, dt)
                })
                .inner;
            if action == uikit_egui::mic_indicator::MicAction::ForceOff {
                match self.palette_flow.force_off() {
                    uikit_egui::palette_flow::Transition::Closed(uikit_egui::palette_flow::Mode::Command) => {
                        self.surfaces.close_if(Surface::Palette);
                    }
                    uikit_egui::palette_flow::Transition::Closed(uikit_egui::palette_flow::Mode::Voice) => {
                        self.surfaces.close_if(Surface::Voice);
                        self.voice_popup.cancel();
                    }
                    _ => {}
                }
            }
        }

        // THE LEFT RAIL: workspace tabs, vertically, collapsible. The Qt app
        // is a tiling workspace and its tabs are workspace tabs — not a list
        // of surfaces. Surfaces live in panes now, so this rail stays short
        // however many views exist.
        // ADAPTIVE, not two constants. The collapsed rail was 34 points wide —
        // below any touch target on any device, so on a phone the toggle that
        // reopens it could not reliably be hit, and the rail was one mistap
        // from being stuck shut. min_touch_target is 44 on touch and 28 with a
        // pointer, which is the number this should always have been.
        //
        // `rail` REMOVED. It rendered the same `workspace.tabs` as the tab
        // column above, in the same frame, with nothing switching between them
        // — 48 lines whose only capability the column lacked was the chevron,
        // and the column has that now. What the column has and rail did not:
        // fences, pinning, close, drag, web panes, scroll-to.
        //
        // That duplication is a large part of why this surface read as less
        // minimal than the Qt one: three left panels (sidebar, tabbar, rail)
        // against `main.qml`'s single 168-point `vTabBar`, which hosts "work
        // tabs (scrollable) + a default fence + system tabs".


        // THE BOTTOM BAR, which had a model, a renderer, a gesture and no
        // call site. `bars::bar_ui` was reached only from its own tests and
        // `bottom_bar::default_bar` from nothing at all — so the phone's
        // whole navigation surface, the one that gives four lists to a
        // screen with room for one, was never drawn.
        //
        // COMPACT ONLY. On a desktop the sidebar, the tab strip and the
        // rail are all on screen at once and a fan-out bar would be a
        // fourth way to reach the same lists; on a phone none of them fit
        // and this is the only way.
        if adaptive.is_compact() {
            self.draw_bottom_bar(ctx);
        }

        // Mail read in the terminal becomes mail in the mail view.
        self.harvest_mail_from_blocks();
        self.pump_mail_signin();
        if self.mail_signin.is_running() {
            // The worker answers on its own thread; keep frames coming so
            // the answer is drawn when it lands, not on the next click.
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            // SAFE AREA. Nothing in the fleet applied these until now — the
            // field existed, the setter existed, and no app read either, so
            // every port drew under a notch and behind a gesture bar. It is a
            // no-op on a desktop, where the insets are zero, so it can be
            // called unconditionally.
            let safe = adaptive.inset(ui.max_rect());
            if safe != ui.max_rect() {
                // Only build a child when the insets are real. On a desktop
                // they are zero and an extra Ui layer would cost a layout pass
                // per frame for nothing.
                ui.set_clip_rect(safe);
            }

            // THE LOCATOR. shows_locator() returns true in BOTH modes and its
            // comment says why: it "is what stops voice mode from being
            // disorienting; removing it would save fourteen points and cost
            // the user their place". In voice mode the sidebar and tab strip
            // are gone, so without this there is nothing on screen saying
            // which tab you are in.
            //
            // It was ported with the escape chord, the reclaimed-rows maths
            // and its own tests, and never drawn.
            {
                let title = self.workspace.tabs.get(self.workspace.active)
                    .map(|t| t.name.clone()).unwrap_or_default();
                // WHAT THIS SESSION IS ABOUT, beside where you are.
                // `session_auto` shipped pure and tested in v0.1.658 and the Qt
                // build has surfaced it since v0.1.720; this frontend never
                // referenced the module at all, so the segmenter, the namer and
                // the proposals were all carried across and none of their output
                // was. Recomputed only when a command finishes — the derivation
                // walks every block, and the locator draws every frame.
                if self.blocks.blocks.len() != self.session_name_at {
                    self.session_name_at = self.blocks.blocks.len();
                    // AND WHAT TO DO ABOUT IT. `session_auto::propose` shipped pure and
                    // tested in v0.1.658 and has had no caller here since — it wants an
                    // ACTIVITY, and this side only had command blocks, and it wants a real
                    // current repo, which every block claiming one hardcoded path could
                    // never supply. `session_proposals` is the adapter; OSC 7 supplied the
                    // repo.
                    //
                    // Computed on the same edge as the name, for the same reason: the
                    // derivation walks every block and the locator draws every frame.
                    // AND WHAT KIND OF WORK IT IS. `cmd_cluster` was measured to 1.00
                    // purity on the planted set, wired into the Qt build in v0.1.720, and
                    // never mounted in this frontend at all —
                    // `TODO_command_clustering.md`'s "REMAINING (QML): feeding the dominant
                    // cluster label into session_autoname".
                    //
                    // On the same edge as the name and the proposal, for the same reason:
                    // k-means over every block is not a per-frame cost.
                    self.session_cluster = {
                        let cmds: Vec<(String, String)> = self
                            .blocks
                            .blocks
                            .iter()
                            .filter(|b| !b.command.trim().is_empty())
                            .map(|b| (b.command.clone(), b.cwd.clone().unwrap_or_default()))
                            .collect();
                        // A fixed seed: k-means starts from a random pick, and a label that
                        // changed every time a command ran would read as the app being
                        // unsure rather than the session changing.
                        voiceterm_egui::session_cluster::describes(
                            &cmds, 0x5eed, voiceterm_egui::session_cluster::MIN_SHARE)
                    };
                    self.session_proposal = {
                        use voiceterm_egui::session_proposals as sp;
                        let pairs: Vec<(String, String)> = self
                            .blocks
                            .blocks
                            .iter()
                            .map(|b| (b.command.clone(), b.cwd.clone().unwrap_or_default()))
                            .collect();
                        // The LAST block is the candidate; everything before it is context.
                        // A proposal is about what just happened, not about the run as a
                        // whole.
                        match pairs.split_last() {
                            Some(((cmd, cwd), before)) => {
                                let acts = sp::activities(
                                    before.iter().map(|(c, d)| (c.as_str(), d.as_str())),
                                );
                                let repo = self
                                    .blocks
                                    .current_cwd
                                    .as_deref()
                                    .unwrap_or("")
                                    .trim_end_matches('/')
                                    .rsplit('/')
                                    .next()
                                    .unwrap_or("")
                                    .to_string();
                                sp::activity_of(cmd, cwd).and_then(|next| {
                                    voiceterm_egui::session_auto::propose(
                                        &sp::open_files(&acts),
                                        sp::dominant_lang(&acts),
                                        &repo,
                                        &next,
                                        "hx",
                                    )
                                })
                            }
                            None => None,
                        }
                    };
                    self.session_name = voiceterm_egui::session_name::session_name(
                        self.blocks.blocks.iter().map(|b| {
                            (b.command.as_str(), b.cwd.as_deref().unwrap_or(""))
                        }),
                    );
                }
                let mut title = if self.session_name.is_empty() {
                    title
                } else {
                    format!("{title} \u{b7} {}", self.session_name)
                };
                // The cluster label sits beside the name, not instead of it: the name says
                // WHERE (repo, file), the label says WHAT KIND of work, and neither answers
                // the other's question.
                if let Some(k) = &self.session_cluster {
                    title = format!("{title} \u{b7} {k}");
                }
                // The suggestion, as a question rather than an action. It sits in the
                // locator because that line is already about "where am I" — and it is
                // phrased with a `?` because nothing here will do it for you.
                let _ = &mut title;
                // The suggestion as a TAPPABLE chip, not a sentence in the status line. It
                // was drawn into the locator text last commit, which made it a question
                // nobody could answer — a suggestion you cannot accept is a nag with extra
                // steps.
                let hint = self.session_proposal.as_ref().map(|p| {
                    use voiceterm_egui::session_auto::Proposal;
                    match p {
                        Proposal::NewSessionForRepo { repo } => format!("new session for {repo}"),
                        Proposal::NewTabForFile { path, .. } => format!("new tab for {path}"),
                        Proposal::ReentryAfterReboot { restore } => format!("restore {restore}"),
                    }
                });
                let text = self.chrome.locator(
                    self.workspace.active,
                    self.workspace.tabs.len(),
                    &title,
                    self.workspace.active_pane_ids().len(),
                );
                let accepted = voiceterm_egui::chrome_mode::locator_ui_with_action(
                    ui, &text, self.chrome.escaped(), hint.as_deref(), &self.colors);
                if accepted {
                    if let Some(p) = self.session_proposal.take() {
                        self.accept_proposal(p);
                    }
                }
            }

            // SWIPE BETWEEN TABS. The surface is the whole central area, and
            // at_limit is false because the workspace wraps — there is no edge
            // to bounce off, so the barrier never needs painting here.
            //
            // Routed through run_command rather than calling the workspace
            // directly, so a swipe, the palette and the voice grammar all
            // reach tabs by one path. A second route would be a second place
            // for tab switching to diverge.
            let frame = self.tab_swipe.update(ui, ui.max_rect(), false);
            match frame.page_jump {
                Some(uikit_gesture::Direction::Next) => {
                    self.run_command(voiceterm_egui::palette::Command::NextTab)
                }
                Some(uikit_gesture::Direction::Prev) => {
                    self.run_command(voiceterm_egui::palette::Command::PrevTab)
                }
                None => {}
            }
            // Read from the workspace model, not a separate field — this is the
            // other half of the `PaneView` fix (see the doc comment above): the
            // palette's `Command::OpenView` writes to `workspace` via
            // `open_view`, and the quick-launcher's two doors now do too, so
            // this is the single place that has to read it back for either to
            // reach the screen at all.
            let view = self.workspace.pane_view(self.workspace.focused_pane());
            if view != PaneView::Terminal {
                let c = self.colors;
                // SCOPED TO THE PANE. Every view below gives its widgets a fixed
                // `id_salt`, which is unique within one instance and identical between
                // two: `ui.columns` does not scope ids, so two panes showing the same
                // view would resolve to ONE widget — and for a `TextEdit` that means one
                // text cursor between them, which reads as "I can only get one character
                // into this box". Only the focused pane draws its view today, so nothing
                // collides yet; this is one line that keeps it that way when the others
                // start drawing. `tests/views_probe.rs` pins the rule by drawing each
                // view twice in a frame.
                let pane_scope = self.workspace.focused_pane();
                ui.push_id(("pane-view", pane_scope), |ui| {
                match view {
                    PaneView::Mail => {
                        // The spoken selector, above the typed query. "my last
                        // couple emails from bob" parses to a count and a
                        // sender — the grammar every mail intent except
                        // OpenDrafts is built on, and it had no surface at all.
                        ui.horizontal(|ui| {
                            ui.label("say:");
                            ui.add(egui::TextEdit::singleline(&mut self.mail_phrase).id_salt("voiceterm_egui-self_mail_phrase"));
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
                        // THE ACCOUNT. Proton sign-in through the engine the Qt
                        // build carried and never finished (SRP was a stub); the
                        // proofs are real now and the inbox headers come back
                        // over the wire. One line when signed in or collapsed,
                        // the form when open.
                        ui.horizontal(|ui| {
                            let label = match &self.mail_signin.signed_in {
                                Some(who) => format!("Proton: {who}"),
                                None if self.mail_signin.is_running() => "Proton: signing in…".to_string(),
                                None => "Proton: not signed in".to_string(),
                            };
                            let arrow = if self.mail_login_open { "▾" } else { "▸" };
                            if ui.button(format!("{arrow} {label}")).clicked() {
                                self.mail_login_open = !self.mail_login_open;
                            }
                            // Only while the form is closed: open, the form
                            // shows the same message in full under the button.
                            if !self.mail_signin.status.is_empty() && !self.mail_login_open {
                                ui.add(egui::Label::new(egui::RichText::new(&self.mail_signin.status)
                                    .weak().size(11.0)).truncate());
                            }
                        });
                        if self.mail_login_open {
                            if self.mail_signin.needs_totp {
                                ui.horizontal(|ui| {
                                    ui.label("code:");
                                    let r = ui.add(egui::TextEdit::singleline(&mut self.mail_totp_code)
                                        .id_salt("mail-totp").hint_text("6 digits").desired_width(120.0));
                                    let go = ui.button("verify").clicked()
                                        || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                                    if go && !self.mail_totp_code.trim().is_empty() {
                                        let code = std::mem::take(&mut self.mail_totp_code);
                                        self.mail_signin.totp(&code);
                                    }
                                });
                            } else if let Some(creds) = voiceterm_egui::mail_login_view::login_ui(ui, &mut self.mail_login, &c) {
                                self.mail_login.busy = true;
                                self.mail_login.error = None;
                                self.mail_signin.start(&creds.username, &creds.password);
                                // A code typed up front is used the moment the
                                // account asks for one.
                                if let Some(t) = creds.totp { self.mail_totp_code = t; }
                            }
                            ui.separator();
                        }
                        if self.mail_signin.needs_totp && !self.mail_totp_code.is_empty() && !self.mail_login_open {
                            let code = std::mem::take(&mut self.mail_totp_code);
                            self.mail_signin.totp(&code);
                        }
                        // WHERE THE MAIL CAME FROM, and how to look again.
                        // An empty inbox with no explanation is
                        // indistinguishable from a broken one, and the
                        // difference here is usually just that this machine
                        // keeps its mail somewhere else.
                        ui.horizontal(|ui| {
                            if ui.button("rescan mail").clicked() {
                                self.rescan_mail_store();
                            }
                            let note = match &self.mail_store_path {
                                Some(p) => format!("{} — {} message(s)",
                                                   p.display(), self.mail_from_disk.len()),
                                None => "no mail store found ($MAIL, ~/Maildir, ~/mail, \
                                         /var/mail)".to_string(),
                            };
                            ui.label(egui::RichText::new(note).weak().size(11.0));
                        });
                        // THE SECURITY CHECKUP. The mail stack keeps its key in
                        // a hand-rolled file that no desktop wallet knows about,
                        // and its PGP engine can decrypt but cannot verify — so
                        // mail that opens looks authenticated and is not.
                        // Neither fact was visible anywhere in the UI. This is
                        // `secret_kit::wizard` saying both, worst first.
                        {
                            let env: std::collections::BTreeMap<String, String> =
                                std::env::vars().collect();
                            let plan = secret_kit::Plan::detect(&env);
                            let facts = secret_kit::wizard::Facts {
                                // Observed, not assumed: nothing in this tree
                                // talks to a wallet yet, so the file is what is
                                // actually in use whatever the plan would prefer.
                                store_in_use: secret_kit::Store::EncryptedFile,
                                plan: plan.clone(),
                                can_verify: false,
                                can_sign: false,
                                key_expires_in_days: None,
                                has_revocation_cert: false,
                                two_password_mode: false,
                            };
                            let findings = secret_kit::wizard::checkup(&facts);
                            let head = secret_kit::wizard::headline(&findings);
                            let open_id = ui.make_persistent_id("mail-security-checkup");
                            let title = match head {
                                Some(f) => format!("Security: {}", f.title),
                                None => "Security: this setup is sound".to_string(),
                            };
                            egui::CollapsingHeader::new(
                                egui::RichText::new(title).color(match head.map(|f| f.severity) {
                                    Some(secret_kit::wizard::Severity::Critical) => c.danger,
                                    Some(secret_kit::wizard::Severity::Warn) => c.warning,
                                    _ => c.text_weak,
                                }))
                                .id_salt(open_id)
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(plan.explain()).weak().size(11.0));
                                    ui.add_space(4.0);
                                    for f in &findings {
                                        let col = match f.severity {
                                            secret_kit::wizard::Severity::Critical => c.danger,
                                            secret_kit::wizard::Severity::Warn => c.warning,
                                            secret_kit::wizard::Severity::Advice => c.text,
                                            secret_kit::wizard::Severity::Good => c.success,
                                        };
                                        ui.colored_label(col, format!(
                                            "[{}] {}", f.severity.label(), f.title));
                                        ui.label(egui::RichText::new(&f.detail).weak().size(11.0));
                                        if let Some(r) = &f.remedy {
                                            ui.label(egui::RichText::new(format!("→ {r}"))
                                                     .size(11.0).color(c.accent));
                                        }
                                        ui.add_space(6.0);
                                    }
                                });
                        }
                        if let Some(conv) = voiceterm_egui::mail_view::list_ui(ui, &mut self.mail, &c) {
                            // OPENING A THREAD READS IT. `mark_thread_read`
                            // had no caller, so the unread count and the
                            // bold rows survived opening the mail — the
                            // one thing a reader must not do.
                            //
                            // It returns the message ids that changed, for
                            // a backend to push a seen flag to. Nothing
                            // here has one yet: mail harvested from a
                            // terminal is already read, so the list is
                            // empty in practice and the toast says what
                            // happened rather than pretending otherwise.
                            let changed = self.mail.mark_thread_read(&conv);
                            let msg = if changed.is_empty() {
                                format!("opened {conv}")
                            } else {
                                format!("opened {conv} — {} marked read", changed.len())
                            };
                            self.toasts.info(msg, ui.input(|i| i.time));
                        }
                        ui.separator();
                        // The inbox proper (mail_panes, ported): rows over the
                        // real Message model, message pane below on open.
                        use voiceterm_egui::mail as mp;
                        if let Some(act) = mp::inbox_ui(ui, &self.mail_all, false, &c) {
                            match act {
                                mp::MailAction::Open(id) => self.mail_open = Some(id),
                                mp::MailAction::Search(q) => self.mail.query = q,
                                _ => {}
                            }
                        }
                        if let Some(id) = self.mail_open.clone() {
                            // THE LAZY FETCH ITSELF. `self.mail.bodies` is a
                            // cache — `MailView`'s own doc calls it "fetched
                            // lazily" — but nothing ever fetched into it, so
                            // every message showed empty regardless of
                            // source. Fetch once per id, from whichever
                            // source map has it (disk first, matching
                            // `mail_store::merge`'s own precedence), and
                            // cache the result so re-opening the same
                            // message doesn't redo the lookup every frame.
                            if !self.mail.bodies.contains_key(&id) {
                                let text = self.mail_bodies_disk.get(&id)
                                    .or_else(|| self.mail_bodies_terminal.get(&id));
                                let body = match text {
                                    Some(t) => voiceterm_egui::mail_view::Body::Plain(t.clone()),
                                    None => voiceterm_egui::mail_view::Body::Failed(
                                        "no body found for this message".to_string()),
                                };
                                self.mail.bodies.insert(id.clone(), body);
                            }
                            if let Some(m) = self.mail_all.iter().find(|m| m.id == id) {
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
                    PaneView::Web => {
                        // `{N}b` / `{N}B` — the vim-style tab cycle, on WEB panes only.
                        //
                        // `b` typed into the terminal has to reach the shell, so this cannot
                        // be a global binding, and the Qt build scopes it the same way: the
                        // TODO's first line is "a count-prefixed tab cycle for WEB tabs".
                        //
                        // Only when no text field has the keyboard, or the address bar would
                        // stop accepting the letter b.
                        if !self.surfaces.captures_input()
                            && !ui.ctx().wants_keyboard_input()
                        {
                            let typed: Vec<char> = ui.input(|i| {
                                i.events
                                    .iter()
                                    .filter_map(|e| match e {
                                        egui::Event::Text(t) => Some(t.chars().collect::<Vec<_>>()),
                                        // An on-screen keyboard commits rather than sending
                                        // key events — the same gap the terminal had.
                                        egui::Event::Ime(egui::ImeEvent::Commit(t)) => {
                                            Some(t.chars().collect())
                                        }
                                        _ => None,
                                    })
                                    .flatten()
                                    .collect()
                            });
                            for ch in typed {
                                let _consumed = self.feed_tab_count(ch);
                            }
                        }
                        // PER-PANE NOW. `self.browser` used to carry url/
                        // title/actions/etc directly, so every web pane in
                        // the tiling layout read and wrote the SAME state —
                        // two open at once showed the same page in both.
                        // `PaneKind::Web{url}` has been per-pane in the model
                        // since `split_web` was written; this keys the
                        // presentation state the same way. `or_default`
                        // rather than pre-seeding at creation time: a pane
                        // whose kind is already `Web{url}` (opened via
                        // `open_web_pane_at`) still needs an entry here on
                        // first render, and starting blank then loading is
                        // simpler than threading the initial url through a
                        // second path.
                        let web_pane_id = self.workspace.focused_pane();
                        let pane = self.web_panes.entry(web_pane_id).or_default();
                        // A freshly-opened pane's `PaneKind::Web{url}` may
                        // already carry a real url (open_web_pane_at) that
                        // this per-pane entry hasn't seen yet — seed it once,
                        // the same "not equal, so (re)load" rule the graph
                        // view's focus-seeding uses.
                        if let Some(voiceterm_egui::terminal_workspace::PaneKind::Web { url }) = {
                            use voiceterm_egui::terminal_workspace::TermPanes as _;
                            self.workspace.model().pane_kind(web_pane_id)
                        }{
                            if *url != pane.url && !url.is_empty() {
                                pane.load(url);
                            }
                        }
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
                            let r = ui.add(egui::TextEdit::singleline(&mut pane.url).id_salt("voiceterm_egui-self_browser_url"));
                            let entered = r.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if entered || r.lost_focus() {
                                let u = pane.url.clone();
                                pane.load(&u);
                            }
                            // THE STAR. `Browser::is_bookmarked`/`toggle_bookmark`
                            // existed with a doc comment saying "drives the
                            // star's filled state" and no star anywhere — the
                            // sitemap, the walk player, the connections graph
                            // and the archived-page browser all read
                            // `bookmarks.all()` as though pages get saved, and
                            // nothing in this pane ever called `save`. Disabled
                            // on an empty URL: starring nothing is not a
                            // command, it is a blank bookmark waiting to
                            // confuse the sitemap that reads this list.
                            let can_star = !pane.url.trim().is_empty();
                            let starred = pane.is_bookmarked(&self.browser.bookmarks);
                            let glyph = if starred { "\u{2605}" } else { "\u{2606}" }; // ★ / ☆
                            if ui.add_enabled(can_star, egui::Button::new(glyph)).clicked() {
                                let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;
                                pane.toggle_bookmark(&mut self.browser.bookmarks, now_ms);
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
                        let pane = self.web_panes.get_mut(&web_pane_id).expect("seeded above");
                        if let Some(path) = voiceterm_egui::explore::sitemap_ui(ui, &groups, &c) {
                            pane.load(&path);
                        }

                        // THE WALK PLAYER. explore's header says session_walk's
                        // QML side "consumes a wire string built from these;
                        // the camera animation + slider live in main.qml — so
                        // the PLAYER is exactly what did not come across". It
                        // came across as walk_ui and was never called.
                        //
                        // Fed from the same bookmarks the sitemap uses, for
                        // the same reason stated there: built from the
                        // browser's OWN pages rather than a fixture, because a
                        // walk through invented paths would look like a
                        // feature and be a decoration. last_session clusters
                        // on time, and saved_at_ms is a real timestamp.
                        let walk_pages: Vec<_> = self.browser.bookmarks.all().iter()
                            .map(|b| voiceterm_egui::session_walk::WalkPage {
                                id: b.url.clone(),
                                title: b.title.clone(),
                                domain: voiceterm_egui::explore::domain_of(&b.url),
                                url: b.url.clone(),
                                preview: String::new(),
                                created_at: b.saved_at_ms / 1000,
                            })
                            .collect();
                        let session = voiceterm_egui::session_walk::last_session(&walk_pages);
                        self.walk.load(session);
                        let pane = self.web_panes.get_mut(&web_pane_id).expect("seeded above");
                        if let Some(i) = voiceterm_egui::explore::walk_ui(ui, &mut self.walk, &c) {
                            if let Some(pg) = self.walk.pages().get(i) {
                                pane.load(&pg.url.clone());
                            }
                        }
                        if let Some(why) = pane.empty_reason() {
                            ui.colored_label(c.text_weak, why);
                        }
                        for a in pane.ranked() {
                            let on = a.actionable();
                            ui.add_enabled(on, egui::Button::new(a.label()));
                            if let Some(r) = a.blocked_reason() {
                                ui.colored_label(c.text_weak, r);
                            }
                        }
                    }
                    PaneView::Graph => {
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
                        // WHICH GRAPH. This pane could only ever draw the typed
                        // scratch document. The notes themselves — the actual
                        // zettelkasten, already the corpus that backlinks and
                        // TF-IDF are rebuilt from — had no way in. Both sources
                        // now produce (ids, topology) or say why they cannot,
                        // and ONE draw path below gets the focus seeding, the
                        // depth controls and the breadcrumbs, so the notes are
                        // not given a lesser view than the scratch buffer.
                        ui.horizontal(|ui| {
                            ui.colored_label(c.text_weak, "graph of:");
                            ui.selectable_value(&mut self.graph_from_notes, true, "my notes");
                            ui.selectable_value(&mut self.graph_from_notes, false, "typed source");
                        });
                        let built: Option<(Vec<String>, voiceterm_egui::graph_view::nifty::Topology)> =
                        if self.graph_from_notes {
                            let notes: Vec<(String, String)> = self.deck.cards.iter()
                                .map(|card| (card.title.clone(), card.body.clone())).collect();
                            if notes.is_empty() {
                                ui.colored_label(c.text_weak,
                                    "no notes yet — capture some, or switch to the typed source");
                                None
                            } else {
                                Some((notes.iter().map(|(t, _)| t.clone()).collect(),
                                      voiceterm_egui::graph_view::topology_from_notes(&notes)))
                            }
                        } else {
                            // Parsed from the source, exactly as the Qt bin's
                            // nifty_topology does: parse, then take ids and edges.
                            ui.add(egui::TextEdit::multiline(&mut self.graph_source).id_salt("voiceterm_egui-self_graph_source")
                                   .desired_rows(3).hint_text("graph source"));
                            let doc = nifty_core::niftyplot::parse(&self.graph_source);
                            if doc.nodes.is_empty() {
                                // An empty graph is a real state — an unwritten
                                // diagram — but it must SAY so rather than drawing
                                // an empty ring that looks like a failed render.
                                ui.colored_label(c.text_weak, "no nodes — type a graph above");
                                None
                            } else {
                                let ids: Vec<String> = doc.nodes.iter().map(|n| n.id.clone()).collect();
                                let edges: Vec<(String, String)> = doc.edges.iter()
                                    .map(|e| (e.from.clone(), e.to.clone())).collect();
                                Some((ids.clone(),
                                      voiceterm_egui::graph_view::nifty::Topology::new(&ids, &edges)))
                            }
                        };
                        if let Some((ids, topo)) = built {
                            // SEED THE FOCUS. `GraphView::layout` returns nothing
                            // while `focus` is empty, and a focus that is not IN
                            // the graph being drawn is reset rather than
                            // breadcrumbed to a node that does not exist.
                            if !ids.iter().any(|id| *id == self.graph.focus) {
                                if let Some(first) = ids.first() {
                                    self.graph.focus = first.clone();
                                    self.graph.trail.clear();
                                }
                            }
                            // DEPTH AND BACK: unit-tested since before this pane
                            // existed, but nothing called them until now.
                            ui.horizontal(|ui| {
                                if ui.small_button("−").clicked() { self.graph.shallower(); }
                                ui.label(format!("depth {}", self.graph.depth));
                                if ui.small_button("+").clicked() { self.graph.deeper(); }
                                if !self.graph.trail.is_empty() && ui.small_button("← back").clicked() {
                                    self.graph.back();
                                }
                            });
                            voiceterm_egui::graph_view::graph_ui(ui, &mut self.graph, &topo, &c);
                            // APPLY what the palace resolved. Fork and Tag sat
                            // in `pending` with nothing reading them, so both
                            // keys were inert. Only meaningful against the
                            // notes — a fork of a scratch niftyplot node has
                            // nowhere to go — so the typed source just clears
                            // them and says so rather than silently dropping.
                            let raised = std::mem::take(&mut self.graph.pending);
                            if !raised.is_empty() {
                                if self.graph_from_notes {
                                    use voiceterm_egui::palace::Action;
                                    let focus = self.graph.focus.clone();
                                    let now = ui.input(|inp| inp.time);
                                    let mut touched = false;
                                    for a in raised {
                                        match a {
                                            Action::Fork => {
                                                match voiceterm_egui::note_edit::fork_note(
                                                    &mut self.deck.cards, &focus) {
                                                    Some(t) => {
                                                        self.toasts.info(&format!("forked {focus} → {t}"), now);
                                                        self.graph.focus = t;
                                                        self.graph.trail.clear();
                                                        touched = true;
                                                    }
                                                    None => self.toasts.warning(
                                                        &format!("no note called {focus} to fork"), now),
                                                }
                                            }
                                            Action::Tag(t) => {
                                                use voiceterm_egui::note_edit::TagOutcome::*;
                                                let said = match voiceterm_egui::note_edit::tag_note(
                                                    &mut self.deck.cards, &focus, &t) {
                                                    Added(tag) => { touched = true; format!("#{tag} on {focus}") }
                                                    AlreadyPresent(tag) => format!("{focus} already has #{tag}"),
                                                    Unusable => format!("{t:?} is not a usable tag"),
                                                    NoSuchNote => format!("no note called {focus}"),
                                                };
                                                self.toasts.info(&said, now);
                                            }
                                            _ => {}
                                        }
                                    }
                                    if touched {
                                        // The indexes are rebuilt from the SAME
                                        // cards, exactly as the capture path
                                        // does, so backlinks and TF-IDF never
                                        // describe notes the deck no longer has.
                                        let notes: Vec<(String, String)> = self.deck.cards.iter()
                                            .map(|cd| (cd.title.clone(), cd.body.clone())).collect();
                                        self.backlinks.rebuild(&notes);
                                        let mut df = voiceterm_egui::nlp::DocFreq::new();
                                        for (_, body) in &notes { df.add_document(body); }
                                        self.doc_freq = df;
                                    }
                                } else {
                                    let now = ui.input(|inp| inp.time);
                                    self.toasts.warning(
                                        "fork and tag apply to notes — switch the graph source", now);
                                }
                            }
                        }
                    }
                    PaneView::Table => {
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
                        if let Some(submitted_line) = voiceterm_egui::table_view::table_view_ui(
                            ui, &mut self.table, &shown.headers, &shown.rows, &c)
                        {
                            // THE BUTTONS DISCARDED THEIR OWN RETURN VALUE. The
                            // "undo"/"reset" buttons return `Some("undo")` /
                            // `Some("reset")` from `table_view_ui` — but until
                            // now this call site only checked `.is_some()` and
                            // then called `submit_with_engine`, which reads
                            // `self.table.input` (the TEXT FIELD), not the
                            // string the button actually returned. The field
                            // is empty after every prior submit, and `submit`
                            // treats an empty line as "nothing typed" and
                            // returns immediately — so both buttons silently
                            // did nothing, while typing "undo" and pressing
                            // Enter worked, because that path writes the field
                            // itself before returning the same string.
                            //
                            // `submitted_line` is correct for BOTH sources: in
                            // the typed-Enter case it already equals what's in
                            // the field (the widget wrote it); in the
                            // button-click case it's the only place the
                            // intent lives at all. Write it back before
                            // submitting so button and typed input go through
                            // the identical parse path, and capture it now —
                            // submit() clears the field on success, so this is
                            // the last point it's readable for the re-parse
                            // below.
                            let line = submitted_line.clone();
                            self.table.input = submitted_line;
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
                    PaneView::Docs => {
                        // READING MODE. corpus_tape records a paragraph read
                        // aloud, and the corpus it fills is what acoustic_cache
                        // later learns from — "a bad recording here becomes a
                        // bad fingerprint there, and a bad fingerprint promotes
                        // wrong text". So the quality gate is the feature.
                        use voiceterm_egui::corpus_tape as ct;
                        ui.horizontal(|ui| {
                            ui.label("read aloud:");
                            ui.add(egui::TextEdit::singleline(&mut self.tape_reference).id_salt("voiceterm_egui-self_tape_reference"));
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
                        // PROOF OF LIFE FOR `doc::render_page`. The module's
                        // own comment on the function: "doc.rs has had the
                        // page model... and NO RENDERER... the model that
                        // exists to replace web2qml has never been on
                        // screen." The renderer exists now (`render_page`,
                        // `render_page_with`, `FormState`, `submit` — 30
                        // tests in the kit) and STILL has zero callers
                        // anywhere in this app: `browser::page_ui_for`, the
                        // settings bridge for it, is dead too.
                        //
                        // This is deliberately NOT wired into the real Web
                        // pane. That needs the pane-opening gap fixed first
                        // (LinkAction::Open is a no-op — see the comment
                        // where it's handled) and a per-pane state decision
                        // (self.browser is one shared field; the page model
                        // is per-document by construction). Both are real
                        // design calls, not a slot-in.
                        //
                        // What this answers instead, narrowly: does the
                        // renderer actually work. `FORMS`-shaped sample
                        // (login field, checkbox, select, textarea) so a
                        // form round-trip is visible without touching any
                        // of that.
                        ui.collapsing("page renderer preview (doc.rs, not yet wired to Web)", |ui| {
                            const SAMPLE: &str = r#"<html><body>
<form action="/submit"><input type="text" name="q" placeholder="Search"><button type="submit">Go</button></form>
<div><input type="checkbox" name="agree"><label>I agree</label></div>
<select><option>One</option><option>Two</option></select>
<textarea>default text</textarea>
</body></html>"#;
                            // Parsed once per frame here — fine for a fixed
                            // demo string; a real caller would parse on load,
                            // per the crate's own "flatten once" doc.
                            let page = uikit_web::doc::parse_page(SAMPLE);
                            match uikit_web::doc::render_page(
                                ui, &page, &mut self.page_demo_form, &c)
                            {
                                Some(uikit_web::doc::PageAction::Submit(s)) => {
                                    self.toasts.info(
                                        format!("{} {} → {}", s.method, s.action, s.query),
                                        ui.input(|i| i.time));
                                }
                                Some(uikit_web::doc::PageAction::Follow(href)) => {
                                    self.toasts.info(format!("follow: {href}"), ui.input(|i| i.time));
                                }
                                Some(uikit_web::doc::PageAction::OpenIsland { tag, src }) => {
                                    self.toasts.info(format!("island: {tag} {src}"), ui.input(|i| i.time));
                                }
                                None => {}
                            }
                        });
                        // THE SAVED NOTES JOIN THE SHIPPED DOCS, refreshed here rather than
                        // at startup: a note written this session would otherwise not appear
                        // until the app was restarted, and "add to the web" means opening
                        // the note in the thing that already renders markdown rather than a
                        // second viewer that renders it slightly differently.
                        //
                        // Only when the set has actually changed. `set_docs` prunes the
                        // reader's navigation stack against the new list, so replacing it
                        // every frame would drop the page being read the moment a note was
                        // saved.
                        self.refresh_note_docs();
                        if let Some(name) = voiceterm_egui::manpage_view::manpage_ui(
                            ui, &mut self.docs, &c)
                        {
                            // The view returns a NAVIGATION target, not a
                            // command: a link inside a document names another
                            // document, and following it is the reader's job.
                            let _ = name;
                        }
                    }
                    PaneView::Models => {
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
                            ui.add(egui::TextEdit::singleline(&mut self.model_query).id_salt("voiceterm_egui-self_model_query"));
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
                        let ctx: String = (0..self.app.terminal.grid.rows)
                            .map(|y| self.app.terminal.grid.row_text(y))
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
                    PaneView::Archive => {
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
                    PaneView::Chat => {
                        // The local model's conversation. The panel owns the
                        // transcript and returns what the user sent; there is
                        // no llama.cpp in this crate, so the reply is an
                        // explicit notice rather than a fabricated answer —
                        // an invented assistant turn is worse than none.
                        //
                        // THE WRONG FORMAT, ENTIRELY. This used to build a
                        // plain-English string ("user: ...\nassistant: ...")
                        // and hand it to `reload`, which expects the wire
                        // format `parse_messages` actually parses:
                        // `M<0x1F><role><0x1F><content>`. A plain-English
                        // blob matches none of that, so `parse_messages`
                        // found zero records and `reload` silently emptied
                        // the transcript on every single send — confirmed
                        // live: typed a message, pressed Enter, the field
                        // cleared (the send DID fire) and nothing ever
                        // appeared, still showing the empty-state placeholder.
                        //
                        // `reload` exists to sync from an external backend's
                        // full-transcript dump — there is no such backend
                        // here, and `ChatPanel::messages`/`ChatMessage` are
                        // both public for exactly this reason, so push
                        // directly instead of round-tripping through a wire
                        // format built for a backend that doesn't exist in
                        // this build. Appends, so the conversation persists
                        // turn to turn — `reload` REPLACES the list, which
                        // would have erased history on every message even
                        // once the format matched.
                        // THE HEADER BUTTONS WERE ALSO DISCARDED. `.ui(ui)`
                        // is `ui_with(ui).0` — the sent text only, dropping
                        // `.1`, the `ChatAction` New/Close report. Header
                        // and buttons render (from the earlier QML-outline
                        // fix upstream) and looked live, but clicking either
                        // did nothing: confirmed by clicking "New" here and
                        // watching the same two messages stay on screen.
                        // `ui_with` is what actually carries both.
                        use voiceterm_egui::chat_panel::{ChatAction, ChatMessage, Role};
                        let (sent, action) = self.chat.ui_with(ui);
                        if let Some(sent) = sent {
                            self.chat.messages.push(ChatMessage { role: Role::User, content: sent });
                            self.chat.messages.push(ChatMessage { role: Role::Assistant,
                                content: "(no local model attached in this build)".to_string() });
                        }
                        match action {
                            Some(ChatAction::New) => self.chat.messages.clear(),
                            Some(ChatAction::Close) => { self.workspace.open_view(PaneView::Terminal); }
                            None => {}
                        }
                    }
                    PaneView::Notes => {
                        // CAPTURE → CARDS → BACKLINKS, in that order, because
                        // that is the dependency: the splitter produces the
                        // notes the index links. Editing the capture re-splits
                        // (the deck keeps its selection BY IDENTITY, which is
                        // why a re-split does not silently move it).
                        use voiceterm_egui::{backlinks_view as bl, cards_view as cv};
                        self.ingest_ui(ui, &c);
                        ui.separator();
                        ui.label(egui::RichText::new("captured text").weak().size(11.0));
                        let changed = ui.add(
                            egui::TextEdit::multiline(&mut self.captured).id_salt("voiceterm_egui-self_captured")
                                .desired_rows(4).desired_width(f32::INFINITY)).changed();
                        if changed {
                            let src = cv::CardSource {
                                url: self.focused_web_url(),
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
                        // THE PREVIEW SHEET, over the grid the deck just drew.
                        // deck_ui already owns selection, so this only has to
                        // ask whether there is one — no second source of truth
                        // for which card is open.
                        if let Some(a) = voiceterm_egui::card_preview::card_preview_ui(
                            ui, &self.deck, &c)
                        {
                            use voiceterm_egui::card_preview::PreviewAction as P;
                            match a {
                                // Typed, not run — same rule as a clicked
                                // search hit: the operator presses Enter.
                                P::Stage(body) => {
                                    self.stage_command(&body);
                                    self.deck.clear_selection();
                                }
                                P::OpenSource(url) => {
                                    self.session.pty_out.extend_from_slice(
                                        format!("xdg-open {url}").as_bytes());
                                    self.deck.clear_selection();
                                }
                                P::MergeNext => {
                                    // The deck decides whether a merge is
                                    // possible; a false return means this was
                                    // the last card and nothing happened.
                                    let _ = self.deck.merge_with_next();
                                }
                                P::Dismiss => self.deck.clear_selection(),
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
                            ui.add(egui::TextEdit::singleline(&mut self.backlinks.query).id_salt("voiceterm_egui-self_backlinks_query"));
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
                        // CLIPS, WITH PROVENANCE. A separate spine from the
                        // capture/cards/backlinks system above — that one
                        // splits free-typed prose into cards; this is
                        // TODO_notes_clip.md's "highlight → card": select
                        // something in the terminal, run `Command::
                        // ClipSelection`, and it lands here with a citation
                        // saying which command produced it. Read-only
                        // display of `Note::to_markdown()`'s raw output for
                        // this first slice — a rendered markdown view (the
                        // same doc.rs/manpage_view pipeline the Docs pane
                        // uses) is real future work, not attempted here.
                        if !self.clip_note.blocks.is_empty() {
                            ui.separator();
                            ui.label(egui::RichText::new(format!(
                                "clips — {}", self.clip_note.title)).weak().size(11.0));
                            egui::ScrollArea::vertical().id_salt("clip_note").max_height(200.0)
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(
                                        self.clip_note.to_markdown()).monospace());
                                });
                        }
                    }
                    PaneView::Build => {
                        // Compiler output as a navigable list. No cargo is
                        // invoked here — the output is pasted, which is the
                        // same no-subprocess contract the Models tab keeps.
                        ui.horizontal(|ui| {
                            ui.label("paste cargo output:");
                            if ui.button("parse").clicked() {
                                self.build.set_output(&self.build_input);
                            }
                        });
                        ui.add(egui::TextEdit::multiline(&mut self.build_input).id_salt("voiceterm_egui-self_build_input")
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
                            ui.add(egui::TextEdit::singleline(&mut self.build.query).id_salt("voiceterm_egui-self_build_query"));
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
                    PaneView::Notebook => {
                        // Step 1 of TODO_notebook_view.md's own sequencing:
                        // segmented cells, classified, rendered plainly.
                        // `notebook::classify` has existed since v0.1.608
                        // with zero UI in either frontend — the pure core
                        // was built, dogfooding it never happened. Rich
                        // Table/Map widgets and touch gestures (collapse,
                        // pin, swipe) are explicitly NOT this step; the
                        // doc's own advice is "don't build all three before
                        // dogfooding step 1".
                        //
                        // A separate view rather than swapping the live
                        // terminal's own render/gesture/input loop in place
                        // (the TODO's original framing) — read-only here,
                        // over whatever `self.blocks` already has, so there
                        // is nothing in the terminal's own proven path this
                        // can regress.
                        use voiceterm_egui::notebook::Structured;
                        use voiceterm_egui::notebook_cells::fold_summary;
                        if self.blocks.blocks.is_empty() {
                            ui.colored_label(c.text_weak,
                                "no command blocks yet — run something in the terminal");
                        }
                        // Step 3's useful half: FOLDS. The pane had none, so a session
                        // with a build log in it was a page of scrolling to reach the
                        // cell you wanted. Folded cells keep their command and a line
                        // saying what is inside — a fold that hides what it hid is a row
                        // you have to open to identify.
                        self.notebook_cells.retain_ids(|id| self.blocks.blocks.iter().any(|b| b.id == id));
                        if !self.blocks.blocks.is_empty() {
                            ui.horizontal(|ui| {
                                let n = self.notebook_cells.folded_count();
                                if ui.small_button(if n > 0 { "unfold all" } else { "fold all" }).clicked() {
                                    let ids: Vec<u32> = self.blocks.blocks.iter().map(|b| b.id).collect();
                                    self.notebook_cells.set_all(ids.into_iter(), n == 0);
                                }
                                if n > 0 {
                                    ui.colored_label(c.text_weak, format!("{n} folded"));
                                }
                            });
                        }
                        // WHAT THE NOTES SAID about the last word tapped, with somewhere to
                        // go. Each hit is a button that opens that note in the docs reader —
                        // the answer is only worth having if it leads somewhere.
                        // Decided inside the closure, acted on after it: the strip's buttons
                        // touch `self.docs` and `self.workspace`, which the closure cannot.
                        let mut open_note: Option<String> = None;
                        let mut dismiss_ask = false;
                        let mut make_card = false;
                        if let Some((word, hits)) = self.word_ask.clone() {
                            ui.horizontal(|ui| {
                                ui.colored_label(c.text_weak, format!("\u{201c}{word}\u{201d}"));
                                if hits.is_empty() {
                                    // A real answer, and said plainly. "Nothing" is
                                    // information; silence is indistinguishable from a tap
                                    // that missed.
                                    ui.colored_label(c.text_weak, "\u{2014} no notes about this");
                                }
                                for h in &hits {
                                    if ui.small_button(&h.title)
                                        .on_hover_text("open this note")
                                        .clicked()
                                    {
                                        open_note = Some(h.name.clone());
                                    }
                                }
                                // MAKE A CARD from the line, not the word. Cards in this app
                                // come from the note — `split_into_cards` runs over it — so
                                // this appends a clip rather than pushing a card into a deck
                                // that is rebuilt from the note anyway and would drop it.
                                if self.word_ask_line.is_some()
                                    && ui.small_button("+ card")
                                        .on_hover_text("clip this line into the note")
                                        .clicked()
                                {
                                    make_card = true;
                                }
                                if ui.small_button("\u{d7}").on_hover_text("dismiss").clicked() {
                                    dismiss_ask = true;
                                }
                            });
                        }
                        if let Some(name) = open_note {
                            // Refreshed FIRST: `open` refuses a document the reader does not
                            // hold, and the reader only learns about notes when the DOCS pane
                            // draws — so this click, made from the notebook, would silently do
                            // nothing while the same click from the docs pane worked. That is
                            // a difference nobody reports; they just stop using the feature.
                            self.refresh_note_docs();
                            if self.docs.open(&name) {
                                self.workspace.open_view(PaneView::Docs);
                            }
                            self.word_ask = None;
                        }
                        if make_card {
                            if let Some((line, command, at)) = self.word_ask_line.clone() {
                                self.clip_line(&line, &command, at);
                            }
                            self.word_ask = None;
                            self.word_ask_line = None;
                        }
                        if dismiss_ask {
                            self.word_ask = None;
                            self.word_ask_line = None;
                        }
                        let cells = &mut self.notebook_cells;
                        // THE WAY BACK, above everything. A dismissal with no visible undo
                        // is indistinguishable from output going missing — and the block is
                        // still there, so the view must not pretend otherwise.
                        if cells.hidden_count() > 0 {
                            let n = cells.hidden_count();
                            ui.horizontal(|ui| {
                                ui.colored_label(c.text_weak, format!("{n} cell(s) dismissed"));
                                if ui.small_button("show all").clicked() {
                                    cells.unhide_all();
                                }
                            });
                        }
                        egui::ScrollArea::vertical().id_salt("notebook").show(ui, |ui| {
                            for b in &self.blocks.blocks {
                                // Swiped out of the view. The block, its output and its
                                // scrollback are untouched: this is a view filter.
                                if cells.is_hidden(b.id) {
                                    continue;
                                }
                                ui.separator();
                                // CLASSIFIED ONCE, not once a frame. `classify` tries a
                                // markdown table, CSV, TSV, aligned columns and two
                                // key/value forms over the whole output; this loop ran it
                                // for every block on every repaint.
                                let what = cells.classify(b.id, &b.output).clone();
                                let folded = cells.is_folded(b.id);
                                let pinned = cells.is_pinned(b.id);
                                let mut toggle = false;
                                let mut pin = false;
                                // Which header was clicked, decided inside the draw and
                                // acted on after: `cells` is borrowed for the table's own
                                // state while the grid draws.
                                let mut sort_col: Option<usize> = None;
                                let mut swipe_dx: Option<f32> = None;
                                // The word a tap landed on, acted on after the draw: the
                                // search reads the note corpus and `cells` is borrowed here.
                                let mut word_tap: Option<String> = None;
                                let mut word_line: Option<(String, String, u64)> = None;
                                let mut swipe_end = false;
                                ui.horizontal(|ui| {
                                    if ui.small_button(if folded { "\u{25b8}" } else { "\u{25be}" }).clicked() {
                                        toggle = true;
                                    }
                                    // The command line is the swipe handle as well as the
                                    // fold toggle: `click_and_drag` gives both, and a
                                    // separate grab strip would be one more thing to aim at
                                    // on a phone.
                                    let r = ui.add(egui::Label::new(
                                        egui::RichText::new(format!("$ {}", b.command)).strong())
                                        .sense(egui::Sense::click_and_drag()));
                                    if r.clicked() {
                                        toggle = true;
                                    }
                                    if r.dragged() {
                                        swipe_dx = Some(r.drag_delta().x);
                                    } else if r.drag_stopped() {
                                        // Short of the threshold: forget the travel, or the
                                        // cell dismisses itself on a later unrelated touch.
                                        swipe_end = true;
                                    }
                                    match b.exit_code {
                                        Some(0) => { ui.colored_label(
                                            egui::Color32::from_rgb(90, 180, 100), "●"); }
                                        Some(code) => { ui.colored_label(c.warning, format!("● {code}")); }
                                        None => {}
                                    }
                                    // THE PIN, on the right of the row. `retain_ids` already
                                    // drops it with the block, so a pinned cell that scrolls
                                    // out of history takes its pin with it rather than
                                    // leaving the strip pointing at nothing.
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let on = pinned;
                                            if ui
                                                .selectable_label(on, "\u{1f4cc}")
                                                .on_hover_text(if on { "unpin" } else { "pin: keep this one in view" })
                                                .clicked()
                                            {
                                                pin = true;
                                            }
                                        },
                                    );
                                });
                                if let Some(w) = word_tap {
                                    let corpus = voiceterm_egui::note_store::documents();
                                    // `search_tapped`, not `search`: a tap is one chosen
                                    // word, and `search` drops anything under three
                                    // characters -- so tapping "AI" or "UI" found nothing.
                                    let hits = voiceterm_egui::note_search::search_tapped(&w, &corpus, 3);
                                    // HELD, not toasted. A toast names the related notes and
                                    // then takes them away — the answer to "what does this
                                    // belong with" is only useful if you can go there, and a
                                    // three-second window to read a title and act on it is
                                    // not an action.
                                    self.word_ask = Some((w, hits));
                                    self.word_ask_line = word_line;
                                }
                                if let Some(dx) = swipe_dx {
                                    if cells.drag_cell(b.id, dx) {
                                        // Hidden as of this frame; nothing below would draw
                                        // anything useful for it.
                                        continue;
                                    }
                                }
                                if swipe_end {
                                    cells.drag_release(b.id);
                                }
                                if let Some(col) = sort_col {
                                    // `usize::MAX` is the clear button — a sentinel rather
                                    // than a second flag, since both are "the operator
                                    // touched the table's controls" and they cannot both
                                    // happen in one frame.
                                    let v = cells.table_mut(b.id, Default::default);
                                    if col == usize::MAX {
                                        v.clear_ops();
                                    } else {
                                        // One sort at a time: pushing a second would stack
                                        // it under the first and look like the click did
                                        // nothing.
                                        v.clear_ops();
                                        v.push_op(voiceterm_egui::table_ops::TableOp::Sort {
                                            column: col,
                                            ascending: true,
                                        });
                                    }
                                }
                                if pin {
                                    // Pinning UNFOLDS: a pinned cell showing "42 lines
                                    // hidden" is the wrong kind of pin. Unpinning leaves it
                                    // open — see `notebook_cells`.
                                    if pinned { cells.toggle_pin(b.id); } else { cells.pin_and_show(b.id); }
                                }
                                if toggle {
                                    cells.toggle_fold(b.id);
                                }
                                if folded {
                                    ui.colored_label(c.text_weak, fold_summary(&what, &b.output));
                                    continue;
                                }
                                match what {
                                    Structured::Plain => {
                                        // TAP A WORD → what does it belong with. Step 4 of
                                        // TODO_notes_clip.md. The pointer gives a position;
                                        // egui's galley turns that into a character index,
                                        // and `note_search::word_at` turns THAT into a word
                                        // — the part that has to agree with the matcher's
                                        // idea of a boundary, or a tap on "cargo-build"
                                        // searches for something the matcher cannot find.
                                        let job = egui::text::LayoutJob::simple(
                                            b.output.clone(),
                                            egui::FontId::monospace(
                                                ui.style().text_styles[&egui::TextStyle::Body].size),
                                            c.text,
                                            ui.available_width(),
                                        );
                                        let galley = ui.painter().layout_job(job);
                                        let r = ui.add(
                                            egui::Label::new(galley.clone())
                                                .sense(egui::Sense::click()));
                                        if r.clicked() {
                                            if let Some(p) = r.interact_pointer_pos() {
                                                let cur = galley.cursor_from_pos(p - r.rect.min);
                                                word_tap = voiceterm_egui::note_search::word_at(
                                                    &b.output, cur.index)
                                                    .map(str::to_string);
                                                // The LINE too, kept for "make a card": a
                                                // card whose body is one word is not an
                                                // atomic note, and the line is what the
                                                // reader was actually looking at.
                                                word_line = voiceterm_egui::note_search::line_at(
                                                    &b.output, cur.index)
                                                    .map(str::to_string)
                                                    .map(|l| (l, b.command.clone(), b.started_at));
                                            }
                                        }
                                    }
                                    Structured::Table(t) => {
                                        // SORTABLE NOW. The comment that was here said this
                                        // stayed read-only because "giving each notebook
                                        // cell its own persistent state (a HashMap keyed by
                                        // block id) is real, separate work; a sort button
                                        // that resets every frame would be worse than no
                                        // button". `NotebookCells::table_mut` is that state,
                                        // so the reason is gone.
                                        //
                                        // Still not the full `table_view_ui`: that carries a
                                        // command line and a status area meant for a whole
                                        // pane, and a notebook cell is one output among
                                        // dozens. A header you can click to sort is the part
                                        // that earns its space here.
                                        let base = t.clone();
                                        let view = cells.table_mut(b.id, || base);
                                        let shown = view.apply();
                                        let depth = view.ops.len();
                                        ui.horizontal(|ui| {
                                            ui.colored_label(c.text_weak, format!(
                                                "\u{25A4} table \u{2014} {} rows \u{d7} {} cols",
                                                shown.rows.len(), shown.headers.len()));
                                            // Only when there is something to clear: a
                                            // button that does nothing teaches that buttons
                                            // here do nothing.
                                            if depth > 0
                                                && ui.small_button("clear")
                                                    .on_hover_text("drop the sort")
                                                    .clicked()
                                            {
                                                sort_col = Some(usize::MAX);
                                            }
                                        });
                                        egui::Grid::new(("notebook_table", b.id))
                                            .striped(true).show(ui, |ui| {
                                            for (i, h) in shown.headers.iter().enumerate() {
                                                if ui.add(egui::Label::new(
                                                    egui::RichText::new(h).strong().small())
                                                    .sense(egui::Sense::click()))
                                                    .on_hover_text("sort by this column")
                                                    .clicked()
                                                {
                                                    sort_col = Some(i);
                                                }
                                            }
                                            ui.end_row();
                                            for row in &shown.rows {
                                                for cell in row {
                                                    ui.label(egui::RichText::new(cell).small());
                                                }
                                                ui.end_row();
                                            }
                                        });
                                    }
                                    Structured::Map(pairs) => {
                                        ui.colored_label(c.text_weak,
                                            format!("key/value — {} pairs", pairs.len()));
                                        // Bold key, matching the Table
                                        // branch's bold header row — same
                                        // "the structure is visually
                                        // distinct from the values" idea,
                                        // applied to the other Structured
                                        // kind rather than left as plain
                                        // "key: value" text only.
                                        egui::Grid::new(("notebook_map", b.id)).show(ui, |ui| {
                                            for (k, v) in &pairs {
                                                ui.label(egui::RichText::new(k).strong().small());
                                                ui.label(egui::RichText::new(v).small());
                                                ui.end_row();
                                            }
                                        });
                                    }
                                }
                            }
                        });
                    }
                    PaneView::Terminal => {}
                }
                });
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
            // THE KEYBOARD HALF OF MODALITY. The backdrop blocks the
            // pointer, but this loop reads raw ctx.input events, and egui
            // does not remove events because an open surface's TextEdit
            // looked at them — so without this gate, typing into any open
            // surface also typed into the shell. The QML never had the
            // problem: a modal popup takes focus and the terminal item is
            // simply not delivered keys. (Esc is doubly safe here:
            // Modal::should_close CONSUMES it before this runs.)
            // THIS HOST ALWAYS ENCODES.
            //
            // Not conditional on having a shell, which is what I tried
            // first and got wrong twice. The widget's fallback IS its
            // line buffer, `terminal.input` — and this host never draws
            // it. It renders `session.grid`. So handing input to the
            // widget when no shell is attached makes typing INVISIBLE
            // until Enter, which is worse than the doubling it was
            // meant to fix.
            //
            // Whether a shell exists decides where the bytes GO — the
            // pty, or feed_pty for local echo — not who produces them.
            // One encoder, two sinks.
            //
            // The widget keeps its line buffer running (see
            // keys::widget_handles): it emits no bytes, and `submitted`
            // is what the audit chain and the risk gate below are built
            // on.
            let input_owner = uikit_term::keys::InputOwner::Host;
            self.app.set_input_owner(input_owner);
            debug_assert!(
                uikit_term::keys::host_encodes(input_owner),
                "this host renders session.grid, not the widget's line buffer, \
                 so it must always be the encoder"
            );
            // THE TERMINAL CLAIMS THE KEYBOARD, so Tab reaches the shell instead of
            // selecting the next sidebar button. egui decides what Tab MEANS in
            // `Memory::begin_pass`, before any app code runs, so consuming the key here was
            // always too late — the fix is a real focusable widget with an event filter.
            // See `kbd_focus`, which also explains why the old behaviour was intermittent.
            let term_id = egui::Id::new(("terminal-keyboard", self.workspace.focused_pane()));
            // The pane's own rect. Registered BEFORE the terminal's own widgets so a click
            // on something drawn later still reaches it — egui resolves overlapping
            // interactions in favour of the widget added last.
            let claim_rect = ui.max_rect();
            let claim = voiceterm_egui::kbd_focus::claim(
                ui,
                term_id,
                claim_rect,
                !self.surfaces.captures_input(),
            );
            // ASK FOR IME while the terminal has the keyboard. winit only delivers IME
            // events after `set_ime_allowed(true)`, and eframe calls that from
            // `PlatformOutput::ime`, which egui sets only for a focused TextEdit. The
            // terminal is not one, so nothing ever asked — and an on-screen keyboard had no
            // reason to send its commits here. Letters arrive as commits; the editing keys
            // arrive as real key events, which is why backspace worked and `a` did not.
            if matches!(claim, voiceterm_egui::kbd_focus::Claim::Held) {
                // The cursor's own row, so the compositor's keyboard and candidate window do
                // not cover the line being typed.
                let ch_h = ui.text_style_height(&egui::TextStyle::Monospace);
                let cur_y = claim_rect.top()
                    + (self.app.terminal.grid.cur_row as f32 * ch_h).min(claim_rect.height());
                let cursor = egui::Rect::from_min_size(
                    egui::pos2(claim_rect.left(), cur_y),
                    egui::vec2(1.0, ch_h),
                );
                voiceterm_egui::kbd_focus::request_ime(ui.ctx(), claim_rect, cursor);
            }
            let key_outs = if self.surfaces.captures_input() {
                Vec::new()
            } else if claim == voiceterm_egui::kbd_focus::Claim::Yielded {
                // A text field has it — a find bar, the palette, an inline editor. Not the
                // terminal's keys to read.
                Vec::new()
            } else {
                // THE LIVE MODE, NOT THE DEFAULT.
                //
                // In application-cursor mode (DECCKM — set by fzf, vim,
                // less, and readline in some configs) arrows are SS3
                // `ESC O A`, not CSI `ESC [ A`. Passing
                // TermMode::default() here hardcodes "never in app
                // mode", so the encoder's app-cursor branch and the
                // parser's DECCKM tracking were both correct and both
                // unreachable — arrows did nothing useful inside exactly
                // the programs pty_input's header names as the reported
                // symptom (Ctrl+R fzf history, where you cannot move
                // through the match).
                //
                // FROM THE GRID THE SHELL ACTUALLY FEEDS.
                //
                // There are two grids. `session.grid` is what feed_pty
                // writes and what this view draws; `app.terminal.grid`
                // belongs to the uikit-term widget and, for the focused
                // pane, is never fed at all. Asking the widget's grid
                // for the mode read a parser with no input — it returned
                // the default forever, so the previous "fix" here
                // changed the expression and not the behaviour.
                //
                // Fourth link in the same chain: encoder, parser, call
                // site, and now the right parser.
                // WITH THE SELECTION, which decides who gets Ctrl+C. egui-winit converts it
                // to `Event::Copy` before any key event exists, so the terminal never saw it
                // — with text selected that keystroke is a copy, and with nothing selected it
                // is the interrupt a shell user reaches for when something is wrong.
                voiceterm_egui::pty_input::drain_egui_owned_sel(
                    ui, self.app.terminal.grid.key_mode(),
                    self.workspace.tabs.len() > 1,
                    Some(term_id),
                    self.app.selection.is_some())
            };
            for out in key_outs
            {
                // THE BYTES, WHICH THIS LOOP USED TO THROW AWAY.
                //
                // `drain_egui` consumes the frame's key events and returns an
                // `Outcome` per press: `chord` for a UI action, `bytes` for
                // terminal input. Only `chord` was read. So every encoded
                // keystroke — arrows, Ctrl-C, Backspace, Alt-word-motions, the
                // whole reason pty_input exists — was computed correctly and
                // dropped on the floor, and the terminal took no keyboard input
                // at all.
                //
                // It is `pty_out` they belong in, not a direct `feed_pty`: that
                // buffer is what a host drains to a real shell, and the local
                // echo below replays only its unseen tail. Appending here means
                // the same bytes both reach a future PTY and appear on screen,
                // through one path rather than two that can disagree.
                if !out.bytes.is_empty() {
                    // HOLD THE ENTER OF A DANGEROUS LINE.
                    //
                    // The risk confirmation used to hang off the widget's
                    // `submitted`, which fires on the SAME keypress whose
                    // Enter byte the raw path had already sent — so the
                    // dialog asked whether to run a command that had
                    // already run. A confirmation you answer afterwards
                    // is not a confirmation.
                    //
                    // Only the Enter byte is withheld. The characters are
                    // already in the shell's line editor, so confirming
                    // just sends `\r`, and cancelling leaves the line
                    // typed and editable rather than silently discarding
                    // what someone wrote.
                    let line = self.app.terminal.input.clone();
                    if voiceterm_egui::cmd_risk::withhold_enter(
                        &out.bytes,
                        &line,
                        self.password_mode,
                        self.pending_confirm.is_some(),
                    ) {
                        use voiceterm_egui::confirm_dialog::{ConfirmDialog, TimeoutAction};
                        self.pending_confirm = Some((
                            String::new(), // the line is already typed; only \r is pending
                            ConfirmDialog::new(
                                "Run this command?",
                                format!("`{line}` can modify or delete data."),
                                "Run",
                            )
                            .with_timeout(20.0)
                            .with_timeout_action(TimeoutAction::Cancel),
                        ));
                    } else {
                        // MIRROR A LINE DISCARD. Ctrl-C and Ctrl-U empty
                        // the shell's input line; the widget keeps its
                        // own copy, and withhold_enter reads that copy to
                        // decide whether a command needs confirming. Let
                        // them drift and a confirmation quotes text that
                        // was never entered — or, worse, a stale prefix
                        // makes a real command classify wrong.
                        //
                        // Terminal::interrupt existed for this and had no
                        // caller anywhere.
                        if uikit_term::keys::clears_the_line(&out.bytes) {
                            self.app.terminal.interrupt();
                        }
                        // Same rule as send_input: typing returns to the
                        // live tail. `out.snap_to_bottom` is the
                        // encoder's own answer to this and had no
                        // consumer, because there was nothing to scroll.
                        if out.snap_to_bottom || uikit_term::keys::clears_the_line(&out.bytes) {
                            self.app.terminal.grid.scroll_to_bottom();
                        }
                        self.session.pty_out.extend_from_slice(&out.bytes);
                    }
                }
                // `out.snap_to_bottom` is deliberately not consumed. This view
                // draws a fixed grid with no scrollback, so it is always
                // showing the tail and there is nothing to scroll. Wiring it to
                // the one `stick_to_bottom` ScrollArea in this file would point
                // it at a different panel. A host that adds scrollback should
                // read it then — the encoder already computes it correctly.
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
                        // Through apply() with the same phrases the voice and
                        // menu paths use, for the reason given above NewTab:
                        // `tabs`/`active` are a projection of `model`, and the
                        // chord must not become a second way to say this that
                        // can drift from the first.
                        //
                        // "horizontal" means side-by-side here — see
                        // parse_command, which follows tmux's -h/-v rather
                        // than naming the split LINE. Konsole calls the same
                        // thing "Split View Left/Right".
                        Chord::SplitRight => {
                            self.workspace.apply(
                                &voiceterm_egui::gesture_menus::WsCall::Command(
                                    "split horizontal".into()));
                        }
                        Chord::SplitDown => {
                            self.workspace.apply(
                                &voiceterm_egui::gesture_menus::WsCall::Command(
                                    "split vertical".into()));
                        }
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
            let cursor_row_text =
                self.app.terminal.grid.row_text(self.app.terminal.grid.cur_row);
            self.password_mode = voiceterm_egui::password_prompt::is_password_prompt(
                &voiceterm_egui::password_prompt::TermState {
                    alt_screen: false,
                    foreground: None,
                    visible: Some(&cursor_row_text),
                });
            // The widget's own find bar, which searches app.terminal's
            // grid — fed since feed_pty started mirroring there, so it
            // has the shell's history for the first time. The
            // hand-rolled bar that searched session.grid is gone: it
            // found matches in a buffer nobody was looking at and
            // scrolled a view nobody could see.
            self.app.search_ui(ui);
            if self.password_mode {
                // SAID OUT LOUD. A terminal that silently stops echoing looks
                // broken; one that says why looks careful — and this is also
                // the only signal that the line will not be audited.
                ui.colored_label(self.colors.warning, "password prompt — input hidden, not recorded");
            }
            let submitted = if chorded { None } else { self.app.handle_input(ui) };
            let key_bytes = self.app.take_pending_bytes();
            self.session.pty_out.extend_from_slice(&key_bytes);
            // THE SCREEN. Drawn only when there is something on it, so a fresh
            // session shows the block log rather than 24 blank rows.
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
                // NO BYTES FROM HERE. The raw key path already sent this
                // line, character by character, as it was typed — that is
                // what made the shell run everything twice. `submitted`
                // is now purely a NOTIFICATION that a line was entered:
                // it feeds the audit chain above, and nothing else.
                //
                // The risk confirmation moved to the key path too, where
                // it can withhold the Enter byte. Asking here was always
                // too late: `\r` had already gone to the shell on the
                // same keypress that produced this line.
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
            if self.session.pty_out.len() > self.session.pty_echoed {
                let fresh = self.session.pty_out[self.session.pty_echoed..].to_vec();
                // THE BLOCK TRACKER, FED HERE AND NOWHERE ELSE.
                //
                // I first hooked this into the keyboard drain, which sees
                // only what was typed. Nine other places append to
                // `pty_out` — voice commands, the keycap row, the gesture
                // menus, the staged command from the bottom bar — and none
                // of them would have produced a block, so the history would
                // have quietly recorded typing and nothing else.
                //
                // This is the one funnel every byte bound for the shell
                // passes through, which is what makes it the right place:
                // there is no way to send a command that misses it.
                self.record_typed(&fresh);
                match &self.pty {
                    // A REAL SHELL: send the keys, and let its output be
                    // the echo. Feeding the grid ourselves as well would
                    // double every character, because the shell echoes
                    // what it receives — the local echo below exists only
                    // for the case where nothing is listening.
                    Some(host) => {
                        if let Err(e) = host.write(&fresh) {
                            self.toasts.warning(format!("shell: {e}"), ui.input(|i| i.time));
                        }
                    }
                    // No shell (spawn failed, or a platform without one):
                    // echo locally so typing is at least visible, which
                    // is what this branch has always done.
                    None => self.feed_pty(&fresh),
                }
                self.session.pty_echoed = self.session.pty_out.len();
            }

            // KEEP THE SHELL'S IDEA OF THE SIZE IN STEP with the grid,
            // or every full-screen program draws for the wrong terminal —
            // vim in an 80x24 world inside a 200x50 window. `resize`
            // skips no-ops itself, so calling it per frame is cheap and
            // means nothing has to track when the window changed.
            if let Some(host) = &mut self.pty {
                host.resize(
                    self.app.terminal.grid.cols.min(u16::MAX as usize) as u16,
                    self.app.terminal.grid.rows.min(u16::MAX as usize) as u16,
                );
            }

            // WHAT THE SHELL SAID. Drained every frame and fed to the same
            // parser the local echo uses, so there is one path into the
            // grid rather than two that can disagree about cursor state.
            // Drained into a local FIRST, then the borrow ends: `feed_pty`
            // takes `&mut self` and holding `&self.pty` across it is
            // E0502. Same shape as aperture's typeout fix — a borrow of
            // one field held across a whole-self method.
            let drained = self.pty.as_ref().map(|h| (h.drain(), h.ended()));
            if let Some((out, ended)) = drained {
                if !out.is_empty() {
                    // The tracker sees the SHELL'S OUTPUT too, line by line.
                    // `feed_line` closes the open block when it recognises a
                    // prompt, which is what gives a block an end without the
                    // shell reporting an exit code — and the method had no
                    // caller, so no block opened here would ever have
                    // closed.
                    // Feed_pty first: it learns the cwd from OSC 7, and `finish` in the line
                    // loop below reads tracker state. Not load-bearing for the block's cwd —
                    // a block is stamped when `submit_command` OPENS it, at Enter, by which
                    // time the prompt's announcement has long since arrived. (I reordered
                    // this believing feed_line opened blocks. It does not; it only closes
                    // them. The order is kept because state-before-consumers is the right
                    // default, not because it fixes anything.)
                    self.feed_pty(&out);
                    let text = String::from_utf8_lossy(&out).into_owned();
                    for line in text.split_inclusive(['\n', '\r']) {
                        let clean = line.trim_end_matches(['\n', '\r']);
                        // Nothing to feed for a bare line ending; feeding it
                        // would append a blank line to every block for each
                        // CRLF pair.
                        if !clean.is_empty() { self.blocks.feed_line(clean); }
                    }
                }
                if ended {
                    // Say so once. A terminal whose shell has exited but
                    // still accepts keystrokes is the confusing state
                    // this avoids.
                    if !self.pty_ended_noted {
                        self.pty_ended_noted = true;
                        self.toasts.info("shell exited", ui.input(|i| i.time));
                    }
                }
            }
            // BACKGROUND PANES. Only `self.pty` (the focused pane) was drained above — every
            // other pane's real shell was spawned and left running, but its output sat unread
            // in the OS pipe buffer forever: an unfocused pane looked frozen at whatever it
            // showed the moment it lost focus, no matter how long its shell kept producing
            // output. Mirrors the focused-pty drain above, but writes into that SPECIFIC pane's
            // own `pane_apps` entry (never `self.app`, which is a different pane's live state) —
            // the same per-pane isolation the render loop already reads from.
            for (id, host) in self.pane_ptys.iter() {
                let Some(h) = host else { continue };
                let out = h.drain();
                if out.is_empty() {
                    continue;
                }
                // INTO THE PANE'S SESSION, WHICH IS WHAT GETS DRAWN.
                //
                // This wrote into `pane_apps[id].terminal` — the
                // uikit-term widget's grid — while the view renders
                // `session.grid`. So a background pane's output landed
                // somewhere nothing draws, and was still missing when
                // the pane regained focus. The drain exists to stop an
                // unfocused pane looking frozen; writing to the wrong
                // grid moved the problem rather than fixing it.
                // Into the pane's own App — for an unfocused pane that
                // is where its displayed grid is parked, exactly as
                // swap_focus left it. I had moved this to pane_sessions
                // while I believed session.grid was the drawn one; with
                // one grid per pane living in the App, this is simply
                // the parked half of the same rule.
                if let Some(app) = self.pane_apps.get_mut(id) {
                    app.terminal.feed(&String::from_utf8_lossy(&out));
                    // And answer its queries on ITS OWN pty. Without this,
                    // helix started in a background split still stalls 2-3 s,
                    // and the fix above would only cover the focused pane.
                    let replies = app.terminal.take_replies();
                    if !replies.is_empty() {
                        let _ = h.write(&replies);
                    }
                }
                // `h.ended()` (shell exited while backgrounded) is intentionally not surfaced
                // here: the toast path above is keyed to the single focused-pane pty_ended_noted
                // flag, and a per-pane version of that notification is a separate, larger piece
                // of UI (which pane, shown where) rather than a one-line addition.
            }
            // The demo has no PTY to write to. Rather than dropping the bytes
            // again, they are kept on the app where a host can drain them, and
            // capped so a long session cannot grow without bound.
            const PTY_OUT_CAP: usize = 64 * 1024;
            if self.session.pty_out.len() > PTY_OUT_CAP {
                let drop = self.session.pty_out.len() - PTY_OUT_CAP;
                self.session.pty_out.drain(..drop);
                // The cap drains from the FRONT, so the mark has to move with
                // it or the next frame re-echoes everything the drain shifted
                // down. Saturating because a large drain can pass the mark.
                self.session.pty_echoed = self.session.pty_echoed.saturating_sub(drop);
            }
            let area = ui.available_rect_before_wrap();
            let colors = self.colors; // PaletteColors is Copy — keeps the borrows below simple

            // PER-PANE TERMINALS. `self.app` is always the focused pane's terminal; the rest
            // are parked in `pane_apps`. When focus moves, the two are SWAPPED rather than the
            // whole app being reworked around a map — so each pane keeps its own scrollback
            // and selection, and every existing `self.app` call site is untouched.
            let focus_now = self.workspace.focused_pane();
            // Both the block-log app AND the live session travel with focus,
            // through pane_host::swap_focus — which also closes the old
            // inline swap's hole: a fresh pane now starts FRESH instead of
            // inheriting the previous pane's state (and losing it for the
            // previous pane). pane_host::tests pins both properties.
            voiceterm_egui::pane_host::swap_focus(
                &mut self.app, &mut self.pane_apps,
                self.pane_focus, focus_now,
                || uikit_term::App::new(80, 24));
            voiceterm_egui::pane_host::swap_focus(
                &mut self.session, &mut self.pane_sessions,
                self.pane_focus, focus_now,
                voiceterm_egui::pane_host::PaneSession::new);
            // The shell swaps with its pane too, or the focused pane
            // would be typing into the previous pane's shell — output
            // appearing in the wrong terminal, which is more confusing
            // than no output at all.
            //
            // `Option<PtyHost>` is the swapped type, and the fresh value
            // is None rather than a new spawn: the spawn belongs in the
            // `diff.spawn` loop below, where a failure can be reported
            // once for the pane that caused it. Spawning here would
            // silently start a second shell on every focus change.
            voiceterm_egui::pane_host::swap_focus(
                &mut self.pty, &mut self.pane_ptys,
                self.pane_focus, focus_now,
                || None);
            self.pane_focus = focus_now;
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
                self.pane_sessions.insert(*id, voiceterm_egui::pane_host::PaneSession::new());
                // A PANE WITHOUT A SHELL IS AN EMPTY TERMINAL.
                //
                // `pty` was a single Option on the app while sessions and
                // grids are per-pane, so a pane opened from the hold menu
                // got a fresh grid, a fresh session — and nothing to talk
                // to. Keys went into its `pty_out` and no output ever came
                // back, which is exactly "text entry does not update the
                // viewport": the bottom bar worked because it does not go
                // through a shell.
                //
                // One shell per pane, spawned here where the pane is. A
                // failure is logged and left as None: that pane falls back
                // to local echo, which is worse than a shell and much
                // better than a dead rectangle.
                let host = match voiceterm_egui::pty_host::PtyHost::spawn(80, 24, repaint_waker(ui.ctx())) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        eprintln!("[voiceterm] pane {id}: no shell: {e}");
                        None
                    }
                };
                // Inserted either way — see the field doc: a remembered
                // None is what stops the next focus swap retrying a spawn
                // that already failed, once per switch, forever.
                self.pane_ptys.insert(*id, host);
            }
            for id in &diff.retire {
                self.pane_apps.remove(id);
                // the session goes with the pane — and with it its pty
                self.pane_sessions.remove(id);
                // Dropping the host kills and reaps the child (see
                // `PtyHost::drop`), so closing a pane does not leave a
                // shell running with nothing attached to it.
                self.pane_ptys.remove(id);
                // Forget the failure too, or a pane id reused by a later
                // session would inherit a verdict about a different pane.
                self.pty_spawn_failed.remove(id);
            }

            // THE FOCUSED PANE FALLS BETWEEN THE TWO RULES ABOVE.
            //
            // `hosted` includes `pane_focus`, so the spawn loop never
            // considers the focused pane — correct while it HAS a shell.
            // But `swap_focus` hands a newly focused pane `fresh()`,
            // which for `Option<PtyHost>` is `None`. So splitting a pane
            // produced a focused terminal with no shell, permanently:
            // the next frame has `prev == now`, `swap_focus` is a no-op,
            // and nothing else ever looks. Keys went to `pty_out`, fell
            // through to local echo, and no command ever ran.
            //
            // Spawning here rather than inside the loop keeps the
            // background rule intact: parked panes are reconciled by id,
            // the focused pane is reconciled by whether it actually has
            // a shell in hand.
            //
            // GUARDED ON PANE KIND. `split_web`'s own comment calls web
            // panes "never focused" as though that were enforced; it is
            // only a default — `Workspace::focus_pane` has no restriction,
            // and the unfocused-pane tile loop below calls it unconditionally
            // on click. Without this check, clicking a web pane's tile would
            // focus it and this block would spawn it a real shell it has no
            // use for, on the next frame, unconditionally. That is the actual
            // invariant "never focused" was protecting — not focus itself.
            let focused_is_web = self.workspace.pane_view(self.pane_focus)
                == voiceterm_egui::shell::PaneView::Web;
            if !focused_is_web && voiceterm_egui::pane_host::focused_needs_shell(
                self.pty.is_some(),
                self.pty_spawn_failed.contains(&self.pane_focus),
            ) {
                match voiceterm_egui::pty_host::PtyHost::spawn(80, 24, repaint_waker(ui.ctx())) {
                    Ok(h) => self.pty = Some(h),
                    Err(e) => {
                        eprintln!("[voiceterm] pane {}: no shell: {e}", self.pane_focus);
                        self.pty_spawn_failed.insert(self.pane_focus);
                    }
                }
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
                        // THE VIEW'S OWN RENDERER, where one exists. mail_view,
                        // table_view and graph_view are already built, already
                        // tested, and already called for the full-screen views
                        // — a pane was labelling itself while the code to draw
                        // it sat one module away.
                        //
                        // Drawn inside a child UI clipped to the pane rect, so
                        // a list longer than the pane scrolls within it rather
                        // than painting over its neighbours.
                        let mut child = ui.new_child(
                            egui::UiBuilder::new().max_rect(r.shrink(6.0)),
                        );
                        child.set_clip_rect(r.shrink(2.0));
                        match p.view {
                            voiceterm_egui::shell::PaneView::Mail => {
                                voiceterm_egui::mail_view::list_ui(
                                    &mut child, &mut self.mail, &colors);
                            }
                            // Table is NOT wired, deliberately. table_view_ui
                            // needs headers and rows, and the full-screen call
                            // gets them from the block the user opened — a
                            // pane has no such source, and inventing one would
                            // put fabricated data on screen. The pane needs a
                            // way to be pointed at a block first; that is a
                            // model change, not a render call.
                            _ => {
                                // Views with no renderer yet still NAME
                                // themselves. A blank box reads as broken,
                                // and an unlabelled pane cannot be told from
                                // one that failed to draw.
                                child.painter().text(
                                    r.center(),
                                    egui::Align2::CENTER_CENTER,
                                    p.view.label(),
                                    egui::FontId::proportional(12.0),
                                    colors.text_weak,
                                );
                            }
                        }
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
            // The selection re-read that used to live here is gone:
            // feed_pty now feeds app.terminal, so handle_gestures reads
            // a grid with the shell's text in it and its word/line
            // selection is correct on its own.
            // Long-press on the pane opens the 3x3 marking menu at the press point (detached host:
            // the terminal isn't a widget Response). The line selection the long-press also made is
            // exactly what the menu's "copy" cell copies.
            // THE LIVE VT GRID, in the FOCUSED pane. This block used to sit
            // above the pane layout in flow position, so every typed byte
            // echoed into a thin strip at the top of the panel while the
            // bordered pane below showed the demo log — the field report's
            // garbled line (its `<NN` tail is this block's cursor marker).
            // Painting here, after the focused app, puts the live session in
            // the pane the border says is focused. One grid still serves
            // whichever pane holds focus; a per-pane grid (session follows
            // its pane) stays on-device work, recorded in C11_TODO.
            // THE SECOND RENDER OF THE TERMINAL, REMOVED.
            //
            // A ~40-line block here re-drew the whole grid from
            // `session.grid` through its own row cache and galley cache.
            // Its comment said it painted "after the focused app" so the
            // live session would land in the pane the border marks as
            // focused — but `self.app.render_at` sits ~150 lines further
            // down and paints last, so every one of those rows was
            // shaped, laid out, painted and then covered.
            //
            // That inverted ordering IS the original "text does not
            // appear in the terminal" bug: the shell's output went to
            // session.grid, this block drew it, and render_at painted
            // app.terminal's grid on top. Making feed_pty mirror into
            // app.terminal fixed the symptom; this removes the wasted
            // half.
            //
            // Per frame, on a phone, that is a full grid of text layout
            // for pixels nobody sees. Verified by screenshot before and
            // after: identical.
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
                        // Open/Read/NewTab all resolve to the same thing right
                        // now: open a new pane beside the current one, already
                        // pointed at the url, and focus it. `open_web_pane_at`
                        // plus the per-pane `web_panes` state (both added this
                        // session — see their own doc comments) were the two
                        // pieces this needed; the Web pane's render arm seeds
                        // its `WebPane` entry from `PaneKind::Web{url}` on
                        // first draw, so there's exactly one place that turns
                        // a model url into presentation state, not two.
                        //
                        // Focusing it is new too: `focused_needs_shell`'s
                        // call site now skips spawning a shell for a focused
                        // pane whose view is `Web` (see that guard's comment)
                        // — without it, this would have quietly spawned a
                        // real shell process for a pane that has no use for
                        // one, the exact failure `split_web`'s "never
                        // focused" comment was actually protecting against.
                        Some(LinkAction::Open(u)) | Some(LinkAction::Read(u)) | Some(LinkAction::NewTab(u)) => {
                            let new_id = self.workspace.open_web_pane_at(&u);
                            self.workspace.focus_pane(new_id);
                        }
                        None => {}
                    }
                }
            }
            if let voiceterm_egui::slide_menu::SlideOutcome::Selected(id, variant) =
                self.pane_menu.show_detached(ui, &pane_menu())
            {
                match pane_dispatch(&id, variant) {
                    // Onto the grid that is DRAWN. Clearing the widget's
                    // terminal emptied a grid this view never renders, so
                    // "Clear" did nothing you could see.
                    Some(PaneAction::Clear) => self.app.terminal.clear(),
                    Some(PaneAction::CopySelection) => {
                        // Same reason: `Terminal::screen_text` reads the
                        // widget's grid, which nothing feeds for the
                        // focused pane, so "copy screen" copied an empty
                        // string. Built from the session grid instead —
                        // the same rows the user is looking at.
                        let text = self
                            .app
                            .selection
                            .take()
                            .map(|s| s.text)
                            .unwrap_or_else(|| self.app.terminal.screen_text());
                        ctx.copy_text(text);
                        self.toasts.success("Copied", ctx.input(|i| i.time));
                    }
                    // Opens THIS find bar, not the widget's: that one
                    // searches a grid nothing feeds and was never
                    // rendered by any frame here.
                    Some(PaneAction::CommandPalette) => self.app.open_search(),
                    // Through apply(), the same WsCall path the keyboard chords use —
                    // this arm was `{}` and the 3x3 menu's new-tab/split/close silently
                    // did nothing (field report, c11). Mic remains on-device wiring.
                    Some(PaneAction::Split(cmd)) | Some(PaneAction::Close(cmd)) => {
                        // the variant carries the EXACT ws_command string (gesture_menus:236)
                        self.workspace.apply(&voiceterm_egui::gesture_menus::WsCall::Command(cmd.into()));
                    }
                    Some(PaneAction::NewTab) => {
                        self.workspace.apply(&voiceterm_egui::gesture_menus::WsCall::Command("new tab".into()));
                    }
                    Some(PaneAction::Mic) => self.start_voice(),
                    // Onto app.terminal.grid, which is what render_at
                    // draws. Paging session.grid moved a buffer nobody
                    // sees — the mistake this whole area kept making,
                    // just in the other direction once I learned which
                    // grid is on screen.
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
            // HONOUR THE PREFERENCE. `voice.mic_enabled` is drawn as a
            // checkbox in the settings panel, saved, reloaded — and was
            // read NOWHERE. Turning the microphone off in settings left
            // the FAB on screen and push-to-talk working, which is worse
            // than the setting not existing: someone who turned it off
            // for privacy would reasonably believe the mic was off.
            //
            // Seven prefs are in this state (see C12-45); this is the one
            // with an unambiguous consumer, so it is the one wired here.
            if self.app.prefs.voice.mic_enabled {
                let fab = fab_rect(term_rect.shrink(10.0), 56.0);
                let resp = mic_fab_ui(ui, fab, &self.colors);
                if let Some(voiceterm_egui::action_compass::TermAction::Listen)
                    = self.compass.ui_from_host(ui, &resp, &mut self.app) {
                    self.start_voice();
                }
            }
        });
    }
}

/// Must match packaging/org.voiceterm.VoiceTerm.desktop exactly. A mismatch fails
/// SILENTLY: the app runs and has no icon, no dock grouping and the wrong
/// name in the switcher.
const APP_ID: &str = "org.voiceterm.VoiceTerm";


/// A waker for `PtyHost`: the reader thread calls this when the shell
/// speaks, so the frame that draws its output actually runs.
///
/// egui coalesces repeated requests inside one frame, so calling it per
/// read rather than per byte costs nothing.
fn repaint_waker(ctx: &egui::Context) -> voiceterm_egui::pty_host::Waker {
    let ctx = ctx.clone();
    std::sync::Arc::new(move || ctx.request_repaint())
}

fn main() -> eframe::Result<()> {
    // WHY THE WINDOW WILL NOT OPEN, said before the panic in a dependency does.
    //
    // A build that cannot reach a display server dies inside `wayland-sys`:
    // `wayland_client_handle()` tries libwayland-client.so.0, then .so, and `.expect()`s the
    // result. What reaches the user is a panic naming a line in a crate they have never
    // heard of — on a phone, in a terminal they cannot copy out of — and the message points
    // the wrong way, because the usual cause is that the library IS installed and this
    // binary cannot dlopen anything, having been linked statically.
    if let Some(why) = uikit_egui::backend_check::preflight() {
        eprintln!("{why}");
    }
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
    // NO DEMO BLOCKS. The tracker used to be seeded here with a hardcoded
    // script — an `ls --color`, an echo, a fake exit code — and then never
    // touched again, so everything reading it was reading that script: the
    // blocks drawer, recall, the memory poll's count, and the bottom bar's
    // quick-launch list. It is fed from real typing and real shell output
    // now (see the key drain and the pty drain), which means it starts
    // empty. Empty is the correct state before anyone has run anything, and
    // the drawer already has a view for it.
    //
    // The grid below still gets its sample output. That is a different
    // thing and worth keeping: it exercises colour, wrapping and multibyte
    // rendering on the first frame, which is the point of a demo, and it
    // does not pretend to be history.
    // DEMO CONTENT IS OPT-IN NOW. The comment above explains why the block
    // tracker stopped being seeded; the same argument applies to everything
    // else here and was not followed through. A shipped app started with a
    // fake shell transcript, a fake cwd (`/home/kessler/proj`) and two fake
    // workspaces called "kesslerd" with invented counts — which is what the
    // operator saw as "multiple layers of tabs ... scaffolding from the demo
    // lurking". It was.
    //
    // The sample output is still worth HAVING: it exercises colour, wrapping
    // and multibyte on the first frame, which no empty terminal does. So it
    // is kept and gated. `VOICETERM_DEMO=1` restores every bit of it.
    let demo = std::env::var_os("VOICETERM_DEMO").is_some();
    let mut blocks = uikit_term::blocks::BlockTracker::new();
    if demo {
    // A starting value only. Real blocks get theirs from OSC 7 (see `feed_pty`), which
    // corrects this on the shell's first prompt — before that there is nothing better to
    // say, and a block with no cwd at all names itself worse than one with a stale guess.
    blocks.set_cwd("/home/kessler/proj");
    app.terminal.feed("$ ls --color\r\n");
    app.terminal.feed("\x1b[1;34mCargo.toml\x1b[0m  \x1b[1;32mbuild.sh\x1b[0m  \x1b[1;34msrc\x1b[0m  \x1b[1;31mold.bak\x1b[0m\r\n");
    app.terminal.feed("$ echo \"a very long line that should wrap across more than one terminal row so we can see how wrapping and the cursor position interact once it spills over\"\r\n");
    app.terminal.feed("a very long line that should wrap across more than one terminal row so we can see how wrapping and the cursor position interact once it spills over\r\n");
    app.terminal.feed("$ echo café 🚀 你好\r\n"); // multibyte + emoji + CJK, straight in the source
    app.terminal.feed("café 🚀 你好\r\n");
    app.terminal.feed("$ ");
    }
    let colors = uikit_egui::palette_colors("voice");
    // No invented workspaces. An empty list is the correct state before the
    // user has made one, and the sidebar already has a view for it.
    let sidebar = Sidebar::new(if demo {
        vec![
            Session { name: "kesslerd".into(), count: 5 },
            Session { name: "kesslerd".into(), count: 2 },
        ]
    } else {
        Vec::new()
    });
    eframe::run_native(
        "voiceterm (egui)",
        eframe::NativeOptions {
        // Glow SAID, not left to eframe: its 0.34 defaults include
        // wgpu but not glow, and wgpu's default device descriptor
        // asks for 8 colour attachments — which dies at startup on
        // a phone GPU allowing 5. A glow-only feature list is not
        // enough; gitmind-egui declared one and still got wgpu
        // through feature unification.
        renderer: eframe::Renderer::Glow,
        viewport: uikit_egui::desktop::viewport(APP_ID, "VoiceTerm", [900.0, 700.0]),
        ..Default::default()
    },
        Box::new(move |_cc| {
            let mut app = VoicetermApp {
                osc7: Default::default(),
                keycaps_open: true,
                clip_note: voiceterm_egui::note_clip::Note::new("Session clips"),
                clip_note_path: None,
                jotting: false,
                word_ask: None,
                word_ask_line: None,
                // `b` forward, `B` back, capped so a long digit run cannot produce an
                // absurd count — 999 tabs is not a request, it is a stuck key.
                counts: voiceterm_egui::count_prefix::CountPrefix::new(&['b', 'B'], 999),
                compact_defaults_applied: false,
                ingest: Default::default(),
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
                        // The third door to the web settings, after the
                        // palette and voice. Already HTML rather than
                        // markdown, so it goes in directly — `doc()`
                        // converts, and converting hand-written HTML back
                        // and forth would only lose it.
                        voiceterm_egui::manpage_view::Doc {
                            name: "web".into(),
                            title: "web rendering".into(),
                            html: voiceterm_egui::web_prefs::manual_html().to_string(),
                        },
                    ]);
                    v
                },
                page_demo_form: uikit_web::doc::FormState::default(),
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
                session: voiceterm_egui::pane_host::PaneSession::new(),
                pane_sessions: std::collections::HashMap::new(),
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
                // Fixed up from the model immediately after construction
                // — see below. Hardcoding 0 here was wrong: the comment
                // said "TermWorkspace's first pane is id 0", which is
                // true of TermWorkspace and false of the shell::Workspace
                // wrapper this app actually uses, whose first pane is 1.
                pane_focus: 0,

                theme: boot_theme,
                theme_menu: voiceterm_egui::slide_menu::SlideMenu::new("voiceterm-theme-menu").columns(3).hold(0.35).item_size(64.0),
                summary_menu: voiceterm_egui::slide_menu::SlideMenu::new("voiceterm-summary-menu").columns(2),
                blocks,
                line_buf: Default::default(),
                tabbar_scroll_to: None,
                toasts: uikit_egui::toast::Toasts::new(),
                // 80x24 is a placeholder: the first frame calls
                // `resize` with the real grid size, and starting at the
                // conventional default means a shell that prints a banner
                // before then wraps sensibly rather than at zero columns.
                // NO SHELL HERE. `focused_needs_shell` owns shell
                // creation for the focused pane and runs on the first
                // frame; spawning one here too produced TWO — observed
                // as two `/bin/sh -l` children of the app on a real run.
                // Only `self.pty` is ever written to, so the second sat
                // idle holding a pty and a reader thread for the life of
                // the process.
                //
                // One rule, and it is the one with the failure memory
                // (`pty_spawn_failed`), so a machine with no usable shell
                // is asked once rather than once per frame.
                pty: None,
                pty_ended_noted: false,
                pane_ptys: std::collections::HashMap::new(),
                pty_spawn_failed: std::collections::HashSet::new(),
                launcher: uikit_egui::quick_launcher::LauncherState::default(),
                a11y: uikit_egui::a11y::A11yScene::new(),
                mail: Default::default(),
                browser: Default::default(),
                web_panes: Default::default(),
                graph: Default::default(),
                archive: Default::default(),
                mail_messages: Vec::new(),
                mail_all: Vec::new(),
                mail_login: Default::default(),
                mail_signin: Default::default(),
                mail_proton: Vec::new(),
                mail_totp_code: String::new(),
                mail_login_open: false,
                mail_scanned_blocks: 0,
                mail_from_disk: Vec::new(),
                mail_bodies_disk: std::collections::HashMap::new(),
                mail_bodies_terminal: std::collections::HashMap::new(),
                mail_store_path: None,
                mail_open: None,
                audit: Default::default(),
                audit_lines: Vec::new(),
                password_mode: false,
                palette_flow: Default::default(),
                ctrl_p_gesture: Default::default(),
                voice_popup: Default::default(),
                mic_indicator: Default::default(),
                acoustic: Default::default(),
                acoustic_distance: 0.20,
                acoustic_confirms: 2,
                acoustic_same_speaker: true,
                // has_active_model = false: this build ships no .gguf, and
                // claiming one would send the wizard to the wrong phase.
                wizard: Some(voiceterm_egui::setup_wizard::SetupWizard::new(false)),
                chat: Default::default(),
                glow: Default::default(),
                pending_confirm: None, zk_share: None,
                identity: voiceterm_egui::identity::load_or_create("voiceterm"),
                notebook_cells: Default::default(),
                session_name: String::new(),
                session_name_at: usize::MAX,
                session_cluster: None,
                session_proposal: None,
                fs_search: Default::default(),
                cmd_sheet: Default::default(),
                last_memlog_s: 0.0,
                peer: None,
                peer_capture: None,
                // VOICETERM_DEMO also seeds the note deck. The zettelkasten is
                // empty until someone captures something, which is correct —
                // and means the notes panes, the graph, the backlinks and the
                // QR share have nothing to draw on a fresh launch, so none of
                // them could be looked at without first typing a page of text
                // into the capture box by hand.
                deck: {
                    let mut d: voiceterm_egui::cards_view::Deck = Default::default();
                    if demo {
                        let src = voiceterm_egui::cards_view::CardSource {
                            url: "demo://zettelkasten".into(),
                            paragraph_id: String::new(),
                        };
                        d.set_cards(voiceterm_egui::cards_view::note_cards::split_into_cards(DEMO_NOTES, &src));
                    }
                    d
                },
                captured: if demo { DEMO_NOTES.to_string() } else { String::new() },
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
            // Open by default: a control the user has not met yet must not
            // start hidden, or the tabs look lost rather than collapsed.
            tabbar_open: true,
            tabbar_width: 168.0, // main.qml:4015 termSidebarWidth
                // One page = one tab. 320 px matches the snapclone convention
                // the layer was written against, so the detent lands where a
                // thumb expects it on every app in the fleet.
                tab_swipe: uikit_egui::GestureLayer::horizontal_page(320.0),
                walk: Default::default(),
                surfaces: Surfaces::default(),
                palette_query: String::new(),
                palette_sel: 0,
                workspaces_filter: String::new(),
                bar_note: String::new(),
                web_prefs: voiceterm_egui::web_prefs::load(),
                graph_source: String::new(),
                graph_from_notes: true,
            };
            // Set once, here, rather than left for the assistant's first
            // reply to explain (the previous state — see b6e55cf5's commit
            // message): a limitation worth knowing before typing a
            // message, not discovered only after sending one. `ChatPanel`
            // has no way to know its own backend's state; this is the
            // caller telling it, same as `provider`/`busy`.
            app.chat.backend_status = Some(
                "no local model attached in this build — replies are a placeholder, not a real response".to_string());
            // Take the focused pane id FROM THE MODEL rather than
            // hardcoding it.
            //
            // The old literal `0` carried the comment "TermWorkspace's
            // first pane is id 0", which is true of TermWorkspace and
            // says nothing about the shell::Workspace wrapper this app
            // uses. It happens to agree today — a probe on a real run
            // printed `focus=0 panes=[0, 1]` — so this is not a bug fix,
            // it is removing a coincidence that a change to the default
            // layout would quietly turn into one.
            //
            // Recorded because I first read the two shells that run at
            // startup as a double-spawn and "fixed" it. They are
            // correct: the default workspace has TWO panes, and one
            // shell each is the intended behaviour.
            app.pane_focus = app.workspace.focused_pane();
            // ONE SCAN AT STARTUP, so the mail views have something the
            // moment they are opened rather than only after the person
            // finds the rescan button. Not repeated: a Maildir scan is
            // thousands of stats, and mail that arrives later is what the
            // button is for.
            app.rescan_mail_store();
            Ok(Box::new(app))
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


#[cfg(test)]
mod doc_markdown_tests {
    use super::*;

    #[test]
    fn plain_text_is_just_escaped() {
        assert_eq!(inline_md("hello world"), "hello world");
        assert_eq!(inline_md("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }

    #[test]
    fn bold_renders_as_a_tag_not_literal_asterisks() {
        assert_eq!(inline_md("**bold**"), "<b>bold</b>");
        assert_eq!(inline_md("say **this** now"), "say <b>this</b> now");
    }

    #[test]
    fn inline_code_renders_as_a_tag() {
        assert_eq!(inline_md("run `cargo test`"), "run <code>cargo test</code>");
    }

    #[test]
    fn a_link_renders_as_an_anchor_not_literal_brackets() {
        assert_eq!(inline_md("[vision.md](vision.md)"),
                   "<a href=\"vision.md\">vision.md</a>");
    }

    /// The exact case observed live: a bold link, which is how README.md
    /// actually writes its file list. Regression pin for the bug this
    /// function replaced `escape(t)` to fix.
    #[test]
    fn a_bold_link_is_not_left_as_literal_markdown_syntax() {
        assert_eq!(
            inline_md("**[vision.md](vision.md)** — long-term goals"),
            "<b><a href=\"vision.md\">vision.md</a></b> — long-term goals");
    }

    #[test]
    fn an_unterminated_delimiter_falls_back_to_literal_text() {
        // No closing `**`: must not consume or corrupt the rest of the line.
        assert_eq!(inline_md("this **never closes"), "this **never closes");
        assert_eq!(inline_md("a `dangling code span"), "a `dangling code span");
    }

    #[test]
    fn markup_characters_inside_a_span_are_still_escaped() {
        // The span content goes through `escape` too, not straight into the
        // tag — a link label containing `<` must not inject a stray tag.
        assert_eq!(inline_md("[a < b](x)"), "<a href=\"x\">a &lt; b</a>");
    }

    #[test]
    fn a_code_span_suppresses_nested_markdown_but_bold_does_not() {
        // Markdown convention: code spans have the highest precedence and
        // switch off formatting inside them; bold and links do not, which
        // is exactly what the bold-link case above depends on.
        assert_eq!(inline_md("`**not bold**`"), "<code>**not bold**</code>");
    }

    #[test]
    fn a_full_paragraph_line_through_doc_matches_inline_md() {
        let d = doc("t", "T", "**[vision.md](vision.md)** — goals");
        assert!(d.html.contains("<b><a href=\"vision.md\">vision.md</a></b>"),
                "doc() must route paragraph lines through inline_md: {}", d.html);
    }

    #[test]
    fn headers_and_fences_are_unaffected() {
        let d = doc("t", "T", "# Title\n\n```\nlet x = 1;\n```\n\n**bold** after");
        assert!(d.html.contains("<h1>Title</h1>"));
        assert!(d.html.contains("<pre>let x = 1;\n</pre>"));
        assert!(d.html.contains("<p><b>bold</b> after</p>"));
    }
    /// The bottom bar's height is a PORTED value, so it is checked against the
    /// source it was ported from rather than against itself.
    ///
    /// Reported as "the bottom bar is way too thick unlike the -qt version".
    /// The panel set no height at all and took egui's default button height
    /// plus panel margins; the QML gives it a fixed slot.
    #[test]
    fn the_bottom_bar_matches_the_qml_slot_height() {
        let qml = include_str!("../../../qml/main.qml");
        // main.qml: `Layout.preferredHeight: (urlBarVisible && ...) ? 50 : 0`
        // on topBar, which bottomMountSlot's height tracks.
        // The FULL topBar line, not just "? 50 : 0" -- that fragment appears
        // ten times in main.qml and would pass while the bar's own height
        // changed underneath it.
        assert!(
            qml.contains(
                "Layout.preferredHeight: (urlBarVisible && tabBar.currentIndex !== 1) ? 50 : 0"),
            "topBar's height line changed; BOTTOM_BAR_H needs re-deriving");
        assert_eq!(BOTTOM_BAR_H, 50.0);
    }
}

/// The share's screens over the pure `zk_share::Share`: the picker with the paintbrush,
/// the two gates as the kit's confirm dialogs, the kit's huge view for the codes.
struct ZkShareUi {
    share: voiceterm_egui::zk_share::Share,
    /// The gate on screen, if one is.
    dialog: Option<voiceterm_egui::confirm_dialog::ConfirmDialog>,
    /// The encoded frames, built once gate two is passed.
    frames: Vec<uikit_egui::qrbig::Matrix>,
    /// The huge view's open state (the identicon's, for the guard and the fade).
    shown: Option<uikit_egui::qr::Expanded>,
    /// Why Share was refused, shown on the picker.
    note: String,
}

impl ZkShareUi {
    /// Draw whichever screen the stage calls for. Returns true when the share is over.
    fn show(&mut self, ctx: &egui::Context, cards: &[voiceterm_egui::cards_view::Card], c: &PaletteColors) -> bool {
        use voiceterm_egui::confirm_dialog::Outcome;
        use voiceterm_egui::zk_share::{Brush, Stage};
        match self.share.stage {
            Stage::Pick => {
                let mut close = false;
                egui::Window::new("Share notes as a QR code")
                    .collapsible(false)
                    .resizable(true)
                    .default_width(420.0)
                    .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.label(egui::RichText::new("Tick the notes to share, or paint them: brush down, drag over rows.").small().color(c.text_weak));
                        ui.horizontal(|ui| {
                            let (pub_on, priv_on) = (self.share.brush == Brush::Public, self.share.brush == Brush::Private);
                            if ui.selectable_label(pub_on, "\u{1f58c} paint public").clicked() {
                                self.share.brush = if pub_on { Brush::Off } else { Brush::Public };
                            }
                            if ui.selectable_label(priv_on, "\u{1f58c} paint private").clicked() {
                                self.share.brush = if priv_on { Brush::Off } else { Brush::Private };
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(egui::RichText::new(format!("{} public · {:.1} KB · {} code{}",
                                    self.share.public_count(), self.share.public_bytes() as f32 / 1024.0,
                                    self.share.frame_count(), if self.share.frame_count() == 1 { "" } else { "s" })).small().color(c.text_weak));
                            });
                        });
                        ui.separator();
                        let brushing = self.share.brush != Brush::Off && ui.input(|i| i.pointer.primary_down());
                        let hover = ui.input(|i| i.pointer.hover_pos());
                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            let rows = self.share.rows.clone();
                            for r in &rows {
                                let resp = ui.horizontal(|ui| {
                                    if r.is_heading {
                                        let mut on = r.public;
                                        if ui.checkbox(&mut on, "").changed() {
                                            self.share.set_section(r.index, on);
                                        }
                                        ui.label(egui::RichText::new(&r.title).strong());
                                    } else {
                                        ui.add_space(18.0);
                                        let mut on = r.public;
                                        if ui.checkbox(&mut on, "").changed() {
                                            self.share.toggle(r.index);
                                        }
                                        ui.label(&r.title);
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(egui::RichText::new(if r.public { "public" } else { "private" }).small()
                                            .color(if r.public { c.accent } else { c.text_weak }));
                                    });
                                }).response;
                                // THE BRUSH: with it down, the row under the pointer takes its mark
                                if brushing {
                                    if let Some(p) = hover {
                                        if resp.rect.contains(p) {
                                            self.share.brush_over(r.index);
                                        }
                                    }
                                }
                                if r.public {
                                    ui.painter().rect_stroke(resp.rect.expand(1.0), 4.0, egui::Stroke::new(1.0, uikit_egui::glass::tint(c.accent, 90)), egui::StrokeKind::Outside);
                                }
                            }
                        });
                        if !self.note.is_empty() {
                            ui.colored_label(c.warning, &self.note);
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            if uikit_egui::glass::primary_button(ui, "Share\u{2026}", c.accent, c.bg_base).clicked() {
                                match self.share.go() {
                                    Ok(()) => self.note.clear(),
                                    Err(e) => self.note = e,
                                }
                            }
                            if uikit_egui::glass::secondary_button(ui, "Cancel", c.bg_card, c.border).clicked() {
                                close = true;
                            }
                        });
                    });
                if brushing_repaint(ctx, self.share.brush) {
                    ctx.request_repaint();
                }
                close
            }
            Stage::ConfirmWhat | Stage::ConfirmWho => {
                let what = self.share.stage == Stage::ConfirmWhat;
                let share = &self.share;
                let dlg = self.dialog.get_or_insert_with(|| share.gate().expect("a gate stage has a gate"));
                match dlg.show(ctx) {
                    Outcome::Confirmed => {
                        self.dialog = None;
                        if what {
                            self.share.what_confirmed();
                        } else {
                            self.share.who_confirmed();
                            let payload = self.share.payload(cards);
                            self.frames = uikit_egui::qrbig::frames(&payload).iter().filter_map(|f| uikit_egui::qrbig::encode(f)).collect();
                            self.shown = Some(uikit_egui::qr::Expanded::open(ctx.input(|i| i.time)));
                        }
                    }
                    Outcome::Cancelled | Outcome::Discarded => {
                        self.dialog = None;
                        self.share.back();
                    }
                    Outcome::Pending => {}
                }
                false
            }
            Stage::Showing => {
                let caption = format!("{} notes · anyone who scans can read them", self.share.public_count());
                uikit_egui::qrbig::huge_ui(ctx, &mut self.shown, &self.frames, &caption, c)
            }
        }
    }
}

/// A brush that is down wants a frame per pointer move.
fn brushing_repaint(ctx: &egui::Context, brush: voiceterm_egui::zk_share::Brush) -> bool {
    brush != voiceterm_egui::zk_share::Brush::Off && ctx.input(|i| i.pointer.primary_down())
}
