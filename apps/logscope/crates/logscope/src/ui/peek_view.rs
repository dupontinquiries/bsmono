//! peek_view.rs — log viewer with persistent selection overlay + drag handles.

use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, Label, Orientation,
    PolicyType, Revealer, RevealerTransitionType,
    ScrolledWindow, TextBuffer, TextTagTable, TextView, WrapMode,
};

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use crossbeam_channel::unbounded;

// ── syntax tag names ──────────────────────────────────────────────────────────
const T_TS:    &str = "ts";
const T_ERR:   &str = "err";
const T_WARN:  &str = "warn";
const T_DEBUG: &str = "debug";
const T_KEY:   &str = "key";
const T_NUM:   &str = "num";
const T_PATH:  &str = "path";
const T_KW:    &str = "kw";     // language keywords
const T_STR:   &str = "str";    // string literals
const T_CMT:   &str = "cmt";    // comments
const T_TYPE:  &str = "typ";    // type names / PascalCase identifiers
const T_SEL:   &str = "sel_hl"; // persistent selection highlight

// ── public struct ─────────────────────────────────────────────────────────────

pub struct PeekView {
    pub root:          GtkBox,
    pub ollama_cancel: Arc<AtomicBool>,
    /// Called whenever an Ollama response completes — forwards to chat tab.
    pub on_ollama_done: Rc<RefCell<Option<Box<dyn Fn(String, String)>>>>,
    /// Called when "Ask about this" is clicked on locked selection: (label, content).
    pub on_ask_about_this: Rc<RefCell<Option<Box<dyn Fn(String, String)>>>>,
    buffer:    TextBuffer,
    view:      TextView,
    #[allow(dead_code)]
    scroll:    ScrolledWindow,
    syntax_on: Rc<Cell<bool>>,
    lines:     Rc<RefCell<Vec<String>>>,
}

impl PeekView {
    pub fn new() -> Self {
        let tags   = build_tag_table();
        let buffer = TextBuffer::new(Some(&tags));
        let view   = TextView::with_buffer(&buffer);
        view.set_monospace(true);
        view.set_editable(false);
        view.set_cursor_visible(true);
        view.set_wrap_mode(WrapMode::None);
        view.add_css_class("peek-source-view");

        let scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .hexpand(true).vexpand(true)
            .child(&view)
            .build();

        let syntax_on     = Rc::new(Cell::new(true));
        let lines         = Rc::new(RefCell::new(Vec::<String>::new()));
        let ollama_cancel = Arc::new(AtomicBool::new(false));
        let on_ollama_done: Rc<RefCell<Option<Box<dyn Fn(String,String)>>>> =
            Rc::new(RefCell::new(None));
        let on_ask_about_this: Rc<RefCell<Option<Box<dyn Fn(String,String)>>>> =
            Rc::new(RefCell::new(None));

        // ── Toolbar ───────────────────────────────────────────────────────
        let toolbar = GtkBox::new(Orientation::Horizontal, 0);
        toolbar.set_margin_start(2); toolbar.set_margin_end(2);
        toolbar.set_margin_top(2);   toolbar.set_margin_bottom(2);
        toolbar.set_overflow(gtk4::Overflow::Hidden);

        // ⚙ popover: jump + syntax + shorten
        let pop_box = GtkBox::new(Orientation::Vertical, 10);
        pop_box.set_margin_start(12); pop_box.set_margin_end(12);
        pop_box.set_margin_top(10);   pop_box.set_margin_bottom(10);

        let syn_row = GtkBox::new(Orientation::Horizontal, 8);
        let syn_lbl = Label::builder().label("Syntax").hexpand(true).halign(gtk4::Align::Start).build();
        let syn_sw  = gtk4::Switch::new();
        syn_sw.set_active(true);
        syn_sw.set_valign(gtk4::Align::Center);
        syn_row.append(&syn_lbl); syn_row.append(&syn_sw);
        pop_box.append(&syn_row);

        let shorten_row = GtkBox::new(Orientation::Horizontal, 8);
        let shorten_lbl = Label::builder().label("Compress lines").hexpand(true).halign(gtk4::Align::Start).build();
        shorten_lbl.add_css_class("dim-label");
        let shorten_btn = Button::with_label("Apply ^^");
        shorten_btn.add_css_class("flat");
        let blockout_btn = Button::with_label("XX block");
        blockout_btn.add_css_class("flat");
        blockout_btn.set_tooltip_text(Some("Replace repeated line prefixes with XX"));
        shorten_row.append(&shorten_lbl);
        shorten_row.append(&shorten_btn);
        shorten_row.append(&blockout_btn);
        pop_box.append(&shorten_row);

        pop_box.append(&gtk4::Separator::new(Orientation::Horizontal));

        let jlbl = Label::builder().label("Scroll").halign(gtk4::Align::Start).build();
        jlbl.add_css_class("dim-label");
        pop_box.append(&jlbl);

        let jrow = GtkBox::new(Orientation::Horizontal, 4);
        jrow.set_homogeneous(true);
        for (lbl, d) in [("▲1K",-1000f64),("▲100",-100.0),("▲1",-1.0),("▼1",1.0),("▼100",100.0),("▼1K",1000.0)] {
            let b = Button::with_label(lbl); b.add_css_class("flat");
            let sc = scroll.clone();
            b.connect_clicked(move |_| line_scroll(&sc, d));
            jrow.append(&b);
        }
        pop_box.append(&jrow);

        let popover = gtk4::Popover::new();
        popover.set_child(Some(&pop_box));

        let ctrl_btn = gtk4::MenuButton::new();
        ctrl_btn.set_icon_name("view-more-symbolic");
        ctrl_btn.add_css_class("flat");
        ctrl_btn.set_popover(Some(&popover));

        // Syntax switch
        {
            let s = Rc::clone(&syntax_on); let b = buffer.clone(); let ls = Rc::clone(&lines);
            syn_sw.connect_active_notify(move |sw| {
                let now = sw.is_active(); s.set(now);
                if now { apply_highlighting(&b, &ls.borrow()); }
                else { clear_tags(&b); }
            });
        }

        // Shorten button
        {
            let b = buffer.clone(); let ls = Rc::clone(&lines);
            shorten_btn.connect_clicked(move |_| {
                // Collect to owned strings BEFORE dropping borrow to avoid
                // runtime borrow conflict between borrow() and borrow_mut().
                let source_lines: Vec<String> = ls.borrow().clone();
                let refs: Vec<&str> = source_lines.iter().map(String::as_str).collect();
                let cfg = crate::shorten::ShortenConfig::default();
                let result = crate::shorten::shorten(&refs, &cfg);
                let new_lines: Vec<String> = result.lines.iter().map(|s| {
                    if s.ends_with('\n') { s.clone() } else { format!("{s}\n") }
                }).collect();
                let text = new_lines.join("");
                b.set_text(&text);
                *ls.borrow_mut() = new_lines;
                apply_highlighting(&b, &ls.borrow());
            });
        }

        // Blockout button
        {
            let b  = buffer.clone(); let ls = Rc::clone(&lines);
            blockout_btn.connect_clicked(move |_| {
                let source_lines: Vec<String> = ls.borrow().clone();
                let refs: Vec<&str> = source_lines.iter().map(String::as_str).collect();
                let blocked = crate::shorten::blockout(&refs);
                let new_lines: Vec<String> = blocked.into_iter().map(|s| {
                    if s.ends_with('\n') { s } else { format!("{s}\n") }
                }).collect();
                let text = new_lines.join("");
                b.set_text(&text);
                *ls.borrow_mut() = new_lines;
                apply_highlighting(&b, &ls.borrow());
            });
        }

        // ── Selection extend buttons ──────────────────────────────────────
        let sel_row  = GtkBox::new(Orientation::Horizontal, 0);
        sel_row.set_overflow(gtk4::Overflow::Hidden);
        let sel_info = Label::new(Some(""));
        sel_info.add_css_class("peek-sel-info");
        sel_info.set_halign(gtk4::Align::Center);
        sel_info.set_hexpand(false);

        let su1k=sbtn("▲1K"); let su10=sbtn("▲10"); let su1=sbtn("▲1");
        let sd1=sbtn("▼1");   let sd10=sbtn("▼10"); let sd1k=sbtn("▼1K");

        sel_row.append(&su1k); sel_row.append(&su10); sel_row.append(&su1);
        sel_row.append(&sel_info);
        sel_row.append(&sd1);  sel_row.append(&sd10); sel_row.append(&sd1k);

        {
            let lbl = sel_info.clone();
            buffer.connect_mark_set(move |buf, _, mark| {
                if mark == &buf.get_insert() || mark == &buf.selection_bound() {
                    let (has, n) = sel_line_count(buf);
                    let text = if has { format!("{n}L") } else { String::new() };
                    lbl.set_text(&text);
                }
            });
        }

        for (btn, delta) in [(&su1k,-1000i32),(&su10,-10),(&su1,-1),(&sd1,1),(&sd10,10),(&sd1k,1000)] {
            let br = buffer.clone(); let d = delta;
            btn.connect_clicked(move |_| extend_selection(&br, d));
        }

        let tb_spacer = GtkBox::new(Orientation::Horizontal, 0);
        tb_spacer.set_hexpand(true);
        toolbar.append(&ctrl_btn);
        toolbar.append(&tb_spacer);
        toolbar.append(&sel_row);

        // ── Persistent selection overlay ──────────────────────────────────
        // An Overlay wrapping the scroll area; a floating Frame + drag handles
        // appear when the user locks a selection.
        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&scroll));
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);

        // The persistent selection frame — hidden by default
        let sel_frame = build_selection_frame(&buffer, &view, &overlay, &on_ask_about_this);
        overlay.add_overlay(&sel_frame);
        sel_frame.set_visible(false);
        sel_frame.set_valign(gtk4::Align::Start);
        sel_frame.set_halign(gtk4::Align::Fill);
        sel_frame.set_margin_start(4); sel_frame.set_margin_end(4);

        // "Lock selection" button in the toolbar
        let lock_btn = Button::from_icon_name("view-pin-symbolic");
        lock_btn.add_css_class("flat");
        lock_btn.set_tooltip_text(Some("Lock selection as persistent highlight"));
        toolbar.append(&lock_btn);

        {
            let buf  = buffer.clone();
            let sf   = sel_frame.clone();
            lock_btn.connect_clicked(move |_| {
                if let Some((s, e)) = buf.selection_bounds() {
                    // Apply persistent highlight tag
                    let s0 = buf.start_iter(); let e0 = buf.end_iter();
                    buf.remove_tag_by_name(T_SEL, &s0, &e0);
                    buf.apply_tag_by_name(T_SEL, &s, &e);
                    sf.set_visible(true);
                }
            });
        }

        // ── Ollama bar + revealer ─────────────────────────────────────────
        let (model_bar, analyse_bar, result_revealer) =
            build_analyse_overlay(&buffer, &ollama_cancel, &on_ollama_done);

        // ── Root ──────────────────────────────────────────────────────────
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_hexpand(true); root.set_vexpand(true);
        root.append(&toolbar);
        root.append(&gtk4::Separator::new(Orientation::Horizontal));
        root.append(&overlay);
        root.append(&model_bar);
        root.append(&analyse_bar);
        root.append(&result_revealer);

        PeekView { root, ollama_cancel, on_ollama_done, on_ask_about_this, buffer, view, scroll, syntax_on, lines }
    }

    /// Register callback called with (prompt_label, response_text) after each Ollama completion.
    pub fn set_on_ollama_done<F: Fn(String, String) + 'static>(&self, f: F) {
        *self.on_ollama_done.borrow_mut() = Some(Box::new(f));
    }

    /// Register callback for "Ask about this" on locked selection.
    pub fn set_on_ask_about_this<F: Fn(String, String) + 'static>(&self, f: F) {
        *self.on_ask_about_this.borrow_mut() = Some(Box::new(f));
    }

    pub fn load(&self, lines: &[String]) {
        let text = lines.join("");
        self.buffer.set_text(&text);
        *self.lines.borrow_mut() = lines.to_vec();
        if self.syntax_on.get() && !lines.is_empty() {
            apply_highlighting(&self.buffer, lines);
        }
        let mut start = self.buffer.start_iter();
        self.view.scroll_to_iter(&mut start, 0.0, true, 0.0, 0.0);
        // Clear any persistent highlight when new content is loaded
        let s0 = self.buffer.start_iter(); let e0 = self.buffer.end_iter();
        self.buffer.remove_tag_by_name(T_SEL, &s0, &e0);
    }

    pub fn scroll_to_token(&self, token: &str) -> Option<usize> {
        let lower = token.to_lowercase();
        let (line_idx, _) = self.lines.borrow().iter()
            .enumerate()
            .find(|(_, l)| l.to_lowercase().contains(&lower))?;
        if let Some(mut iter) = self.buffer.iter_at_line(line_idx as i32) {
            self.view.scroll_to_iter(&mut iter, 0.1, false, 0.0, 0.0);
        }
        Some(line_idx + 1)
    }
}

// ── Persistent selection frame ────────────────────────────────────────────────

fn build_selection_frame(
    buffer: &TextBuffer,
    _view: &TextView,
    _overlay: &gtk4::Overlay,
    on_ask: &Rc<RefCell<Option<Box<dyn Fn(String, String)>>>>,
) -> gtk4::Frame {
    let frame = gtk4::Frame::new(None);
    frame.add_css_class("sel-persistent-frame");
    frame.set_hexpand(true);

    let outer = GtkBox::new(Orientation::Vertical, 0);
    outer.set_hexpand(true);

    // ── Top handle bar ────────────────────────────────────────────────────
    let top_bar = GtkBox::new(Orientation::Horizontal, 4);
    top_bar.add_css_class("sel-handle-bar");
    top_bar.set_margin_start(4); top_bar.set_margin_end(4);

    // Top cardinal buttons: ▲1 ▲10 ▲100 ▲1000 (extend upward)
    let top_btns = GtkBox::new(Orientation::Horizontal, 2);
    top_btns.add_css_class("sel-cardinal-btns");
    for (lbl, d) in [("▲1000",-1000i32),("▲100",-100),("▲10",-10),("▲1",-1)] {
        let b = handle_btn(lbl); let br = buffer.clone();
        b.connect_clicked(move |_| extend_selection(&br, d));
        top_btns.append(&b);
    }

    // Drag handle — visual indicator, label shows selection size
    let top_handle = Label::new(Some("≡ drag"));
    top_handle.add_css_class("sel-handle");
    top_handle.set_hexpand(true);
    top_handle.set_halign(gtk4::Align::Center);

    // ✕ close button top-right
    let close_btn = Button::from_icon_name("window-close-symbolic");
    close_btn.add_css_class("flat");
    close_btn.add_css_class("sel-close-btn");
    close_btn.set_tooltip_text(Some("Remove persistent highlight"));

    top_bar.append(&top_btns);
    top_bar.append(&top_handle);
    top_bar.append(&close_btn);

    // ── Middle: selection info ────────────────────────────────────────────
    let info_row = GtkBox::new(Orientation::Horizontal, 8);
    info_row.set_margin_start(8); info_row.set_margin_end(8);
    info_row.set_margin_top(4);   info_row.set_margin_bottom(4);

    let sel_count_lbl = Label::new(Some(""));
    sel_count_lbl.add_css_class("dim-label");
    sel_count_lbl.set_hexpand(true);
    sel_count_lbl.set_halign(gtk4::Align::Start);

    let copy_sel_btn = Button::with_label("Copy");
    copy_sel_btn.add_css_class("flat");

    info_row.append(&sel_count_lbl);
    info_row.append(&copy_sel_btn);

    // ── Bottom handle bar ─────────────────────────────────────────────────
    let bot_bar = GtkBox::new(Orientation::Horizontal, 4);
    bot_bar.add_css_class("sel-handle-bar");
    bot_bar.set_margin_start(4); bot_bar.set_margin_end(4);

    let bot_handle = Label::new(Some("≡ drag"));
    bot_handle.add_css_class("sel-handle");
    bot_handle.set_hexpand(true);
    bot_handle.set_halign(gtk4::Align::Center);

    // Bottom cardinal buttons: ▼1 ▼10 ▼100 ▼1000 (extend downward)
    let bot_btns = GtkBox::new(Orientation::Horizontal, 2);
    bot_btns.add_css_class("sel-cardinal-btns");
    for (lbl, d) in [("▼1",1i32),("▼10",10),("▼100",100),("▼1000",1000)] {
        let b = handle_btn(lbl); let br = buffer.clone();
        b.connect_clicked(move |_| extend_selection(&br, d));
        bot_btns.append(&b);
    }

    bot_bar.append(&bot_handle);
    bot_bar.append(&bot_btns);

    outer.append(&top_bar);
    outer.append(&gtk4::Separator::new(Orientation::Horizontal));
    outer.append(&info_row);
    outer.append(&gtk4::Separator::new(Orientation::Horizontal));
    outer.append(&bot_bar);

    frame.set_child(Some(&outer));

    // Update sel_count_lbl when selection changes
    {
        let lbl = sel_count_lbl.clone();
        buffer.connect_mark_set(move |b, _, mark| {
            if mark == &b.get_insert() || mark == &b.selection_bound() {
                let (has, n) = sel_line_count(b);
                if has { lbl.set_text(&format!("{n} lines selected")); }
            }
        });
    }

    // Ask about this button
    let ask_btn = Button::with_label("Ask about this →");
    ask_btn.add_css_class("flat");
    ask_btn.add_css_class("chat-snippet-ask-btn");
    info_row.append(&ask_btn);

    // Copy selection
    {
        let buf = buffer.clone();
        copy_sel_btn.connect_clicked(move |_| {
            if let Some((s, e)) = buf.selection_bounds() {
                if let Some(d) = gtk4::gdk::Display::default() {
                    d.clipboard().set_text(&buf.text(&s, &e, false));
                }
            }
        });
    }

    // Ask about this — fires the registered callback with selection content
    {
        let buf = buffer.clone();
        let ask_cb = Rc::clone(on_ask);
        ask_btn.connect_clicked(move |_| {
            if let Some(cb) = ask_cb.borrow().as_ref() {
                // Use the T_SEL-tagged range if available, else current selection
                let text = if let Some((s, e)) = buf.selection_bounds() {
                    buf.text(&s, &e, false).to_string()
                } else {
                    buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string()
                };
                if !text.trim().is_empty() {
                    cb("Peek selection".to_string(), text);
                }
            }
        });
    }

    // Close clears the highlight and hides frame
    {
        let buf = buffer.clone(); let f = frame.clone();
        close_btn.connect_clicked(move |_| {
            let s0 = buf.start_iter(); let e0 = buf.end_iter();
            buf.remove_tag_by_name(T_SEL, &s0, &e0);
            f.set_visible(false);
        });
    }

    frame
}

fn handle_btn(label: &str) -> Button {
    let b = Button::with_label(label);
    b.add_css_class("sel-handle-btn");
    b
}

// ── TextTag table ─────────────────────────────────────────────────────────────

fn build_tag_table() -> TextTagTable {
    use gtk4::pango;
    let table = TextTagTable::new();
    macro_rules! tag {
        ($name:expr, $body:expr) => {{ let t = gtk4::TextTag::new(Some($name)); $body(&t); table.add(&t); }};
    }
    tag!(T_TS,    |t: &gtk4::TextTag| { t.set_foreground(Some("#888888")); });
    tag!(T_ERR,   |t: &gtk4::TextTag| { t.set_foreground(Some("#e01b24")); t.set_weight(700); });
    tag!(T_WARN,  |t: &gtk4::TextTag| { t.set_foreground(Some("#c08000")); });
    tag!(T_DEBUG, |t: &gtk4::TextTag| { t.set_foreground(Some("#666666")); });
    tag!(T_KEY,   |t: &gtk4::TextTag| { t.set_foreground(Some("#3584e4")); });
    tag!(T_NUM,   |t: &gtk4::TextTag| { t.set_foreground(Some("#26a269")); });
    tag!(T_PATH,  |t: &gtk4::TextTag| { t.set_foreground(Some("#9141ac")); t.set_underline(pango::Underline::Single); });
    // New language tags
    tag!(T_KW,    |t: &gtk4::TextTag| { t.set_foreground(Some("#c061cb")); t.set_weight(700); });
    tag!(T_STR,   |t: &gtk4::TextTag| { t.set_foreground(Some("#e9a84c")); });
    tag!(T_CMT,   |t: &gtk4::TextTag| { t.set_foreground(Some("#5e8a6e")); t.set_style(pango::Style::Italic); });
    tag!(T_TYPE,  |t: &gtk4::TextTag| { t.set_foreground(Some("#89b4fa")); });
    // Persistent selection overlay — semi-transparent accent background
    tag!(T_SEL,   |t: &gtk4::TextTag| {
        // Persistent selection: left-margin indent approach.
        // We avoid background (opaque in GTK4, kills syntax colors).
        // Instead: increase left margin to visually "indent" selected lines,
        // plus a strong underline and bold weight so the selection is unmistakable.
        t.set_left_margin(12);
        t.set_weight(700);
        t.set_underline(gtk4::pango::Underline::Low);
        t.set_underline_rgba(Some(&gtk4::gdk::RGBA::new(0.208, 0.518, 0.894, 1.0)));
    });
    table
}

// ── Syntax highlighting ───────────────────────────────────────────────────────

const RUST_KEYWORDS: &[&str] = &[
    "fn","let","mut","pub","use","mod","impl","struct","enum","trait",
    "type","const","static","extern","unsafe","async","await","match",
    "if","else","while","for","loop","return","break","continue","where",
    "in","ref","move","dyn","box","self","Self","super","crate","as","true","false",
];
const C_KEYWORDS: &[&str] = &[
    "struct","enum","union","typedef","void","int","char","float","double",
    "long","short","unsigned","signed","const","static","extern","inline",
    "return","if","else","while","for","do","switch","case","break","continue",
    "goto","sizeof","NULL","true","false","auto","register","volatile",
];
const PYTHON_KEYWORDS: &[&str] = &[
    "def","class","import","from","as","return","if","elif","else","while",
    "for","in","not","and","or","is","None","True","False","pass","break",
    "continue","raise","try","except","finally","with","lambda","yield",
    "global","nonlocal","del","assert","async","await",
];

fn clear_tags(buf: &TextBuffer) {
    let s = buf.start_iter(); let e = buf.end_iter();
    for t in [T_TS,T_ERR,T_WARN,T_DEBUG,T_KEY,T_NUM,T_PATH,T_KW,T_STR,T_CMT,T_TYPE] {
        buf.remove_tag_by_name(t, &s, &e);
    }
}

fn apply_highlighting(buf: &TextBuffer, lines: &[String]) {
    clear_tags(buf);
    let limit = buf.char_count();
    let mut offset: i32 = 0;

    // Detect language from first non-empty line
    let lang = detect_language(lines);

    let apply = |buf: &TextBuffer, from: i32, to: i32, tag: &str| {
        let f = from.clamp(0, limit); let t = to.clamp(0, limit);
        if f < t { buf.apply_tag_by_name(tag, &buf.iter_at_offset(f), &buf.iter_at_offset(t)); }
    };

    for line in lines {
        if offset >= limit { break; }
        let stripped = line.trim_end_matches('\n');
        if stripped.is_empty() { offset += 1; continue; }
        let line_chars = stripped.chars().count() as i32;
        let line_end   = (offset + line_chars).min(limit);

        // Timestamp (log files)
        let ts = detect_timestamp_prefix_chars(stripped);
        if ts > 0 { apply(buf, offset, offset + ts as i32, T_TS); }

        let upper = stripped.to_ascii_uppercase();

        // Log level keywords
        for (needle, tag) in [
            ("ERROR",T_ERR),("FATAL",T_ERR),("[ERR]",T_ERR),
            ("WARN",T_WARN),("[WARN]",T_WARN),
            ("DEBUG",T_DEBUG),("[DBG]",T_DEBUG),("TRACE",T_DEBUG),
            ("INFO",T_KEY),
        ] {
            if let Some(bp) = upper.find(needle) {
                let cp = offset + stripped[..bp].chars().count() as i32;
                apply(buf, cp, cp + needle.chars().count() as i32, tag);
            }
        }

        // Language-specific highlighting
        match lang {
            Lang::Rust | Lang::C | Lang::Python => {
                highlight_lang(buf, stripped, offset, limit, lang, &apply);
            }
            Lang::Unknown => {
                highlight_numbers_chars(buf, stripped, offset, limit);
                highlight_paths_chars(buf, stripped, offset, limit);
            }
        }

        // ^^ shortening marker
        let trimmed = stripped.trim_start();
        if trimmed.starts_with("^^") {
            apply(buf, offset + (stripped.len() - trimmed.len()) as i32,
                  offset + (stripped.len() - trimmed.len() + 2) as i32, T_KW);
        }

        offset = line_end + 1;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Lang { Unknown, Rust, C, Python }

fn detect_language(lines: &[String]) -> Lang {
    let sample = lines.iter().take(30).map(String::as_str).collect::<Vec<_>>().join("\n");
    // Rust: fn, let, mut, use, impl, ->
    let rust_score = ["fn ", "let ", "mut ", "use ", "impl ", " -> ", "::", "#["].iter()
        .filter(|s| sample.contains(*s)).count();
    // C: struct, #include, ->, (*), ;
    let c_score = ["struct ", "#include", "->", "(*)", ";"].iter()
        .filter(|s| sample.contains(*s)).count();
    // Python: def , import , elif, self.
    let py_score = ["def ", "import ", "elif ", "self."].iter()
        .filter(|s| sample.contains(*s)).count();

    match (rust_score, c_score, py_score) {
        (r,c,p) if r >= c && r >= p && r >= 2 => Lang::Rust,
        (r,c,p) if c > r && c >= p && c >= 2  => Lang::C,
        (r,c,p) if p > r && p > c && p >= 2   => Lang::Python,
        _ => Lang::Unknown,
    }
}

fn highlight_lang<F>(buf: &TextBuffer, line: &str, base: i32, limit: i32, lang: Lang, apply: &F)
where F: Fn(&TextBuffer, i32, i32, &str)
{
    let keywords: &[&str] = match lang {
        Lang::Rust   => RUST_KEYWORDS,
        Lang::C      => C_KEYWORDS,
        Lang::Python => PYTHON_KEYWORDS,
        Lang::Unknown => &[],
    };

    let mut char_idx = 0i32;
    let mut in_str = false;
    #[allow(unused_assignments)]
    let mut str_char = '"';
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();

    let mut i = 0;
    while i < n {
        let ch = chars[i];

        // Comments
        if !in_str {
            if i + 1 < n {
                if (lang == Lang::Rust || lang == Lang::C) && ch == '/' && chars[i+1] == '/' {
                    // Rest of line is a comment
                    let from = (base + char_idx).min(limit);
                    let to   = (base + n as i32).min(limit);
                    apply(buf, from, to, T_CMT);
                    break;
                }
                if lang == Lang::Python && ch == '#' {
                    let from = (base + char_idx).min(limit);
                    let to   = (base + n as i32).min(limit);
                    apply(buf, from, to, T_CMT);
                    break;
                }
            }
        }

        // String literals
        if !in_str && (ch == '"' || ch == '\'') {
            in_str = true; let _ = in_str; str_char = ch;
            let str_start = char_idx;
            i += 1; char_idx += 1;
            while i < n {
                if chars[i] == str_char && (i == 0 || chars[i-1] != '\\') {
                    char_idx += 1; i += 1;
                    break;
                }
                char_idx += 1; i += 1;
            }
            apply(buf, (base+str_start).min(limit), (base+char_idx).min(limit), T_STR);
            in_str = false;
            continue;
        }

        if !in_str {
            // Check for keyword at current position
            if ch.is_ascii_alphabetic() || ch == '_' {
                let word_start = i;
                let char_start = char_idx;
                while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1; char_idx += 1;
                }
                let word: String = chars[word_start..i].iter().collect();
                if keywords.contains(&word.as_str()) {
                    apply(buf, (base+char_start).min(limit), (base+char_idx).min(limit), T_KW);
                } else if word.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
                    && word.len() > 1
                {
                    // PascalCase = type name
                    apply(buf, (base+char_start).min(limit), (base+char_idx).min(limit), T_TYPE);
                } else {
                    // Numbers inline
                    highlight_numbers_chars(buf, &chars[word_start..i].iter().collect::<String>(),
                        base + char_start, limit);
                }
                continue;
            }

            // Standalone numbers
            if ch.is_ascii_digit() {
                let from = (base + char_idx).min(limit);
                let to   = (from + 1).min(limit);
                if from < to { apply(buf, from, to, T_NUM); }
            }
        }

        char_idx += 1;
        i += 1;
    }

    // Paths always highlighted
    highlight_paths_chars(buf, line, base, limit);
}

fn detect_timestamp_prefix_chars(line: &str) -> usize {
    let b = line.as_bytes();
    if b.len() >= 19 && b[4]==b'-' && b[7]==b'-'
        && (b[10]==b'T'||b[10]==b' ') && b[13]==b':' && b[16]==b':'
    {
        let mut i = 19usize;
        if i < b.len() && b[i]==b'.' { i+=1; while i<b.len()&&b[i].is_ascii_digit(){i+=1;} }
        if i < b.len() && (b[i]==b'Z'||b[i]==b'+'||b[i]==b'-') { i=(i+6).min(b.len()); }
        return i;
    }
    let months=["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
    if b.len()>=15 && months.iter().any(|m| line.starts_with(m)) && b[6]==b':' { return 15; }
    0
}

fn highlight_numbers_chars(buf: &TextBuffer, line: &str, base: i32, limit: i32) {
    let mut char_idx = 0i32; let mut prev = false;
    for ch in line.chars() {
        if ch.is_ascii_digit() && !prev {
            let f=(base+char_idx).min(limit); let t=(f+1).min(limit);
            if f<t { buf.apply_tag_by_name(T_NUM,&buf.iter_at_offset(f),&buf.iter_at_offset(t)); }
        }
        prev = ch.is_alphanumeric()||ch=='_';
        char_idx += 1;
    }
}

fn highlight_paths_chars(buf: &TextBuffer, line: &str, base: i32, limit: i32) {
    for prefix in ["/home","/etc","/var","/tmp","/usr","http://","https://","~/.config"] {
        if let Some(bp) = line.find(prefix) {
            let cs = base + line[..bp].chars().count() as i32;
            let pc = line[bp..].chars().take_while(|c| !c.is_whitespace()&&*c!='"'&&*c!='\'').count() as i32;
            let from=cs.min(limit); let to=(cs+pc).min(limit);
            if from<to { buf.apply_tag_by_name(T_PATH,&buf.iter_at_offset(from),&buf.iter_at_offset(to)); }
        }
    }
}

// ── Ollama overlay ────────────────────────────────────────────────────────────

const PROMPTS: &[(&str, &str, &str)] = &[
    ("explain",     "✦ Explain",     "Explain what is happening in these log lines. Be concise (2-4 sentences)."),
    ("summarize",   "Summarize",      "Summarize these log lines in 1-2 sentences."),
    ("whats-wrong", "What's wrong?",  "What error or problem is shown? Give a direct 1-2 sentence diagnosis."),
    ("fix",         "Suggest a fix", "Suggest a concrete fix or next debugging step in 2-3 sentences."),
];

fn build_analyse_overlay(
    buffer: &TextBuffer,
    ollama_cancel: &Arc<AtomicBool>,
    on_done: &Rc<RefCell<Option<Box<dyn Fn(String, String)>>>>,
) -> (gtk4::ActionBar, gtk4::ActionBar, Revealer) {
    // Returns: (model_bar — always visible, analyse_bar — selection only, result_revealer)
    // Model bar — always visible, lets user pick model even before selecting text
    let model_bar = gtk4::ActionBar::new();
    model_bar.add_css_class("peek-analyse-bar");

    let pstore     = crate::ui::components::ProviderStore::new();
    let model_drop = crate::ui::components::build_model_dropdown(&pstore);
    model_drop.add_css_class("ollama-model-drop");
    model_drop.set_width_request(140);
    pstore.string_list.append("scanning…");
    model_bar.pack_start(&model_drop);

    let hint_lbl = Label::new(Some("Select lines to analyse"));
    hint_lbl.add_css_class("dim-label");
    model_bar.set_center_widget(Some(&hint_lbl));

    // Selection bar — only shown when text is selected
    let action_bar = gtk4::ActionBar::new();
    action_bar.add_css_class("peek-analyse-bar");
    action_bar.set_visible(false);

    let sel_label = Label::new(Some(""));
    sel_label.add_css_class("dim-label");
    action_bar.set_center_widget(Some(&sel_label));

    let copy_btn = Button::from_icon_name("edit-copy-symbolic");
    copy_btn.add_css_class("flat");
    copy_btn.set_tooltip_text(Some("Copy selection"));

    // selected_model tracks the current store key (used by prompt actions)
    let selected_model: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    // Discovery: local → known registry hosts → fresh LAN scan
    // Channel sends (host_url, Vec<ModelInfo>) — "" host_url = localhost
    {
        let pstore2 = pstore.clone();
        let drop2   = model_drop.clone();
        let sm      = Rc::clone(&selected_model);
        let (tx, rx) = unbounded::<(String, Vec<crate::model_selector::ModelInfo>)>();

        // Use std::thread (not rayon) since scan_lan_parallel blocks for ~1-2s
        // with its own internal thread pool. Rayon threads are for CPU work.
        std::thread::spawn(move || {
            crate::providers::probe_all_known(&tx);
            crate::providers::scan_lan_parallel(&tx);
        });

        // Accumulate (host, models) pairs; rebuild ModelEntry list on each update
        let acc: Rc<RefCell<Vec<(String, Vec<crate::model_selector::ModelInfo>)>>> =
            Rc::new(RefCell::new(Vec::new()));

        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            let mut got_any  = false;
            let mut scan_done = false;
            loop {
                match rx.try_recv() {
                    Ok((host_url, batch)) => {
                        // Merge this host's models into accumulator
                        let mut a = acc.borrow_mut();
                        if let Some(slot) = a.iter_mut().find(|(h, _)| h == &host_url) {
                            // Extend existing slot (shouldn't happen normally, but be safe)
                            for m in batch { if !slot.1.iter().any(|e| e.name == m.name) { slot.1.push(m); } }
                        } else {
                            a.push((host_url, batch));
                        }
                        got_any = true;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => { scan_done = true; break; }
                }
            }
            if got_any {
                let a = acc.borrow();
                let entries = crate::providers::build_model_entries(&a, &[]);
                if let Some(first) = entries.first() {
                    *sm.borrow_mut() = first.store_key();
                }
                pstore2.set_entries(&drop2, entries);
            }
            if scan_done { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
        });
    }
    { let sm = Rc::clone(&selected_model);
      let ps2 = pstore.clone();
      model_drop.connect_notify_local(Some("selected"), move |dd: &gtk4::DropDown, _| {
          if let Some(e) = ps2.selected_entry(dd) {
              *sm.borrow_mut() = e.store_key();
          }
      });
    }

    let menu = gio::Menu::new();
    for (n,l,_) in PROMPTS { menu.append(Some(l),Some(&format!("ollama.{n}"))); }
    menu.append(Some("Custom prompt…"), Some("ollama.custom"));
    let menu_btn = gtk4::MenuButton::new();
    menu_btn.set_icon_name("system-run-symbolic");
    menu_btn.add_css_class("suggested-action");
    menu_btn.set_menu_model(Some(&menu));
    action_bar.pack_end(&menu_btn);

    { let buf=buffer.clone(); copy_btn.connect_clicked(move|_|{
        if let Some((s,e))=buf.selection_bounds(){if let Some(d)=gtk4::gdk::Display::default(){d.clipboard().set_text(&buf.text(&s,&e,false));}}
    }); }
    { let bar=action_bar.clone(); let lbl=sel_label.clone(); let hl=hint_lbl.clone();
      buffer.connect_mark_set(move|buf,_,mark|{
          if mark==&buf.get_insert()||mark==&buf.selection_bound(){
              let (has,n)=sel_line_count(buf); bar.set_visible(has); hl.set_visible(!has);
              if has{lbl.set_text(&format!("{n} line{} selected",if n==1{""}else{"s"}));}
          }
      });
    }

    let revealer = Revealer::new();
    revealer.set_transition_type(RevealerTransitionType::SlideDown);
    revealer.set_transition_duration(200);

    // Outer container: custom prompt entry + result
    let rev_col = GtkBox::new(Orientation::Vertical, 0);

    // Custom prompt entry row (shown after "Custom prompt…" selected)
    let custom_row = GtkBox::new(Orientation::Horizontal, 6);
    custom_row.set_margin_start(8); custom_row.set_margin_end(8);
    custom_row.set_margin_top(4);
    custom_row.set_visible(false);
    let custom_entry = gtk4::Entry::builder()
        .hexpand(true).placeholder_text("Enter custom prompt…").build();
    custom_entry.add_css_class("chat-input");
    let custom_run_btn = Button::with_label("▶ Run");
    custom_run_btn.add_css_class("suggested-action");
    custom_row.append(&custom_entry);
    custom_row.append(&custom_run_btn);
    rev_col.append(&custom_row);

    let result_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never).vscrollbar_policy(PolicyType::Automatic)
        .max_content_height(300).propagate_natural_height(true).build();

    let row = GtkBox::new(Orientation::Horizontal, 6); row.add_css_class("ollama-result-box");
    row.set_margin_start(8); row.set_margin_end(8); row.set_margin_top(6); row.set_margin_bottom(6);

    let spinner=gtk4::Spinner::new(); spinner.set_valign(gtk4::Align::Start);
    let mode_lbl=Label::new(None); mode_lbl.add_css_class("ollama-mode-label"); mode_lbl.set_valign(gtk4::Align::Start);
    let res_lbl=Label::new(None); res_lbl.set_hexpand(true); res_lbl.set_halign(gtk4::Align::Start);
    res_lbl.set_wrap(true); res_lbl.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    res_lbl.set_selectable(true); res_lbl.add_css_class("ollama-result-text");
    let model_lbl=Label::new(None); model_lbl.add_css_class("dim-label"); model_lbl.set_valign(gtk4::Align::Start);
    let copy_res=Button::from_icon_name("edit-copy-symbolic"); copy_res.add_css_class("flat"); copy_res.set_valign(gtk4::Align::Start);
    let cancel_b=Button::with_label("✕"); cancel_b.add_css_class("flat"); cancel_b.set_valign(gtk4::Align::Start); cancel_b.set_visible(false);
    // "Add to Chat" button — sends result as context to Chat tab
    let to_chat_btn=Button::with_label("↪ Chat"); to_chat_btn.add_css_class("flat"); to_chat_btn.set_valign(gtk4::Align::Start);
    to_chat_btn.set_tooltip_text(Some("Add this result to Chat tab context"));
    let dismiss=Button::from_icon_name("window-close-symbolic"); dismiss.add_css_class("flat"); dismiss.set_valign(gtk4::Align::Start);

    let text_col=GtkBox::new(Orientation::Vertical,2); text_col.set_hexpand(true);
    text_col.append(&mode_lbl); text_col.append(&res_lbl);
    row.append(&spinner); row.append(&text_col); row.append(&model_lbl);
    row.append(&copy_res); row.append(&to_chat_btn); row.append(&cancel_b); row.append(&dismiss);
    result_scroll.set_child(Some(&row));
    rev_col.append(&result_scroll);
    revealer.set_child(Some(&rev_col));

    { let l=res_lbl.clone(); copy_res.connect_clicked(move|_|{if let Some(d)=gtk4::gdk::Display::default(){d.clipboard().set_text(&l.text());}});}
    { let r=revealer.clone(); dismiss.connect_clicked(move|_|{r.set_reveal_child(false);});}
    // "Add to Chat" — forward result to on_done callback (same as completing an action)
    { let rl=res_lbl.clone(); let ml2=mode_lbl.clone(); let done_fwd=Rc::clone(on_done);
      to_chat_btn.connect_clicked(move|_|{
          let text=rl.text().to_string();
          if !text.is_empty() {
              if let Some(f)=done_fwd.borrow().as_ref() { f(ml2.text().to_string(), text); }
          }
      });
    }

    let action_group = gio::SimpleActionGroup::new();
    for (action_name, label, prompt_template) in PROMPTS {
        let action = gio::SimpleAction::new(action_name, None);
        let buf=buffer.clone(); let rev=revealer.clone();
        let rl=res_lbl.clone(); let ml=model_lbl.clone(); let ml2=mode_lbl.clone();
        let spin=spinner.clone(); let cb=cancel_b.clone();
        let gc=Arc::clone(ollama_cancel);
        let ptpl=prompt_template.to_string(); let lstr=label.to_string();
        let sm=Rc::clone(&selected_model);
        let done_cb=Rc::clone(on_done);

        action.connect_activate(move |_,_| {
            let selected=match buf.selection_bounds(){Some((s,e))=>buf.text(&s,&e,false).to_string(),None=>return};
            if selected.trim().is_empty(){return;}
            let entry = sm.borrow().clone();
            let (model, host) = crate::ui::components::resolve_key_to_host(
                &entry, crate::ui::ollama::DEFAULT_HOST
            );
            let lc=Arc::new(AtomicBool::new(false));
            ml2.set_text(&lstr); rl.set_text("…"); ml.set_text(""); spin.start(); cb.set_visible(true); rev.set_reveal_child(true);
            {let l=Arc::clone(&lc);let c=cb.clone();let s=spin.clone();cb.connect_clicked(move|_|{l.store(true,Ordering::Relaxed);c.set_visible(false);s.stop();});}
            let lines:Vec<String>=selected.lines().map(str::to_owned).collect();
            let prompt=format!("{}\n\nLog lines:\n{}",ptpl,lines.join("\n"));
            let (tx,rx)=unbounded::<crate::ui::ollama::StreamChunk>();
            let gc2=Arc::clone(&gc); let lc2=Arc::clone(&lc);
            std::thread::spawn(move||{crate::ui::ollama::prompt_streaming(&prompt,&host,&model,&tx,&gc2,&lc2);});
            let lstr2=lstr.clone(); let done_cb2=Rc::clone(&done_cb);
            stream_into_label_with_done(rx, rl.clone(), ml.clone(), spin.clone(), cb.clone(),
                move |response| { if let Some(f)=done_cb2.borrow().as_ref(){ f(lstr2.clone(), response); } });
        });
        action_group.add_action(&action);
    }

    // "Custom prompt…" action — reveals the custom entry row
    {
        let cr = custom_row.clone();
        let ce = custom_entry.clone();
        let act = gio::SimpleAction::new("custom", None);
        act.connect_activate(move |_,_| {
            cr.set_visible(true);
            ce.grab_focus();
        });
        action_group.add_action(&act);
    }

    // custom_run_btn — fires the custom prompt text against the selection
    {
        let buf=buffer.clone(); let rev=revealer.clone();
        let rl=res_lbl.clone(); let ml=model_lbl.clone(); let ml2=mode_lbl.clone();
        let spin=spinner.clone(); let cb=cancel_b.clone();
        let gc=Arc::clone(ollama_cancel);
        let sm=Rc::clone(&selected_model);
        let done_cb=Rc::clone(on_done);
        let ce=custom_entry.clone(); let cr=custom_row.clone();
        let fire_custom = move || {
            let selected=match buf.selection_bounds(){Some((s,e))=>buf.text(&s,&e,false).to_string(),None=>return};
            let custom_text=ce.text().to_string();
            if selected.trim().is_empty()||custom_text.trim().is_empty() {return;}
            let entry=sm.borrow().clone();
            let (model, host) = crate::ui::components::resolve_key_to_host(
                &entry, crate::ui::ollama::DEFAULT_HOST
            );
            let lc=Arc::new(AtomicBool::new(false));
            ml2.set_text("Custom"); rl.set_text("…"); ml.set_text("");
            spin.start(); cb.set_visible(true); rev.set_reveal_child(true);
            cr.set_visible(false);
            {let l=Arc::clone(&lc);let c=cb.clone();let s=spin.clone();
             cb.connect_clicked(move|_|{l.store(true,Ordering::Relaxed);c.set_visible(false);s.stop();});}
            let lines:Vec<String>=selected.lines().map(str::to_owned).collect();
            let prompt=format!("{}\n\nLog lines:\n{}",custom_text,lines.join("\n"));
            let (tx,rx)=unbounded::<crate::ui::ollama::StreamChunk>();
            let gc2=Arc::clone(&gc); let lc2=Arc::clone(&lc);
            std::thread::spawn(move||{crate::ui::ollama::prompt_streaming(&prompt,&host,&model,&tx,&gc2,&lc2);});
            let done_cb2=Rc::clone(&done_cb);
            stream_into_label_with_done(rx, rl.clone(), ml.clone(), spin.clone(), cb.clone(),
                move |response| { if let Some(f)=done_cb2.borrow().as_ref(){ f("Custom".into(), response); } });
        };
        let fire_rc = Rc::new(fire_custom);
        let fire2 = Rc::clone(&fire_rc);
        custom_run_btn.connect_clicked(move |_| fire2());
        // Enter key in custom entry also fires
        let fire3 = Rc::clone(&fire_rc);
        let key_ctrl = gtk4::EventControllerKey::new();
        key_ctrl.connect_key_pressed(move |_, key, _, mods| {
            if key == gtk4::gdk::Key::Return && !mods.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
                fire3(); return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        custom_entry.add_controller(key_ctrl);
    }

    menu_btn.insert_action_group("ollama", Some(&action_group));

    (model_bar, action_bar, revealer)
}

// ── stream helpers ────────────────────────────────────────────────────────────

fn stream_into_label_with_done<F>(
    rx: crossbeam_channel::Receiver<crate::ui::ollama::StreamChunk>,
    res_lbl: Label, model_lbl: Label, spinner: gtk4::Spinner, cancel_btn: Button,
    on_done: F,
) where F: Fn(String) + 'static {
    let acc = Rc::new(RefCell::new(String::new()));
    glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
        let mut batch=String::new(); let mut done=false;
        loop {
            match rx.try_recv() {
                Ok(crate::ui::ollama::StreamChunk::Token(tok)) => { batch.push_str(&tok); }
                Ok(crate::ui::ollama::StreamChunk::Done{model}) => {
                    spinner.stop(); cancel_btn.set_visible(false);
                    model_lbl.set_text(&format!(" {model}")); done=true; break;
                }
                Ok(crate::ui::ollama::StreamChunk::Cancelled) => {
                    spinner.stop(); cancel_btn.set_visible(false);
                    if acc.borrow().is_empty() { res_lbl.set_text("(cancelled)"); }
                    done=true; break;
                }
                Ok(crate::ui::ollama::StreamChunk::Error(e)) => {
                    spinner.stop(); cancel_btn.set_visible(false);
                    res_lbl.set_text(&format!("⚠ {e}")); done=true; break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(_) => { spinner.stop(); cancel_btn.set_visible(false); done=true; break; }
            }
        }
        if !batch.is_empty() {
            let mut a = acc.borrow_mut();
            a.push_str(&batch);
            if res_lbl.parent().is_some() { res_lbl.set_text(&*a); }
        }
        if done {
            let final_text = acc.borrow().clone();
            // If response contains markdown, add a ⊞ indicator to the label
            // and make it open a rendered popover on click
            if crate::ui::markdown::has_markdown(&final_text) {
                // Append a marker so users know rendered view is available
                let display_text = format!("{}

⊞ tap to view rendered", final_text);
                if res_lbl.parent().is_some() {
                    res_lbl.set_text(&display_text);
                }
                // Click opens a rendered popover
                let ft2 = final_text.clone();
                let rl2  = res_lbl.clone();
                let gc   = gtk4::GestureClick::new();
                gc.connect_pressed(move |_, _, _, _| {
                    let popover = gtk4::Popover::new();
                    let rendered = crate::ui::markdown::render(&ft2);
                    let scroll = gtk4::ScrolledWindow::builder()
                        .child(&rendered)
                        .min_content_width(420)
                        .min_content_height(300)
                        .max_content_height(600)
                        .build();
                    popover.set_child(Some(&scroll));
                    popover.set_parent(&rl2);
                    popover.popup();
                });
                res_lbl.add_controller(gc);
            } else if res_lbl.parent().is_some() {
                res_lbl.set_text(&final_text);
            }
            on_done(final_text);
            glib::ControlFlow::Break
        } else { glib::ControlFlow::Continue }
    });
}

// ── helpers ───────────────────────────────────────────────────────────────────


fn sbtn(label: &str) -> Button {
    let b = Button::with_label(label); b.add_css_class("peek-sel-btn"); b
}

fn extend_selection(buf: &TextBuffer, delta: i32) {
    let (mut start, mut end) = match buf.selection_bounds() {
        Some((s,e)) => (s,e),
        None => { let c=buf.iter_at_mark(&buf.get_insert()); (c.clone(),c) }
    };
    let total = buf.line_count();
    if delta < 0 {
        let nl=(start.line()+delta).clamp(0,total-1);
        if let Some(mut it)=buf.iter_at_line(nl){it.set_line_offset(0);start=it;}
    } else {
        let nl=(end.line()+delta).clamp(0,total-1);
        if let Some(mut it)=buf.iter_at_line(nl){if !it.ends_line(){it.forward_to_line_end();}end=it;}
    }
    buf.select_range(&start,&end);
}

fn line_scroll(scroll: &ScrolledWindow, lines: f64) {
    let adj=scroll.vadjustment();
    let t=(adj.value()+lines*18.0).clamp(0.0,(adj.upper()-adj.page_size()).max(0.0));
    adj.set_value(t);
}

fn sel_line_count(buf: &TextBuffer) -> (bool, usize) {
    match buf.selection_bounds() {
        None=>(false,0),
        Some((s,e))=>if s==e{(false,0)}else{(true,(e.line()-s.line()+1).max(1) as usize)}
    }
}
