use gtk4::prelude::*;
use gtk4::{
    Application, Box as GtkBox, Button, FileDialog, Label,
    ListBox, Orientation, Picture, PolicyType, ScrolledWindow,
    SelectionMode,
};
use libadwaita as adw;
use adw::prelude::*;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossbeam_channel::unbounded;

use crate::analyzer::{AnalyzerMsg, FileInfo};
use crate::charts;
use crate::ui::cargo_tab::CargoTab;
use crate::ui::code_tab::CodeTab;
use crate::ui::bloc_tab::BlocTab;
use crate::ui::modify_tab::ModifyTab;
use crate::ui::chat_tab::ChatTab;
use crate::ui::peek_view::PeekView;
use crate::ui::rotation_tab::RotationTab;
use crate::ui::sources_tab::SourcesTab;

fn chart_dir() -> PathBuf {
    let p = std::env::temp_dir().join("logscope_charts");
    let _ = std::fs::create_dir_all(&p);
    p
}

struct AppState {
    cancel: Arc<AtomicBool>,
    current_file: Option<PathBuf>,
}
impl AppState {
    fn new() -> Self { AppState { cancel: Arc::new(AtomicBool::new(false)), current_file: None } }
    fn cancel_current(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
    }
}

pub fn build(app: &Application) { build_window(app, None); }
pub fn build_with_files(app: &Application, files: &[gio::File], _hint: &str) {
    build_window(app, files.first().and_then(|f| f.path()));
}

fn build_window(app: &Application, initial_file: Option<PathBuf>) {
    let state = Rc::new(RefCell::new(AppState::new()));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Logscope")
        .build();

    let toast_overlay = adw::ToastOverlay::new();

    // ── Use AdwNavigationView: one page with a ViewSwitcherTitle ─────────
    // ViewSwitcherTitle collapses to a menu on narrow screens automatically.
    let toolbar_view = adw::ToolbarView::new();

    let header = adw::HeaderBar::new();
    header.set_centering_policy(adw::CenteringPolicy::Strict);

    let open_btn = Button::from_icon_name("document-open-symbolic");
    open_btn.add_css_class("suggested-action");
    open_btn.set_tooltip_text(Some("Open log file"));
    header.pack_end(&open_btn);

    let info_btn = Button::from_icon_name("dialog-information-symbolic");
    info_btn.set_tooltip_text(Some("File info"));
    info_btn.set_visible(true); // always visible — sidebar is overlay-only
    header.pack_start(&info_btn);

    toolbar_view.add_top_bar(&header);

    // ── Sidebar ───────────────────────────────────────────────────────────
    let (sidebar_box, info_label, stats_label, progress_label, progress_picture) = build_sidebar();

    let split_view = adw::OverlaySplitView::new();
    split_view.set_min_sidebar_width(0.0);
    split_view.set_max_sidebar_width(300.0);
    split_view.set_sidebar_width_fraction(0.28);
    split_view.set_sidebar_position(gtk4::PackType::Start);
    split_view.set_collapsed(true); // always overlay — never inline

    let sidebar_tv = adw::ToolbarView::new();
    let sidebar_hb = adw::HeaderBar::new();
    sidebar_hb.set_show_end_title_buttons(false);
    let close_btn = Button::from_icon_name("go-previous-symbolic");
    sidebar_hb.pack_end(&close_btn);
    sidebar_tv.add_top_bar(&sidebar_hb);
    sidebar_tv.set_content(Some(&sidebar_box));
    split_view.set_sidebar(Some(&sidebar_tv));

    // ── Bottom stats ──────────────────────────────────────────────────────
    let bottom_bar = gtk4::ActionBar::new();
    bottom_bar.add_css_class("bottom-stats-bar");
    bottom_bar.set_visible(false);  // shown when a file is loaded
    let bottom_stats = Label::new(Some("—"));
    bottom_stats.add_css_class("stats-label");
    bottom_stats.set_hexpand(true);
    bottom_stats.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    bottom_bar.set_center_widget(Some(&bottom_stats));

    // ── ViewStack: the real content ───────────────────────────────────────
    // AdwViewStack + AdwViewSwitcherBar is the Adwaita-native way to do tabs
    // on mobile. The bar collapses to icons at any width and never overflows.
    let stack = adw::ViewStack::new();
    stack.set_vexpand(true);
    stack.set_hexpand(true);

    // ViewSwitcherBar at the bottom — shows icons+text on wide, icons on narrow
    let switcher_bar = adw::ViewSwitcherBar::new();
    switcher_bar.set_stack(Some(&stack));
    switcher_bar.set_reveal(true);

    // AdwWindowTitle — simple and correct; ViewSwitcherBar handles tab switching
    let win_title = adw::WindowTitle::new("Logscope", "");
    header.set_title_widget(Some(&win_title));

    // ── Tab 0: Sources ────────────────────────────────────────────────────
    let sources_tab = Rc::new(SourcesTab::new());
    let sources_page = stack.add_titled_with_icon(
        &sources_tab.root, Some("sources"), "Sources", "network-server-symbolic"
    );
    sources_page.set_badge_number(0);

    // ── Tab 1: Peek ───────────────────────────────────────────────────────
    let peek_view = Rc::new(PeekView::new());
    stack.add_titled_with_icon(&peek_view.root, Some("peek"), "Peek", "text-x-generic-symbolic");

    // Sources → Peek callback
    {
        let pv = Rc::clone(&peek_view);
        let st = stack.clone();
        sources_tab.set_on_lines(move |lines| {
            pv.load(&lines);
            st.set_visible_child_name("peek");
        });
    }

    // ── Tab 2: Tokens ─────────────────────────────────────────────────────
    let highlighted_idx: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));
    let token_freqs_store: Rc<RefCell<Vec<(String, u64)>>> = Rc::new(RefCell::new(Vec::new()));

    let token_picture = Picture::new();
    token_picture.set_can_shrink(true);
    token_picture.set_hexpand(true);
    token_picture.set_content_fit(gtk4::ContentFit::Fill);
    token_picture.set_height_request(180);
    token_picture.set_valign(gtk4::Align::Start);

    let token_list = ListBox::new();
    token_list.set_selection_mode(SelectionMode::None);
    token_list.add_css_class("token-freq-list");

    let token_list_scroll = ScrolledWindow::builder()
        .vscrollbar_policy(PolicyType::Automatic)
        .hscrollbar_policy(PolicyType::Never)
        .vexpand(true).child(&token_list).build();

    // Wrap token_picture in a scrolled window to prevent it forcing window expansion
    let token_pic_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Never)
        .propagate_natural_width(false)
        .propagate_natural_height(false)
        .child(&token_picture).build();
    let token_vbox = GtkBox::new(Orientation::Vertical, 0);
    token_vbox.set_hexpand(true); token_vbox.set_vexpand(true);
    token_vbox.append(&token_pic_scroll);
    token_vbox.append(&gtk4::Separator::new(Orientation::Horizontal));
    token_vbox.append(&token_list_scroll);
    stack.add_titled_with_icon(&token_vbox, Some("tokens"), "Tokens", "view-list-symbolic");

    // ── Tab 3: Blocks ─────────────────────────────────────────────────────
    let block_picture = Picture::new();
    block_picture.set_can_shrink(true);
    block_picture.set_vexpand(true);
    block_picture.set_content_fit(gtk4::ContentFit::Contain);
    let blocks_scroll = ScrolledWindow::builder()
        .child(&block_picture).vscrollbar_policy(PolicyType::Never)
        .hscrollbar_policy(PolicyType::Never)
        .propagate_natural_width(false)
        .hexpand(true).vexpand(true).build();
    stack.add_titled_with_icon(&blocks_scroll, Some("blocks"), "Blocks", "histogram-symbolic");

    // ── Tab 4: Rotate ─────────────────────────────────────────────────────
    let rot_tab = Rc::new(RotationTab::new());
    let rot_scroll = ScrolledWindow::builder()
        .vscrollbar_policy(PolicyType::Automatic)
        .hscrollbar_policy(PolicyType::Never)
        .hexpand(true).vexpand(true)
        .child(&rot_tab.root).build();
    stack.add_titled_with_icon(&rot_scroll, Some("rotate"), "Rotate", "view-refresh-symbolic");

    // ── Tab 5: Cargo ──────────────────────────────────────────────────────
    let cargo_tab: Rc<CargoTab> = Rc::new(CargoTab::new());
    let cargo_page = stack.add_titled_with_icon(
        &cargo_tab.root, Some("cargo"), "Cargo", "applications-engineering-symbolic"
    );

    // ── Tab 6: Chat ───────────────────────────────────────────────────────
    let chat_tab = Rc::new(ChatTab::new());
    stack.add_titled_with_icon(&chat_tab.root, Some("chat"), "Chat", "user-available-symbolic");

    // ── Tab 7: Code ────────────────────────────────────────────────────────
    let code_tab = CodeTab::new();
    stack.add_titled_with_icon(&code_tab.root, Some("code"), "Code", "text-x-source-symbolic");

    // ── Tab 8: Modify ──────────────────────────────────────────────────────
    let modify_tab = ModifyTab::new();
    let bloc_tab   = BlocTab::new();
    stack.add_titled_with_icon(&modify_tab.root, Some("modify"), "Modify", "document-edit-symbolic");
    stack.add_titled_with_icon(&bloc_tab.root,   Some("blocs"),  "Bloc",   "text-x-generic-template-symbolic");


    // Wire peek Ollama responses into Chat tab (must be after chat_tab is declared)
    {
        let ct: Rc<ChatTab> = Rc::clone(&chat_tab);
        peek_view.set_on_ollama_done(move |label, response| {
            ct.log_snippet(format!("Peek · {label}"), response);
        });
    }

    // Wire "Ask about this" from peek selection frame into chat pending context
    {
        let ct: Rc<ChatTab> = Rc::clone(&chat_tab);
        peek_view.set_on_ask_about_this(move |label, content| {
            ct.queue_peek_selection(label, content);
        });
    }

    // ── Layout ────────────────────────────────────────────────────────────
    let content_col = GtkBox::new(Orientation::Vertical, 0);
    content_col.set_hexpand(true);
    content_col.set_vexpand(true);
    content_col.append(&stack);
    content_col.append(&switcher_bar);
    content_col.append(&bottom_bar);

    split_view.set_content(Some(&content_col));
    toolbar_view.set_content(Some(&split_view));
    toast_overlay.set_child(Some(&toolbar_view));
    window.set_content(Some(&toast_overlay));

    // Refresh model list on startup and whenever Chat tab is switched to
    chat_tab.refresh_models();
    {
        let ct: Rc<ChatTab> = Rc::clone(&chat_tab);
        stack.connect_notify_local(Some("visible-child"), move |s, _| {
            if s.visible_child_name().as_deref() == Some("chat") {
                ct.refresh_models();
            }
        });
    }

    { let sv = split_view.clone(); info_btn.connect_clicked(move |_| { sv.set_show_sidebar(true); }); }
    { let sv = split_view.clone(); close_btn.connect_clicked(move |_| { sv.set_show_sidebar(false); }); }

    // ── Open file ─────────────────────────────────────────────────────────
    let open_fn: Rc<dyn Fn(PathBuf)> = Rc::new({
        let state             = Rc::clone(&state);
        let peek_view         = Rc::clone(&peek_view);
        let token_picture     = token_picture.clone();
        let token_list        = token_list.clone();
        let token_freqs_store = Rc::clone(&token_freqs_store);
        let highlighted_idx   = Rc::clone(&highlighted_idx);
        let block_picture     = block_picture.clone();
        let info_label        = info_label.clone();
        let stats_label       = stats_label.clone();
        let progress_label    = progress_label.clone();
        let progress_picture  = progress_picture.clone();
        let bottom_stats      = bottom_stats.clone();
        let toast_overlay     = toast_overlay.clone();
        let rot_tab           = Rc::clone(&rot_tab);
        let cargo_tab: Rc<CargoTab> = Rc::clone(&cargo_tab);
        let cargo_page        = cargo_page.clone();
        let stack             = stack.clone();

        move |path: PathBuf| {
            stack.set_visible_child_name("peek");
            *highlighted_idx.borrow_mut() = None;
            *token_freqs_store.borrow_mut() = Vec::new();
            while let Some(c) = token_list.first_child() { token_list.remove(&c); }

            if CargoTab::is_cargo_project(&path) {
                cargo_page.set_badge_number(1);
                cargo_tab.load_project(path.clone());
            } else {
                cargo_page.set_badge_number(0);
                cargo_tab.reset_placeholder();
            }

            start_analysis(
                path.clone(), &state, &peek_view,
                &token_picture, &token_list,
                &token_freqs_store, &highlighted_idx,
                &block_picture,
                &info_label, &stats_label, &progress_label, &progress_picture,
                &bottom_stats, &bottom_bar, &toast_overlay, &rot_tab, &stack,
            );
            rot_tab.load_file(path);
        }
    });

    {
        let open_fn2 = Rc::clone(&open_fn);
        let wr = window.clone();
        open_btn.connect_clicked(move |_| {
            let dialog = FileDialog::builder().title("Open log file").build();
            let ofn    = Rc::clone(&open_fn2);
            dialog.open(Some(&wr), None::<&gio::Cancellable>, move |r| {
                if let Ok(f) = r { if let Some(p) = f.path() { ofn(p); } }
            });
        });
    }

    let pins = crate::analyzer::default_pins();
    if !pins.is_empty() {
        toolbar_view.add_top_bar(&build_pin_bar(&pins, Rc::clone(&open_fn)));
    }

    if let Some(path) = initial_file { open_fn(path); }

    {
        let pv = Rc::clone(&peek_view);
        let ct: Rc<CargoTab> = Rc::clone(&cargo_tab);
        let ch: Rc<ChatTab>  = Rc::clone(&chat_tab);
        window.connect_close_request(move |_| {
            pv.ollama_cancel.store(true, Ordering::Relaxed);
            ct.cancel_all();
            ch.cancel_all();
            // Unload all Ollama models in background — best effort, non-blocking.
            // Uses keep_alive=0 which signals Ollama to release VRAM immediately
            // without deleting the model weights from disk.
            rayon::spawn(move || {
                crate::ui::ollama::unload_all_models(crate::ui::ollama::DEFAULT_HOST);
            });
            glib::Propagation::Proceed
        });
    }

    window.present();
}

// ── Analysis ──────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn start_analysis(
    path: PathBuf,
    state: &Rc<RefCell<AppState>>,
    peek_view: &Rc<PeekView>,
    token_picture: &Picture,
    token_list: &ListBox,
    token_freqs_store: &Rc<RefCell<Vec<(String, u64)>>>,
    highlighted_idx: &Rc<RefCell<Option<usize>>>,
    block_picture: &Picture,
    info_label: &Label,
    stats_label: &Label,
    progress_label: &Label,
    progress_picture: &Picture,
    bottom_stats: &Label,
    bottom_bar: &gtk4::ActionBar,
    toast_overlay: &adw::ToastOverlay,
    rot_tab: &Rc<RotationTab>,
    stack: &adw::ViewStack,
) {
    let mut s = state.borrow_mut();
    s.cancel_current();
    s.current_file = Some(path.clone());

    let info = match FileInfo::from_path(&path) {
        Ok(i)  => i,
        Err(e) => { toast_overlay.add_toast(adw::Toast::new(&format!("Cannot open: {e}"))); return; }
    };

    let fname    = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let size_str = humansize::format_size(info.size_bytes, humansize::DECIMAL);
    info_label.set_markup(&format!(
        "<b>{}</b>\n<small>{}</small>\n<small>{size_str}</small>",
        glib::markup_escape_text(&fname),
        glib::markup_escape_text(&path.display().to_string()),
    ));
    bottom_bar.set_visible(true);
    bottom_stats.set_label(&format!("{fname}  ·  {size_str}  ·  peeking…"));
    stats_label.set_label("Peeking…");
    progress_label.set_label("0%");
    peek_view.load(&[]);
    token_picture.set_filename(None::<&str>);
    block_picture.set_filename(None::<&str>);

    let cancel = Arc::clone(&s.cancel);
    drop(s);

    let (tx, rx) = unbounded::<AnalyzerMsg>();
    crate::analyzer::analyze_async(path, tx, cancel);

    let peek_view         = Rc::clone(peek_view);
    let token_picture     = token_picture.clone();
    let token_list        = token_list.clone();
    let token_freqs_store = Rc::clone(token_freqs_store);
    let highlighted_idx   = Rc::clone(highlighted_idx);
    let block_picture     = block_picture.clone();
    let stats_label       = stats_label.clone();
    let progress_label    = progress_label.clone();
    let progress_picture  = progress_picture.clone();
    let bottom_stats      = bottom_stats.clone();
    let toast_overlay     = toast_overlay.clone();
    let rot_tab           = Rc::clone(rot_tab);
    let stack             = stack.clone();
    let cdir              = chart_dir();

    glib::idle_add_local(move || {
        let mut keep = true;
        loop {
            match rx.try_recv() {
                Ok(msg) => handle_msg(
                    msg, &peek_view,
                    &token_picture, &token_list,
                    &token_freqs_store, &highlighted_idx,
                    &block_picture,
                    &stats_label, &progress_label, &progress_picture,
                    &bottom_stats,
                    &toast_overlay, &rot_tab, &stack, &cdir, &mut keep,
                ),
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => { keep=false; break; }
            }
        }
        if keep { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
    });
}

#[allow(clippy::too_many_arguments)]
fn handle_msg(
    msg: AnalyzerMsg,
    peek_view: &Rc<PeekView>,
    token_picture: &Picture,
    token_list: &ListBox,
    token_freqs_store: &Rc<RefCell<Vec<(String, u64)>>>,
    highlighted_idx: &Rc<RefCell<Option<usize>>>,
    block_picture: &Picture,
    stats_label: &Label,
    progress_label: &Label,
    progress_picture: &Picture,
    bottom_stats: &Label,
    toast_overlay: &adw::ToastOverlay,
    rot_tab: &Rc<RotationTab>,
    stack: &adw::ViewStack,
    cdir: &PathBuf,
    keep: &mut bool,
) {
    match msg {
        AnalyzerMsg::PeekReady { lines, token_freqs } => {
            peek_view.load(&lines);
            if !token_freqs.is_empty() {
                if let Ok(p) = charts::token_frequency_chart_highlighted(&token_freqs, None, cdir) {
                    token_picture.set_filename(Some(&p));
                }
                *token_freqs_store.borrow_mut() = token_freqs.clone();
                populate_token_list(
                    token_list, &token_freqs, token_picture,
                    token_freqs_store, highlighted_idx, rot_tab, peek_view, toast_overlay, stack, cdir,
                );
            }
            stats_label.set_label(&format!("Peek: {} lines", lines.len()));
        }
        AnalyzerMsg::Progress { bytes_done, total_bytes, unique_blocks, repeated_blocks } => {
            let pct = if total_bytes > 0 { bytes_done as f64 / total_bytes as f64 * 100.0 } else { 0.0 };
            progress_label.set_label(&format!("{pct:.0}%"));
            if let Ok(p) = charts::progress_chart(bytes_done, total_bytes, cdir) {
                progress_picture.set_filename(Some(&p));
            }
            let done  = humansize::format_size(bytes_done, humansize::DECIMAL);
            let total = humansize::format_size(total_bytes, humansize::DECIMAL);
            stats_label.set_label(&format!("{done}/{total}\n{unique_blocks}u {repeated_blocks}r"));
            bottom_stats.set_label(&format!("{pct:.0}%  {done}/{total}  ·  {unique_blocks}u  {repeated_blocks}r"));
        }
        AnalyzerMsg::BlockHistogram(h) => {
            if let Ok(p) = charts::block_histogram_chart(&h, cdir) { block_picture.set_filename(Some(&p)); }
        }
        AnalyzerMsg::Done { elapsed_secs, total_bytes, total_blocks, unique_blocks, top_tokens } => {
            let size = humansize::format_size(total_bytes, humansize::DECIMAL);
            stats_label.set_label(&format!("Done {elapsed_secs:.1}s\n{size} · {total_blocks}b · {unique_blocks}u"));
            bottom_stats.set_label(&format!("✓  {size}  ·  {total_blocks} blocks  ·  {elapsed_secs:.1}s"));
            progress_label.set_label("100%");
            *token_freqs_store.borrow_mut() = top_tokens.clone();
            let hi = *highlighted_idx.borrow();
            if let Ok(p) = charts::token_frequency_chart_highlighted(&top_tokens, hi, cdir) {
                token_picture.set_filename(Some(&p));
            }
            while let Some(c) = token_list.first_child() { token_list.remove(&c); }
            populate_token_list(
                token_list, &top_tokens, token_picture,
                token_freqs_store, highlighted_idx, rot_tab, peek_view, toast_overlay, stack, cdir,
            );
            toast_overlay.add_toast(adw::Toast::new("Analysis complete"));
            *keep = false;
        }
        AnalyzerMsg::Error(e) => {
            let t = adw::Toast::new(&format!("Error: {e}")); t.set_timeout(0);
            toast_overlay.add_toast(t); *keep = false;
        }
    }
}

// ── Token list ────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn populate_token_list(
    list: &ListBox,
    freqs: &[(String, u64)],
    token_picture: &Picture,
    token_freqs_store: &Rc<RefCell<Vec<(String, u64)>>>,
    highlighted_idx: &Rc<RefCell<Option<usize>>>,
    rot_tab: &Rc<RotationTab>,
    peek_view: &Rc<PeekView>,
    toast_overlay: &adw::ToastOverlay,
    stack: &adw::ViewStack,
    cdir: &PathBuf,
) {
    let n = freqs.len().min(30);

    for (idx, (token, count)) in freqs.iter().take(n).enumerate() {
        let color_normal = charts::bar_color_css(idx, n, false);
        let accent_class = format!("token-accent-{idx}");

        let accent = gtk4::Box::new(Orientation::Vertical, 0);
        accent.set_width_request(4);
        accent.set_vexpand(true);
        accent.add_css_class(&accent_class);

        let css = gtk4::CssProvider::new();
        css.load_from_string(&format!(
            ".{accent_class} {{ background-color: {color_normal}; border-radius: 2px 0 0 2px; }}"
        ));
        if let Some(d) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(&d, &css, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }

        let text_box = gtk4::Box::new(Orientation::Vertical, 0);
        text_box.set_hexpand(true);
        text_box.set_margin_start(10); text_box.set_margin_end(6);
        text_box.set_margin_top(6);    text_box.set_margin_bottom(6);

        let tok_lbl = Label::builder()
            .label(token.as_str()).halign(gtk4::Align::Start).hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End).build();
        tok_lbl.add_css_class("token-freq-name");
        tok_lbl.set_tooltip_text(Some(token.as_str()));

        let count_lbl = Label::builder()
            .label(&humansize::format_size(*count, humansize::DECIMAL))
            .halign(gtk4::Align::Start).build();
        count_lbl.add_css_class("token-freq-count");

        text_box.append(&tok_lbl);
        text_box.append(&count_lbl);

        let btn_box = gtk4::Box::new(Orientation::Vertical, 2);
        btn_box.set_valign(gtk4::Align::Center);
        btn_box.set_margin_end(4);

        let peek_btn   = Button::from_icon_name("system-search-symbolic");
        peek_btn.add_css_class("flat"); peek_btn.add_css_class("token-action-btn");
        let filter_btn = Button::from_icon_name("view-filter-symbolic");
        filter_btn.add_css_class("flat"); filter_btn.add_css_class("token-action-btn");

        btn_box.append(&peek_btn);
        btn_box.append(&filter_btn);

        let row_inner = gtk4::Box::new(Orientation::Horizontal, 0);
        row_inner.append(&accent);
        row_inner.append(&text_box);
        row_inner.append(&btn_box);

        let row = gtk4::ListBoxRow::new();
        row.set_selectable(true); row.set_activatable(true);
        row.set_child(Some(&row_inner));
        row.add_css_class("token-freq-row");
        list.append(&row);

        {
            let tp = token_picture.clone();
            let tfs = Rc::clone(token_freqs_store);
            let hi  = Rc::clone(highlighted_idx);
            let cd  = cdir.clone();
            row.connect_activate(move |_| {
                let prev   = *hi.borrow();
                let new_hi = if prev == Some(idx) { None } else { Some(idx) };
                *hi.borrow_mut() = new_hi;
                if let Some(d) = gtk4::gdk::Display::default() {
                    for j in 0..n {
                        let color = charts::bar_color_css(j, n, new_hi==Some(j));
                        let cls   = format!("token-accent-{j}");
                        let cp    = gtk4::CssProvider::new();
                        cp.load_from_string(&format!(".{cls} {{ background-color: {color}; border-radius: 2px 0 0 2px; }}"));
                        gtk4::style_context_add_provider_for_display(&d, &cp, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);
                    }
                }
                let freqs2 = tfs.borrow();
                if let Ok(p) = charts::token_frequency_chart_highlighted(&freqs2, new_hi, &cd) {
                    tp.set_filename(Some(&p));
                }
            });
        }

        {
            let ts = token.clone(); let pv = Rc::clone(peek_view);
            let st = stack.clone();  let ov = toast_overlay.clone();
            peek_btn.connect_clicked(move |_| {
                st.set_visible_child_name("peek");
                let msg = match pv.scroll_to_token(&ts) {
                    Some(n) => format!("Line {n}  —  \"{ts}\""),
                    None    => format!("\"{ts}\" not found"),
                };
                let t = adw::Toast::new(&msg); t.set_timeout(4);
                ov.add_toast(t);
            });
        }

        {
            let ts = token.clone(); let rt = Rc::clone(rot_tab); let st = stack.clone();
            filter_btn.connect_clicked(move |_| {
                st.set_visible_child_name("rotate");
                rt.add_token_filter(&ts);
            });
        }
    }
}

// ── Sidebar ───────────────────────────────────────────────────────────────────

fn build_sidebar() -> (GtkBox, Label, Label, Label, Picture) {
    let sidebar = GtkBox::new(Orientation::Vertical, 8);
    sidebar.add_css_class("sidebar");
    sidebar.set_vexpand(true);
    sidebar.set_margin_start(8); sidebar.set_margin_end(8);
    sidebar.set_margin_top(12);  sidebar.set_margin_bottom(12);

    let info_label = Label::builder()
        .label("No file open").halign(gtk4::Align::Start).wrap(true)
        .wrap_mode(gtk4::pango::WrapMode::WordChar).build();
    info_label.add_css_class("file-info");

    let stats_label = Label::builder()
        .label("—").halign(gtk4::Align::Start).wrap(true).build();
    stats_label.add_css_class("stats-label");

    let progress_label = Label::builder()
        .label("0%").halign(gtk4::Align::Center).build();
    progress_label.add_css_class("progress-pct");

    let progress_picture = Picture::new();
    progress_picture.set_height_request(50);
    progress_picture.set_can_shrink(true);

    sidebar.append(&info_label);
    sidebar.append(&gtk4::Separator::new(Orientation::Horizontal));
    sidebar.append(&stats_label);
    sidebar.append(&gtk4::Separator::new(Orientation::Horizontal));
    sidebar.append(&progress_label);
    sidebar.append(&progress_picture);

    (sidebar, info_label, stats_label, progress_label, progress_picture)
}

// ── Pin bar ───────────────────────────────────────────────────────────────────

fn build_pin_bar(pins: &[PathBuf], open_fn: Rc<dyn Fn(PathBuf)>) -> ScrolledWindow {
    let hbox = GtkBox::new(Orientation::Horizontal, 6);
    hbox.set_margin_start(8); hbox.set_margin_end(8);
    hbox.set_margin_top(4);   hbox.set_margin_bottom(4);

    for pin in pins {
        let name = pin.file_name().unwrap_or_default().to_string_lossy().to_string();
        let btn  = Button::with_label(&name);
        btn.add_css_class("pill");
        btn.set_tooltip_text(Some(&pin.display().to_string()));
        let pp = pin.clone(); let ofn = Rc::clone(&open_fn);
        btn.connect_clicked(move |_| ofn(pp.clone()));
        hbox.append(&btn);
    }

    ScrolledWindow::builder()
        .child(&hbox)
        .vscrollbar_policy(PolicyType::Never)
        .hscrollbar_policy(PolicyType::Automatic)
        .build()
}
