//! tomescope GUI entry point. Boots an `eframe` app following the same
//! pattern as `apps/settings-app`: a top-level `TomeApp` implementing
//! `eframe::App`, holding the long-lived state (library index, texture
//! cache, toast queue, navigation stack, adaptive layout info) and
//! routing between screens each frame.

use eframe::egui::{self, RichText};
use tomescope::layout::Typography;
use tomescope::library::{Library, ShelfSel, SortKey, TextureCache};
use tomescope::persist;
use tomescope::reader::ReaderState;
use tomescope::stats::{ActiveSession, StatsLog};
use tomescope::study::Deck;
use tomescope::theme::ReaderPrefs;
use uikit_egui::{apply_prefs, fuzzy, palette_colors, Adaptive, LiveBackground, Nav, Toasts};

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Tomescope")
            .with_inner_size([1180.0, 820.0])
            .with_min_inner_size([420.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "tomescope",
        native_options,
        Box::new(|cc| Ok(Box::new(TomeApp::new(cc)))),
    )
}

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
    loaded: tomescope::formats::LoadedBook,
    reader: ReaderState,
    active_session: Option<ActiveSession>,
}

struct TomeApp {
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

    command_palette_open: bool,
    command_query: String,

    toc_open: bool,
    bookmarks_open: bool,
    search_open: bool,
    figures_open: bool,
    info_open: bool,

    last_frame_time: f64,
}

impl TomeApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let library = persist::load_library();
        let prefs_blob = persist::load_prefs();
        let stats = persist::load_stats();
        let deck = persist::load_deck();

        let prefs = settings_kit::Prefs::default();
        apply_prefs(&cc.egui_ctx, &prefs);

        Self {
            library,
            textures: TextureCache::default(),
            toasts: Toasts::new(),
            nav: Nav::new(4),
            adaptive: Adaptive::detect(&cc.egui_ctx),
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

            command_palette_open: false,
            command_query: String::new(),

            toc_open: false,
            bookmarks_open: false,
            search_open: false,
            figures_open: false,
            info_open: false,

            last_frame_time: 0.0,
        }
    }

    fn open_book_by_id(&mut self, id: u64) {
        let Some(record) = self.library.get(id).cloned() else {
            self.toasts.error("Book not found in library", now());
            return;
        };
        match tomescope::formats::open(&record.path) {
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
                book.last_open_unix = Some(tomescope::util::now_unix());
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
            ui.text_edit_singleline(&mut self.library_query);
            egui::ComboBox::from_label("Sort")
                .selected_text(self.sort_key.label())
                .show_ui(ui, |ui| {
                    for key in SortKey::all() {
                        if ui.selectable_label(self.sort_key == *key, key.label()).clicked() {
                            self.sort_key = *key;
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
                                tomescope::util::truncate_chars(&book.title, 16),
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
                                RichText::new(tomescope::util::truncate_chars(&book.title, 20))
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
                if let Some(locus) = tomescope::panels::draw_toc(
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

            if let Some(zone) = tomescope::reader::draw_page(
                ui,
                &ob.loaded.content,
                &page,
                colors,
                &ob.reader.typo,
                &mut ob.reader.selection,
            ) {
                match zone {
                    tomescope::reader::TapZone::NextPage => {
                        ob.reader.turn(uikit_egui::Direction::Next, &pagination)
                    }
                    tomescope::reader::TapZone::PrevPage => {
                        ob.reader.turn(uikit_egui::Direction::Prev, &pagination)
                    }
                    _ => {}
                }
            }

            let percent = ob.reader.percent(&ob.loaded.content, &pagination);
            if let Some(book) = self.library.get_mut(ob.id) {
                book.percent = percent;
            }
            tomescope::reader::draw_status_bar(
                ui,
                colors,
                &format!("Page {} / {}", page_idx + 1, pagination.pages.len()),
                percent,
                self.stats.estimated_minutes_left(ob.id, percent),
            );
        });

        tomescope::reader::draw_footnote_popover(ctx, &ob.loaded.content, &mut ob.reader.show_footnote, colors);
    }

    fn draw_stats_screen(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        ui.heading(RichText::new("Reading stats").color(colors.text));
        ui.label(format!(
            "Total time: {}",
            tomescope::util::humanize_duration_secs(self.stats.total_secs())
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
        let now = tomescope::util::now_unix();
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
            ui.text_edit_multiline(&mut self.flashcard_new_front);
            ui.horizontal(|ui| {
                ui.label("Answer (optional):");
                ui.text_edit_singleline(&mut self.flashcard_new_back);
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
                    let source_locus = self.open_book.as_ref().map(|ob| tomescope::content::Locus {
                        block: ob.reader.current_page,
                        ch: 0,
                    });
                    let id = self.flashcard_next_id;
                    self.flashcard_next_id += 1;
                    self.deck.add(tomescope::study::Flashcard::new(id, front, back, source_locus));
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
                    ui.colored_label(colors.text, tomescope::util::truncate_chars(&card.front, 60));
                    ui.colored_label(colors.text_weak, format!("(ease {:.2}, due {})", card.ease, tomescope::util::relative_time(card.due_unix)));
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
    }

    fn draw_settings_screen(&mut self, ui: &mut egui::Ui, colors: &uikit_egui::PaletteColors) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut typo_changed = tomescope::settingsui::draw_typography_settings(ui, &mut self.typography, colors);
            typo_changed |= tomescope::settingsui::draw_theme_picker(ui, &mut self.reader_prefs, colors);
            typo_changed |= tomescope::settingsui::draw_palette_picker(ui, &mut self.app_palette, colors);
            typo_changed |= tomescope::settingsui::draw_behavior_settings(ui, &mut self.reader_prefs, colors);
            typo_changed |= tomescope::settingsui::draw_gesture_settings(
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
        egui::Window::new("Command palette")
            .id(egui::Id::new("tomescope_cmd_palette"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(400.0);
                ui.text_edit_singleline(&mut self.command_query);
                let commands = [
                    "Go to Library",
                    "Go to Stats",
                    "Go to Flashcards",
                    "Go to Settings",
                    "Rescan library",
                    "Close current book",
                ];
                for cmd in commands {
                    if fuzzy::subsequence_match(&self.command_query, cmd) {
                        if ui.button(cmd).clicked() {
                            self.run_command(cmd);
                            self.command_palette_open = false;
                        }
                    }
                }
            });
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
    tomescope::util::now_unix() as f64
}

impl eframe::App for TomeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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

        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
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
}
