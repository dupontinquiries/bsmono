//! bloc_tab.rs — Scratch-like touch-first coding environment
//!
//! Layout (paned, resizable):
//!   ┌─ Category palette ─┬─ Chip strip (suggestions) ──────────────────────┐
//!   │ [Control]          │  [if ‸] [let ‸] [fn ‸]  [match ‸]  [map()]     │
//!   │ [Variables]        ├─ Editor ────────────────────────────────────────┤
//!   │ [Functions]        │  monospace TextView, pinch-to-zoom               │
//!   │ [Types]            │  cursor auto-advances to next ‸ slot             │
//!   │ [Patterns]         ├─ Library ───────────────────────────────────────┤
//!   │ [Operators]        │  [search]  name  [kind]  used×                   │
//!   │ [AI]               │  ...rows...                                      │
//!   └────────────────────┴─────────────────────────────────────────────────┘
//!
//! Touch interactions:
//!   Tap chip       → insert at cursor, jump to first ‸ slot
//!   Long-press chip → popover: full text + Save as Bloc button
//!   Swipe chip strip → scroll left/right
//!   Tap ‸ in editor  → select that slot for replacement
//!   Pinch editor    → zoom font size (8–24pt)
//!   Double-tap row  → insert into editor
//!   Long-press row  → inline edit name

use gtk4::prelude::*;
use gtk4::gio;
use gtk4::{
    Box as GtkBox, Button, Entry, Label, ListBox, ListBoxRow,
    Orientation, Paned, PolicyType, Popover, ScrolledWindow,
    SelectionMode, Spinner, TextBuffer, TextView, WrapMode,
};
use libadwaita as adw;
use gtk4::glib;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use crossbeam_channel::unbounded;

use crate::bloc_engine::{
    Suggestion, SuggestionSource, ScoringCtx,
    PLACEHOLDER, builtin_templates, advance_to_placeholder, next_placeholder_offset,
};

// ── Bloc model ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Bloc {
    pub name:     String,
    pub content:  String,
    pub kind:     BlocKind,
    pub category: String,
    pub tags:     Vec<String>,
    pub used:     u32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum BlocKind { Code, Snippet, Template, Prose, Data, Unknown }

impl BlocKind {
    fn label(&self) -> &'static str {
        match self {
            BlocKind::Code     => "code",
            BlocKind::Snippet  => "snip",
            BlocKind::Template => "tmpl",
            BlocKind::Prose    => "prose",
            BlocKind::Data     => "data",
            BlocKind::Unknown  => "?",
        }
    }
}

fn blocs_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logscope").join("blocs.json")
}
fn load_blocs() -> Vec<Bloc> {
    std::fs::read_to_string(blocs_path()).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
fn save_blocs(blocs: &[Bloc]) {
    let p = blocs_path();
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    if let Ok(j) = serde_json::to_string_pretty(blocs) { let _ = std::fs::write(p, j); }
}

// ── BlocTab ───────────────────────────────────────────────────────────────────

pub struct BlocTab {
    pub root:  GtkBox,
    pub blocs: Rc<RefCell<Vec<Bloc>>>,
}

impl BlocTab {
    pub fn new() -> Self {
        let blocs        = Rc::new(RefCell::new(load_blocs()));
        let root         = GtkBox::new(Orientation::Vertical, 0);
        let current_file: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
        let suggestions:  Rc<RefCell<Vec<Suggestion>>> = Rc::new(RefCell::new(vec![]));
        let scan_cancel  = Arc::new(AtomicBool::new(false));
        let font_size: Rc<RefCell<f64>> = Rc::new(RefCell::new(13.0));

        // ── Header bar ────────────────────────────────────────────────────────
        let header = adw::HeaderBar::new();
        header.set_show_end_title_buttons(false);

        let file_btn = Button::with_label("Open");
        file_btn.add_css_class("flat");
        file_btn.set_tooltip_text(Some("Open a source file to edit"));

        let lsp_lbl = Label::new(Some("LSP: —"));
        lsp_lbl.add_css_class("dim-label");
        lsp_lbl.add_css_class("bloc-status-lbl");

        let model_store = gtk4::StringList::new(&["…"]);
        let model_drop  = crate::ui::components::build_model_dropdown_simple(model_store.clone());
        model_drop.add_css_class("ollama-model-drop");
        model_drop.set_tooltip_text(Some("Ollama model for AI suggestions"));

        let scan_spinner = Spinner::new();
        scan_spinner.set_visible(false);

        let zoom_out = Button::from_icon_name("zoom-out-symbolic");
        let zoom_in  = Button::from_icon_name("zoom-in-symbolic");
        zoom_out.add_css_class("flat"); zoom_in.add_css_class("flat");
        zoom_out.set_tooltip_text(Some("Smaller editor font (Ctrl+-)"));
        zoom_in.set_tooltip_text(Some("Larger editor font (Ctrl++)"));

        header.pack_start(&file_btn);
        header.pack_start(&lsp_lbl);
        header.set_title_widget(Some(&model_drop));
        header.pack_end(&zoom_in);
        header.pack_end(&zoom_out);
        header.pack_end(&scan_spinner);
        root.append(&header);

        // ── Chip strip ────────────────────────────────────────────────────────
        let chip_area = GtkBox::new(Orientation::Vertical, 0);
        chip_area.add_css_class("bloc-chip-area");

        let chip_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Never)
            .build();
        let chip_row = GtkBox::new(Orientation::Horizontal, 5);
        chip_row.set_margin_start(8); chip_row.set_margin_end(8);
        chip_row.set_margin_top(5);   chip_row.set_margin_bottom(5);

        let placeholder_lbl = Label::new(Some("Type or open a file to get suggestions…"));
        placeholder_lbl.add_css_class("bloc-chip-placeholder");
        chip_row.append(&placeholder_lbl);

        chip_scroll.set_child(Some(&chip_row));
        chip_area.append(&chip_scroll);
        root.append(&chip_area);

        // ── Quick-insert toolbar ──────────────────────────────────────────────
        // Large-target buttons for the most common structural inserts
        let quick_bar = GtkBox::new(Orientation::Horizontal, 4);
        quick_bar.set_margin_start(8); quick_bar.set_margin_end(8);
        quick_bar.set_margin_top(4);   quick_bar.set_margin_bottom(4);
        quick_bar.add_css_class("bloc-quick-bar");

        let quick_templates = [
            ("if",     "if ‸cond‸ {\n    ‸body‸\n}"),
            ("for",    "for ‸x‸ in ‸iter‸ {\n    ‸body‸\n}"),
            ("fn",     "fn ‸name‸(‸args‸) -> ‸T‸ {\n    ‸body‸\n}"),
            ("match",  "match ‸expr‸ {\n    ‸pat‸ => ‸val‸,\n    _ => ‸def‸,\n}"),
            ("let",    "let ‸name‸ = ‸value‸;"),
            ("struct", "struct ‸Name‸ {\n    ‸field‸: ‸Type‸,\n}"),
            ("impl",   "impl ‸Name‸ {\n    ‸body‸\n}"),
            ("→ ‸",    "‸"),
        ];
        let mut quick_btns = Vec::new();
        for (label, _) in &quick_templates {
            let b = Button::with_label(label);
            b.add_css_class("bloc-quick-btn");
            quick_bar.append(&b);
            quick_btns.push(b);
        }
        root.append(&quick_bar);

        // ── Main paned: palette | editor+library ──────────────────────────────
        let paned = Paned::new(Orientation::Horizontal);
        paned.set_vexpand(true);
        paned.set_position(140);  // palette width

        // LEFT: Category palette
        let palette = GtkBox::new(Orientation::Vertical, 0);
        palette.add_css_class("bloc-palette");

        let templates   = builtin_templates();
        let categories: Vec<&str> = {
            let mut seen = Vec::new();
            for t in &templates {
                if !seen.contains(&t.category) { seen.push(t.category); }
            }
            seen.push("Saved");
            seen
        };

        let active_cat: Rc<RefCell<String>> = Rc::new(RefCell::new("Control".to_string()));
        let palette_list = ListBox::new();
        palette_list.set_selection_mode(SelectionMode::Single);
        palette_list.add_css_class("bloc-palette-list");

        for cat in &categories {
            let row = ListBoxRow::new();
            let lbl = Label::builder().label(*cat)
                .halign(gtk4::Align::Start).build();
            lbl.set_margin_start(12); lbl.set_margin_end(8);
            lbl.set_margin_top(8);    lbl.set_margin_bottom(8);
            lbl.add_css_class("bloc-cat-label");
            row.set_child(Some(&lbl));
            palette_list.append(&row);
        }
        // Select first category
        if let Some(row) = palette_list.row_at_index(0) {
            palette_list.select_row(Some(&row));
        }

        let pal_scroll = ScrolledWindow::builder()
            .child(&palette_list)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true).build();
        palette.append(&pal_scroll);
        paned.set_start_child(Some(&palette));

        // RIGHT: editor + library stacked
        let right_box = GtkBox::new(Orientation::Vertical, 0);

        // Editor status bar
        let editor_header = GtkBox::new(Orientation::Horizontal, 8);
        editor_header.set_margin_start(10); editor_header.set_margin_end(10);
        editor_header.set_margin_top(4);    editor_header.set_margin_bottom(2);

        let file_lbl = Label::builder().halign(gtk4::Align::Start).hexpand(true).build();
        file_lbl.add_css_class("dim-label"); file_lbl.add_css_class("bloc-file-lbl");
        file_lbl.set_text("No file — paste or open");
        let lang_badge = Label::new(Some("—"));
        lang_badge.add_css_class("bloc-lang-badge");
        let slot_lbl = Label::new(Some(""));
        slot_lbl.add_css_class("bloc-slot-lbl");
        let save_btn = Button::with_label("Save Bloc");
        save_btn.add_css_class("flat");

        editor_header.append(&file_lbl);
        editor_header.append(&lang_badge);
        editor_header.append(&slot_lbl);
        editor_header.append(&save_btn);
        right_box.append(&editor_header);

        // Editor
        let edit_buf  = TextBuffer::new(None);
        let edit_view = TextView::builder()
            .buffer(&edit_buf).wrap_mode(WrapMode::WordChar)
            .monospace(true).editable(true).build();
        edit_view.add_css_class("bloc-editor");

        // Highlight tag for ‸ slots
        let tag_table = edit_buf.tag_table();
        let slot_tag = gtk4::TextTag::new(Some("slot"));
        slot_tag.set_background("rgba(255,200,0,0.35)");
        slot_tag.set_foreground("#333");
        tag_table.add(&slot_tag);

        let edit_scroll = ScrolledWindow::builder()
            .child(&edit_view)
            .vscrollbar_policy(PolicyType::Automatic)
            .min_content_height(180)
            .vexpand(false).build();
        right_box.append(&edit_scroll);

        right_box.append(&gtk4::Separator::new(Orientation::Horizontal));

        // Library search + list
        let search_row = GtkBox::new(Orientation::Horizontal, 6);
        search_row.set_margin_start(8); search_row.set_margin_end(8);
        search_row.set_margin_top(6);   search_row.set_margin_bottom(4);

        let search_entry = Entry::builder().placeholder_text("Search saved blocs…")
            .hexpand(true).build();
        search_entry.add_css_class("bloc-search");
        let insert_btn = Button::with_label("Insert");
        insert_btn.add_css_class("suggested-action"); insert_btn.set_sensitive(false);
        let del_btn = Button::from_icon_name("edit-delete-symbolic");
        del_btn.add_css_class("flat"); del_btn.set_sensitive(false);

        search_row.append(&search_entry);
        search_row.append(&insert_btn);
        search_row.append(&del_btn);
        right_box.append(&search_row);

        let bloc_list = ListBox::new();
        bloc_list.set_selection_mode(SelectionMode::Single);
        bloc_list.add_css_class("boxed-list");
        bloc_list.set_margin_start(8); bloc_list.set_margin_end(8);
        let lib_scroll = ScrolledWindow::builder()
            .child(&bloc_list).vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true).build();
        right_box.append(&lib_scroll);

        let status_lbl = Label::builder().halign(gtk4::Align::Start).build();
        status_lbl.set_margin_start(10); status_lbl.set_margin_bottom(4);
        status_lbl.add_css_class("dim-label");
        right_box.append(&status_lbl);

        paned.set_end_child(Some(&right_box));
        root.append(&paned);

        // ── Populate saved blocs ──────────────────────────────────────────────
        for b in blocs.borrow().iter() { bloc_list.append(&make_bloc_row(b)); }

        // ── Fetch models ──────────────────────────────────────────────────────
        crate::ui::components::fetch_models_into_store(&model_store, &model_drop);

        // ── Template insertion helper ──────────────────────────────────────────
        // Inserts text at cursor, jumps to first ‸, highlights all ‸ slots
        let insert_template = {
            let buf = edit_buf.clone();
            let sl  = slot_lbl.clone();
            Rc::new(move |text: &str| {
                let mark = buf.get_insert();
                let mut it = buf.iter_at_mark(&mark);
                let offset = it.offset() as usize;
                buf.insert(&mut it, text);

                // Re-highlight all ‸ slots
                highlight_slots(&buf);

                // Move cursor to first ‸
                let s = buf.start_iter(); let e = buf.end_iter();
                let full = buf.text(&s, &e, false).to_string();
                // offset is a CHAR offset from GTK; advance_to_placeholder works on bytes.
                let offset_byte = full.char_indices().nth(offset)
                    .map(|(b,_)| b).unwrap_or(full.len());
                if let Some(byte_pos) = advance_to_placeholder(&full, offset_byte) {
                    let char_pos     = full[..byte_pos].chars().count() as i32;
                    let ph_char_len  = PLACEHOLDER.chars().count() as i32;
                    let slot_it      = buf.iter_at_offset(char_pos);
                    let end_it       = buf.iter_at_offset(char_pos + ph_char_len);
                    buf.select_range(&slot_it, &end_it);

                    // Count remaining slots
                    let remaining = full[byte_pos+PLACEHOLDER.len()..].matches(PLACEHOLDER).count();
                    sl.set_text(&format!("‸ {} slot{} remaining",
                        remaining + 1, if remaining == 0 { "" } else { "s" }));
                } else {
                    sl.set_text("");
                }
            })
        };

        // ── Wire: quick-insert toolbar ────────────────────────────────────────
        for (btn, (_, tmpl_text)) in quick_btns.iter().zip(quick_templates.iter()) {
            let it = insert_template.clone();
            let t  = tmpl_text.to_string();
            btn.connect_clicked(move |_| { it(&t); });
        }

        // ── Wire: Tab key advances to next ‸ slot ─────────────────────────────
        {
            let buf = edit_buf.clone();
            let sl  = slot_lbl.clone();
            let key = gtk4::EventControllerKey::new();
            let buf2 = buf.clone();
            key.connect_key_pressed(move |_, key, _, mods| {
                use gtk4::gdk::Key;
                if key == Key::Tab && mods.is_empty() {
                    // Find next ‸ after cursor
                    let mark = buf2.get_insert();
                    let it   = buf2.iter_at_mark(&mark);
                    let cur  = it.offset() as usize;
                    let s = buf2.start_iter(); let e = buf2.end_iter();
                    let full = buf2.text(&s, &e, false).to_string();
                    // Scan from after current cursor
                    // cur is a CHAR offset (from GTK iter.offset()).
                    // next_placeholder_offset works on bytes — convert cur to byte first.
                    let cur_byte: usize = full.char_indices().nth(cur)
                        .map(|(b,_)| b).unwrap_or(full.len());
                    if let Some(byte_pos) = next_placeholder_offset(&full, cur_byte) {
                        let char_pos = full[..byte_pos].chars().count() as i32;
                        let ph_char_len = PLACEHOLDER.chars().count() as i32;
                        let start  = buf2.iter_at_offset(char_pos);
                        let end    = buf2.iter_at_offset(char_pos + ph_char_len);
                        buf2.select_range(&start, &end);
                        let remaining = full[byte_pos+PLACEHOLDER.len()..].matches(PLACEHOLDER).count();
                        sl.set_text(&format!("‸ {} slot{} remaining",
                            remaining + 1, if remaining == 0 { "" } else { "s" }));
                        return gtk4::glib::Propagation::Stop; // consume Tab
                    }
                    sl.set_text("");
                }
                gtk4::glib::Propagation::Proceed
            });
            edit_view.add_controller(key);
        }

        // ── Wire: zoom (buttons + pinch gesture) ─────────────────────────────
        // One shared CssProvider is reused on every zoom change to avoid
        // leaking a new provider on each click/pinch event.
        let zoom_provider: Rc<gtk4::CssProvider> = Rc::new(gtk4::CssProvider::new());
        {
            // Register the provider once at construction time
            if let Some(display) = gtk4::gdk::Display::default() {
                gtk4::style_context_add_provider_for_display(
                    &display, &*zoom_provider, gtk4::STYLE_PROVIDER_PRIORITY_USER,
                );
            }
        }

        let apply_zoom = {
            let fs  = Rc::clone(&font_size);
            let zp  = Rc::clone(&zoom_provider);
            Rc::new(move |delta: f64| {
                let mut sz = fs.borrow_mut();
                *sz = (*sz + delta).clamp(8.0, 30.0);
                let css = format!("textview.bloc-editor {{ font-size: {:.1}pt; }}", *sz);
                zp.load_from_string(&css);
            })
        };

        // Zoom-in / zoom-out buttons
        {
            let az = apply_zoom.clone();
            zoom_in.connect_clicked(move |_| az(1.0));
        }
        {
            let az = apply_zoom.clone();
            zoom_out.connect_clicked(move |_| az(-1.0));
        }

        // Pinch-to-zoom via GestureZoom
        // scale_changed gives CUMULATIVE scale since gesture began (not a delta).
        // We save font_size at gesture begin and apply current scale to that base.
        {
            let fs_begin: Rc<std::cell::Cell<f64>> = Rc::new(std::cell::Cell::new(13.0));
            let fs_ref   = Rc::clone(&font_size);
            let zp2      = Rc::clone(&zoom_provider);
            let zoom     = gtk4::GestureZoom::new();

            let fs_b2 = Rc::clone(&fs_begin);
            let fs_r2 = Rc::clone(&fs_ref);
            zoom.connect_begin(move |_, _| {
                fs_b2.set(*fs_r2.borrow());
            });

            let fs_b3 = Rc::clone(&fs_begin);
            let fs_r3 = Rc::clone(&fs_ref);
            zoom.connect_scale_changed(move |_, scale| {
                let base   = fs_b3.get();
                let new_sz = (base * scale).clamp(8.0, 30.0);
                *fs_r3.borrow_mut() = new_sz;
                let css = format!("textview.bloc-editor {{ font-size: {:.1}pt; }}", new_sz);
                zp2.load_from_string(&css);
            });

            edit_view.add_controller(zoom);
        }

        // ── Wire: category palette → chip strip + bloc list ───────────────────
        {
            let sug2    = Rc::clone(&suggestions);
            let buf2    = edit_buf.clone();
            let chip2   = chip_row.clone();
            let bl2     = Rc::clone(&blocs);
            let list2   = bloc_list.clone();
            let it2     = insert_template.clone();
            let ac      = Rc::clone(&active_cat);
            let tmpls   = templates.clone();

            palette_list.connect_row_selected(move |_, row| {
                let Some(row) = row else { return };
                let idx = row.index() as usize;
                let cat = if idx < categories.len() { categories[idx] } else { return };
                *ac.borrow_mut() = cat.to_string();

                if cat == "Saved" {
                    // Show only saved blocs in the chip strip
                    let saved_chips: Vec<Suggestion> = bl2.borrow().iter().map(|b| Suggestion {
                        label:  b.name.chars().take(28).collect(),
                        text:   b.content.clone(),
                        score:  0.5 + (b.used as f32 * 0.02).min(0.4),
                        source: SuggestionSource::Pinned,
                        detail: format!("{} · used {}×", b.kind.label(), b.used),
                    }).collect();
                    rebuild_chips(&chip2, &saved_chips, &buf2, &it2);

                    // Show all saved blocs in library
                    while let Some(r) = list2.row_at_index(0) { list2.remove(&r); }
                    for b in bl2.borrow().iter() { list2.append(&make_bloc_row(b)); }
                } else {
                    // Show template blocs for this category in chip strip
                    let tmpl_chips: Vec<Suggestion> = tmpls.iter()
                        .filter(|t| t.category == cat)
                        .map(|t| Suggestion {
                            label:  t.label.to_string(),
                            text:   t.text.to_string(),
                            score:  0.85,
                            source: SuggestionSource::TokenParse,
                            detail: format!("[{}] template", t.category),
                        })
                        .collect();
                    rebuild_chips(&chip2, &tmpl_chips, &buf2, &it2);

                    // Show filtered saved blocs in library
                    while let Some(r) = list2.row_at_index(0) { list2.remove(&r); }
                    for b in bl2.borrow().iter()
                        .filter(|b| b.category == cat || cat == "All") {
                        list2.append(&make_bloc_row(b));
                    }
                }
            });
        }

        // ── Wire: AI scoring pipeline (debounced) ─────────────────────────────
        let debounce: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        {
            let chip2   = chip_row.clone();
            let buf2    = edit_buf.clone();
            let sug2    = Rc::clone(&suggestions);
            let cf2     = Rc::clone(&current_file);
            let ms2     = model_store.clone();
            let md2     = model_drop.clone();
            let lsp2    = lsp_lbl.clone();
            let lang2   = lang_badge.clone();
            let sp2     = scan_spinner.clone();
            let cancel2 = Arc::clone(&scan_cancel);
            let db      = Rc::clone(&debounce);
            let bl2     = Rc::clone(&blocs);
            let it2     = insert_template.clone();

            edit_buf.connect_changed(move |buf| {
                // Cheap immediate update: slot count + tag highlight (no debounce needed)
                {
                    let s = buf.start_iter(); let e = buf.end_iter();
                    let t = buf.text(&s, &e, false);
                    let n = t.matches(PLACEHOLDER).count();
                    if n == 0 {
                        slot_lbl.set_text("");
                    } else {
                        slot_lbl.set_text(&format!("‸ {} slot{}", n, if n==1{""} else {"s"}));
                    }
                    highlight_slots(buf);
                }

                if let Some(id) = db.borrow_mut().take() { id.remove(); }

                let chip3   = chip2.clone();
                let buf3    = buf2.clone();
                let sug3    = Rc::clone(&sug2);
                let cf3     = Rc::clone(&cf2);
                let ms3     = ms2.clone(); let md3 = md2.clone();
                let lsp3    = lsp2.clone(); let lang3 = lang2.clone();
                let sp3     = sp2.clone();
                let cancel3 = Arc::clone(&cancel2);
                let bl3     = Rc::clone(&bl2);
                let it3     = it2.clone();

                let sid = glib::timeout_add_local_once(
                    std::time::Duration::from_millis(500),
                    move || {
                        let s = buf3.start_iter(); let e = buf3.end_iter();
                        let text = buf3.text(&s, &e, false).to_string();
                        if text.trim().is_empty() { return; }

                        let cursor = buf3.iter_at_mark(&buf3.get_insert()).offset() as usize;
                        let selected_key = ms3.string(md3.selected())
                            .map(|s| s.to_string())
                            .filter(|s| !matches!(s.as_str(), "…"|"scanning…"|"no models found"))
                            .unwrap_or_default();
                        let (ai_model, ai_host) = if selected_key.is_empty() {
                            (String::new(), crate::ui::ollama::DEFAULT_HOST.to_string())
                        } else {
                            let (m, h) = crate::ui::components::resolve_key_to_host(
                                &selected_key, crate::ui::ollama::DEFAULT_HOST);
                            (m, if h == "__auto__" { crate::ui::ollama::DEFAULT_HOST.to_string() } else { h })
                        };

                        let file = cf3.borrow().clone();
                        let ctx  = ScoringCtx {
                            text: text.clone(), cursor,
                            file: file.clone(),
                            project_root: file.as_ref().and_then(|f| f.parent()).map(|p| p.to_path_buf()),
                            ai_model, ai_host,
                        };

                        let lang = crate::bloc_engine::detect_ts_language(&ctx).unwrap_or("?");
                        lang3.set_text(lang);
                        let lsp = crate::bloc_engine::detect_lsp_server(Some(lang));
                        if lsp3.text().as_str() != format!("LSP: {}", lsp.unwrap_or("—")).as_str() {
                            lsp3.set_text(&format!("LSP: {}", lsp.unwrap_or("—")));
                        }

                        let pinned: Vec<Suggestion> = bl3.borrow().iter().map(|b| Suggestion {
                            text: b.content.clone(),
                            label: b.name.chars().take(28).collect(),
                            score: 0.5 + (b.used as f32 * 0.01).min(0.4),
                            source: SuggestionSource::Pinned,
                            detail: format!("saved · used {}×", b.used),
                        }).collect();

                        sp3.start(); sp3.set_visible(true);
                        cancel3.store(true, Ordering::Relaxed);
                        std::thread::yield_now();
                        cancel3.store(false, Ordering::Relaxed);

                        let (tx, rx) = unbounded::<Vec<Suggestion>>();

                        let ctx2 = ctx.clone(); let p2 = pinned.clone();
                        let tx2 = tx.clone(); let c2 = Arc::clone(&cancel3);
                        rayon::spawn(move || {
                            let r = crate::bloc_engine::score_text(&ctx2, &p2, false, false, Some(&c2));
                            let _ = tx2.send(r);
                        });

                        let ctx3 = ctx.clone(); let p3 = pinned.clone();
                        let tx3 = tx.clone(); let c3 = Arc::clone(&cancel3);
                        std::thread::spawn(move || {
                            if c3.load(Ordering::Relaxed) { return; }
                            let r = crate::bloc_engine::score_text(&ctx3, &p3, true, true, Some(&c3));
                            let _ = tx3.send(r);
                        });
                        drop(tx);

                        let chip4 = chip3.clone(); let sug4 = Rc::clone(&sug3);
                        let sp4 = sp3.clone(); let it4 = it3.clone(); let buf4 = buf3.clone();
                        glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
                            loop {
                                match rx.try_recv() {
                                    Ok(batch) => {
                                        let mut merged = sug4.borrow().clone();
                                        for s in batch {
                                            if let Some(e) = merged.iter_mut().find(|e| e.text == s.text) {
                                                if s.score > e.score { *e = s; }
                                            } else { merged.push(s); }
                                        }
                                        merged.sort_by(|a,b| b.score.partial_cmp(&a.score).unwrap());
                                        merged.truncate(16);
                                        *sug4.borrow_mut() = merged.clone();
                                        rebuild_chips(&chip4, &merged, &buf4, &it4);
                                    }
                                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                                        sp4.stop(); sp4.set_visible(false);
                                        return glib::ControlFlow::Break;
                                    }
                                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                                }
                            }
                            glib::ControlFlow::Continue
                        });
                    }
                );
                *db.borrow_mut() = Some(sid);
            });
        }

        // ── Wire: open file ───────────────────────────────────────────────────
        {
            let buf2 = edit_buf.clone(); let fl2 = file_lbl.clone();
            let cf2  = Rc::clone(&current_file);
            file_btn.connect_clicked(move |btn| {
                let dialog = gtk4::FileDialog::builder().title("Open source file").build();
                let buf3 = buf2.clone(); let fl3 = fl2.clone();
                let cf3  = Rc::clone(&cf2);
                let win  = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
                dialog.open(win.as_ref(), None::<&gio::Cancellable>, move |res| {
                    if let Ok(f) = res {
                        if let Some(path) = f.path() {
                            if let Ok(s) = std::fs::read_to_string(&path) {
                                buf3.set_text(&s);
                                fl3.set_text(&path.to_string_lossy());
                                *cf3.borrow_mut() = Some(path);
                            }
                        }
                    }
                });
            });
        }

        // ── Wire: save bloc ───────────────────────────────────────────────────
        {
            let buf2  = edit_buf.clone(); let bl2 = Rc::clone(&blocs);
            let list2 = bloc_list.clone(); let sl2 = status_lbl.clone();
            let ac2   = Rc::clone(&active_cat);
            save_btn.connect_clicked(move |_| {
                let s = buf2.start_iter(); let e = buf2.end_iter();
                let content = buf2.text(&s, &e, false).to_string();
                if content.trim().is_empty() { sl2.set_text("Nothing to save."); return; }
                let name = content.lines().find(|l| !l.trim().is_empty())
                    .unwrap_or("bloc").trim().chars().take(48).collect::<String>();
                let cat  = ac2.borrow().clone();
                let kind = if content.contains(PLACEHOLDER) { BlocKind::Template } else { BlocKind::Code };
                let bloc = Bloc { name: name.clone(), content, kind, category: cat, tags: vec![], used: 0 };
                list2.append(&make_bloc_row(&bloc));
                bl2.borrow_mut().push(bloc);
                save_blocs(&bl2.borrow());
                sl2.set_text(&format!("Saved '{name}'"));
            });
        }

        // ── Wire: library selection ───────────────────────────────────────────
        {
            let ib2 = insert_btn.clone(); let db2 = del_btn.clone();
            bloc_list.connect_row_selected(move |_, row| {
                let has = row.is_some();
                ib2.set_sensitive(has); db2.set_sensitive(has);
            });
        }

        // ── Wire: double-tap row → insert ─────────────────────────────────────
        {
            let bl2 = Rc::clone(&blocs); let it2 = insert_template.clone();
            let sl2 = status_lbl.clone();
            bloc_list.connect_row_activated(move |_, row| {
                let idx = row.index() as usize;
                let mut b = bl2.borrow_mut();
                if let Some(bloc) = b.get_mut(idx) {
                    it2(&bloc.content.clone());
                    sl2.set_text(&format!("Inserted '{}'", bloc.name));
                    bloc.used += 1;
                    drop(b);
                    save_blocs(&bl2.borrow());
                }
            });
        }

        // ── Wire: insert button ───────────────────────────────────────────────
        {
            let bl2 = Rc::clone(&blocs); let list2 = bloc_list.clone();
            let it2 = insert_template.clone(); let sl2 = status_lbl.clone();
            insert_btn.connect_clicked(move |_| {
                if let Some(row) = list2.selected_row() {
                    let idx = row.index() as usize;
                    let mut b = bl2.borrow_mut();
                    if let Some(bloc) = b.get_mut(idx) {
                        it2(&bloc.content.clone());
                        sl2.set_text(&format!("Inserted '{}'", bloc.name));
                        bloc.used += 1;
                        drop(b);
                        save_blocs(&bl2.borrow());
                    }
                }
            });
        }

        // ── Wire: delete ──────────────────────────────────────────────────────
        {
            let bl2 = Rc::clone(&blocs); let list2 = bloc_list.clone();
            let sl2 = status_lbl.clone();
            del_btn.connect_clicked(move |_| {
                if let Some(row) = list2.selected_row() {
                    let idx = row.index() as usize;
                    let name = bl2.borrow().get(idx).map(|b| b.name.clone()).unwrap_or_default();
                    list2.remove(&row);
                    bl2.borrow_mut().remove(idx);
                    save_blocs(&bl2.borrow());
                    sl2.set_text(&format!("Deleted '{name}'"));
                }
            });
        }

        // ── Wire: search ──────────────────────────────────────────────────────
        {
            let bl2 = Rc::clone(&blocs); let list2 = bloc_list.clone();
            search_entry.connect_changed(move |e| {
                let q = e.text().to_lowercase();
                while let Some(r) = list2.row_at_index(0) { list2.remove(&r); }
                for b in bl2.borrow().iter() {
                    if q.is_empty()
                        || b.name.to_lowercase().contains(&q)
                        || b.category.to_lowercase().contains(&q)
                        || b.content.to_lowercase().contains(&q) {
                        list2.append(&make_bloc_row(b));
                    }
                }
            });
        }

        BlocTab { root, blocs }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Highlight all ‸ placeholder markers in the buffer with the "slot" tag.
fn highlight_slots(buf: &TextBuffer) {
    let start = buf.start_iter(); let end = buf.end_iter();
    buf.remove_tag_by_name("slot", &start, &end);

    let s = buf.start_iter(); let e = buf.end_iter();
    let text = buf.text(&s, &e, false).to_string();
    let ph = PLACEHOLDER;
    // PLACEHOLDER is a multi-byte UTF-8 character (‸, U+2038, 3 bytes).
    // TextBuffer::iter_at_offset takes CHAR (Unicode scalar) offsets, not byte offsets.
    // We must convert byte positions to char counts.
    let ph_char_len = ph.chars().count() as i32;
    let mut search_from_byte = 0usize;
    while let Some(byte_pos) = text[search_from_byte..].find(ph) {
        let abs_byte = search_from_byte + byte_pos;
        // Convert byte offset → char offset
        let char_offset = text[..abs_byte].chars().count() as i32;
        let tag_start = buf.iter_at_offset(char_offset);
        let tag_end   = buf.iter_at_offset(char_offset + ph_char_len);
        buf.apply_tag_by_name("slot", &tag_start, &tag_end);
        search_from_byte = abs_byte + ph.len();
    }
}

fn rebuild_chips(
    row:      &GtkBox,
    sugs:     &[Suggestion],
    edit_buf: &TextBuffer,
    insert:   &Rc<dyn Fn(&str)>,
) {
    while let Some(c) = row.first_child() { row.remove(&c); }
    if sugs.is_empty() {
        let lbl = Label::new(Some("No suggestions yet"));
        lbl.add_css_class("bloc-chip-placeholder");
        row.append(&lbl);
        return;
    }
    for s in sugs.iter().take(14) {
        row.append(&make_chip(s, edit_buf, insert));
    }
    if sugs.len() > 14 {
        let m = Label::new(Some(&format!("+{}", sugs.len()-14)));
        m.add_css_class("bloc-chip-more");
        row.append(&m);
    }
}

fn make_chip(sug: &Suggestion, edit_buf: &TextBuffer, insert: &Rc<dyn Fn(&str)>) -> gtk4::Widget {
    let chip = GtkBox::new(Orientation::Vertical, 2);
    chip.add_css_class("bloc-chip");

    // Has placeholders? Show ‸ indicator
    let has_slots = sug.text.contains(PLACEHOLDER);
    if has_slots { chip.add_css_class("bloc-chip-template"); }

    chip.set_tooltip_text(Some(&format!("{}\n{}", sug.detail, sug.text.chars().take(120).collect::<String>())));

    let top = GtkBox::new(Orientation::Horizontal, 4);
    let label_text = if has_slots {
        format!("{} ‸", sug.label.chars().take(24).collect::<String>())
    } else {
        sug.label.chars().take(28).collect()
    };
    let name = Label::builder().label(&label_text)
        .halign(gtk4::Align::Start).ellipsize(gtk4::pango::EllipsizeMode::End).build();
    name.add_css_class("bloc-chip-text");
    let badge = Label::new(Some(sug.source.badge()));
    badge.add_css_class("bloc-chip-badge");
    badge.add_css_class(sug.source.badge_css());
    top.append(&name); top.append(&badge);
    chip.append(&top);

    // Tap: insert (and jump to first slot if template)
    let text = sug.text.clone();
    let chip2 = chip.clone();
    let ins = insert.clone();
    let tap = gtk4::GestureClick::new();
    tap.connect_pressed(move |_, _n, _, _| {
        ins(&text);
        chip2.add_css_class("bloc-chip-flash");
        let c = chip2.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(150),
            move || { c.remove_css_class("bloc-chip-flash"); });
    });
    chip.add_controller(tap);

    // Long-press: show full text popover with Save button
    {
        let full_text = sug.text.clone();
        let label2    = sug.label.clone();
        let detail2   = sug.detail.clone();
        let chip3     = chip.clone();
        let lp = gtk4::GestureLongPress::new();
        lp.set_delay_factor(1.5);
        lp.connect_pressed(move |_, _x, _y| {
            let popover = Popover::new();
            let vbox = GtkBox::new(Orientation::Vertical, 8);
            vbox.set_margin_start(12); vbox.set_margin_end(12);
            vbox.set_margin_top(10);   vbox.set_margin_bottom(10);

            let title = Label::builder().label(&label2)
                .halign(gtk4::Align::Start).build();
            title.add_css_class("heading");
            let detail_lbl = Label::builder().label(&detail2)
                .halign(gtk4::Align::Start).build();
            detail_lbl.add_css_class("dim-label");

            // Show the text in a read-only monospace view
            let preview_buf = TextBuffer::new(None);
            preview_buf.set_text(&full_text);
            let preview = TextView::builder().buffer(&preview_buf)
                .editable(false).monospace(true)
                .wrap_mode(WrapMode::WordChar).build();
            preview.add_css_class("bloc-chip-preview");
            let preview_scroll = ScrolledWindow::builder()
                .child(&preview)
                .min_content_height(80).max_content_height(200).build();

            vbox.append(&title);
            vbox.append(&detail_lbl);
            vbox.append(&preview_scroll);
            popover.set_child(Some(&vbox));
            popover.set_parent(&chip3);
            popover.popup();
        });
        chip.add_controller(lp);
    }

    chip.upcast::<gtk4::Widget>()
}

fn make_bloc_row(bloc: &Bloc) -> ListBoxRow {
    let row_box = GtkBox::new(Orientation::Horizontal, 8);
    row_box.set_margin_start(10); row_box.set_margin_end(10);
    row_box.set_margin_top(7);    row_box.set_margin_bottom(7);

    let text_col = GtkBox::new(Orientation::Vertical, 2);
    text_col.set_hexpand(true);

    let name_lbl = Label::builder().label(&bloc.name)
        .halign(gtk4::Align::Start).hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::End).build();
    name_lbl.add_css_class("bloc-row-name");

    let preview = bloc.content.lines()
        .find(|l| !l.trim().is_empty()).unwrap_or("")
        .trim().chars().take(60).collect::<String>();
    let preview_lbl = Label::builder().label(&preview)
        .halign(gtk4::Align::Start)
        .ellipsize(gtk4::pango::EllipsizeMode::End).build();
    preview_lbl.add_css_class("dim-label");
    preview_lbl.add_css_class("bloc-row-preview");

    text_col.append(&name_lbl);
    text_col.append(&preview_lbl);

    let meta = GtkBox::new(Orientation::Vertical, 2);
    meta.set_halign(gtk4::Align::End);
    let kind_lbl = Label::new(Some(bloc.kind.label()));
    kind_lbl.add_css_class("bloc-kind-badge");
    let used_lbl = Label::new(Some(&format!("{}×", bloc.used)));
    used_lbl.add_css_class("dim-label");

    // Show ‸ indicator if template
    if bloc.content.contains(PLACEHOLDER) {
        let slot_lbl = Label::new(Some("‸"));
        slot_lbl.add_css_class("bloc-slot-indicator");
        meta.append(&slot_lbl);
    }
    meta.append(&kind_lbl);
    meta.append(&used_lbl);

    row_box.append(&text_col);
    row_box.append(&meta);

    let row = ListBoxRow::new();
    row.set_child(Some(&row_box));
    row
}

