//! cargo_tab.rs — Rust project diagnostics viewer with cancel support.

use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, Label, Orientation,
    PolicyType, ScrolledWindow, TextBuffer, TextView, WrapMode,
};

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossbeam_channel::unbounded;

// ── public struct ─────────────────────────────────────────────────────────────

pub struct CargoTab {
    pub root:      GtkBox,
    snippets:      Rc<RefCell<Vec<DiagSnippet>>>,
    cargo_cancel:  Arc<AtomicBool>,
    pub ollama_cancel: Arc<AtomicBool>,
    /// When true, warning-level cards are hidden
    hide_warnings: Rc<Cell<bool>>,
}

#[derive(Clone)]
struct DiagSnippet {
    file:         String,
    line:         u32,
    level:        String,
    message:      String,
    code:         Option<String>,
    source_lines: Vec<String>,
}

impl CargoTab {
    pub fn new() -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.add_css_class("cargo-tab");
        let ph = placeholder_label();
        root.append(&ph);
        CargoTab {
            root,
            snippets:      Rc::new(RefCell::new(Vec::new())),
            cargo_cancel:  Arc::new(AtomicBool::new(false)),
            ollama_cancel: Arc::new(AtomicBool::new(false)),
            hide_warnings: Rc::new(Cell::new(false)),
        }
    }

    pub fn is_cargo_project(path: &Path) -> bool {
        find_workspace_root(path).is_some()
    }

    /// Cancel all running operations (cargo check + Ollama calls).
    pub fn cancel_all(&self) {
        self.cargo_cancel.store(true, Ordering::Relaxed);
        self.ollama_cancel.store(true, Ordering::Relaxed);
    }

    pub fn reset_placeholder(&self) {
        self.cancel_all();
        while let Some(c) = self.root.first_child() { self.root.remove(&c); }
        self.snippets.borrow_mut().clear();
        self.root.append(&placeholder_label());
    }

    pub fn load_project(&self, path: PathBuf) {
        // Cancel any previous run before starting new one
        self.cancel_all();
        // Reset cancel flags for the new run
        self.cargo_cancel.store(false, Ordering::Relaxed);
        self.ollama_cancel.store(false, Ordering::Relaxed);

        while let Some(c) = self.root.first_child() { self.root.remove(&c); }
        self.snippets.borrow_mut().clear();

        let workspace = match find_workspace_root(&path) {
            Some(p) => p,
            None    => return,
        };

        // ── Header with Cancel button ─────────────────────────────────────
        let header = GtkBox::new(Orientation::Horizontal, 8);
        header.set_margin_start(12); header.set_margin_end(12);
        header.set_margin_top(10);  header.set_margin_bottom(6);

        let title = Label::builder()
            .label(&format!("Workspace: {}", workspace.display()))
            .halign(gtk4::Align::Start).hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::Start)
            .build();
        title.add_css_class("heading");

        let cancel_btn = Button::with_label("Cancel");
        cancel_btn.add_css_class("destructive-action");
        cancel_btn.add_css_class("flat");
        cancel_btn.set_tooltip_text(Some("Cancel cargo check"));

        let refresh_btn = Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.add_css_class("flat");
        refresh_btn.set_tooltip_text(Some("Re-run cargo check"));
        refresh_btn.set_visible(false); // shown when check finishes

        header.append(&title);
        header.append(&cancel_btn);
        header.append(&refresh_btn);
        self.root.append(&header);

        let status = Label::builder()
            .label("Running cargo check…")
            .halign(gtk4::Align::Start)
            .build();
        status.set_margin_start(12);
        status.add_css_class("dim-label");
        self.root.append(&status);
        self.root.append(&gtk4::Separator::new(Orientation::Horizontal));

        // ── Diagnostics list ──────────────────────────────────────────────
        let diag_list = GtkBox::new(Orientation::Vertical, 0);
        diag_list.set_margin_start(12); diag_list.set_margin_end(12);
        diag_list.set_margin_top(8);
        diag_list.set_vexpand(true);

        let diag_scroll = ScrolledWindow::builder()
            .vscrollbar_policy(PolicyType::Automatic)
            .hscrollbar_policy(PolicyType::Never)
            .vexpand(true).child(&diag_list)
            .build();
        self.root.append(&diag_scroll);

        // ── Export / action bar ───────────────────────────────────────────
        let export_bar = gtk4::ActionBar::new();
        export_bar.add_css_class("cargo-export-bar");

        let export_count = Label::new(Some("checking…"));
        export_count.add_css_class("dim-label");
        export_bar.set_center_widget(Some(&export_count));

        // Model selector — shared two-line dropdown, same as peek and chat
        let model_store = gtk4::StringList::new(&["loading…"]);
        let model_drop  = crate::ui::components::build_model_dropdown_simple(model_store.clone());
        model_drop.add_css_class("ollama-model-drop");
        model_drop.set_tooltip_text(Some("Ollama model for explanations"));
        export_bar.pack_start(&model_drop);

        let cargo_selected_model: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

        // Fetch models — local + LAN registry + background scan
        crate::ui::components::fetch_models_into_store(&model_store, &model_drop);

        // Keep selected_model in sync with dropdown changes
        {
            let sm  = Rc::clone(&cargo_selected_model);
            let ms2 = model_store.clone();
            model_drop.connect_notify_local(Some("selected"), move |dd: &gtk4::DropDown, _| {
                if let Some(s) = ms2.string(dd.selected()) {
                    *sm.borrow_mut() = s.to_string();
                }
            });
        }

        let export_btn = Button::with_label("⎘ Copy for AI");
        export_btn.add_css_class("suggested-action");
        export_btn.set_tooltip_text(Some("Copy errors (and optionally warnings) as markdown"));
        export_bar.pack_end(&export_btn);

        // Filter warnings toggle
        let filter_warn_btn = gtk4::ToggleButton::with_label("⚠ hide warnings");
        filter_warn_btn.add_css_class("flat");
        filter_warn_btn.set_tooltip_text(Some("Hide warning-level diagnostics from view and export"));
        export_bar.pack_start(&filter_warn_btn);
        self.root.append(&export_bar);

        // Export — respects hide_warnings filter
        {
            let snips = Rc::clone(&self.snippets);
            let hw    = Rc::clone(&self.hide_warnings);
            export_btn.connect_clicked(move |btn| {
                let snips_ref = snips.borrow();
                let filtered: Vec<_> = snips_ref.iter()
                    .filter(|s| !(hw.get() && s.level == "warning"))
                    .collect();
                let md = build_markdown_filtered(&filtered);
                if let Some(disp) = gtk4::gdk::Display::default() {
                    disp.clipboard().set_text(&md);
                    btn.set_label("Copied ✓");
                    let btn2 = btn.clone();
                    glib::timeout_add_seconds_local(2, move || {
                        btn2.set_label("⎘ Copy for AI"); glib::ControlFlow::Break
                    });
                }
            });
        }

        // Filter warnings toggle — walk diag_list and toggle card visibility
        {
            let dl = diag_list.clone();
            let hw = Rc::clone(&self.hide_warnings);
            filter_warn_btn.connect_toggled(move |btn| {
                let hide = btn.is_active();
                hw.set(hide);
                // Walk each card in diag_list and show/hide based on level badge
                let mut child = dl.first_child();
                while let Some(c) = child {
                    // Cards have css class cargo-diag-card; check if they contain a warning badge
                    let is_warning = widget_contains_css_class(&c, "cargo-warning");
                    if is_warning { c.set_visible(!hide); }
                    child = c.next_sibling();
                }
            });
        }

        // ── Spawn cargo check ─────────────────────────────────────────────
        let (tx, rx)   = unbounded::<CargoMsg>();
        let cargo_cancel = Arc::clone(&self.cargo_cancel);
        let ollama_cancel = Arc::clone(&self.ollama_cancel);

        {
            let ws = workspace.clone();
            let c  = Arc::clone(&cargo_cancel);
            std::thread::spawn(move || run_cargo_check(ws, tx, c));
        }

        // Cancel button — stops cargo check and marks done
        {
            let cc = Arc::clone(&cargo_cancel);
            let oc = Arc::clone(&ollama_cancel);
            let st = status.clone();
            let cb = cancel_btn.clone();
            let rb = refresh_btn.clone();
            cancel_btn.connect_clicked(move |_| {
                cc.store(true, Ordering::Relaxed);
                oc.store(true, Ordering::Relaxed);
                st.set_text("Cancelled");
                cb.set_visible(false);
                rb.set_visible(true);
            });
        }

        // idle_add_local — drain messages, update UI
        let dl   = diag_list.clone();
        let st   = status.clone();
        let snip = Rc::clone(&self.snippets);
        let cnt  = export_count.clone();
        let ws2  = workspace.clone();
        let oc   = Arc::clone(&ollama_cancel);
        let cb   = cancel_btn.clone();
        let rb   = refresh_btn.clone();
        let csm  = Rc::clone(&cargo_selected_model);
        let hw   = Rc::clone(&self.hide_warnings);

        glib::idle_add_local(move || {
            if cargo_cancel.load(Ordering::Relaxed) {
                cb.set_visible(false);
                rb.set_visible(true);
                return glib::ControlFlow::Break;
            }

            let mut keep = true;
            loop {
                match rx.try_recv() {
                    Ok(CargoMsg::Diagnostic(d)) => {
                        if let Some(card) = build_card(&d, &ws2, &oc, &csm) {
                            // Respect active hide-warnings filter
                            if hw.get() && d.level == "warning" {
                                card.set_visible(false);
                            }
                            dl.append(&card);
                            snip.borrow_mut().push(d);
                            cnt.set_text(&format!("{} diagnostic(s)", snip.borrow().len()));
                        }
                    }
                    Ok(CargoMsg::Done { success }) => {
                        let n = snip.borrow().len();
                        st.set_text(&if success {
                            format!("✓  cargo check passed ({n} warnings)")
                        } else {
                            format!("✗  {n} diagnostic(s)")
                        });
                        cnt.set_text(&format!("{n} diagnostic(s)"));
                        cb.set_visible(false);
                        rb.set_visible(true);
                        keep = false; break;
                    }
                    Ok(CargoMsg::Error(e)) => {
                        st.set_text(&format!("cargo check failed: {e}"));
                        cb.set_visible(false);
                        rb.set_visible(true);
                        keep = false; break;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(_) => { keep = false; break; }
                }
            }
            if keep { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
        });
    }
}

fn placeholder_label() -> Label {
    let l = Label::builder()
        .label("Open a file inside a Cargo workspace to view diagnostics")
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .vexpand(true)
        .build();
    l.add_css_class("dim-label");
    l
}

// ── cargo runner ─────────────────────────────────────────────────────────────

enum CargoMsg {
    Diagnostic(DiagSnippet),
    Done { success: bool },
    Error(String),
}

fn run_cargo_check(workspace: std::path::PathBuf, tx: crossbeam_channel::Sender<CargoMsg>, cancel: Arc<AtomicBool>) {
    let workspace = workspace.as_path();
    let cargo_ok = std::process::Command::new("cargo")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !cargo_ok {
        let _ = tx.send(CargoMsg::Error("cargo not found on PATH".into()));
        return;
    }

    let output = match std::process::Command::new("cargo")
        .args(["check", "--message-format=json", "--all-targets"])
        .current_dir(workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
    {
        Ok(o)  => o,
        Err(e) => { let _ = tx.send(CargoMsg::Error(e.to_string())); return; }
    };

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if cancel.load(Ordering::Relaxed) { break; }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v, Err(_) => continue,
        };
        if v["reason"] != "compiler-message" { continue; }
        let msg     = &v["message"];
        let level   = msg["level"].as_str().unwrap_or("note").to_owned();
        let message = msg["rendered"].as_str()
            .or_else(|| msg["message"].as_str())
            .unwrap_or("").to_owned();
        let code = msg["code"]["code"].as_str().map(str::to_owned);

        if let Some(span) = msg["spans"].as_array()
            .and_then(|a| a.iter().find(|s| s["is_primary"] == true).or_else(|| a.first()))
        {
            let file       = span["file_name"].as_str().unwrap_or("?").to_owned();
            let line_start = span["line_start"].as_u64().unwrap_or(1) as u32;
            let line_end   = span["line_end"].as_u64().unwrap_or(line_start as u64) as u32;
            let source_lines = read_source_context(workspace, &file, line_start, line_end);
            let _ = tx.send(CargoMsg::Diagnostic(DiagSnippet {
                file, line: line_start, level, message, code, source_lines,
            }));
        }
    }
    let _ = tx.send(CargoMsg::Done { success: output.status.success() });
}

// ── card builder ─────────────────────────────────────────────────────────────

fn build_card(
    d: &DiagSnippet,
    _workspace: &Path,
    ollama_cancel: &Arc<AtomicBool>,
    selected_model: &Rc<RefCell<String>>,
) -> Option<GtkBox> {
    if d.level == "note" && d.code.is_none() { return None; }

    let card = GtkBox::new(Orientation::Vertical, 0);
    card.add_css_class("cargo-diag-card");
    card.set_margin_bottom(10);

    // Header
    let hdr = GtkBox::new(Orientation::Horizontal, 8);
    hdr.set_margin_start(8); hdr.set_margin_end(8);
    hdr.set_margin_top(6);   hdr.set_margin_bottom(4);

    let badge = Label::new(Some(&d.level.to_uppercase()));
    badge.add_css_class("cargo-level-badge");
    badge.add_css_class(match d.level.as_str() {
        "error"   => "cargo-error",
        "warning" => "cargo-warning",
        _         => "cargo-note",
    });

    if let Some(code) = &d.code {
        let cl = Label::new(Some(&format!("[{code}]")));
        cl.add_css_class("cargo-code");
        hdr.append(&cl);
    }
    hdr.append(&badge);

    let loc = Label::builder()
        .label(&format!("{}:{}", d.file, d.line))
        .halign(gtk4::Align::Start).hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::Start)
        .build();
    loc.add_css_class("dim-label");
    hdr.append(&loc);
    card.append(&hdr);

    // Source snippet
    if !d.source_lines.is_empty() {
        let buf = TextBuffer::new(None);
        buf.set_text(&d.source_lines.join("\n"));
        let tv = TextView::with_buffer(&buf);
        tv.set_monospace(true);
        tv.set_editable(false);
        tv.set_wrap_mode(WrapMode::None);
        tv.add_css_class("cargo-snippet-view");
        let sw = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(150)
            .child(&tv)
            .build();
        sw.set_margin_start(8); sw.set_margin_end(8);
        card.append(&sw);
    }

    // Message + actions
    let msg_box = GtkBox::new(Orientation::Vertical, 4);
    msg_box.add_css_class("cargo-msg-box");
    msg_box.set_margin_start(8); msg_box.set_margin_end(8);
    msg_box.set_margin_top(4);  msg_box.set_margin_bottom(6);

    let msg_buf = TextBuffer::new(None);
    msg_buf.set_text(&d.message);
    let msg_tv = TextView::with_buffer(&msg_buf);
    msg_tv.set_monospace(true);
    msg_tv.set_editable(false);
    msg_tv.set_wrap_mode(WrapMode::WordChar);
    msg_tv.add_css_class("cargo-msg-text");
    msg_box.append(&msg_tv);

    let act_row = GtkBox::new(Orientation::Horizontal, 8);
    act_row.set_margin_top(4);

    let copy_btn = Button::with_label("Copy");
    copy_btn.add_css_class("flat");

    // Ollama dropdown for this card
    let card_menu = gio::Menu::new();
    card_menu.append(Some("✦ Explain"),       Some("ollama-card.explain"));
    card_menu.append(Some("What's wrong?"),   Some("ollama-card.whats-wrong"));
    card_menu.append(Some("Suggest a fix"),   Some("ollama-card.fix"));

    let ollama_menu_btn = gtk4::MenuButton::new();
    ollama_menu_btn.set_label("✦ Ollama ▾");
    ollama_menu_btn.add_css_class("flat");
    ollama_menu_btn.add_css_class("pill");
    ollama_menu_btn.set_menu_model(Some(&card_menu));

    let card_cancel_btn = Button::with_label("✕");
    card_cancel_btn.add_css_class("flat");
    card_cancel_btn.set_tooltip_text(Some("Cancel Ollama request"));
    card_cancel_btn.set_visible(false);

    let explain_lbl = Label::new(None);
    explain_lbl.set_hexpand(true);
    explain_lbl.set_halign(gtk4::Align::Start);
    explain_lbl.set_wrap(true);
    explain_lbl.add_css_class("ollama-result-text");

    act_row.append(&copy_btn);
    act_row.append(&ollama_menu_btn);
    act_row.append(&card_cancel_btn);
    act_row.append(&explain_lbl);
    msg_box.append(&act_row);
    card.append(&msg_box);

    // Copy
    { let msg = d.message.clone();
      copy_btn.connect_clicked(move |_| {
          if let Some(disp) = gtk4::gdk::Display::default() {
              disp.clipboard().set_text(&msg);
          }
      }); }

    // Ollama action group for this card
    {
        let context = format!("File: {}, line {}\nError: {}", d.file, d.line, d.message);
        let source  = d.source_lines.join("\n");
        let global_cancel = Arc::clone(ollama_cancel);

        let card_actions = [
            ("explain",     "Explain what is happening in this compiler error. Be concise (2-4 sentences)."),
            ("whats-wrong", "What is the root cause of this Rust compiler error? Give a direct 1-2 sentence diagnosis."),
            ("fix",         "Suggest a concrete fix for this Rust compiler error in 2-3 sentences."),
        ];

        let action_group = gio::SimpleActionGroup::new();

        for (action_name, prompt_suffix) in card_actions {
            let action = gio::SimpleAction::new(action_name, None);
            let ctx    = context.clone();
            let src    = source.clone();
            let rl     = explain_lbl.clone();
            let cb     = card_cancel_btn.clone();
            let gc     = Arc::clone(&global_cancel);
            let ps     = prompt_suffix.to_string();
            let sm     = Rc::clone(selected_model);

            action.connect_activate(move |_, _| {
                let entry = sm.borrow().clone();
                let (model, host) = crate::ui::components::resolve_key_to_host(
                    &entry, crate::ui::ollama::DEFAULT_HOST
                );
                let local_cancel = Arc::new(AtomicBool::new(false));
                rl.set_text("…");
                cb.set_visible(true);

                {
                    let lc  = Arc::clone(&local_cancel);
                    let cb2 = cb.clone();
                    cb.connect_clicked(move |_| {
                        lc.store(true, Ordering::Relaxed);
                        cb2.set_visible(false);
                    });
                }

                let prompt = format!("{ps}\n\nContext:\n{ctx}\n\nSource:\n{src}");
                let (tx, rx) = unbounded::<crate::ui::ollama::StreamChunk>();
                let gc2 = Arc::clone(&gc);
                let lc  = Arc::clone(&local_cancel);

                std::thread::spawn(move || {
                    crate::ui::ollama::prompt_streaming(
                        &prompt, &host, &model,
                        &tx, &gc2, &lc,
                    );
                });

                // Drain into label — batched at 60ms to avoid per-token reflows
                let rl2 = rl.clone();
                let cb2 = cb.clone();
                let acc = Rc::new(RefCell::new(String::new()));
                glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
                    let mut batch = String::new();
                    let mut done  = false;
                    loop {
                        match rx.try_recv() {
                            Ok(crate::ui::ollama::StreamChunk::Token(tok)) => {
                                batch.push_str(&tok);
                            }
                            Ok(crate::ui::ollama::StreamChunk::Done { .. }) |
                            Ok(crate::ui::ollama::StreamChunk::Cancelled) => {
                                cb2.set_visible(false);
                                done = true; break;
                            }
                            Ok(crate::ui::ollama::StreamChunk::Error(e)) => {
                                rl2.set_text(&format!("Ollama error: {e}"));
                                cb2.set_visible(false);
                                done = true; break;
                            }
                            Err(crossbeam_channel::TryRecvError::Empty) => break,
                            Err(_) => { cb2.set_visible(false); done = true; break; }
                        }
                    }
                    if !batch.is_empty() {
                        let mut a = acc.borrow_mut();
                        a.push_str(&batch);
                        rl2.set_text(&*a);
                    } else if done && rl2.text().is_empty() {
                        rl2.set_text("(cancelled)");
                    }
                    if done {
                        // Show rendered markdown on click if response has markdown
                        let final_text = acc.borrow().clone();
                        if crate::ui::markdown::has_markdown(&final_text) && rl2.parent().is_some() {
                            // Append clickable indicator
                            let shown = format!("{final_text}

⊞ click for rendered view");
                            rl2.set_text(&shown);
                            let ft2 = final_text.clone();
                            let rl3 = rl2.clone();
                            let gc  = gtk4::GestureClick::new();
                            gc.connect_pressed(move |_, _, _, _| {
                                let pop = gtk4::Popover::new();
                                let rendered = crate::ui::markdown::render(&ft2);
                                let scr = gtk4::ScrolledWindow::builder()
                                    .child(&rendered)
                                    .min_content_width(380).min_content_height(200)
                                    .max_content_height(500).build();
                                pop.set_child(Some(&scr));
                                pop.set_parent(&rl3);
                                pop.popup();
                            });
                            rl2.add_controller(gc);
                        }
                        glib::ControlFlow::Break
                    } else { glib::ControlFlow::Continue }
                });
            });

            action_group.add_action(&action);
        }

        ollama_menu_btn.insert_action_group("ollama-card", Some(&action_group));
    }

    Some(card)
}

// ── markdown export ───────────────────────────────────────────────────────────

#[allow(dead_code)]
fn build_markdown(snippets: &[DiagSnippet]) -> String {
    let refs: Vec<&DiagSnippet> = snippets.iter().collect();
    build_markdown_filtered(&refs)
}

fn build_markdown_filtered(snippets: &[&DiagSnippet]) -> String {
    let mut out = String::from("# Cargo diagnostics\n\n");
    out.push_str("*Generated by Logscope*\n\n");
    for (i, s) in snippets.iter().enumerate() {
        out.push_str(&format!("## {}. [{}] {} — {}:{}\n\n",
            i+1, s.code.as_deref().unwrap_or("?"), s.level.to_uppercase(), s.file, s.line));
        if !s.source_lines.is_empty() {
            out.push_str("```rust\n");
            for l in &s.source_lines { out.push_str(l); out.push('\n'); }
            out.push_str("```\n\n");
        }
        out.push_str("**Message:**\n```\n");
        out.push_str(&s.message);
        out.push_str("\n```\n\n");
    }
    out
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn find_workspace_root(path: &Path) -> Option<PathBuf> {
    let start = if path.is_file() { path.parent()? } else { path };
    let mut dir = start;
    loop {
        if dir.join("Cargo.toml").exists() { return Some(dir.to_path_buf()); }
        dir = dir.parent()?;
    }
}

fn read_source_context(workspace: &Path, rel_file: &str, start: u32, end: u32) -> Vec<String> {
    let text = match std::fs::read_to_string(workspace.join(rel_file)) {
        Ok(t) => t, Err(_) => return Vec::new(),
    };
    let lines: Vec<&str> = text.lines().collect();
    let lo = (start.saturating_sub(4)) as usize;
    let hi = ((end + 3) as usize).min(lines.len());
    lines[lo..hi].iter().map(|s| s.to_string()).collect()
}

/// Recursively check if a widget or any of its children has a given CSS class.
/// Used to detect warning cards by their "cargo-warning" badge class.
fn widget_contains_css_class(widget: &gtk4::Widget, css_class: &str) -> bool {
    if widget.has_css_class(css_class) { return true; }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if widget_contains_css_class(&c, css_class) { return true; }
        child = c.next_sibling();
    }
    false
}
