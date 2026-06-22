//! code_tab.rs — code blocks viewer for Cargo projects
//!
//! Layout:
//!   ┌─ File tree (left) ──────┬─ Code view (center) ──┬─ AI panel (right) ─┐
//!   │ Cargo.toml        [cfg] │ fn main() {           │ [sticky prompt bar] │
//!   │ src/main.rs       [bin] │   ...                 │                     │
//!   │ src/ui/mod.rs     [ui]  │ }                     │ [response area]     │
//!   │ src/providers.rs  [lib] │                       │                     │
//!   └─────────────────────────┴───────────────────────┴─────────────────────┘
//!   ┌─ Bottom bar ────────────────────────────────────────────────────────────┐
//!   │ [Insert next block ▸]  [model dropdown]  [prompt selector]  [▶ Ask]    │
//!   └─────────────────────────────────────────────────────────────────────────┘

use gtk4::prelude::*;
use gtk4::glib;
use gtk4::{
    Box as GtkBox, Button, Label, ListBox, ListBoxRow,
    Orientation, PolicyType, ScrolledWindow,
    SelectionMode, TextView, TextBuffer, WrapMode,
};

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crossbeam_channel::unbounded;

// ── File classification ───────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum FileKind {
    Manifest,   // Cargo.toml, Cargo.lock
    Binary,     // src/main.rs, src/bin/
    Library,    // lib.rs
    Ui,         // src/ui/
    Test,       // tests/, #[cfg(test)]
    Config,     // build.rs, .cargo/
    Generated,  // target/
    Source,     // everything else .rs
    Other,
}

impl FileKind {
    fn classify(path: &PathBuf) -> Self {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        let path_str = path.to_string_lossy().to_lowercase();

        if name == "cargo.toml" || name == "cargo.lock" { return FileKind::Manifest; }
        if name == "build.rs" || path_str.contains(".cargo") { return FileKind::Config; }
        if path_str.contains("/target/") { return FileKind::Generated; }
        if path_str.contains("/tests/") || path_str.contains("_test") { return FileKind::Test; }
        if path_str.contains("/ui/") || path_str.contains("/widgets/") { return FileKind::Ui; }
        if name == "main.rs" || path_str.contains("/bin/") { return FileKind::Binary; }
        if name == "lib.rs" { return FileKind::Library; }
        if name.ends_with(".rs") { return FileKind::Source; }
        FileKind::Other
    }

    fn badge(&self) -> &'static str {
        match self {
            FileKind::Manifest  => "cfg",
            FileKind::Binary    => "bin",
            FileKind::Library   => "lib",
            FileKind::Ui        => "ui",
            FileKind::Test      => "test",
            FileKind::Config    => "cfg",
            FileKind::Generated => "gen",
            FileKind::Source    => "src",
            FileKind::Other     => "…",
        }
    }

    fn badge_css(&self) -> &'static str {
        match self {
            FileKind::Manifest  => "code-badge-cfg",
            FileKind::Binary    => "code-badge-bin",
            FileKind::Library   => "code-badge-lib",
            FileKind::Ui        => "code-badge-ui",
            FileKind::Test      => "code-badge-test",
            FileKind::Config    => "code-badge-cfg",
            FileKind::Generated => "code-badge-gen",
            FileKind::Source    => "code-badge-src",
            FileKind::Other     => "code-badge-other",
        }
    }

    fn priority(&self) -> u8 {
        match self {
            FileKind::Binary   => 0,
            FileKind::Library  => 1,
            FileKind::Manifest => 2,
            FileKind::Ui       => 3,
            FileKind::Source   => 4,
            FileKind::Test     => 5,
            FileKind::Config   => 6,
            FileKind::Generated => 8,
            FileKind::Other    => 9,
        }
    }
}

// ── Code block ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct CodeBlock {
    path:    PathBuf,
    start:   usize,   // line number
    end:     usize,
    kind:    FileKind,
    summary: String,  // e.g. "fn build_window" or "impl ChatTab"
}

/// Split a file into logical blocks at fn/impl/struct/enum/mod boundaries
fn extract_blocks(path: &PathBuf) -> Vec<CodeBlock> {
    let Ok(content) = std::fs::read_to_string(path) else { return vec![]; };
    let kind = FileKind::classify(path);
    let mut blocks = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    let mut block_start = 0;
    let mut current_summary = String::new();

    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        let is_boundary = t.starts_with("pub fn ")
            || t.starts_with("fn ")
            || t.starts_with("pub struct ")
            || t.starts_with("struct ")
            || t.starts_with("pub enum ")
            || t.starts_with("enum ")
            || t.starts_with("impl ")
            || t.starts_with("pub mod ")
            || t.starts_with("mod ")
            || t.starts_with("pub trait ")
            || t.starts_with("trait ")
            || t.starts_with("#[cfg(")
            || (t.starts_with("//") && t.len() > 40 && t.contains("──"));

        if is_boundary && i > block_start {
            if !current_summary.is_empty() {
                blocks.push(CodeBlock {
                    path: path.clone(),
                    start: block_start,
                    end: i,
                    kind: kind.clone(),
                    summary: current_summary.clone(),
                });
            }
            block_start = i;
            current_summary = t.to_string().chars().take(60).collect();
        }
    }
    // Last block
    if block_start < lines.len() {
        blocks.push(CodeBlock {
            path: path.clone(),
            start: block_start,
            end: lines.len(),
            kind,
            summary: if current_summary.is_empty() {
                path.file_name().unwrap_or_default().to_string_lossy().into()
            } else {
                current_summary
            },
        });
    }

    blocks
}

/// Find all relevant source files in the current directory tree
fn scan_project_files() -> Vec<(PathBuf, FileKind)> {
    let mut files = Vec::new();
    scan_dir(std::path::Path::new("."), &mut files, 0);
    files.sort_by(|a, b| a.1.priority().cmp(&b.1.priority()).then(a.0.cmp(&b.0)));
    files
}

fn scan_dir(dir: &std::path::Path, out: &mut Vec<(PathBuf, FileKind)>, depth: u32) {
    if depth > 6 { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        // Skip hidden, target, node_modules
        if name.starts_with('.') || name == "target" || name == "node_modules" { continue; }
        if path.is_dir() {
            scan_dir(&path, out, depth + 1);
        } else {
            let kind = FileKind::classify(&path);
            if kind != FileKind::Generated && kind != FileKind::Other {
                out.push((path, kind));
            } else if matches!(path.extension().and_then(|e| e.to_str()), Some("rs"|"toml"|"json"|"md")) {
                out.push((path, kind));
            }
        }
    }
}

// ── Prompts ───────────────────────────────────────────────────────────────────

const CODE_PROMPTS: &[(&str, &str, &str)] = &[
    ("predict",  "Predict next block",
     "Based on the code shown, predict the most likely next function or impl block. \
      Output only the code block, no explanation."),
    ("explain",  "Explain",
     "Explain what this code does in 2-3 sentences. Be concise and precise."),
    ("improve",  "Suggest improvements",
     "Suggest 2-3 concrete improvements to this code block. Be specific."),
    ("docstring","Add doc comment",
     "Write a Rust doc comment (///) for this function/struct. Output only the doc comment lines."),
    ("tests",   "Generate tests",
     "Write unit tests for this code block. Use #[cfg(test)] and #[test]."),
    ("refactor","Refactor",
     "Refactor this code to be more idiomatic Rust. Show only the changed code."),
];

// ── CodeTab ───────────────────────────────────────────────────────────────────

pub struct CodeTab {
    pub root: GtkBox,
}

impl CodeTab {
    pub fn new() -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(true);

        // ── State ──────────────────────────────────────────────────────────
        let files: Rc<RefCell<Vec<(PathBuf, FileKind)>>> = Rc::new(RefCell::new(Vec::new()));
        let selected_path: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
        let ollama_cancel = std::sync::Arc::new(
            std::sync::atomic::AtomicBool::new(false)
        );
        let current_prompt_idx: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));

        // ── Top toolbar ────────────────────────────────────────────────────
        let toolbar = gtk4::ActionBar::new();
        toolbar.add_css_class("code-toolbar");

        let scan_btn = Button::from_icon_name("folder-open-symbolic");
        scan_btn.add_css_class("flat");
        scan_btn.set_tooltip_text(Some("Scan project files"));

        let path_lbl = Label::new(Some("No project loaded"));
        path_lbl.add_css_class("dim-label");
        path_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);

        let file_count_lbl = Label::new(Some(""));
        file_count_lbl.add_css_class("dim-label");

        toolbar.pack_start(&scan_btn);
        toolbar.set_center_widget(Some(&path_lbl));
        toolbar.pack_end(&file_count_lbl);
        root.append(&toolbar);

        // ── Main area: file list | code view | AI panel ────────────────────
        let main_box = GtkBox::new(Orientation::Horizontal, 0);
        main_box.set_vexpand(true);

        // ── File list (left) ───────────────────────────────────────────────
        let file_list = ListBox::new();
        file_list.set_selection_mode(SelectionMode::Single);
        file_list.add_css_class("code-file-list");

        let file_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .min_content_width(160)
            .max_content_width(240)
            .vexpand(true)
            .child(&file_list)
            .build();

        // ── Code view (center) ─────────────────────────────────────────────
        let code_buf = TextBuffer::new(None);
        let code_view = TextView::builder()
            .buffer(&code_buf)
            .editable(false)
            .monospace(true)
            .wrap_mode(WrapMode::None)
            .hexpand(true)
            .vexpand(true)
            .build();
        code_view.add_css_class("code-source-view");
        // Left margin for selected-line indicator
        code_view.set_left_margin(6);
        code_view.set_right_margin(6);
        code_view.set_top_margin(4);

        let code_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .hexpand(true).vexpand(true)
            .child(&code_view)
            .build();

        // ── AI panel (right) ───────────────────────────────────────────────
        let ai_panel = GtkBox::new(Orientation::Vertical, 0);
        ai_panel.set_vexpand(true);
        ai_panel.add_css_class("code-ai-panel");

        // Sticky prompt selector bar
        let prompt_bar = gtk4::ActionBar::new();
        prompt_bar.add_css_class("code-prompt-bar");

        let prompt_btn = Button::new();
        let prompt_lbl_widget = Label::new(Some(CODE_PROMPTS[0].1));
        prompt_lbl_widget.add_css_class("code-prompt-name");
        prompt_btn.set_child(Some(&prompt_lbl_widget));
        prompt_btn.add_css_class("flat");
        prompt_btn.add_css_class("code-prompt-btn");
        prompt_btn.set_tooltip_text(Some("Click to cycle prompts"));

        // Model dropdown for code tab
        let model_store = crate::ui::components::ProviderStore::new();
        let model_drop  = crate::ui::components::build_model_dropdown(&model_store);
        model_drop.add_css_class("ollama-model-drop");
        model_store.string_list.append("scanning…");

        let ask_btn = Button::from_icon_name("system-run-symbolic");
        ask_btn.add_css_class("suggested-action");
        ask_btn.set_tooltip_text(Some("Ask AI about selected block"));

        let cancel_btn = Button::from_icon_name("process-stop-symbolic");
        cancel_btn.add_css_class("flat");
        cancel_btn.set_visible(false);

        let render_toggle = gtk4::ToggleButton::new();
        render_toggle.set_icon_name("text-html-symbolic");
        render_toggle.add_css_class("flat");
        render_toggle.set_tooltip_text(Some("Toggle markdown rendering of AI response"));
        render_toggle.set_sensitive(false);

        // Shared state for the once-wired toggle handler
        let md_text_store: std::rc::Rc<std::cell::RefCell<String>> =
            std::rc::Rc::new(std::cell::RefCell::new(String::new()));

        prompt_bar.pack_start(&prompt_btn);
        prompt_bar.pack_end(&ask_btn);
        prompt_bar.pack_end(&cancel_btn);
        prompt_bar.pack_end(&render_toggle);
        prompt_bar.pack_end(&model_drop);

        // AI response area
        let ai_buf = TextBuffer::new(None);
        let ai_view = TextView::builder()
            .buffer(&ai_buf)
            .editable(false)
            .wrap_mode(WrapMode::WordChar)
            .hexpand(true)
            .vexpand(true)
            .build();
        ai_view.add_css_class("code-ai-response");
        ai_view.set_left_margin(8);
        ai_view.set_right_margin(8);
        ai_view.set_top_margin(8);

        let ai_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .hexpand(true).vexpand(true)
            .child(&ai_view)
            .build();

        let spinner = gtk4::Spinner::new();
        spinner.set_visible(false);
        spinner.set_margin_top(4);

        // Insert button (sticky — always at bottom of AI panel)
        let insert_btn = Button::with_label("↪ Insert block");
        insert_btn.add_css_class("code-insert-btn");
        insert_btn.set_sensitive(false);
        insert_btn.set_tooltip_text(Some("Insert AI response into selected file at cursor position"));
        insert_btn.set_margin_top(6);
        insert_btn.set_margin_bottom(6);
        insert_btn.set_margin_start(8);
        insert_btn.set_margin_end(8);

        // Content container: swapped between raw TextView and markdown render
        let ai_content_box = GtkBox::new(Orientation::Vertical, 0);
        ai_content_box.set_hexpand(true);
        ai_content_box.set_vexpand(true);
        ai_content_box.append(&ai_scroll);

        ai_panel.append(&prompt_bar);
        ai_panel.append(&gtk4::Separator::new(Orientation::Horizontal));
        ai_panel.append(&spinner);
        ai_panel.append(&ai_content_box);
        ai_panel.append(&gtk4::Separator::new(Orientation::Horizontal));
        ai_panel.append(&insert_btn);

        // Wire render toggle ONCE — reads from md_text_store set by each done block
        {
            let acb = ai_content_box.clone();
            let ais = ai_scroll.clone();
            let mts = std::rc::Rc::clone(&md_text_store);
            render_toggle.connect_toggled(move |t| {
                let ft = mts.borrow().clone();
                if ft.is_empty() { return; }
                if let Some(c) = acb.first_child() { acb.remove(&c); }
                if t.is_active() {
                    let r = crate::ui::markdown::render(&ft);
                    let s = gtk4::ScrolledWindow::builder()
                        .hscrollbar_policy(PolicyType::Never)
                        .vscrollbar_policy(PolicyType::Automatic)
                        .hexpand(true).vexpand(true).child(&r).build();
                    acb.append(&s);
                } else {
                    acb.append(&ais);
                }
            });
        }

        // Assemble main area
        main_box.append(&file_scroll);
        main_box.append(&gtk4::Separator::new(Orientation::Vertical));
        main_box.append(&code_scroll);
        main_box.append(&gtk4::Separator::new(Orientation::Vertical));
        main_box.append(&ai_panel);

        root.append(&gtk4::Separator::new(Orientation::Horizontal));
        root.append(&main_box);

        // ── Wire: scan button ──────────────────────────────────────────────
        {
            let files2 = Rc::clone(&files);
            let fl     = file_list.clone();
            let pl     = path_lbl.clone();
            let fcl    = file_count_lbl.clone();
            let ms     = model_store.clone();
            let md     = model_drop.clone();

            let do_scan = Rc::new(move || {
                // Scan on rayon, results via channel
                let (tx, rx) = unbounded::<Vec<(PathBuf, FileKind)>>();
                rayon::spawn(move || {
                    let _ = tx.send(scan_project_files());
                });

                let files3 = Rc::clone(&files2);
                let fl2 = fl.clone();
                let pl2 = pl.clone();
                let fcl2 = fcl.clone();
                glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                    match rx.try_recv() {
                        Ok(scanned) => {
                            let count = scanned.len();
                            *files3.borrow_mut() = scanned.clone();
                            pl2.set_text(&std::env::current_dir()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default());
                            fcl2.set_text(&format!("{count} files"));

                            // Rebuild file list
                            while let Some(c) = fl2.first_child() { fl2.remove(&c); }
                            for (path, kind) in &scanned {
                                let row = build_file_row(path, kind);
                                fl2.append(&row);
                            }
                            glib::ControlFlow::Break
                        }
                        Err(crossbeam_channel::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(_) => glib::ControlFlow::Break,
                    }
                });

                // Also load models
                {
                    let ms2 = ms.clone();
                    let md2 = md.clone();
                    let (mtx, mrx) = unbounded::<Vec<crate::model_selector::ModelInfo>>();
                    std::thread::spawn(move || {
                        if let Ok(reg) = crate::providers::ProviderRegistry::open() {
                            let mut all = crate::providers::probe_host(&reg, crate::ui::ollama::DEFAULT_HOST);
                            for kh in reg.known_hosts() {
                                if kh.is_recent() { all.extend(crate::providers::probe_host(&reg, &kh.url)); }
                            }
                            let _ = mtx.send(all);
                        }
                    });
                    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                        if let Ok(models) = mrx.try_recv() {
                            crate::ui::components::populate_model_store(&ms2.string_list, &md2, &models);
                            return glib::ControlFlow::Break;
                        }
                        glib::ControlFlow::Continue
                    });
                }
            });

            let do_scan2 = Rc::clone(&do_scan);
            scan_btn.connect_clicked(move |_| { do_scan2(); });

            // Auto-scan on tab creation
            do_scan();
        }

        // ── Wire: file selection → load code view ──────────────────────────
        {
            let files2  = Rc::clone(&files);
            let sel_path = Rc::clone(&selected_path);
            let cb2      = code_buf.clone();
            let ai_buf2  = ai_buf.clone();
            let ins_btn  = insert_btn.clone();

            file_list.connect_row_selected(move |_, row| {
                let Some(r) = row else { return };
                let idx = r.index() as usize;
                let f = files2.borrow();
                let Some((path, _)) = f.get(idx) else { return };

                *sel_path.borrow_mut() = Some(path.clone());

                // Load file content
                let content = std::fs::read_to_string(path).unwrap_or_default();
                cb2.set_text(&content);
                ai_buf2.set_text("");
                ins_btn.set_sensitive(false);
            });
        }

        // ── Wire: prompt cycle button ──────────────────────────────────────
        {
            let idx2 = Rc::clone(&current_prompt_idx);
            let plw  = prompt_lbl_widget.clone();
            prompt_btn.connect_clicked(move |_| {
                let mut i = idx2.borrow_mut();
                *i = (*i + 1) % CODE_PROMPTS.len();
                plw.set_text(CODE_PROMPTS[*i].1);
            });
        }

        // ── Wire: ask button ───────────────────────────────────────────────
        {
            let sel_path2  = Rc::clone(&selected_path);
            let idx2       = Rc::clone(&current_prompt_idx);
            let ai_buf2    = ai_buf.clone();
            let code_buf2  = code_buf.clone();
            let model_drop2 = model_drop.clone();
            let model_store2 = model_store.clone();
            let ask_btn2   = ask_btn.clone();
            let cancel_btn2 = cancel_btn.clone();
            let spinner2   = spinner.clone();
            let ins_btn2   = insert_btn.clone();
            let cancel_ref = std::sync::Arc::clone(&ollama_cancel);

            ask_btn.connect_clicked(move |_| {
                // Get selected file content
                let s = code_buf2.start_iter();
                let e = code_buf2.end_iter();
                let code = code_buf2.text(&s, &e, false).to_string();
                if code.trim().is_empty() { return; }

                let prompt_idx = *idx2.borrow();
                let (_id, _label, prompt_template) = CODE_PROMPTS[prompt_idx];

                // Build prompt
                let path_str = sel_path2.borrow()
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let full_prompt = format!(
                    "{}\n\nFile: {path_str}\n\n```rust\n{code}\n```",
                    prompt_template
                );

                // Get host/model
                let host = crate::ui::ollama::DEFAULT_HOST.to_string();
                let model = model_store2.selected_entry(&model_drop2)
                    .map(|e| e.model)
                    .unwrap_or_default();
                let model = if model.is_empty() || model == "scanning…" {
                    crate::ui::ollama::resolve_model(&host)
                } else { model };

                // Reset cancel flag, show spinner
                cancel_ref.store(false, std::sync::atomic::Ordering::Relaxed);
                ai_buf2.set_text("…");
                render_toggle.set_sensitive(false);
                render_toggle.set_active(false);
                if let Some(c) = ai_content_box.first_child() { ai_content_box.remove(&c); }
                ai_content_box.append(&ai_scroll);
                spinner2.set_visible(true);
                spinner2.start();
                ask_btn2.set_sensitive(false);
                cancel_btn2.set_visible(true);
                ins_btn2.set_sensitive(false);

                let (tx, rx) = unbounded::<crate::ui::ollama::StreamChunk>();
                let cancel_arc = std::sync::Arc::clone(&cancel_ref);
                let lc = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

                std::thread::spawn(move || {
                    crate::ui::ollama::prompt_streaming(
                        &full_prompt, &host, &model, &tx,
                        &cancel_arc, &lc,
                    );
                });

                // Drain tokens
                let render_toggle2  = render_toggle.clone();
                let ai_content_box2 = ai_content_box.clone();
                let ai_scroll2      = ai_scroll.clone();
                let ab  = ai_buf2.clone();
                let sp  = spinner2.clone();
                let ab2 = ask_btn2.clone();
                let cb3 = cancel_btn2.clone();
                let ib  = ins_btn2.clone();
                let rt2  = render_toggle2.clone();
                let acb2 = ai_content_box2.clone();
                let ais2 = ai_scroll2.clone();
                let mts2 = std::rc::Rc::clone(&md_text_store);
                let mut first = true;

                glib::timeout_add_local(std::time::Duration::from_millis(30), move || {
                    loop {
                        match rx.try_recv() {
                            Ok(crate::ui::ollama::StreamChunk::Token(t)) => {
                                if first { ab.set_text(""); first = false; }
                                let end = ab.end_iter();
                                ab.insert(&mut ab.end_iter().clone(), &t);
                                let _ = end; // suppress warning
                            }
                            Ok(crate::ui::ollama::StreamChunk::Done { .. }) |
                            Ok(crate::ui::ollama::StreamChunk::Cancelled) |
                            Ok(crate::ui::ollama::StreamChunk::Error(_)) => {
                                sp.stop(); sp.set_visible(false);
                                ab2.set_sensitive(true);
                                cb3.set_visible(false);
                                ib.set_sensitive(true);
                                // Enable markdown toggle if response has markdown
                                let s2 = ab.start_iter(); let e2 = ab.end_iter();
                                let final_text = ab.text(&s2, &e2, false).to_string();
                                if crate::ui::markdown::has_markdown(&final_text) {
                                    rt2.set_sensitive(true);
                                    rt2.set_active(true);
                                    // Swap to rendered view
                                    if let Some(c) = acb2.first_child() { acb2.remove(&c); }
                                    let rendered = crate::ui::markdown::render(&final_text);
                                    let md_scroll = ScrolledWindow::builder()
                                        .hscrollbar_policy(PolicyType::Never)
                                        .vscrollbar_policy(PolicyType::Automatic)
                                        .hexpand(true).vexpand(true)
                                        .child(&rendered).build();
                                    acb2.append(&md_scroll);
                                    // Wire toggle to swap back
                                    // Update shared text store for the once-wired toggle
                                    *mts2.borrow_mut() = final_text.clone();
                                }
                                return glib::ControlFlow::Break;
                            }
                            Err(crossbeam_channel::TryRecvError::Empty) => break,
                            Err(_) => {
                                sp.stop(); sp.set_visible(false);
                                ab2.set_sensitive(true);
                                cb3.set_visible(false);
                                return glib::ControlFlow::Break;
                            }
                        }
                    }
                    glib::ControlFlow::Continue
                });
            });
        }

        // ── Wire: cancel ───────────────────────────────────────────────────
        {
            let cancel_ref = std::sync::Arc::clone(&ollama_cancel);
            cancel_btn.connect_clicked(move |_| {
                cancel_ref.store(true, std::sync::atomic::Ordering::Relaxed);
            });
        }

        // ── Wire: insert block ─────────────────────────────────────────────
        {
            let sel_path2 = Rc::clone(&selected_path);
            let ai_buf2   = ai_buf.clone();
            let code_buf2 = code_buf.clone();
            let ins_btn2  = insert_btn.clone();

            insert_btn.connect_clicked(move |_| {
                let Some(path) = sel_path2.borrow().clone() else { return };
                let s = ai_buf2.start_iter(); let e = ai_buf2.end_iter();
                let response = ai_buf2.text(&s, &e, false).to_string();
                if response.trim().is_empty() { return; }

                // Append AI response to end of file
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&path) {
                    let _ = writeln!(f, "\n\n// AI-generated block\n{response}");
                    // Reload code view
                    let content = std::fs::read_to_string(&path).unwrap_or_default();
                    code_buf2.set_text(&content);
                    ins_btn2.set_sensitive(false);
                }
            });
        }

        CodeTab { root }
    }
}

fn build_file_row(path: &PathBuf, kind: &FileKind) -> ListBoxRow {
    let row_box = GtkBox::new(Orientation::Horizontal, 6);
    row_box.set_margin_start(8); row_box.set_margin_end(8);
    row_box.set_margin_top(5);   row_box.set_margin_bottom(5);

    // Show path relative to cwd
    let display = path.strip_prefix("./").unwrap_or(path).display().to_string();
    let name_lbl = Label::new(Some(&display));
    name_lbl.set_hexpand(true);
    name_lbl.set_halign(gtk4::Align::Start);
    name_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::Start);
    name_lbl.add_css_class("code-file-name");

    let badge = Label::new(Some(kind.badge()));
    badge.add_css_class("code-badge");
    badge.add_css_class(kind.badge_css());

    row_box.append(&name_lbl);
    row_box.append(&badge);

    let row = ListBoxRow::new();
    row.set_child(Some(&row_box));
    row
}
