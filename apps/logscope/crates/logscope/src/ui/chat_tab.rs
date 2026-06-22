//! chat_tab.rs — multi-turn Ollama chat with:
//!   • SQLite-backed session persistence (conversation_db)
//!   • Memory-pressure-aware model switching (model_selector)
//!   • Context injection from peek selection zone + snippet log
//!   • File tokenization via compress_for_context
//!   • Sub-session delegate model for quick tasks

use gtk4::prelude::*;
use gtk4::gio;
use gtk4::{
    Box as GtkBox, Button, Entry, Label,
    Orientation, PolicyType, ScrolledWindow,
    TextView, TextBuffer, WrapMode,
};

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use crossbeam_channel::unbounded;

use crate::conversation_db::{ConversationDb, Role as DbRole};
use crate::model_selector::{
    ModelInfo, ModelSelector, MemPressure,
    compress_for_context, estimate_tokens, summarize_history,
};

// ── in-memory message (superset of DbRole) ────────────────────────────────────

#[derive(Clone, Debug)]
enum Role { User, Assistant, System }

#[derive(Clone, Debug)]
struct Message {
    role:    Role,
    content: String,
    model:   String,
}

impl Message {
    #[allow(dead_code)]
    fn to_db_role(&self) -> DbRole {
        match self.role {
            Role::User      => DbRole::User,
            Role::Assistant => DbRole::Assistant,
            Role::System    => DbRole::System,
        }
    }
}

// ── pending context (from peek selection / snippets) ──────────────────────────

#[derive(Clone, Debug)]
pub struct PendingContext {
    pub label:   String,  // e.g. "Peek selection (lines 42–58)"
    pub content: String,  // the actual text
}

// ── public struct ─────────────────────────────────────────────────────────────

pub struct ChatTab {
    pub root:    GtkBox,
    cancel:      Arc<AtomicBool>,
    history:     Rc<RefCell<Vec<Message>>>,
    msg_list:    GtkBox,
    chat_scroll: ScrolledWindow,
    host_entry:  gtk4::Entry,
    model_pstore:   crate::ui::components::ProviderStore,
    model_drop:     gtk4::DropDown,
    delegate_pstore: crate::ui::components::ProviderStore,
    delegate_drop:  gtk4::DropDown,
    pub pending_context:  Rc<RefCell<Vec<PendingContext>>>,
    /// Base64 images queued to send with the next message (vision models)
    pub pending_images:   Rc<RefCell<Vec<String>>>,
    db:          Option<ConversationDb>,
    session_id:  Rc<RefCell<Option<i64>>>,
    selector:    Rc<RefCell<ModelSelector>>,
    ctx_bar:     GtkBox,
    img_bar:     GtkBox,   // vision image attachment bar
    /// Shared speed table — written by streaming threads, read by model selector
    #[allow(dead_code)]
    pub speed_table: crate::model_selector::SpeedTable,
}

impl ChatTab {
    pub fn new() -> Self {
        let cancel   = Arc::new(AtomicBool::new(false));
        let root     = GtkBox::new(Orientation::Vertical, 0);
        let history  = Rc::new(RefCell::new(Vec::<Message>::new()));
        let pending_context: Rc<RefCell<Vec<PendingContext>>> = Rc::new(RefCell::new(Vec::new()));
        let pending_images:  Rc<RefCell<Vec<String>>>           = Rc::new(RefCell::new(Vec::new()));
        let session_id: Rc<RefCell<Option<i64>>> = Rc::new(RefCell::new(None));

        // Open DB (non-fatal if unavailable)
        let db = ConversationDb::open().ok();

        // Initial model selector with empty set (populated after LAN scan)
        let selector = Rc::new(RefCell::new(ModelSelector::new(vec![])));

        // Shared speed table — measures realtime tokens/sec per model+host
        let speed_table = crate::model_selector::new_speed_table();

        // ── Session bar ───────────────────────────────────────────────────
        let session_bar = gtk4::ActionBar::new();

        let session_lbl = Label::builder()
            .label("New session").hexpand(true).halign(gtk4::Align::Start).build();
        session_lbl.add_css_class("dim-label");
        session_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        session_bar.set_center_widget(Some(&session_lbl));

        let new_session_btn = Button::from_icon_name("document-new-symbolic");
        new_session_btn.add_css_class("flat");
        new_session_btn.set_tooltip_text(Some("New session"));

        let sessions_btn = Button::from_icon_name("document-open-symbolic");
        sessions_btn.add_css_class("flat");
        sessions_btn.set_tooltip_text(Some("Load session"));

        let save_btn = Button::from_icon_name("document-save-symbolic");
        save_btn.add_css_class("flat");
        save_btn.set_tooltip_text(Some("Save session to DB"));

        let mem_lbl = Label::new(Some(""));
        mem_lbl.add_css_class("chat-mem-label");
        mem_lbl.set_tooltip_text(Some("Memory pressure"));

        // DB status — shows path + size so users know where data lives
        let db_path_str = {
            let p = crate::conversation_db::pub_db_path();
            format!("DB: {}", p.display())
        };
        let db_lbl = Label::new(Some("DB: …"));
        db_lbl.add_css_class("dim-label");
        db_lbl.set_tooltip_text(Some(&db_path_str));
        db_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::Start);
        // Refresh DB size label on session bar
        {
            let db_path_clone = crate::conversation_db::pub_db_path();
            let lbl = db_lbl.clone();
            glib::timeout_add_seconds_local(5, move || {
                let size = db_path_clone.metadata().map(|m| m.len()).unwrap_or(0);
                if size > 0 {
                    lbl.set_text(&format!("DB: {}", humansize::format_size(size, humansize::DECIMAL)));
                } else {
                    lbl.set_text("DB: (empty)");
                }
                glib::ControlFlow::Continue
            });
        }

        session_bar.pack_start(&new_session_btn);
        session_bar.pack_start(&sessions_btn);
        session_bar.pack_start(&save_btn);
        session_bar.pack_end(&mem_lbl);
        session_bar.pack_end(&db_lbl);
        root.append(&session_bar);

        // ── Host + model bar ──────────────────────────────────────────────
        let top_bar = GtkBox::new(Orientation::Horizontal, 4);
        top_bar.set_margin_start(8); top_bar.set_margin_end(8);
        top_bar.set_margin_top(2);   top_bar.set_margin_bottom(2);

        // Load saved preferences (host, primary model, delegate model)
        let (saved_host, saved_primary, saved_delegate) = load_prefs();

        let host_entry = Entry::builder()
            .text(&saved_host)
            .hexpand(true)
            .max_width_chars(32)
            .placeholder_text("http://host:11434")
            .tooltip_text("Ollama host URL. Press Enter to fetch models.\nFor remote machines: set OLLAMA_HOST=0.0.0.0 on that machine first.")
            .build();
        host_entry.add_css_class("chat-host-entry");

        let model_pstore = crate::ui::components::ProviderStore::new();
        let model_drop   = crate::ui::components::build_model_dropdown(&model_pstore);
        model_drop.set_tooltip_text(Some("Primary model — for planning"));
        model_drop.set_hexpand(false);

        let delegate_pstore = crate::ui::components::ProviderStore::new();
        let delegate_drop   = crate::ui::components::build_model_dropdown(&delegate_pstore);
        delegate_drop.set_tooltip_text(Some("Delegate model — small/fast, for expert passes & synthesis"));
        delegate_drop.set_hexpand(false);

        let scan_btn = Button::from_icon_name("network-wireless-symbolic");
        scan_btn.add_css_class("flat");
        scan_btn.set_tooltip_text(Some(
            "Scan LAN for Ollama instances (port 11434).\n\
             Remote machines must have OLLAMA_HOST=0.0.0.0 set — \
             the default 127.0.0.1 binding is not reachable over the network."
        ));

        let sep = gtk4::Separator::new(Orientation::Vertical);
        sep.set_margin_top(6); sep.set_margin_bottom(6);
        top_bar.append(&host_entry);
        top_bar.append(&model_drop);
        top_bar.append(&sep);
        top_bar.append(&delegate_drop);
        top_bar.append(&scan_btn);
        root.append(&top_bar);
        root.append(&gtk4::Separator::new(Orientation::Horizontal));

        // ── Pending context bar ───────────────────────────────────────────
        // Shows when context items are queued for injection
        let ctx_bar = GtkBox::new(Orientation::Horizontal, 6);
        ctx_bar.set_margin_start(8); ctx_bar.set_margin_end(8);
        ctx_bar.set_margin_top(4);   ctx_bar.set_margin_bottom(4);
        ctx_bar.set_visible(false);

        let ctx_icon = Label::new(Some("📎"));
        let ctx_lbl  = Label::builder().hexpand(true).halign(gtk4::Align::Start).build();
        ctx_lbl.add_css_class("chat-ctx-label");
        let ctx_clear = Button::from_icon_name("edit-clear-symbolic");
        ctx_clear.add_css_class("flat");
        ctx_clear.set_tooltip_text(Some("Clear queued context"));

        ctx_bar.append(&ctx_icon);
        ctx_bar.append(&ctx_lbl);
        ctx_bar.append(&ctx_clear);
        root.append(&ctx_bar);

        // ── Image attachment bar (shown for vision models) ────────────────
        let img_bar = GtkBox::new(Orientation::Horizontal, 6);
        img_bar.set_margin_start(10); img_bar.set_margin_end(10);
        img_bar.set_margin_top(4);    img_bar.set_margin_bottom(2);
        img_bar.set_visible(false);
        img_bar.add_css_class("chat-img-bar");

        let attach_btn     = gtk4::Button::from_icon_name("image-x-generic-symbolic");
        let screenshot_btn = gtk4::Button::from_icon_name("camera-photo-symbolic");
        let area_shot_btn  = gtk4::Button::from_icon_name("edit-select-all-symbolic");
        let img_count_lbl  = gtk4::Label::new(Some("No images"));
        let clear_img_btn  = gtk4::Button::from_icon_name("edit-clear-symbolic");
        for b in &[&attach_btn, &screenshot_btn, &area_shot_btn, &clear_img_btn] {
            b.add_css_class("flat");
        }
        attach_btn.set_tooltip_text(Some("Attach image file"));
        screenshot_btn.set_tooltip_text(Some("Full screenshot"));
        area_shot_btn.set_tooltip_text(Some("Area screenshot"));
        clear_img_btn.set_tooltip_text(Some("Clear images"));
        img_count_lbl.add_css_class("dim-label");
        img_count_lbl.set_hexpand(true);
        img_bar.append(&attach_btn);
        img_bar.append(&screenshot_btn);
        img_bar.append(&area_shot_btn);
        img_bar.append(&img_count_lbl);
        img_bar.append(&clear_img_btn);
        root.append(&img_bar);

        // ── Chat scroll area ──────────────────────────────────────────────
        let msg_list = GtkBox::new(Orientation::Vertical, 8);
        msg_list.set_margin_start(10); msg_list.set_margin_end(10);
        msg_list.set_margin_top(12);  msg_list.set_margin_bottom(12);
        // Don't vexpand the list itself — the ScrolledWindow handles sizing
        msg_list.set_vexpand(false);

        let chat_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)   // ScrolledWindow expands, not the list inside
            .child(&msg_list)
            .build();
        // Ensure the scroll window never shrinks below a usable minimum
        chat_scroll.set_min_content_height(120);
        root.append(&chat_scroll);

        root.append(&gtk4::Separator::new(Orientation::Horizontal));

        // ── Input area ────────────────────────────────────────────────────
        let input_col = GtkBox::new(Orientation::Vertical, 4);
        input_col.set_margin_start(8); input_col.set_margin_end(8);
        input_col.set_margin_top(6);   input_col.set_margin_bottom(6);

        let input_buf  = TextBuffer::new(None);
        let input_view = TextView::with_buffer(&input_buf);
        input_view.set_wrap_mode(WrapMode::WordChar);
        input_view.set_editable(true);
        input_view.add_css_class("chat-input");
        input_view.set_height_request(68);

        let btns_row = GtkBox::new(Orientation::Horizontal, 4);

        let token_lbl  = Label::new(Some("0 tokens"));
        token_lbl.add_css_class("chat-token-lbl");

        let clear_btn  = Button::with_label("Clear");
        clear_btn.add_css_class("flat");
        let export_btn = Button::with_label("Export");
        export_btn.add_css_class("flat");
        let spinner    = gtk4::Spinner::new();
        spinner.set_valign(gtk4::Align::Center);
        let stop_btn   = Button::with_label("■");
        stop_btn.add_css_class("flat");
        stop_btn.set_visible(false);
        let send_btn   = Button::with_label("Send ↵");
        send_btn.add_css_class("suggested-action");
        send_btn.set_hexpand(true);

        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        btns_row.append(&token_lbl);
        btns_row.append(&clear_btn);
        btns_row.append(&export_btn);
        btns_row.append(&spacer);
        btns_row.append(&spinner);
        btns_row.append(&stop_btn);
        btns_row.append(&send_btn);

        input_col.append(&input_view);
        input_col.append(&btns_row);
        root.append(&input_col);

        // ── Wire: token counter ───────────────────────────────────────────
        {
            let lbl = token_lbl.clone();
            let hist = Rc::clone(&history);
            input_buf.connect_changed(move |buf| {
                let s = buf.start_iter(); let e = buf.end_iter();
                let text = buf.text(&s, &e, false);
                let input_tokens  = estimate_tokens(&text);
                let history_tokens: usize = hist.borrow().iter()
                    .map(|m| estimate_tokens(&m.content)).sum();
                lbl.set_text(&format!("~{} / hist ~{}",
                    input_tokens, history_tokens));
            });
        }

        // ── Wire: ctx_clear ───────────────────────────────────────────────
        {
            let pc = Rc::clone(&pending_context);
            let cb = ctx_bar.clone();
            ctx_clear.connect_clicked(move |_| {
                pc.borrow_mut().clear();
                cb.set_visible(false);
            });
        }

        // ── Wire: primary model changed → update selector ─────────────────
        {
            let sel = Rc::clone(&selector);
            let mps = model_pstore.clone();
            let he2 = host_entry.clone();
            let dps = delegate_pstore.clone();
            let dd2 = delegate_drop.clone();
            let ib_vision = img_bar.clone();
            model_drop.connect_notify_local(Some("selected"), move |md, _| {
                if mps.updating.get() { return; }
                if let Some(e) = mps.selected_entry(md) {
                    sel.borrow_mut().set_primary_by_name(&e.model);
                    let delegate_name = dps.string_list.string(dd2.selected())
                        .map(|s| s.to_string()).unwrap_or_default();
                    save_prefs(&he2.text(), &e.store_key(), &delegate_name);
                    // Show image bar only for vision-capable models
                    ib_vision.set_visible(crate::ui::ollama::is_vision_model(&e.model));
                }
            });
        }

        // ── Wire: delegate model changed → update selector + save prefs ───
        {
            let sel = Rc::clone(&selector);
            let dps = delegate_pstore.clone();
            let mps2 = model_pstore.clone();
            let md2 = model_drop.clone();
            let he2 = host_entry.clone();
            delegate_drop.connect_notify_local(Some("selected"), move |dd, _| {
                if dps.updating.get() { return; }
                if let Some(e) = dps.selected_entry(dd) {
                    sel.borrow_mut().set_delegate_by_name(&e.model);
                    let primary_name = mps2.string_list.string(md2.selected())
                        .map(|s| s.to_string()).unwrap_or_default();
                    save_prefs(&he2.text(), &primary_name, &e.store_key());
                }
            });
        }

        // Save prefs when host changes too
        {
            let md2 = model_drop.clone(); let mps2 = model_pstore.clone();
            let dd2 = delegate_drop.clone(); let dps2 = delegate_pstore.clone();
            host_entry.connect_changed(move |he| {
                let p = mps2.string_list.string(md2.selected()).map(|s|s.to_string()).unwrap_or_default();
                let d = dps2.string_list.string(dd2.selected()).map(|s|s.to_string()).unwrap_or_default();
                save_prefs(&he.text(), &p, &d);
            });
        }

        // Re-fetch models when user presses Enter in the host entry
        {
            let ms  = model_pstore.clone();
            let md  = model_drop.clone();
            let ds  = delegate_pstore.clone();
            let dd  = delegate_drop.clone();
            let sel = Rc::clone(&selector);
            let ml  = mem_lbl.clone();
            host_entry.connect_activate(move |he| {
                let host = he.text().to_string();
                he.remove_css_class("error");
                // Show "…" while fetching
                while ms.string_list.n_items() > 0 { ms.string_list.remove(ms.string_list.n_items()-1); }
                ms.string_list.append("connecting…");
                let ms2=ms.clone(); let md2=md.clone();
                let ds2=ds.clone(); let dd2=dd.clone();
                let sel2=Rc::clone(&sel); let ml2=ml.clone();
                let he2=he.clone();
                let (tx, rx) = unbounded::<Result<Vec<ModelInfo>, String>>();
                std::thread::spawn(move || {
                    let models = discover_models(&host);
                    if models.is_empty() {
                        let _ = tx.send(Err(format!("No Ollama found at {host}")));
                    } else {
                        let _ = tx.send(Ok(models));
                    }
                });
                glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                    match rx.try_recv() {
                        Ok(Ok(models)) => {
                            he2.remove_css_class("error");
                            // preserve selection — populate_model_store handles it
                            populate_provider_store(&ms2, &md2, &models);
                            populate_provider_store(&ds2, &dd2, &models);
                            *sel2.borrow_mut() = ModelSelector::new(models);
                            update_mem_label(&ml2);
                            glib::ControlFlow::Break
                        }
                        Ok(Err(e)) => {
                            he2.add_css_class("error");
                            while ms2.string_list.n_items() > 0 { ms2.string_list.remove(ms2.string_list.n_items()-1); }
                            ms2.string_list.append(&format!("⚠ {e}"));
                            glib::ControlFlow::Break
                        }
                        Err(crossbeam_channel::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(_) => glib::ControlFlow::Break,
                    }
                });
            });
        }

        // ── Wire: scan LAN ────────────────────────────────────────────────
        {
            let ms  = model_pstore.clone();
            let md  = model_drop.clone();
            let he  = host_entry.clone();
            let sel = Rc::clone(&selector);
            let ml  = mem_lbl.clone();
            let btn = scan_btn.clone();
            let ds  = delegate_pstore.clone();
            let dd  = delegate_drop.clone();
            let saved_primary2  = saved_primary.clone();
            let saved_delegate2 = saved_delegate.clone();
            scan_btn.connect_clicked(move |_| {
                btn.set_sensitive(false);
                let current_host = he.text().to_string();

                // Shared accumulator for incremental merge
                let acc: Rc<RefCell<Vec<ModelInfo>>> = Rc::new(RefCell::new(Vec::new()));

                // Channel carries Vec<ModelInfo> — one batch per host as it responds
                let (tx, rx) = unbounded::<Vec<ModelInfo>>();

                // Seed with current host first (no LAN delay)
                let tx2 = tx.clone();
                let ch  = current_host.clone();
                std::thread::spawn(move || {
                    let local = discover_models(&ch);
                    if !local.is_empty() { let _ = tx2.send(local); }
                    scan_lan_ollama_streaming(tx2);
                });

                let ms2=ms.clone(); let md2=md.clone();
                let sel2=Rc::clone(&sel); let ml2=ml.clone(); let btn2=btn.clone();
                let ds2=ds.clone(); let dd2=dd.clone();
                let acc2 = Rc::clone(&acc);
                let sp2  = saved_primary2.clone();
                let sdel2= saved_delegate2.clone();

                glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
                    let recency = load_recency();
                    let mut got_any = false;
                    let mut scan_done = false;

                    // Drain all pending batches
                    loop {
                        match rx.try_recv() {
                            Ok(batch) => {
                                // Record new LAN hosts into providers DB
                                if let Ok(reg) = crate::providers::ProviderRegistry::open() {
                                    for m in &batch {
                                        if !m.host.is_empty() {
                                            let prefix = crate::providers::local_network_prefix();
                                            let _ = reg.record_host(&m.host, "", &prefix);
                                        }
                                    }
                                }
                                merge_and_sort(&mut acc2.borrow_mut(), batch, &recency);
                                got_any = true;
                            }
                            Err(crossbeam_channel::TryRecvError::Empty) => break,
                            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                                scan_done = true; break;
                            }
                        }
                    }

                    if got_any {
                        let models = acc2.borrow().clone();
                        populate_provider_store(&ms2, &md2, &models);
                        populate_provider_store(&ds2, &dd2, &models);
                        restore_selection(&md2, &ms2.string_list, &sp2);
                        if !sdel2.is_empty() { restore_selection(&dd2, &ds2.string_list, &sdel2); }
                        else if models.len() > 1 { dd2.set_selected((models.len()-1) as u32); }
                        *sel2.borrow_mut() = ModelSelector::new(models);
                        update_mem_label(&ml2);
                    }

                    if scan_done {
                        btn2.set_sensitive(true);
                        glib::ControlFlow::Break
                    } else {
                        glib::ControlFlow::Continue
                    }
                });
            });
        }

        // Initial model fetch — local immediately, then background LAN scan
        {
            let ms   = model_pstore.clone();
            let md   = model_drop.clone();
            let ds   = delegate_pstore.clone();
            let dd   = delegate_drop.clone();
            let sel  = Rc::clone(&selector);
            let ml   = mem_lbl.clone();
            let host = saved_host.clone();
            let sp   = saved_primary.clone();
            let sdel = saved_delegate.clone();

            // Channel sends Vec<ModelInfo> — one batch per host as it responds
            let (tx, rx) = unbounded::<Vec<ModelInfo>>();
            // std::thread: this blocks for ~1-2s during LAN scan
            std::thread::spawn(move || {
                // 1. Local host first (fast)
                if let Ok(reg) = crate::providers::ProviderRegistry::open() {
                    let local = crate::providers::probe_host(&reg, &host);
                    if !local.is_empty() { let _ = tx.send(local); }
                    // 2. Previously discovered LAN hosts
                    for kh in reg.known_hosts() {
                        if kh.url == host { continue; }
                        if !kh.is_recent() { continue; }
                        let models = crate::providers::probe_host(&reg, &kh.url);
                        if !models.is_empty() { let _ = tx.send(models); }
                    }
                } else {
                    let local = discover_models(&host);
                    if !local.is_empty() { let _ = tx.send(local); }
                }
                // 3. Background LAN scan
                scan_lan_ollama_streaming(tx);
            });

            let acc: Rc<RefCell<Vec<ModelInfo>>> = Rc::new(RefCell::new(Vec::new()));
            // Pre-read once; avoid re-reading /proc on every batch
            let lan_prefix = crate::providers::local_network_prefix();

            glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
                let recency = load_recency();
                let mut scan_done = false;
                let mut got_new = false;
                let mut new_hosts: Vec<String> = Vec::new();

                loop {
                    match rx.try_recv() {
                        Ok(batch) => {
                            for m in &batch {
                                if !m.host.is_empty() { new_hosts.push(m.host.clone()); }
                            }
                            merge_and_sort(&mut acc.borrow_mut(), batch, &recency);
                            got_new = true;
                        }
                        Err(crossbeam_channel::TryRecvError::Empty) => break,
                        Err(crossbeam_channel::TryRecvError::Disconnected) => { scan_done = true; break; }
                    }
                }
                // Write new hosts to DB once per timer tick, not per model
                if !new_hosts.is_empty() {
                    if let Ok(reg) = crate::providers::ProviderRegistry::open() {
                        for host in new_hosts {
                            let _ = reg.record_host(&host, "", &lan_prefix);
                        }
                    }
                }

                // Only update UI when new data arrived — don't hammer dropdowns every tick
                if got_new {
                    let models = acc.borrow().clone();
                    if !models.is_empty() {
                        populate_provider_store(&ms, &md, &models);
                        populate_provider_store(&ds, &dd, &models);
                        restore_selection(&md, &ms.string_list, &sp);
                        if !sdel.is_empty() { restore_selection(&dd, &ds.string_list, &sdel); }
                        else if models.len() > 1 { dd.set_selected((models.len()-1) as u32); }
                        *sel.borrow_mut() = ModelSelector::new(models);
                        update_mem_label(&ml);
                    }
                }

                if scan_done { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
            });
        }

        // Periodic memory pressure check — every 10s
        {
            let sel2 = Rc::clone(&selector);
            let ml2  = mem_lbl.clone();
            let md2  = model_drop.clone();
            glib::timeout_add_seconds_local(10, move || {
                let pressure = crate::model_selector::read_mem_pressure();
                let changed = sel2.borrow_mut().adapt_to_pressure(pressure);
                if changed {
                    let primary = sel2.borrow().primary_idx;
                    md2.set_selected(primary as u32);
                }
                update_mem_label(&ml2);
                glib::ControlFlow::Continue
            });
        }

        // Poll /api/ps every 30s to update which models are loaded in VRAM
        {
            let st2 = speed_table.clone();
            let he2 = host_entry.clone();
            glib::timeout_add_seconds_local(30, move || {
                let host = he2.text().to_string();
                let st3  = st2.clone();
                std::thread::spawn(move || {
                    let url = format!("{host}/api/ps");
                    if let Ok(resp) = ureq::get(&url)
                        .timeout(std::time::Duration::from_secs(5)).call()
                    {
                        if let Ok(body) = resp.into_json::<serde_json::Value>() {
                            crate::model_selector::update_ps_status(&st3, &host, &body);
                        }
                    }
                });
                glib::ControlFlow::Continue
            });
        }

        // ── Wire: send ────────────────────────────────────────────────────
        {
            let hist      = Rc::clone(&history);
            let ml        = msg_list.clone();
            let cs        = chat_scroll.clone();
            let ib        = input_buf.clone();
            let send2     = send_btn.clone();
            let stop2     = stop_btn.clone();
            let spin2     = spinner.clone();
            let he        = host_entry.clone();
            let pc        = Rc::clone(&pending_context);
            let pi        = Rc::clone(&pending_images);
            let ib2_vis   = img_bar.clone();
            let img_lbl   = img_count_lbl.clone();
            let cb        = ctx_bar.clone();
            let sel       = Rc::clone(&selector);
            let session_r = Rc::clone(&session_id);
            let db2       = db.clone();
            let sl        = session_lbl.clone();
            let cancel_ref= Arc::clone(&cancel);
            let st        = speed_table.clone();

            let fire = Rc::new(move || {
                let text = {
                    let s=ib.start_iter(); let e=ib.end_iter();
                    ib.text(&s,&e,false).trim().to_string()
                };
                if text.is_empty() { return; }
                ib.set_text("");

                // Check memory pressure and possibly switch model
                let pressure = crate::model_selector::read_mem_pressure();
                let _ = sel.borrow_mut().adapt_to_pressure(pressure);

                let default_host = he.text().to_string();
                // Resolve model + host — handle @auto (best host) and @specific_host
                let (model, host) = {
                    let sel_ref = sel.borrow();
                    let entry = if pressure == MemPressure::High {
                        sel_ref.delegate().store_key()
                    } else {
                        sel_ref.primary().store_key()
                    };
                    let (m, h) = split_model_host(&entry, &default_host);
                    if h == "__auto__" {
                        // Speed-table auto-select: pick fastest host for this model
                        let candidates: Vec<String> = sel_ref.models.iter()
                            .filter(|mi| mi.name == m && !mi.host.is_empty())
                            .map(|mi| mi.host.clone())
                            .collect();
                        let best = crate::model_selector::best_host_for_model(
                            &st, &m, &candidates, &default_host
                        );
                        (m, best)
                    } else {
                        (m, h)
                    }
                };

                // Build user content: inject pending context first
                let mut full_content = String::new();
                let contexts = pc.borrow().clone();
                if !contexts.is_empty() {
                    for ctx in &contexts {
                        full_content.push_str(&format!("[Context: {}]\n{}\n\n", ctx.label, ctx.content));
                    }
                    pc.borrow_mut().clear();
                    cb.set_visible(false);
                }
                full_content.push_str(&text);

                // Add to history
                hist.borrow_mut().push(Message {
                    role: Role::User, content: full_content.clone(), model: String::new()
                });

                // Show user bubble
                append_bubble(&ml, &full_content, "user", "");
                scroll_to_bottom(&cs);

                // Drain pending images for multimodal send
                let images: Vec<String> = pi.borrow_mut().drain(..).collect();
                if !images.is_empty() {
                    ib2_vis.set_visible(false); // hide bar after send
                    img_lbl.set_text("No images");
                }

                // Build Ollama messages array (compresses old history if over budget)
                // For vision: replace the last user message with a multimodal message
                let msgs = if images.is_empty() {
                    build_messages_array(&hist.borrow(), &model, 4096)
                } else {
                    let mut m = build_messages_array(&hist.borrow(), &model, 4096);
                    // Replace last user message with multimodal version
                    if let Some(last) = m.last_mut() {
                        *last = crate::ui::ollama::build_multimodal_message(
                            &full_content, &images
                        );
                    }
                    m
                };

                // Ensure session exists
                let sid = {
                    let mut sid_ref = session_r.borrow_mut();
                    if sid_ref.is_none() {
                        if let Some(db) = &db2 {
                            if let Ok(id) = db.create_session(&text[..text.len().min(60)], &model) {
                                *sid_ref = Some(id);
                                sl.set_text(&text[..text.len().min(40)]);
                            }
                        }
                    }
                    *sid_ref
                };

                // Persist user message
                if let (Some(db), Some(sid)) = (&db2, sid) {
                    let _ = db.append_message(sid, DbRole::User, &full_content, "", 0);
                }

                cancel_ref.store(false, Ordering::Relaxed);
                send2.set_sensitive(false);
                spin2.start();
                stop2.set_visible(true);

                let (reply_lbl, reply_box, md_toggle) = assistant_bubble_placeholder(&ml);
                scroll_to_bottom(&cs);

                let (tx, rx) = unbounded::<crate::ui::ollama::StreamChunk>();
                let gc2 = Arc::clone(&cancel_ref);

                // Route to tiered MoE when:
                //   • memory is HIGH, OR
                //   • history > 20 messages (large context = slow large model)
                let use_tiered = pressure == MemPressure::High
                    || hist.borrow().len() > 20;

                if use_tiered && sel.borrow().models.len() > 1 {
                    // Tiered path: large→plan, small→experts, delegate→synthesis
                    let sel_snap = sel.borrow().clone();
                    let host2    = host.clone();
                    let question = full_content.clone();
                    // Build a brief context summary from recent history
                    let ctx_summary = {
                        let h = hist.borrow();
                        let pairs: Vec<(&str, &str)> = h.iter().rev().take(6)
                            .map(|m| (match m.role {
                                Role::User=>"user", Role::Assistant=>"assistant", Role::System=>"system"
                            }, m.content.as_str()))
                            .collect();
                        crate::model_selector::summarize_history(&pairs)
                    };
                    std::thread::spawn(move || {
                        run_tiered_summary(host2, sel_snap, question, ctx_summary, gc2, tx);
                    });
                } else {
                    // Direct path: single large model via /api/chat
                    let host2 = host.clone(); let model2 = model.clone();
                    let no_cancel = Arc::new(AtomicBool::new(false));
                    let st2 = st.clone();
                    std::thread::spawn(move || {
                        chat_streaming(&host2, &model2, &msgs, &tx, &gc2, &no_cancel, Some(&st2));
                    });
                }

                let hist2      = Rc::clone(&hist);
                let send3      = send2.clone();
                let stop3      = stop2.clone();
                let spin3      = spin2.clone();
                let cs2        = cs.clone();
                let model3     = model.clone();
                let db3        = db2.clone();
                let sid_r      = Rc::clone(&session_r);
                let acc        = Rc::new(RefCell::new(String::new()));
                let reply_box2 = reply_box.clone();
                let md_toggle2 = md_toggle.clone();
                let msg_list2  = ml.clone();
                let fire_r2    = Rc::clone(&fire);

                glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
                    let mut batch = String::new(); let mut done = false;
                    loop {
                        match rx.try_recv() {
                            Ok(crate::ui::ollama::StreamChunk::Token(t)) => batch.push_str(&t),
                            Ok(crate::ui::ollama::StreamChunk::Done{..}) |
                            Ok(crate::ui::ollama::StreamChunk::Cancelled) => { done=true; break; }
                            Ok(crate::ui::ollama::StreamChunk::Error(e)) => {
                                let mut a = acc.borrow_mut();
                                a.push_str(&format!("\n\n⚠ {e}"));
                                reply_lbl.set_text(&*a);
                                done = true; break;
                            }
                            Err(crossbeam_channel::TryRecvError::Empty) => break,
                            Err(_) => { done=true; break; }
                        }
                    }
                    if !batch.is_empty() {
                        let mut a=acc.borrow_mut(); a.push_str(&batch);
                        // Only update UI if the bubble is still in the tree
                        if reply_lbl.parent().is_some() {
                            reply_lbl.set_text(&*a);
                            scroll_if_near_bottom(&cs2);
                        }
                        drop(a);
                    }
                    if done {
                        spin3.stop(); stop3.set_visible(false); send3.set_sensitive(true);
                        let final_text = acc.borrow().clone();
                        hist2.borrow_mut().push(Message {
                            role: Role::Assistant,
                            content: final_text.clone(),
                            model: model3.clone(),
                        });

                        // ── Finalize streaming bubble ──────────────────────
                        // Update raw label to final text (already done by set_text above)
                        // If response has markdown: switch to rendered view + show toggle
                        if crate::ui::markdown::has_markdown(&final_text) {
                            let rendered = crate::ui::markdown::render(&final_text);
                            rendered.set_hexpand(true);
                            // Swap content: remove streaming label, add rendered view
                            if let Some(c) = reply_box2.first_child() { reply_box2.remove(&c); }
                            reply_box2.append(&rendered);
                            let raw_lbl_final = reply_lbl.clone();
                            let rb_final = reply_box2.clone();
                            let ft2 = final_text.clone();
                            md_toggle2.set_visible(true);
                            md_toggle2.set_active(true);
                            md_toggle2.set_label("⊞ Rendered");
                            md_toggle2.connect_toggled(move |t| {
                                if let Some(c) = rb_final.first_child() { rb_final.remove(&c); }
                                if t.is_active() {
                                    t.set_label("⊞ Rendered");
                                    let r = crate::ui::markdown::render(&ft2);
                                    r.set_hexpand(true);
                                    rb_final.append(&r);
                                } else {
                                    t.set_label("⟨/⟩ Source");
                                    rb_final.append(&raw_lbl_final);
                                }
                            });
                        }

                        let m3 = model3.clone();
                        rayon::spawn(move || { record_recency(&m3); });
                        if let (Some(db), Some(sid)) = (&db3, *sid_r.borrow()) {
                            let _ = db.append_message(sid, DbRole::Assistant, &final_text, &model3, 0);
                        }
                        // Regenerate button — re-runs the last user message
                        {
                            let regen_btn = Button::with_label("↺  Regenerate");
                            regen_btn.add_css_class("flat");
                            regen_btn.add_css_class("chat-regen-btn");
                            regen_btn.set_halign(gtk4::Align::Start);
                            regen_btn.set_margin_start(6);
                            regen_btn.set_margin_bottom(2);
                            msg_list2.append(&regen_btn);

                            let hist_r  = Rc::clone(&hist2);
                            let ml_r    = msg_list2.clone();
                            let fire_rg = Rc::clone(&fire_r2);
                            regen_btn.connect_clicked(move |btn| {
                                // Remove self (the regen button)
                                ml_r.remove(btn);
                                // Pop last assistant turn from history
                                {
                                    let mut h = hist_r.borrow_mut();
                                    while h.last().map(|m| m.role == Role::Assistant).unwrap_or(false) {
                                        h.pop();
                                    }
                                }
                                // Remove last assistant bubble card from UI
                                let mut removed = false;
                                while !removed {
                                    match ml_r.last_child() {
                                        Some(w) if w.has_css_class("chat-asst-card") => {
                                            ml_r.remove(&w); removed = true;
                                        }
                                        Some(w) => { ml_r.remove(&w); }
                                        None => break,
                                    }
                                }
                                // Re-fire last user message
                                fire_rg();
                            });
                        }
                        glib::ControlFlow::Break
                    } else { glib::ControlFlow::Continue }
                });
            });

            let fire2 = Rc::clone(&fire);
            send_btn.connect_clicked(move |_| fire2());

            // Enter sends, Shift+Enter newline
            let fire3 = Rc::clone(&fire);
            let key_ctrl = gtk4::EventControllerKey::new();
            key_ctrl.connect_key_pressed(move |_, key, _, mods| {
                if key == gtk4::gdk::Key::Return
                    && !mods.contains(gtk4::gdk::ModifierType::SHIFT_MASK)
                {
                    fire3();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            input_view.add_controller(key_ctrl);
        }

        // ── Wire: image attachment buttons ───────────────────────────────
        {
            let pi  = Rc::clone(&pending_images);
            let lbl = img_count_lbl.clone();
            attach_btn.connect_clicked(move |btn| {
                let dialog = gtk4::FileDialog::builder()
                    .title("Attach Image").build();
                let pi2  = Rc::clone(&pi);
                let lbl2 = lbl.clone();
                let win  = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
                dialog.open(win.as_ref(), None::<&gio::Cancellable>, move |res| {
                    if let Ok(f) = res {
                        if let Some(path) = f.path() {
                            match crate::ui::ollama::load_image_base64(&path) {
                                Ok(b64) => {
                                    pi2.borrow_mut().push(b64);
                                    let n = pi2.borrow().len();
                                    lbl2.set_text(&format!("{n} image{}", if n==1 {""}else{"s"}));
                                }
                                Err(e) => { lbl2.set_text(&format!("Error: {e}")); }
                            }
                        }
                    }
                });
            });
        }
        {
            let pi  = Rc::clone(&pending_images);
            let lbl = img_count_lbl.clone();
            screenshot_btn.connect_clicked(move |_| {
                // Channel carries Result<base64_string, error_string> — both Send
                let (tx, rx) = crossbeam_channel::unbounded::<Result<String,String>>();
                std::thread::spawn(move || {
                    let path = std::env::temp_dir().join("logscope_shot.png");
                    let res = crate::ui::ollama::take_screenshot(&path)
                        .and_then(|()| crate::ui::ollama::load_image_base64(&path));
                    let _ = tx.send(res);
                });
                // Drain on main thread — Rc and GTK only touched here
                let pi2  = Rc::clone(&pi);
                let lbl2 = lbl.clone();
                glib::idle_add_local_once(move || {
                    match rx.try_recv() {
                        Ok(Ok(b64)) => {
                            pi2.borrow_mut().push(b64);
                            let n = pi2.borrow().len();
                            lbl2.set_text(&format!("{n} image{}", if n==1{""}else{"s"}));
                        }
                        Ok(Err(e)) => { lbl2.set_text(&format!("Screenshot: {e}")); }
                        Err(_) => {
                            // Thread not done yet — poll again shortly
                            let pi3 = pi2.clone(); let lbl3 = lbl2.clone();
                            let rx2 = rx.clone();
                            glib::timeout_add_local_once(
                                std::time::Duration::from_millis(100),
                                move || {
                                    match rx2.try_recv() {
                                        Ok(Ok(b64)) => {
                                            pi3.borrow_mut().push(b64);
                                            let n = pi3.borrow().len();
                                            lbl3.set_text(&format!("{n} image{}", if n==1{""}else{"s"}));
                                        }
                                        Ok(Err(e)) => { lbl3.set_text(&format!("Screenshot: {e}")); }
                                        _ => { lbl3.set_text("Screenshot timed out"); }
                                    }
                                }
                            );
                        }
                    }
                });
            });
        }
        {
            let pi  = Rc::clone(&pending_images);
            let lbl = img_count_lbl.clone();
            area_shot_btn.connect_clicked(move |_| {
                let (tx, rx) = crossbeam_channel::unbounded::<Result<String,String>>();
                std::thread::spawn(move || {
                    let path = std::env::temp_dir().join("logscope_area.png");
                    let res = crate::ui::ollama::take_screenshot_area(&path)
                        .and_then(|()| crate::ui::ollama::load_image_base64(&path));
                    let _ = tx.send(res);
                });
                let pi2  = Rc::clone(&pi);
                let lbl2 = lbl.clone();
                // Poll until the area-select tool returns (user may take a few seconds)
                glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                    match rx.try_recv() {
                        Ok(Ok(b64)) => {
                            pi2.borrow_mut().push(b64);
                            let n = pi2.borrow().len();
                            lbl2.set_text(&format!("{n} image{}", if n==1{""}else{"s"}));
                            glib::ControlFlow::Break
                        }
                        Ok(Err(e)) => {
                            lbl2.set_text(&format!("Area shot: {e}"));
                            glib::ControlFlow::Break
                        }
                        Err(crossbeam_channel::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(_) => glib::ControlFlow::Break,
                    }
                });
            });
        }
        {
            let pi  = Rc::clone(&pending_images);
            let lbl = img_count_lbl.clone();
            clear_img_btn.connect_clicked(move |_| {
                pi.borrow_mut().clear();
                lbl.set_text("No images");
            });
        }

        // Stop
        { let c=Arc::clone(&cancel); stop_btn.connect_clicked(move|_|{ c.store(true,Ordering::Relaxed); }); }

        // Clear — also cancels any active streaming
        {
            let hist=Rc::clone(&history); let ml=msg_list.clone();
            let session_r=Rc::clone(&session_id);
            let cancel_c = Arc::clone(&cancel);
            clear_btn.connect_clicked(move|_|{
                cancel_c.store(true, Ordering::Relaxed); // stop any in-flight stream
                hist.borrow_mut().clear();
                while let Some(c)=ml.first_child(){ml.remove(&c);}
                *session_r.borrow_mut() = None;
            });
        }

        // Export
        {
            let hist=Rc::clone(&history);
            export_btn.connect_clicked(move|btn|{
                let md=build_export_markdown(&hist.borrow());
                if let Some(d)=gtk4::gdk::Display::default(){
                    d.clipboard().set_text(&md);
                    btn.set_label("Copied ✓");
                    let b2=btn.clone();
                    glib::timeout_add_seconds_local(2,move||{ b2.set_label("Export"); glib::ControlFlow::Break });
                }
            });
        }

        // New session
        {
            let hist=Rc::clone(&history); let ml=msg_list.clone();
            let sid_r=Rc::clone(&session_id); let sl=session_lbl.clone();
            new_session_btn.connect_clicked(move|_|{
                hist.borrow_mut().clear();
                while let Some(c)=ml.first_child(){ml.remove(&c);}
                *sid_r.borrow_mut()=None;
                sl.set_text("New session");
            });
        }

        // Session browser: popover showing recent sessions
        {
            let db2  = db.clone();
            let hist = Rc::clone(&history);
            let ml   = msg_list.clone();
            let sid_r = Rc::clone(&session_id);
            let sl   = session_lbl.clone();
            sessions_btn.connect_clicked(move |btn| {
                let Some(db) = &db2 else { return };
                let sessions = match db.list_sessions() { Ok(s) => s, Err(_) => return };

                let pop = gtk4::Popover::new();
                let vbox = GtkBox::new(Orientation::Vertical, 4);
                vbox.set_margin_start(8); vbox.set_margin_end(8);
                vbox.set_margin_top(8);   vbox.set_margin_bottom(8);
                vbox.set_width_request(280);

                let title_lbl = Label::new(Some("Recent sessions"));
                title_lbl.add_css_class("heading");
                title_lbl.set_halign(gtk4::Align::Start);
                vbox.append(&title_lbl);
                vbox.append(&gtk4::Separator::new(Orientation::Horizontal));

                if sessions.is_empty() {
                    let empty = Label::new(Some("No saved sessions yet"));
                    empty.add_css_class("dim-label");
                    vbox.append(&empty);
                }

                for sess in sessions.iter().take(15) {
                    let row_box = GtkBox::new(Orientation::Horizontal, 8);
                    let name_lbl = Label::builder()
                        .label(&sess.title)
                        .halign(gtk4::Align::Start)
                        .hexpand(true)
                        .ellipsize(gtk4::pango::EllipsizeMode::End)
                        .build();
                    let date_lbl = Label::builder()
                        .label(&sess.created_at.format("%m/%d %H:%M").to_string())
                        .halign(gtk4::Align::End)
                        .build();
                    date_lbl.add_css_class("dim-label");
                    row_box.append(&name_lbl);
                    row_box.append(&date_lbl);

                    let btn_row = gtk4::Button::new();
                    btn_row.set_child(Some(&row_box));
                    btn_row.add_css_class("flat");
                    btn_row.add_css_class("chat-session-row");
                    vbox.append(&btn_row);

                    // Wire: clicking a session row loads it
                    let sid   = sess.id;
                    let title = sess.title.clone();
                    let db3   = db.clone();
                    let hist2 = Rc::clone(&hist);
                    let ml2   = ml.clone();
                    let sid_r2 = Rc::clone(&sid_r);
                    let sl2   = sl.clone();
                    let pop2  = pop.clone();
                    btn_row.connect_clicked(move |_| {
                        pop2.popdown();
                        if let Ok(msgs) = db3.load_messages(sid) {
                            hist2.borrow_mut().clear();
                            while let Some(c) = ml2.first_child() { ml2.remove(&c); }
                            for m in &msgs {
                                append_bubble(&ml2, &m.content, m.role.as_str(), &m.model);
                                hist2.borrow_mut().push(Message {
                                    role: match m.role {
                                        DbRole::User      => Role::User,
                                        DbRole::Assistant => Role::Assistant,
                                        DbRole::System    => Role::System,
                                    },
                                    content: m.content.clone(),
                                    model:   m.model.clone(),
                                });
                            }
                            *sid_r2.borrow_mut() = Some(sid);
                            sl2.set_text(&title);
                        }
                    });
                }

                let scr = gtk4::ScrolledWindow::builder()
                    .child(&vbox)
                    .max_content_height(400)
                    .propagate_natural_height(true)
                    .build();
                pop.set_child(Some(&scr));
                pop.set_parent(btn);
                pop.popup();
            });
        }

        ChatTab {
            root, cancel, history, msg_list, chat_scroll,
            host_entry, model_pstore, model_drop,
            delegate_pstore, delegate_drop,
            pending_context, pending_images, db, session_id, selector, ctx_bar,
            img_bar: img_bar.clone(),
            speed_table,
        }
    }

    pub fn cancel_all(&self) { self.cancel.store(true, Ordering::Relaxed); }

    /// Re-fetch the model list from the current host — call on window show.
    /// Re-query Ollama for the current model list. Called when the app opens
    /// and whenever the Chat tab becomes visible.
    pub fn refresh_models(&self) {
        let ms  = self.model_pstore.clone();
        let md  = self.model_drop.clone();
        let ds  = self.delegate_pstore.clone();
        let dd  = self.delegate_drop.clone();
        let sel = Rc::clone(&self.selector);
        let host = self.host_entry.text().to_string();
        let (tx, rx) = unbounded::<Vec<crate::model_selector::ModelInfo>>();
        std::thread::spawn(move || { let _ = tx.send(discover_models(&host)); });
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            match rx.try_recv() {
                Ok(models) => {
                    // populate_model_store now preserves the current selection key
                    populate_provider_store(&ms, &md, &models);
                    populate_provider_store(&ds, &dd, &models);
                    *sel.borrow_mut() = crate::model_selector::ModelSelector::new(models);
                    glib::ControlFlow::Break
                }
                Err(crossbeam_channel::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(_) => glib::ControlFlow::Break,
            }
        });
    }

    /// Add a snippet from peek/cargo Ollama sessions into the chat history.
    /// The snippet is stored in the DB and shown as a context card.
    pub fn log_snippet(&self, source: String, response: String) {
        if response.trim().is_empty() { return; }

        // Persist to DB
        if let (Some(db), Some(sid)) = (&self.db, *self.session_id.borrow()) {
            let _ = db.add_context(sid, &source, &response);
        }

        let ctx_msg = Message { role: Role::System,
            content: format!("[Snippet from {source}]: {response}"), model: String::new() };
        self.history.borrow_mut().push(ctx_msg);

        // Show snippet card in UI
        let card = GtkBox::new(Orientation::Vertical, 4);
        card.add_css_class("chat-snippet-card");
        card.set_margin_bottom(4);

        let src_lbl = Label::builder().label(&format!("↪ {source}"))
            .halign(gtk4::Align::Start).build();
        src_lbl.add_css_class("chat-snippet-source");
        card.append(&src_lbl);

        let resp_lbl = Label::builder().label(&response)
            .halign(gtk4::Align::Start).wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar).selectable(true).build();
        resp_lbl.add_css_class("chat-bubble-text");
        card.append(&resp_lbl);

        // "Ask about this" → pre-fills pending context
        let ask_btn = Button::with_label("Ask about this →");
        ask_btn.add_css_class("flat"); ask_btn.add_css_class("chat-snippet-ask-btn");
        {
            let pc  = Rc::clone(&self.pending_context);
            let cb  = self.ctx_bar.clone();
            let rl  = response.clone(); let sl = source.clone();
            ask_btn.connect_clicked(move |_| {
                pc.borrow_mut().push(PendingContext { label: sl.clone(), content: rl.clone() });
                update_ctx_bar(&cb, &pc.borrow());
            });
        }
        card.append(&ask_btn);

        self.msg_list.append(&card);
        scroll_to_bottom(&self.chat_scroll);
    }

    /// Queue a peek selection zone as context for the next message.
    /// Called from window.rs when "Ask about this" is clicked on a locked selection.
    pub fn queue_peek_selection(&self, label: String, content: String) {
        // Compress if very large
        let (compressed, tokens) = compress_for_context(&content, 1500);
        let label2 = if tokens < estimate_tokens(&content) {
            format!("{label} (compressed)")
        } else { label };
        self.pending_context.borrow_mut().push(PendingContext { label: label2, content: compressed });
        update_ctx_bar(&self.ctx_bar, &self.pending_context.borrow());
    }

    /// Queue a file for context injection (tokenized + compressed).
    #[allow(dead_code)]
    pub fn queue_file_context(&self, path: &std::path::Path, max_tokens: usize) {
        if let Ok(text) = std::fs::read_to_string(path) {
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let (compressed, _) = compress_for_context(&text, max_tokens);
            self.pending_context.borrow_mut().push(PendingContext {
                label: format!("File: {name}"), content: compressed,
            });
            update_ctx_bar(&self.ctx_bar, &self.pending_context.borrow());
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn update_ctx_bar(bar: &GtkBox, contexts: &[PendingContext]) {
    if contexts.is_empty() { bar.set_visible(false); return; }
    bar.set_visible(true);
    // Update label — find the Label child
    let mut child = bar.first_child();
    while let Some(c) = child {
        if let Ok(lbl) = c.clone().downcast::<Label>() {
            if lbl.has_css_class("chat-ctx-label") {
                let names: Vec<&str> = contexts.iter().map(|c| c.label.as_str()).collect();
                lbl.set_text(&format!("Context: {}", names.join(", ")));
                return;
            }
        }
        child = c.next_sibling();
    }
}

/// Build Ollama /api/chat messages array, compressing history if needed.
fn build_messages_array(history: &[Message], _model: &str, max_history_tokens: usize) -> Vec<serde_json::Value> {
    let mut total_tokens: usize = 0;
    let mut msgs: Vec<serde_json::Value> = Vec::new();

    // Walk history in reverse to fit within token budget
    let mut rev_msgs: Vec<&Message> = Vec::new();
    for m in history.iter().rev() {
        let t = estimate_tokens(&m.content);
        if total_tokens + t > max_history_tokens && !rev_msgs.is_empty() { break; }
        total_tokens += t;
        rev_msgs.push(m);
    }
    rev_msgs.reverse();

    // If we had to truncate, inject a summary system message
    if rev_msgs.len() < history.len() {
        let omitted = &history[..history.len() - rev_msgs.len()];
        let pairs: Vec<(&str, &str)> = omitted.iter()
            .map(|m| (match m.role { Role::User=>"user", Role::Assistant=>"assistant", Role::System=>"system" }, m.content.as_str()))
            .collect();
        let summary = summarize_history(&pairs);
        msgs.push(serde_json::json!({ "role": "system", "content": summary }));
    }

    for m in &rev_msgs {
        let role = match m.role {
            Role::User      => "user",
            Role::Assistant => "assistant",
            Role::System    => "system",
        };
        msgs.push(serde_json::json!({ "role": role, "content": m.content }));
    }

    msgs
}

/// Build a DropDown with a two-line factory: bold model name + dim host source.
/// The StringList items are stored as "modelname" (local) or "modelname@http://host:port".
/// Split a store key for dropdown display.
/// "llama3:8b@auto"                     → ("llama3:8b",  "⚡ auto")
/// "llama3:8b@http://192.168.1.5:11434" → ("llama3:8b",  "192.168.1.5")
/// "llama3:8b"                           → ("llama3:8b",  "local")
/// Blend local + remote models into a unified list.
///
/// For each unique model name we produce:
///   1. One "@auto" entry (pick best host at send time via speed table)
///   2. One pinned entry per host that has it
///
/// Local-only models just show as plain "name" (no @auto clutter).
/// Merge a fresh batch of models from one host into an existing list,
/// then re-sort the whole thing.
///
/// Sort priority:
///   1. Currently loaded in VRAM (size_vram > 0) — fastest, no cold-start
///   2. Recently used (appears in `recency` list, by position)
///   3. Largest model first (params_m desc) — most capable
///
/// For models available on multiple hosts, generates one @auto virtual entry
/// (pick best host at send time via speed table) plus one pinned entry per host.
fn merge_and_sort(
    existing:  &mut Vec<ModelInfo>,
    new_batch: Vec<ModelInfo>,
    recency:   &[String],
) {
    // Add new models, avoiding exact (name, host) duplicates
    for m in new_batch {
        let key = m.store_key();
        if !existing.iter().any(|e| e.store_key() == key) {
            existing.push(m);
        }
    }

    // Rebuild: group by base name → variants
    use std::collections::HashMap;
    let mut by_name: HashMap<String, Vec<ModelInfo>> = HashMap::new();
    for m in existing.drain(..) {
        by_name.entry(m.name.clone()).or_default().push(m);
    }

    // Score each base name for sorting
    let score_name = |name: &str, variants: &[ModelInfo]| -> (u8, usize, u64) {
        // tier 0 = loaded in VRAM on any host, 1 = not loaded
        let loaded = variants.iter().any(|m| m.size_vram > 0);
        let loaded_tier = if loaded { 0u8 } else { 1u8 };
        // recency position (lower index = more recent)
        let recency_pos = recency.iter().position(|r| r == name).unwrap_or(usize::MAX);
        // largest params_m across variants
        let params = variants.iter().map(|m| m.params_m).max().unwrap_or(0);
        (loaded_tier, recency_pos, std::u64::MAX - params) // invert params for ascending sort
    };

    let mut names: Vec<String> = by_name.keys().cloned().collect();
    names.sort_by_key(|n| score_name(n, &by_name[n]));

    for name in names {
        let variants = by_name.remove(&name).unwrap();
        let multi_host = variants.len() > 1
            || variants.iter().any(|m| !m.host.is_empty());

        if multi_host {
            let mut auto_entry = variants[0].clone();
            auto_entry.name     = name.clone();
            auto_entry.host     = String::new();
            auto_entry.autohost = true;
            // If any variant is loaded, mark auto as loaded too (for display)
            auto_entry.size_vram = variants.iter().map(|m| m.size_vram).max().unwrap_or(0);
            existing.push(auto_entry);
        }
        for mut m in variants {
            m.autohost = false;
            existing.push(m);
        }
    }
}

/// Legacy wrapper — builds from scratch.
fn populate_provider_store(ps: &crate::ui::components::ProviderStore, drop: &gtk4::DropDown, models: &[ModelInfo]) {
    // Group models by host for build_model_entries
        let mut by_host: HashMap<String, Vec<ModelInfo>> = HashMap::new();
    for m in models {
        by_host.entry(m.host.clone()).or_default().push(m.clone());
    }
    let batches: Vec<(String, Vec<ModelInfo>)> = by_host.into_iter().collect();
    let recency = load_recency();
    let entries = crate::providers::build_model_entries(&batches, &recency);
    ps.set_entries(drop, entries);
}

/// Restore a saved model name in a dropdown, falling back to index 0.
fn restore_selection(drop: &gtk4::DropDown, store: &gtk4::StringList, saved: &str) {
    crate::ui::components::restore_selection(drop, store, saved);
}

fn update_mem_label(lbl: &Label) {
    let (text, css) = match crate::model_selector::read_mem_pressure() {
        MemPressure::Low    => ("● mem: ok",   "chat-mem-ok"),
        MemPressure::Medium => ("● mem: mid",  "chat-mem-mid"),
        MemPressure::High   => ("⚠ mem: high", "chat-mem-high"),
    };
    lbl.set_text(text);
    lbl.remove_css_class("chat-mem-ok");
    lbl.remove_css_class("chat-mem-mid");
    lbl.remove_css_class("chat-mem-high");
    lbl.add_css_class(css);
}

fn short_model_name(name: &str) -> &str {
    name.split(':').next().unwrap_or(name)
}

/// Query one Ollama host and return its models.
/// Each model gets two entries in the returned list:
///   1. "modelname" (or "modelname@auto" if there are multiple hosts) — auto-selects best host
///   2. "modelname@http://host:port" — pinned to this specific host
fn discover_models(host: &str) -> Vec<ModelInfo> {
    if let Ok(reg) = crate::providers::ProviderRegistry::open() {
        crate::providers::probe_host(&reg, host)
    } else {
        // Fallback: direct /api/tags query without registry recording
        crate::ui::ollama::list_models(host)
            .unwrap_or_default()
            .into_iter()
            .map(|name| ModelInfo::from_ollama_tag(&name, 0))
            .collect()
    }
}

fn chat_streaming(
    host:        &str,
    model:       &str,
    msgs:        &[serde_json::Value],
    tx:          &crossbeam_channel::Sender<crate::ui::ollama::StreamChunk>,
    gc:          &Arc<AtomicBool>,
    lc:          &Arc<AtomicBool>,
    speed_table: Option<&crate::model_selector::SpeedTable>,
) {
    use std::io::BufRead;
    use std::time::Instant;

    if gc.load(Ordering::Relaxed) || lc.load(Ordering::Relaxed) {
        let _ = tx.send(crate::ui::ollama::StreamChunk::Cancelled);
        return;
    }

    let model = if model.is_empty() { crate::ui::ollama::resolve_model(host) } else { model.to_string() };
    let body = serde_json::json!({ "model": model, "messages": msgs, "stream": true });

    let t_start = Instant::now();
    let resp = match ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .build()
        .post(&format!("{host}/api/chat"))
        .set("Content-Type","application/json")
        .send_string(&body.to_string())
    {
        Ok(r) => r,
        Err(e) => { let _ = tx.send(crate::ui::ollama::StreamChunk::Error(e.to_string())); return; }
    };

    let reader = std::io::BufReader::new(resp.into_reader());
    let mut final_model  = model.to_string();
    let mut ttft_ms      = 0f64;   // time to first token
    let mut token_count  = 0u32;
    let mut first_token  = true;

    for line in reader.lines() {
        if gc.load(Ordering::Relaxed) || lc.load(Ordering::Relaxed) {
            let _ = tx.send(crate::ui::ollama::StreamChunk::Cancelled); return;
        }
        let line = match line {
            Ok(l) => l,
            Err(e) => { let _ = tx.send(crate::ui::ollama::StreamChunk::Error(e.to_string())); return; }
        };
        if line.is_empty() { continue; }
        let json: serde_json::Value = match serde_json::from_str(&line) { Ok(v)=>v, Err(_)=>continue };

        if let Some(tok) = json["message"]["content"].as_str() {
            if !tok.is_empty() {
                if first_token {
                    ttft_ms = t_start.elapsed().as_secs_f64() * 1000.0;
                    first_token = false;
                }
                token_count += tok.chars().count() as u32 / 4 + 1; // rough token estimate
                let _ = tx.send(crate::ui::ollama::StreamChunk::Token(tok.to_owned()));
            }
        }
        if let Some(m) = json["model"].as_str() { final_model = m.to_owned(); }

        if json["done"].as_bool().unwrap_or(false) {
            // Record speed measurement
            let elapsed = t_start.elapsed().as_secs_f64();
            if let Some(st) = speed_table {
                if elapsed > 0.1 && token_count > 2 {
                    let tps = token_count as f64 / elapsed;
                    crate::model_selector::record_speed(st, &final_model, host, ttft_ms, tps);
                }
            }
            let _ = tx.send(crate::ui::ollama::StreamChunk::Done { model: final_model });
            return;
        }
    }
    let _ = tx.send(crate::ui::ollama::StreamChunk::Done { model: final_model });
}

fn append_bubble(list: &GtkBox, text: &str, role: &str, model: &str) {
    let add_gap = list.last_child().map(|w| {
        let is_user = w.has_css_class("chat-user-card");
        (role == "user") != is_user
    }).unwrap_or(false);

    let card = GtkBox::new(Orientation::Vertical, 3);
    card.add_css_class(if role == "user" { "chat-user-card" } else { "chat-asst-card" });
    if add_gap { card.add_css_class("chat-turn-gap"); }
    card.set_halign(if role == "user" { gtk4::Align::End } else { gtk4::Align::Fill });
    card.set_hexpand(true);

    if role == "user" {
        // User bubble: text + copy button in header row
        let hdr = GtkBox::new(Orientation::Horizontal, 4);
        if !model.is_empty() {
            let ml = Label::builder().label(&short_model_name(model).to_uppercase())
                .halign(gtk4::Align::Start).build();
            ml.add_css_class("chat-model-label");
            hdr.append(&ml);
        }
        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        hdr.append(&spacer);
        let copy_btn = gtk4::Button::from_icon_name("edit-copy-symbolic");
        copy_btn.add_css_class("flat"); copy_btn.add_css_class("chat-copy-btn");
        copy_btn.set_tooltip_text(Some("Copy message"));
        let text_copy = text.to_string();
        copy_btn.connect_clicked(move |btn| {
            if let Some(d) = gtk4::gdk::Display::default() { d.clipboard().set_text(&text_copy); }
            btn.set_icon_name("object-select-symbolic");
            let btn2 = btn.clone();
            glib::timeout_add_local_once(std::time::Duration::from_secs(1),
                move || { btn2.set_icon_name("edit-copy-symbolic"); });
        });
        hdr.append(&copy_btn);
        card.append(&hdr);
        let lbl = Label::builder().label(text)
            .halign(gtk4::Align::Start).wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .selectable(true).hexpand(true).build();
        lbl.add_css_class("chat-bubble-text");
        card.append(&lbl);
    } else {
        // Assistant bubble: header row with model name + rendered/source toggle
        let hdr = GtkBox::new(Orientation::Horizontal, 6);
        if !model.is_empty() {
            let ml = Label::builder().label(&short_model_name(model).to_uppercase())
                .halign(gtk4::Align::Start).hexpand(true).build();
            ml.add_css_class("chat-model-label");
            hdr.append(&ml);
        } else {
            let sp = GtkBox::new(Orientation::Horizontal, 0);
            sp.set_hexpand(true);
            hdr.append(&sp);
        }
        let use_md = crate::ui::markdown::has_markdown(text);
        let toggle = gtk4::ToggleButton::builder()
            .label(if use_md { "⊞ Rendered" } else { "⟨/⟩ Source" })
            .active(use_md).build();
        toggle.add_css_class("flat");
        toggle.add_css_class("chat-md-toggle");
        toggle.set_visible(use_md); // only show toggle if content has markdown
        hdr.append(&toggle);
        let copy_btn_a = gtk4::Button::from_icon_name("edit-copy-symbolic");
        copy_btn_a.add_css_class("flat"); copy_btn_a.add_css_class("chat-copy-btn");
        copy_btn_a.set_tooltip_text(Some("Copy response"));
        let text_a = text.to_string();
        copy_btn_a.connect_clicked(move |btn| {
            if let Some(d) = gtk4::gdk::Display::default() { d.clipboard().set_text(&text_a); }
            btn.set_icon_name("object-select-symbolic");
            let btn2 = btn.clone();
            glib::timeout_add_local_once(std::time::Duration::from_secs(1),
                move || { btn2.set_icon_name("edit-copy-symbolic"); });
        });
        hdr.append(&copy_btn_a);
        card.append(&hdr);

        // Content container — swapped by toggle
        let content_box = GtkBox::new(Orientation::Vertical, 0);
        content_box.set_hexpand(true);
        card.append(&content_box);

        // Plain-text view (always built, cheap)
        let raw_lbl = Label::builder().label(text)
            .halign(gtk4::Align::Start).wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .selectable(true).hexpand(true).build();
        raw_lbl.add_css_class("chat-bubble-text");

        if use_md {
            // Start in rendered view
            let rendered = crate::ui::markdown::render(text);
            rendered.set_hexpand(true);
            content_box.append(&rendered);

            let cb2 = content_box.clone();
            let raw2 = raw_lbl.clone();
            let text2 = text.to_string();
            toggle.connect_toggled(move |t| {
                if let Some(c) = cb2.first_child() { cb2.remove(&c); }
                if t.is_active() {
                    t.set_label("⊞ Rendered");
                    // Re-render fresh — avoids keeping two widget trees alive
                    let r = crate::ui::markdown::render(&text2);
                    r.set_hexpand(true);
                    cb2.append(&r);
                } else {
                    t.set_label("⟨/⟩ Source");
                    cb2.append(&raw2);
                }
            });
        } else {
            content_box.append(&raw_lbl);
        }
    }

    list.append(&card);
}

/// A live streaming bubble.  Returns:
///   - `Label`    — the raw text label, updated each token
///   - `GtkBox`   — the content container (replaced on stream completion)
///   - `gtk4::ToggleButton` — the rendered/source toggle (hidden until done)
fn assistant_bubble_placeholder(list: &GtkBox) -> (Label, GtkBox, gtk4::ToggleButton) {
    let card = GtkBox::new(Orientation::Vertical, 2);
    card.add_css_class("chat-asst-card");
    card.set_margin_bottom(4);
    card.set_halign(gtk4::Align::Fill);
    card.set_hexpand(true);

    // Header with model-label placeholder + hidden toggle
    let hdr = GtkBox::new(Orientation::Horizontal, 6);
    let model_lbl = Label::builder().label("").halign(gtk4::Align::Start).hexpand(true).build();
    model_lbl.add_css_class("chat-model-label");
    let toggle = gtk4::ToggleButton::builder().label("⊞ Rendered").active(false).build();
    toggle.add_css_class("flat");
    toggle.add_css_class("chat-md-toggle");
    toggle.set_visible(false); // revealed only if response has markdown
    hdr.append(&model_lbl);
    hdr.append(&toggle);
    card.append(&hdr);

    // Content container
    let content_box = GtkBox::new(Orientation::Vertical, 0);
    content_box.set_hexpand(true);
    card.append(&content_box);

    // Streaming label (dots until tokens arrive)
    let lbl = Label::builder()
        .label("…")
        .halign(gtk4::Align::Start).wrap(true)
        .wrap_mode(gtk4::pango::WrapMode::WordChar)
        .selectable(true).hexpand(true).build();
    lbl.add_css_class("chat-bubble-text");
    content_box.append(&lbl);

    list.append(&card);
    (lbl, content_box, toggle)
}

fn scroll_to_bottom(scroll: &ScrolledWindow) {
    // Queue the scroll after GTK has laid out the new content
    let scroll2 = scroll.clone();
    glib::idle_add_local_once(move || {
        let adj = scroll2.vadjustment();
        adj.set_value(adj.upper() - adj.page_size());
    });
}

/// Only auto-scroll if the user is already near the bottom (within 200px).
/// This prevents the view from hijacking the scroll position when the user
/// is reading earlier messages while a response is streaming in.
fn scroll_if_near_bottom(scroll: &ScrolledWindow) {
    let adj  = scroll.vadjustment();
    let bottom = adj.upper() - adj.page_size();
    if bottom - adj.value() < 200.0 {
        adj.set_value(bottom);
    }
}

fn build_export_markdown(msgs: &[Message]) -> String {
    let mut out = String::from("# Logscope Chat\n\n");
    for msg in msgs {
        let prefix = match msg.role {
            Role::System    => continue,
            Role::User      => "**You**".to_string(),
            Role::Assistant => format!("**AI** *({})*", msg.model),
        };
        out.push_str(&format!("{}\n\n{}\n\n---\n\n", prefix, msg.content));
    }
    out
}

// ── tiered summarization ──────────────────────────────────────────────────────
//
// Called when memory pressure is HIGH or history is very long.
// 1. Large model → planning note (bullets only, fast)
// 2. N small models in parallel → expert answers
// 3. Delegate model → synthesis
// All streaming goes to the same result label; steps are visually separated.

/// Run the full tiered MoE summarization pipeline.
/// `host`, `selector`, `question`, `context_summary` are inputs.
/// Calls `on_token` for each streamed token, `on_done` when finished.
pub fn run_tiered_summary(
    host:            String,
    selector:        crate::model_selector::ModelSelector,
    question:        String,
    context_summary: String,
    cancel:          Arc<AtomicBool>,
    on_token:        crossbeam_channel::Sender<crate::ui::ollama::StreamChunk>,
) {
    use crate::model_selector::{planning_prompt, expert_prompt, synthesis_prompt,
                                 expert_count, ExpertResult};

    // ── Step 1: large model → planning note ───────────────────────────────
    // Support model@host entries — the large model may be on a remote machine
    let (large_model, large_host) = split_model_host(&selector.primary().name, &host);
    let small_models: Vec<(String, String)> = selector.models.iter()
        .filter(|m| m.tier == crate::model_selector::ModelTier::Small)
        .map(|m| split_model_host(&m.name, &host))
        .collect();
    let (delegate, delegate_host) = split_model_host(&selector.delegate().name, &host);
    let n_experts = expert_count(&selector).min(small_models.len().max(1));

    let _ = on_token.send(crate::ui::ollama::StreamChunk::Token(
        format!("\n\n**[Planning with {}]**\n", large_model)
    ));

    let plan_prompt = planning_prompt(&question, &context_summary);
    let (plan_tx, plan_rx) = crossbeam_channel::unbounded::<crate::ui::ollama::StreamChunk>();
    let gc = Arc::clone(&cancel);
    let lc = Arc::new(AtomicBool::new(false));
    crate::ui::ollama::prompt_streaming(
        &plan_prompt, &large_host, &large_model, &plan_tx, &gc, &lc
    );

    // Collect planning note
    let mut planning_note = String::new();
    while let Ok(chunk) = plan_rx.recv() {
        match chunk {
            crate::ui::ollama::StreamChunk::Token(t) => {
                planning_note.push_str(&t);
                let _ = on_token.send(crate::ui::ollama::StreamChunk::Token(t));
            }
            crate::ui::ollama::StreamChunk::Done{..} |
            crate::ui::ollama::StreamChunk::Cancelled => break,
            crate::ui::ollama::StreamChunk::Error(e) => {
                let _ = on_token.send(crate::ui::ollama::StreamChunk::Error(e));
                return;
            }
        }
        if cancel.load(Ordering::Relaxed) { return; }
    }

    // Cap planning note to 800 chars to avoid polluting expert contexts
    let planning_note: String = planning_note.chars().take(800).collect();

    // ── Step 2: small models → expert answers (sequential on ARM, no rayon) ──
    // On ARM without GPU we can't run two models in parallel in memory,
    // so we run them sequentially but format them as if from separate experts.
    let mut expert_results: Vec<ExpertResult> = Vec::new();

    for i in 0..n_experts {
        if cancel.load(Ordering::Relaxed) { return; }
        let (model_name, expert_host) = small_models.get(i)
            .cloned()
            .unwrap_or_else(|| (delegate.clone(), delegate_host.clone()));

        let _ = on_token.send(crate::ui::ollama::StreamChunk::Token(
            format!("\n\n**[Expert {} / {}]**\n", i+1, model_name)
        ));

        let ep = expert_prompt(&question, &planning_note, i+1);
        let (ex_tx, ex_rx) = crossbeam_channel::unbounded::<crate::ui::ollama::StreamChunk>();
        let gc2 = Arc::clone(&cancel);
        let lc2 = Arc::new(AtomicBool::new(false));
        crate::ui::ollama::prompt_streaming(&ep, &expert_host, &model_name, &ex_tx, &gc2, &lc2);

        let mut expert_text = String::new();
        while let Ok(chunk) = ex_rx.recv() {
            match chunk {
                crate::ui::ollama::StreamChunk::Token(t) => {
                    expert_text.push_str(&t);
                    let _ = on_token.send(crate::ui::ollama::StreamChunk::Token(t));
                }
                crate::ui::ollama::StreamChunk::Done{..} |
                crate::ui::ollama::StreamChunk::Cancelled => break,
                crate::ui::ollama::StreamChunk::Error(e) => {
                    let _ = on_token.send(crate::ui::ollama::StreamChunk::Error(e));
                    break;
                }
            }
            if cancel.load(Ordering::Relaxed) { return; }
        }
        expert_results.push(ExpertResult { model: model_name, content: expert_text });
    }

    // ── Step 3: delegate → synthesis ──────────────────────────────────────
    if cancel.load(Ordering::Relaxed) { return; }

    let _ = on_token.send(crate::ui::ollama::StreamChunk::Token(
        format!("\n\n**[Synthesis / {}]**\n", delegate)
    ));

    let synth = synthesis_prompt(&question, &expert_results);
    let (sy_tx, sy_rx) = crossbeam_channel::unbounded::<crate::ui::ollama::StreamChunk>();
    let gc3 = Arc::clone(&cancel);
    let lc3 = Arc::new(AtomicBool::new(false));
    crate::ui::ollama::prompt_streaming(&synth, &delegate_host, &delegate, &sy_tx, &gc3, &lc3);

    while let Ok(chunk) = sy_rx.recv() {
        if cancel.load(Ordering::Relaxed) { return; }
        match chunk {
            crate::ui::ollama::StreamChunk::Token(t) =>
                { let _ = on_token.send(crate::ui::ollama::StreamChunk::Token(t)); }
            crate::ui::ollama::StreamChunk::Done{model} => {
                let _ = on_token.send(crate::ui::ollama::StreamChunk::Done { model });
                return;
            }
            crate::ui::ollama::StreamChunk::Cancelled => {
                let _ = on_token.send(crate::ui::ollama::StreamChunk::Cancelled);
                return;
            }
            crate::ui::ollama::StreamChunk::Error(e) => {
                let _ = on_token.send(crate::ui::ollama::StreamChunk::Error(e));
                return;
            }
        }
    }
    let _ = on_token.send(crate::ui::ollama::StreamChunk::Done { model: delegate });
}

// ── model + host preference persistence ───────────────────────────────────────
//
// Saves chosen primary/delegate model names and host URL to
// ~/.local/share/logscope/prefs.json so the dropdowns restore on next launch.

fn prefs_path() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("logscope")
        .join("prefs.json")
}

fn save_prefs(host: &str, primary: &str, delegate: &str) {
    // Load existing to preserve recency/hosts fields
    let mut v = if let Ok(s) = std::fs::read_to_string(prefs_path()) {
        serde_json::from_str::<serde_json::Value>(&s).unwrap_or(serde_json::json!({}))
    } else { serde_json::json!({}) };
    v["host"]     = serde_json::Value::String(host.to_owned());
    v["primary"]  = serde_json::Value::String(primary.to_owned());
    v["delegate"] = serde_json::Value::String(delegate.to_owned());
    if let Some(p) = prefs_path().parent() { let _ = std::fs::create_dir_all(p); }
    let _ = std::fs::write(prefs_path(), v.to_string());
}

/// Push a model name to the front of the recency list, keeping last 20.
fn record_recency(model_name: &str) {
    let path = prefs_path();
    let mut v = if let Ok(s) = std::fs::read_to_string(&path) {
        serde_json::from_str::<serde_json::Value>(&s).unwrap_or(serde_json::json!({}))
    } else { serde_json::json!({}) };
    let mut recency: Vec<String> = v["recency"]
        .as_array().cloned().unwrap_or_default()
        .into_iter().filter_map(|x| x.as_str().map(str::to_owned)).collect();
    recency.retain(|x| x != model_name);
    recency.insert(0, model_name.to_owned());
    recency.truncate(20);
    v["recency"] = serde_json::json!(recency);
    let _ = std::fs::write(path, v.to_string());
}

/// Save a newly discovered LAN host so it can be re-queried on next launch.
fn load_prefs() -> (String, String, String) {
    let default_host = crate::ui::ollama::DEFAULT_HOST.to_string();
    if let Ok(s) = std::fs::read_to_string(prefs_path()) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            let host     = v["host"].as_str().unwrap_or(&default_host).to_string();
            let primary  = v["primary"].as_str().unwrap_or("").to_string();
            let delegate = v["delegate"].as_str().unwrap_or("").to_string();
            return (host, primary, delegate);
        }
    }
    (default_host, String::new(), String::new())
}

fn load_recency() -> Vec<String> {
    if let Ok(s) = std::fs::read_to_string(prefs_path()) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            return v["recency"].as_array().cloned().unwrap_or_default()
                .into_iter().filter_map(|x| x.as_str().map(str::to_owned)).collect();
        }
    }
    vec![]
}

#[allow(dead_code)]
fn load_known_lan_hosts() -> Vec<String> {
    if let Ok(s) = std::fs::read_to_string(prefs_path()) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            return v["lan_hosts"].as_array().cloned().unwrap_or_default()
                .into_iter().filter_map(|x| x.as_str().map(str::to_owned)).collect();
        }
    }
    vec![]
}

// ── LAN host scanner ──────────────────────────────────────────────────────────
//
// Returns (host_url, Vec<ModelInfo>) for every reachable Ollama instance.
// Each ModelInfo.name is stored as "modelname@http://host:port" so the
// dropdown entries are self-describing.

/// Streaming LAN scanner — sends one ModelInfo batch per discovered host
/// as it responds, rather than waiting for the whole /24 to finish.
/// Caller polls the receiver channel from a glib idle/timeout.
fn scan_lan_ollama_streaming(
    tx: crossbeam_channel::Sender<Vec<ModelInfo>>,
) {
    // scan_lan_parallel uses its own dedicated thread pool internally (not rayon).
    // Run it in a std::thread so the calling rayon thread is freed immediately.
    let (ptx, prx) = crossbeam_channel::unbounded::<(String, Vec<ModelInfo>)>();
    std::thread::spawn(move || {
        crate::providers::scan_lan_parallel(&ptx);
    });
    // Forward batches as they arrive, stripping the host url
    for (_url, batch) in prx {
        if tx.send(batch).is_err() { return; }
    }
}/// Split a store key into (model_name, host_url).
/// "@auto" entries require callers to resolve via best_host_for_model().
pub fn split_model_host(entry: &str, default_host: &str) -> (String, String) {
    let (model, host) = crate::ui::components::resolve_key_to_host(entry, default_host);
    // Map "auto" sentinel back to __auto__ for speed-table resolution
    if host == default_host && entry.ends_with("@auto") {
        (model, "__auto__".to_string())
    } else {
        (model, host)
    }
}
