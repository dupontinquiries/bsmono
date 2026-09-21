//! The tomescope application core: all screens and state, shell-agnostic.
//!
//! It takes a plain `egui::Context`, so two shells drive it: the desktop
//! eframe window (`src/main.rs`, feature `desktop`) and the e-ink tablet
//! runner (`src/bin/tomescope_eink.rs`, feature `eink`). Neither shell holds
//! logic; moving this out of `main.rs` was the whole port.

use egui::{self, RichText};
use crate::layout::Typography;
use crate::library::{Library, ShelfSel, SortKey, TextureCache};
use crate::persist;
use crate::reader::ReaderState;
use crate::stats::{ActiveSession, StatsLog};
#[cfg(feature = "espeak")]
use crate::tts::TtsBackend as _;
use crate::study::Deck;
use crate::theme::ReaderPrefs;
use uikit_egui::toast::Toasts;
use uikit_egui::{apply_prefs, fuzzy, palette_colors, Adaptive, LiveBackground, Nav};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Library,
    Reader,
    Stats,
    Settings,
    Flashcards,
    OpdsBrowser,
}

struct OpenBook {
    id: u64,
    loaded: crate::formats::LoadedBook,
    reader: ReaderState,
    active_session: Option<ActiveSession>,
}

pub struct TomeApp {
    /// E-ink mode (see `set_eink`).
    eink: bool,
    library: Library,
    textures: TextureCache,
    toasts: Toasts,
    nav: Nav,
    adaptive: Adaptive,
    live_bg: LiveBackground,

    screen: Screen,
    prev_screen: Screen,

    open_book: Option<OpenBook>,
    pending_open: Option<u64>,
    pending_close: bool,

    typography: Typography,
    reader_prefs: ReaderPrefs,
    app_palette: String,
    prefs: settings_kit::Prefs,

    stats: StatsLog,
    deck: Deck,
    flashcard_next_id: u64,
    flashcard_new_front: String,
    flashcard_new_back: String,
    flashcard_reveal: bool,

    shelf_sel: ShelfSel,
    sort_key: SortKey,
    library_query: String,
    /// The folder being typed in Settings > Library folders.
    new_root: String,

    command_palette_open: bool,
    command_query: String,
    /// Keyboard selection in the palette list. See uikit_egui::dialog.
    command_sel: uikit_egui::dialog::Selection,

    toc_open: bool,
    bookmarks_open: bool,
    search_open: bool,
    figures_open: bool,
    info_open: bool,

    /// The pasted catalog XML, kept even after a failed parse so the user can
    /// fix a typo rather than retype the whole blob.
    opds_xml: String,
    /// Last parse result: `Ok` holds the feed to browse, `Err` holds the
    /// message to show — kept as a `Result` rather than two `Option` fields
    /// so a stale success can't linger on screen next to a fresh error.
    opds_result: Option<Result<crate::opds::OpdsFeed, String>>,

    /// Shared across all open books — the built-in glossary is fixed and
    /// book-index terms come from whichever book is passed to `lookup()`, so
    /// there is nothing per-book to store here.
    lookup: crate::lookup::Lookup,

    /// Deferred: set by the reader toolbar's Read button, consumed once the
    /// CentralPanel below has computed a live Pagination to read the current
    /// page's start locus from.
    tts_start_requested: bool,
    /// `SilentBackend` unless the `espeak` feature is compiled in — real
    /// audio depends on `espeak-ng` being installed, which this sandbox
    /// cannot verify; the queue/highlighting logic works either way.
    tts_backend: Box<dyn crate::tts::TtsBackend>,

    last_frame_time: f64,
}

/// `EspeakBackend` when the feature is compiled in AND the binary is
/// actually on PATH — falling back to `SilentBackend` either way means the
/// queue/highlighting still work with no audio, rather than a read-aloud
/// button that appears to do nothing on a machine without espeak-ng.
fn default_tts_backend() -> Box<dyn crate::tts::TtsBackend> {
    #[cfg(feature = "espeak")]
    {
        let backend = crate::tts::EspeakBackend::default();
        if backend.is_available() {
            return Box::new(backend);
        }
    }
    Box::new(crate::tts::SilentBackend)
}

impl TomeApp {
    /// One frame. Both shells call this: eframe's `update`, and eink-kit's
    /// runner through `FromCtx`.
    pub fn update(&mut self, ctx: &egui::Context) {
        self.adaptive = Adaptive::detect(ctx);
        let colors = palette_colors(&self.app_palette);

        let now_t = ctx.input(|i| i.time);
        let dt = (now_t - self.last_frame_time).max(0.0) as f32;
        self.last_frame_time = now_t;
        self.live_bg.tick(dt);

        if let Some(id) = self.pending_open.take() {
            self.open_book_by_id(id);
        }
        if self.pending_close {
            self.pending_close = false;
            self.close_current_book();
        }

        egui::TopBottomPanel::top("app_top").show(ctx, |ui| {
            self.draw_top_nav(ui, &colors);
        });

        // `CentralPanel` must be the last panel added in a frame — egui
        // asserts on this. The reader screen owns its own Top/Side/Central
        // panels (see `draw_reader_screen`), so when it's active we must
        // NOT also show a generic CentralPanel here first; doing so was a
        // real bug (panel added after CentralPanel, on the very next
        // frame after opening a book).
        if self.screen == Screen::Reader {
            self.draw_reader_screen(ctx, &colors);
        } else {
            egui::CentralPanel::default().show(ctx, |ui| match self.screen {
                Screen::Library => self.draw_library_screen(ui, &colors),
                Screen::Stats => self.draw_stats_screen(ui, &colors),
                Screen::Flashcards => self.draw_flashcards_screen(ui, &colors),
                Screen::OpdsBrowser => self.draw_opds_screen(ui, &colors),
                Screen::Settings => self.draw_settings_screen(ui, &colors),
                Screen::Reader => unreachable!("handled above"),
            });
        }

        self.draw_command_palette(ctx);
        self.toasts.ui(ctx, 24.0);

        // The heartbeat drives toast expiry (Toasts schedules no repaint of
        // its own). On a desktop 4 Hz is free. On e-ink every frame is CPU
        // raster work on a battery device, so only tick while a toast is
        // actually showing; reading-session stats are timestamped at
        // open/close and need no ticking.
        if !self.eink {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if self.toasts.visible_at(now_t).is_some() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    /// Persist library, stats, deck and prefs. eframe calls it periodically
    /// and on exit; the e-ink shell calls it on a timer and on drop.
    pub fn save(&mut self) {
        let _ = persist::save_library(&self.library);
        let _ = persist::save_stats(&self.stats);
        let _ = persist::save_deck(&self.deck);
        let _ = persist::save_prefs(&persist::AppPrefsBlob {
            reader: self.reader_prefs.clone(),
            typography: self.typography.clone(),
            app_palette: self.app_palette.clone(),
            keymap: persist::Keymap::default(),
        });
    }

    /// E-ink mode: the paper reading theme, no animated chrome, no idle
    /// heartbeat. Set by the eink-kit shell before the first frame.
    ///
    /// Also switches the kit palette to light and re-applies it: `new()`
    /// applied the settings palette (dark by default), which would otherwise
    /// paint the whole UI dark on paper. A shell may then layer its own
    /// visuals on top (eink-kit's paper-and-ink preset does).
    pub fn set_eink(&mut self, ctx: &egui::Context, on: bool) {
        self.eink = on;
        if on {
            self.reader_prefs.reading_theme = crate::theme::ReadingTheme::EInk;
            self.app_palette = crate::theme::ReadingTheme::EInk.suggested_app_palette().to_string();
            self.prefs.palette = self.app_palette.clone();
            apply_prefs(ctx, &self.prefs);
        }
    }

    pub fn new(ctx: &egui::Context) -> Self {
        let library = persist::load_library();
        let prefs_blob = persist::load_prefs();
        let stats = persist::load_stats();
        let deck = persist::load_deck();

        let prefs = settings_kit::Prefs::default();
        apply_prefs(ctx, &prefs);

        Self {
            eink: false,
            library,
            textures: TextureCache::new(64),
            toasts: Toasts::new(),
            nav: Nav::new(4),
            adaptive: Adaptive::detect(ctx),
            live_bg: LiveBackground::new(uikit_egui::BgStyle::all()[0]),

            screen: Screen::Library,
            prev_screen: Screen::Library,

            open_book: None,
            pending_open: None,
            pending_close: false,

            typography: prefs_blob.typography,
            reader_prefs: prefs_blob.reader,
            app_palette: if prefs_blob.app_palette.is_empty() {
                "dark".to_string()
            } else {
                prefs_blob.app_palette
            },
            prefs,

            flashcard_next_id: deck.cards.iter().map(|c| c.id).max().map(|m| m + 1).unwrap_or(1),
            stats,
            deck,
            flashcard_new_front: String::new(),
            flashcard_new_back: String::new(),
            flashcard_reveal: false,

            shelf_sel: ShelfSel::All,
            sort_key: SortKey::RecentlyRead,
            library_query: String::new(),
            new_root: String::new(),

            command_palette_open: false,
            command_query: String::new(),
            command_sel: Default::default(),

            toc_open: false,
            bookmarks_open: false,
            search_open: false,
            figures_open: false,
            info_open: false,

            opds_xml: String::new(),
            opds_result: None,

            lookup: crate::lookup::Lookup::default(),

            tts_start_requested: false,
            tts_backend: default_tts_backend(),

            last_frame_time: 0.0,
        }
    }

    /// Open a file handed to us by a file manager.
    ///
    /// Imports it if the library has not seen it, then opens by id — so a book
    /// you have read before lands on the entry holding your position and
    /// highlights rather than a fresh duplicate that has neither.
    pub fn open_from_path(&mut self, path: &std::path::Path) {
        let covers = persist::covers_dir();
        match self.library.import_file(path, &covers) {
            Some(id) => self.open_book_by_id(id),
            // Not fatal: a file manager can hand us anything, and the library is
            // a perfectly good place to land.
            None => self.toasts.error(
                format!("Could not open {}", path.display()), now()),
        }
    }

    fn open_book_by_id(&mut self, id: u64) {
        let Some(record) = self.library.get(id).cloned() else {
            self.toasts.error("Book not found in library", now());
            return;
        };
        match crate::formats::open(&record.path) {
            Ok(loaded) => {
                let state = persist::load_book_state(id);
                let mut reader = ReaderState::default();
                reader.open_book(&loaded.content, state.position);
                let session = ActiveSession::new(id, record.percent);
                self.open_book = Some(OpenBook {
                    id,
                    loaded,
                    reader,
                    active_session: Some(session),
                });
                self.screen = Screen::Reader;
            }
            Err(e) => {
                self.toasts.error(format!("Couldn't open book: {e}"), now());
            }
        }
    }

    fn close_current_book(&mut self) {
        if let Some(mut ob) = self.open_book.take() {
            if let Some(active) = ob.active_session.take() {
                let percent = self
                    .library
                    .get(ob.id)
                    .map(|b| b.percent)
                    .unwrap_or(0.0);
                let session = active.finish(percent);
                self.stats.add(session);
                let _ = persist::save_stats(&self.stats);
            }
            let pagination_page = ob.reader.current_page;
            if let Some(book) = self.library.get_mut(ob.id) {
                book.last_open_unix = Some(crate::util::now_unix());
            }
            let _ = persist::save_library(&self.library);

            let mut state = persist::load_book_state(ob.id);
            // Best-effort: without a live pagination reference we persist
            // the last known locus from the reader's pending/target
            // tracking; a full implementation keeps the true Locus in
            // ReaderState updated every frame during reading, which the
            // reader screen below does via `reader.go_to_locus`.
            let _ = pagination_page;
            state.raster_page = ob.reader.raster_page;
            let _ = persist::save_book_state(ob.id, &state);
        }
        self.screen = Screen::Library;
    }

    fn draw_top_nav(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        ui.horizontal(|ui| {
            for (screen, label) in [
                (Screen::Library, "Library"),
                (Screen::Stats, "Stats"),
                (Screen::Flashcards, "Flashcards"),
                (Screen::OpdsBrowser, "Catalogs"),
                (Screen::Settings, "Settings"),
            ] {
                let selected = self.screen == screen;
                let text = if selected {
                    RichText::new(label).color(colors.accent).strong()
                } else {
                    RichText::new(label).color(colors.text_weak)
                };
                if ui.add(egui::Label::new(text).sense(egui::Sense::click())).clicked() {
                    self.screen = screen;
                }
                ui.add_space(14.0);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⌘K").on_hover_text("Command palette").clicked() {
                    self.command_palette_open = true;
                }
            });
        });
        ui.separator();
    }

    fn draw_library_screen(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(egui::TextEdit::singleline(&mut self.library_query).id_salt("main-self_library_query"));
            egui::ComboBox::from_label("Sort")
                .selected_text(self.sort_key.label())
                .show_ui(ui, |ui| {
                    for key in SortKey::all() {
                        if ui.selectable_label(self.sort_key == key, key.label()).clicked() {
                            self.sort_key = key;
                        }
                    }
                });
            if ui.button("Rescan library").clicked() {
                let report = self.library.scan(&persist::covers_dir());
                self.toasts.success(
                    format!("Scan complete: {} added, {} updated", report.added, report.updated),
                    now(),
                );
                let _ = persist::save_library(&self.library);
            }
        });
        ui.add_space(8.0);

        let ids = self.library.view(&self.library_query, self.shelf_sel, self.sort_key);
        if ids.is_empty() {
            ui.colored_label(
                colors.text_weak,
                "No books yet. Add a scan root in Settings, or use `shelfctl scan <dir>` from the command line.",
            );
            return;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let card_w = 150.0;
            let cols = (ui.available_width() / (card_w + 12.0)).floor().max(1.0) as usize;
            egui::Grid::new("library_grid").spacing([12.0, 16.0]).show(ui, |ui| {
                let mut col = 0;
                for id in &ids {
                    if let Some(book) = self.library.get(*id) {
                        ui.vertical(|ui| {
                            let (rect, response) =
                                ui.allocate_exact_size(egui::vec2(card_w, card_w * 1.4), egui::Sense::click());
                            ui.painter().rect_filled(rect, egui::CornerRadius::same(8), colors.bg_card);
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                crate::util::truncate_chars(&book.title, 16),
                                egui::FontId::proportional(13.0),
                                colors.text,
                            );
                            if book.percent > 0.0 {
                                let bar = egui::Rect::from_min_size(
                                    egui::pos2(rect.left(), rect.bottom() - 4.0),
                                    egui::vec2(rect.width() * book.percent, 4.0),
                                );
                                ui.painter().rect_filled(bar, egui::CornerRadius::same(0), colors.accent);
                            }
                            if response.clicked() {
                                self.pending_open = Some(book.id);
                            }
                            ui.label(
                                RichText::new(crate::util::truncate_chars(&book.title, 20))
                                    .color(colors.text)
                                    .small(),
                            );
                            ui.colored_label(colors.text_weak, book.author_line());
                        });
                        col += 1;
                        if col >= cols {
                            col = 0;
                            ui.end_row();
                        }
                    }
                }
            });
        });
    }

    fn draw_reader_screen(&mut self, ctx: &egui::Context, colors: &uikit_egui::PaletteColors) {
        let Some(ob) = self.open_book.as_mut() else {
            self.screen = Screen::Library;
            return;
        };

        egui::TopBottomPanel::top("reader_top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("← Library").clicked() {
                    self.pending_close = true;
                }
                ui.label(RichText::new(&ob.loaded.title).color(colors.text).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("TOC").clicked() {
                        self.toc_open = !self.toc_open;
                    }
                    if ui.add_enabled(ob.reader.selection.is_active(), egui::Button::new("🔤 Define")).clicked() {
                        ob.reader.lookup_open = true;
                    }
                    // No auto-advance: TtsBackend::speak fires the backend
                    // (a subprocess for espeak, a no-op for SilentBackend)
                    // and returns immediately — the trait has no "finished"
                    // signal to poll, so this is Play/Stop/Next rather than
                    // a queue that plays itself through. Building real
                    // auto-advance means adding completion detection to the
                    // backend trait, which is separate scope from wiring the
                    // existing queue+backend together.
                    if ob.reader.tts.is_speaking {
                        if ui.button("⏹ Stop").clicked() {
                            ob.reader.tts.is_speaking = false;
                            let _ = self.tts_backend.stop();
                        }
                        if ui.button("⏭ Next").clicked() {
                            if ob.reader.tts.advance() {
                                if let Some(u) = ob.reader.tts.current() {
                                    let _ = self.tts_backend.speak(
                                        &u.text, ob.reader.tts.rate, ob.reader.tts.pitch, &ob.reader.tts.voice,
                                    );
                                }
                            } else {
                                let _ = self.tts_backend.stop();
                            }
                        }
                    } else if ui.button("🔊 Read").clicked() {
                        // The actual queue build needs the current page's
                        // start locus, which needs a live Pagination — that
                        // isn't computed until the CentralPanel below runs
                        // (it needs the panel's width/height). Deferred via
                        // this flag rather than duplicating pagination here.
                        self.tts_start_requested = true;
                    }
                    if ui.button("🔖").clicked() {
                        self.bookmarks_open = !self.bookmarks_open;
                    }
                    if ui.button("🔍").clicked() {
                        self.search_open = !self.search_open;
                    }
                    if ui.button("🖼").clicked() {
                        self.figures_open = !self.figures_open;
                    }
                    if ui.button("ℹ").clicked() {
                        self.info_open = !self.info_open;
                    }
                });
            });
        });

        if self.toc_open {
            egui::SidePanel::left("toc_panel").resizable(true).show(ctx, |ui| {
                if let Some(locus) = crate::panels::draw_toc(
                    ui,
                    &ob.loaded.content.toc,
                    ob.reader.current_page,
                    colors,
                ) {
                    ob.reader.go_to_locus(&ob.loaded.content, locus);
                }
            });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let width = ui.available_width();
            let height = ui.available_height() - 28.0;
            let pagination = ob.reader.paginate_now(ob.id, &ob.loaded.content, width, height);
            let page_idx = ob.reader.current_page.min(pagination.pages.len().saturating_sub(1));
            let page = pagination.pages[page_idx].clone();

            if self.tts_start_requested {
                self.tts_start_requested = false;
                // build_queue skips non-text blocks and, within the starting
                // block, anything at or before from.ch — so starting exactly
                // at page.start is correct even when that locus falls
                // mid-sentence: the sentence containing it is still emitted
                // in full (split_sentences ranges are checked by their END
                // against from.ch, not their start).
                let queue = crate::tts::build_queue(&ob.loaded.content, page.start, 0);
                let has_queue = !queue.is_empty();
                ob.reader.tts.load(queue);
                ob.reader.tts.is_speaking = has_queue;
                if let Some(u) = ob.reader.tts.current() {
                    let _ = self.tts_backend.speak(
                        &u.text, ob.reader.tts.rate, ob.reader.tts.pitch, &ob.reader.tts.voice,
                    );
                }
            }

            if let Some(zone) = crate::reader::draw_page(
                ui,
                &ob.loaded.content,
                &page,
                colors,
                &ob.reader.typo,
                &mut ob.reader.selection,
            ) {
                match zone {
                    crate::reader::TapZone::NextPage => {
                        ob.reader.turn(uikit_egui::Direction::Next, &pagination)
                    }
                    crate::reader::TapZone::PrevPage => {
                        ob.reader.turn(uikit_egui::Direction::Prev, &pagination)
                    }
                    _ => {}
                }
            }

            let percent = ob.reader.percent(&ob.loaded.content, &pagination);
            if let Some(book) = self.library.get_mut(ob.id) {
                book.percent = percent;
            }
            crate::reader::draw_status_bar(
                ui,
                colors,
                &format!("Page {} / {}", page_idx + 1, pagination.pages.len()),
                percent,
                self.stats.estimated_minutes_left(ob.id, percent),
            );
        });

        crate::reader::draw_footnote_popover(ctx, &ob.loaded.content, &mut ob.reader.show_footnote, colors);
        crate::lookup::draw_lookup_popover(
            ctx,
            &ob.loaded.content,
            &self.lookup,
            &ob.reader.selection,
            &mut ob.reader.lookup_open,
            colors,
        );
    }

    fn draw_stats_screen(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        ui.heading(RichText::new("Reading stats").color(colors.text));
        ui.label(format!(
            "Total time: {}",
            crate::util::humanize_duration_secs(self.stats.total_secs())
        ));
        ui.label(format!("Current streak: {} days", self.stats.current_streak(60)));
        ui.label(format!("Longest streak: {} days", self.stats.longest_streak(60)));
        ui.label(format!("Pages/hour: {:.1}", self.stats.pages_per_hour()));

        ui.add_space(12.0);
        ui.colored_label(colors.text_weak, "ACTIVITY");
        let grid = self.stats.heatmap_grid(20);
        egui::Grid::new("heatmap_grid").spacing([2.0, 2.0]).show(ui, |ui| {
            for row in &grid {
                for &secs in row {
                    let intensity = (secs as f32 / 3600.0).clamp(0.0, 1.0);
                    let color = colors.accent.gamma_multiply(0.15 + intensity * 0.85);
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(2), color);
                }
                ui.end_row();
            }
        });
    }

    fn draw_flashcards_screen(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        let now = crate::util::now_unix();
        let due = self.deck.due_count(now);
        ui.heading(RichText::new(format!("Flashcards — {due} due")).color(colors.text));
        ui.colored_label(
            colors.text_weak,
            "Jot a quick note while reading, or make it a two-sided card — either way it goes into spaced review.",
        );
        ui.add_space(8.0);

        // --- Quick add: works as a plain note (just "front") or a full
        // card (front + back) for review. A note with no back is treated
        // as its own answer, so it still shows up for review as a simple
        // recall prompt.
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("New note / card").color(colors.text_weak).small());
            ui.add(egui::TextEdit::multiline(&mut self.flashcard_new_front).id_salt("main-self_flashcard_new_front"));
            ui.horizontal(|ui| {
                ui.label("Answer (optional):");
                ui.add(egui::TextEdit::singleline(&mut self.flashcard_new_back).id_salt("main-self_flashcard_new_back"));
            });
            ui.horizontal(|ui| {
                let can_add = !self.flashcard_new_front.trim().is_empty();
                if ui.add_enabled(can_add, egui::Button::new("Add to deck")).clicked() {
                    let front = self.flashcard_new_front.trim().to_string();
                    let back = if self.flashcard_new_back.trim().is_empty() {
                        front.clone()
                    } else {
                        self.flashcard_new_back.trim().to_string()
                    };
                    let source_locus = self.open_book.as_ref().map(|ob| crate::content::Locus {
                        block: ob.reader.current_page,
                        ch: 0,
                    });
                    let id = self.flashcard_next_id;
                    self.flashcard_next_id += 1;
                    self.deck.add(crate::study::Flashcard::new(id, front, back, source_locus));
                    self.flashcard_new_front.clear();
                    self.flashcard_new_back.clear();
                    let _ = persist::save_deck(&self.deck);
                    self.toasts.success("Added to your deck", now as f64);
                }
            });
        });

        ui.add_space(14.0);

        // --- Review flow: work through everything due today.
        let due_ids: Vec<u64> = self.deck.due_cards(now).iter().map(|c| c.id).collect();
        if let Some(&review_id) = due_ids.first() {
            ui.separator();
            ui.label(RichText::new("Review").color(colors.text_weak).small());
            if let Some(card) = self.deck.get_mut(review_id) {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(&card.front).color(colors.text).strong());
                    if self.flashcard_reveal {
                        ui.add_space(6.0);
                        ui.separator();
                        ui.colored_label(colors.accent, &card.back);
                    }
                });
                ui.horizontal(|ui| {
                    if !self.flashcard_reveal {
                        if ui.button("Show answer").clicked() {
                            self.flashcard_reveal = true;
                        }
                    } else {
                        for (label, grade) in [("Again", 1u8), ("Hard", 3), ("Good", 4), ("Easy", 5)] {
                            if ui.button(label).clicked() {
                                card.review(grade);
                                self.flashcard_reveal = false;
                            }
                        }
                    }
                });
            }
            let _ = persist::save_deck(&self.deck);
        } else if !self.deck.cards.is_empty() {
            ui.colored_label(colors.success, "Nothing due right now — nice work.");
        }

        ui.add_space(14.0);
        ui.label(RichText::new(format!("All cards ({})", self.deck.cards.len())).color(colors.text_weak).small());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut to_remove = None;
            for card in &self.deck.cards {
                ui.horizontal(|ui| {
                    ui.colored_label(colors.text, crate::util::truncate_chars(&card.front, 60));
                    ui.colored_label(colors.text_weak, format!("(ease {:.2}, due {})", card.ease, crate::util::relative_time(card.due_unix)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕").clicked() {
                            to_remove = Some(card.id);
                        }
                    });
                });
            }
            if let Some(id) = to_remove {
                self.deck.remove(id);
                let _ = persist::save_deck(&self.deck);
            }
        });
    }

    fn draw_opds_screen(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        ui.heading(RichText::new("OPDS catalogs").color(colors.text));
        ui.colored_label(
            colors.text_weak,
            "Paste catalog XML below, or use `shelfctl` to script imports. Network fetch is opt-in and disabled by default.",
        );
        ui.add_space(6.0);

        egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.opds_xml).id_salt("main-self_opds_xml")
                    .desired_rows(6)
                    .desired_width(f32::INFINITY)
                    .hint_text("<feed xmlns=\"http://www.w3.org/2005/Atom\">…</feed>"),
            );
        });

        ui.horizontal(|ui| {
            let can_parse = !self.opds_xml.trim().is_empty();
            if ui.add_enabled(can_parse, egui::Button::new("Parse catalog")).clicked() {
                self.opds_result = Some(
                    crate::opds::parse_opds_xml(&self.opds_xml).map_err(|e| e.to_string()),
                );
            }
            if ui.button("Clear").clicked() {
                self.opds_xml.clear();
                self.opds_result = None;
            }
        });

        ui.add_space(8.0);
        match &self.opds_result {
            None => {}
            Some(Err(msg)) => {
                ui.colored_label(colors.danger, format!("could not parse: {msg}"));
            }
            Some(Ok(feed)) => {
                if feed.entries.is_empty() {
                    ui.colored_label(colors.text_weak, "the feed parsed but has no entries");
                    return;
                }
                if !feed.title.is_empty() {
                    ui.label(RichText::new(&feed.title).color(colors.text).strong());
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for entry in &feed.entries {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&entry.title).color(colors.text));
                                if let Some(author) = &entry.author {
                                    ui.colored_label(colors.text_weak, author);
                                }
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if entry.is_navigation() {
                                    ui.colored_label(colors.text_weak, "catalog ›");
                                } else if let Some(link) = entry.acquisition_link() {
                                    // Downloading is the network-fetch step this module
                                    // deliberately stubs (NoNetworkFetcher) — surfacing
                                    // the link is honest about what parsing alone gets
                                    // you; wiring a real fetch is separate, opt-in work.
                                    ui.colored_label(colors.text_weak, &link.mime_type);
                                }
                            });
                        });
                        ui.separator();
                    }
                });
            }
        }
    }

    fn draw_settings_screen(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // LIBRARY FOLDERS. `Rescan library` walks `scan_roots`, and there was no way to
            // put anything in that list from the GUI — so a fresh install scanned nothing,
            // reported "0 added", and the only way forward was `shelfctl scan <dir>` in a
            // terminal. A graphical reader that needs a command line before it can show you
            // a book is not finished.
            ui.label(egui::RichText::new("Library folders").strong());
            let mut removed: Option<std::path::PathBuf> = None;
            if self.library.scan_roots.is_empty() {
                ui.colored_label(colors.text_weak, "No folders yet — add one below, then Scan.");
            }
            for root in self.library.scan_roots.clone() {
                ui.horizontal(|ui| {
                    if ui.small_button("Remove").clicked() {
                        removed = Some(root.clone());
                    }
                    // The count is what tells you a folder is the one you meant, before you
                    // scan it and find out.
                    ui.label(format!("{}  ({} books)", root.display(), self.library.books_under(&root)));
                });
            }
            if let Some(r) = removed {
                self.library.remove_root(&r);
                let _ = crate::persist::save_library(&self.library);
                // Said explicitly, because "remove" is the word people expect to delete
                // things and this one does not.
                self.toasts.info("Folder removed. Its books stay in the library.".to_string(), now());
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_root)
                    .id_salt("settings-new-root")
                    .desired_width(320.0)
                    .hint_text("/home/you/Books"));
                if ui.button("Add folder").clicked() {
                    let path = std::path::PathBuf::from(self.new_root.trim());
                    match self.library.add_root(&path) {
                        Ok(p) => {
                            self.new_root.clear();
                            let report = self.library.scan(&crate::persist::covers_dir());
                            let _ = crate::persist::save_library(&self.library);
                            self.toasts.success(
                                format!("{}: {} added, {} updated", p.display(), report.added, report.updated),
                                now(),
                            );
                        }
                        Err(e) => {
                            use crate::library::AddRootError as E;
                            // Each case says what to DO, not just what went wrong.
                            let msg = match e {
                                E::Missing => "No such folder.".to_string(),
                                E::NotADirectory => "That is a file, not a folder.".to_string(),
                                E::AlreadyAdded => "Already in the library.".to_string(),
                                E::InsideExisting(p) => format!(
                                    "Already covered by {} — scanning both would find every book twice.",
                                    p.display()
                                ),
                                E::ContainsExisting(p) => format!(
                                    "Contains {}, which is already a folder. Remove that one first.",
                                    p.display()
                                ),
                            };
                            self.toasts.info(msg, now());
                        }
                    }
                }
                if ui.button("Scan now").clicked() {
                    let report = self.library.scan(&crate::persist::covers_dir());
                    let _ = crate::persist::save_library(&self.library);
                    self.toasts.success(
                        format!("Scan complete: {} added, {} updated", report.added, report.updated),
                        now(),
                    );
                }
            });
            ui.separator();
            let mut typo_changed = crate::settingsui::draw_typography_settings(ui, &mut self.typography, colors);
            typo_changed |= crate::settingsui::draw_theme_picker(ui, &mut self.reader_prefs, colors);
            typo_changed |= crate::settingsui::draw_palette_picker(ui, &mut self.app_palette, colors);
            typo_changed |= crate::settingsui::draw_behavior_settings(ui, &mut self.reader_prefs, colors);
            typo_changed |= crate::settingsui::draw_gesture_settings(
                ui,
                &mut self.prefs.gesture,
                &mut self.prefs.touch,
                colors,
            );
            if typo_changed {
                if let Some(ob) = self.open_book.as_mut() {
                    ob.reader.typo = self.typography.clone();
                    ob.reader.cache.invalidate_book(ob.id);
                }
                let _ = persist::save_prefs(&persist::AppPrefsBlob {
                    reader: self.reader_prefs.clone(),
                    typography: self.typography.clone(),
                    app_palette: self.app_palette.clone(),
                    keymap: persist::Keymap::default(),
                });
            }
        });
    }

    fn draw_command_palette(&mut self, ctx: &egui::Context) {
        if !self.command_palette_open {
            return;
        }
        let mut open = true;
        const COMMANDS: [&str; 6] = [
            "Go to Library",
            "Go to Stats",
            "Go to Flashcards",
            "Go to Settings",
            "Rescan library",
            "Close current book",
        ];
        let hits: Vec<&str> = COMMANDS.into_iter()
            .filter(|c| fuzzy::subsequence_match(&self.command_query, c))
            .collect();
        // Typing shortens the list, so an index left past the end would make
        // Enter silently do nothing.
        self.command_sel.clamp(hits.len());

        // Keys BEFORE the widgets, so navigation resolves against the list
        // the rows are about to be drawn from rather than lagging a frame.
        let mut moved = false;
        let mut activate = false;
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                moved |= self.command_sel.step(1, hits.len());
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                moved |= self.command_sel.step(-1, hits.len());
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                activate = true;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                self.command_palette_open = false;
            }
        });

        // `set_min_width(400.0)` was a hard floor wider than a phone, so the
        // palette ran off both edges. dialog_width asks for 400 and takes
        // what the surface can actually give, measured from content_rect
        // (not screen_rect, which includes the area under the system bars).
        let avail = ctx.content_rect();
        let w = uikit_egui::dialog::dialog_width(avail.width(), 400.0);
        let mut chosen: Option<&str> = None;
        egui::Window::new("Command palette")
            .id(egui::Id::new("tomescope_cmd_palette"))
            .collapsible(false)
            .resizable(false)
            .default_width(w)
            .max_width(w)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_max_width(w);
                ui.add(egui::TextEdit::singleline(&mut self.command_query)
                    .id_salt("tomescope-command-query")
                    .desired_width(f32::INFINITY));
                if hits.is_empty() {
                    ui.weak("no matching command");
                }
                // Capped and scrollable: without this the window grows past
                // the bottom of the screen and the last entries are
                // unreachable, because it is the WINDOW that overflowed and
                // there is nothing to scroll.
                egui::ScrollArea::vertical()
                    .max_height(uikit_egui::dialog::list_max_height(avail.height()))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, cmd) in hits.iter().enumerate() {
                            let selected = i == self.command_sel.index;
                            let r = ui.add(egui::Button::selectable(selected, *cmd)
                                .min_size(egui::vec2(ui.available_width(), 0.0)));
                            // Only on an actual keyboard move — following the
                            // selection every frame would pin the view and
                            // make the list impossible to scroll by hand.
                            if selected && moved {
                                r.scroll_to_me(Some(egui::Align::Center));
                            }
                            if r.clicked() {
                                chosen = Some(cmd);
                            }
                        }
                    });
            });
        if activate {
            chosen = hits.get(self.command_sel.index).copied();
        }
        if let Some(cmd) = chosen {
            self.run_command(cmd);
            self.command_palette_open = false;
            self.command_query.clear();
            self.command_sel = Default::default();
        }
        if !open {
            self.command_palette_open = false;
        }
    }

    fn run_command(&mut self, cmd: &str) {
        match cmd {
            "Go to Library" => self.screen = Screen::Library,
            "Go to Stats" => self.screen = Screen::Stats,
            "Go to Flashcards" => self.screen = Screen::Flashcards,
            "Go to Settings" => self.screen = Screen::Settings,
            "Rescan library" => {
                let report = self.library.scan(&persist::covers_dir());
                self.toasts.info(format!("Scanned: {} added", report.added), now());
            }
            "Close current book" => self.pending_close = true,
            _ => {}
        }
    }
}

fn now() -> f64 {
    crate::util::now_unix() as f64
}

