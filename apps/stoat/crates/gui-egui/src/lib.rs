// stoat-egui — the window onto stoat-core, on uikit-egui.
//
// Ported from the GTK4/libadwaita shell next door (crates/gui-gtk). What maps
// to what:
//
//   AdwViewStack + AdwViewSwitcherBar -> kit segmented_tabs behind
//       responsive_nav's width-fit decision; when five tabs do not fit (a 360 pt
//       phone: 5 x 72 > 340) the nav collapses to "‹ Page ›" and pages swipe.
//   (nothing)                         -> pager::fit_cols + PlanState: on a wide
//       window the active page and its followers sit side by side. Stack beside
//       Vitals beside Wire is how a stop is actually read on a desktop, and the
//       GTK shell could only show one at a time.
//   AdwCarousel of frames             -> the kit GestureLayer over one frame
//       view. On the Stack page a sideways drag walks frames; on every other
//       page it changes page. One gesture model, two meanings, split by page.
//   AdwToastOverlay                   -> kit Toasts, incl. `ask` for the one
//       destructive action (cache delete).
//   GtkTextView wire log              -> kit log_view.
//   cairo DrawingAreas                -> egui painter.
//
// The Pager WIDGET is not used, only its fit and plan layers: the widget owns
// the horizontal drag in its one-column mode, and here the Stack page needs it.

pub mod hold;
pub mod pages;
pub mod palette;
pub mod shell;
pub mod source;

use egui::{Rect, RichText, Ui, UiBuilder};
use shell::{ShellState, PAGES, PAGE_STACK};
use stoat_core::backend::StepKind;
use uikit_egui::pager::{fit_cols, PageMeta, PlanState, GROW_MARGIN};
use uikit_egui::responsive_nav::{self, NavLayout};
use uikit_egui::segmented_tabs::TabStrip;
use uikit_egui::{GestureLayer, PaletteColors};

const GUTTER: f32 = 20.0;
/// The narrowest a page is usable at; also what the dashboard packs columns by.
const PAGE_MIN_W: f32 = 340.0;

/// Widget-side state: layout plan, tab strip, the two gesture layers. Kept
/// apart from `ShellState` so that state stays drawable-free and testable.
pub struct View {
    plan: PlanState,
    strip: TabStrip,
    metas: Vec<PageMeta>,
    page_swipe: GestureLayer,
    frame_swipe: GestureLayer,
    pub palette: palette::Palette,
    /// Long presses on the Stack page: the kit's touch bridge and recogniser,
    /// and what was drawn where last frame.
    touch: uikit_egui::touch::TouchSource,
    recogniser: uikit_egui::touch::GestureRecognizer,
    pub targets: hold::Targets,
}

impl Default for View {
    fn default() -> Self {
        View {
            plan: PlanState::new(),
            strip: TabStrip::new(&PAGES),
            metas: PAGES.iter().enumerate().map(|(id, t)| PageMeta { id, title: t.to_string(), min_width: PAGE_MIN_W }).collect(),
            page_swipe: GestureLayer::horizontal_page(360.0),
            frame_swipe: GestureLayer::horizontal_page(360.0),
            palette: palette::Palette::default(),
            touch: uikit_egui::touch::TouchSource::new(),
            recogniser: uikit_egui::touch::GestureRecognizer::new(uikit_egui::touch::Config::default()),
            targets: hold::Targets::default(),
        }
    }
}

impl View {
    /// Columns the current plan shows (1 = phone).
    pub fn cols(&self) -> usize {
        self.plan.cols()
    }
}

pub fn draw(ui: &mut Ui, st: &mut ShellState, view: &mut View, c: &PaletteColors) {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    if st.pump() {
        // Nothing arrives from gdb while the target runs, but the vitals ring
        // is filling; keep the graphs moving without spinning the CPU.
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    if let Some((text, sev)) = st.pending_toast.take() {
        st.toasts.show(text, sev, now);
    }
    if std::mem::take(&mut st.confirm_prune) && !st.toasts.awaiting_answer() {
        let (files, bytes) = st.planned();
        st.toasts.ask(format!("Delete {files} files ({})? They were unused for {} days.", stoat_core::cache::human_bytes(bytes), st.prune_days), &["Delete", "Keep"], now);
    }
    shortcuts(&ctx, st);
    // egui's Small is 9 pt; on a compact surface the kit raises it to the
    // readable floor. Idempotent, once a frame, after the palette.
    uikit_egui::adaptive::apply_text_floor(&ctx, &uikit_egui::Adaptive::detect(&ctx));

    // ---- chrome ----
    // One label, two formats. Two labels in a wrapping row would each claim the
    // row from its left edge (that is how egui lays out text that may wrap), and
    // layout_probe rightly reads that as text over text.
    ui.horizontal(|ui| {
        let name = if st.program_name.is_empty() { "stoat" } else { &st.program_name };
        let col = match st.state_label() {
            "running" => c.success,
            "stopped" => c.warning,
            _ => c.text_weak,
        };
        let mut job = egui::text::LayoutJob::default();
        job.append(name, 0.0, egui::TextFormat { font_id: egui::FontId::proportional(15.0), color: c.text, ..Default::default() });
        job.append(st.state_label(), 10.0, egui::TextFormat { font_id: egui::FontId::proportional(12.0), color: col, ..Default::default() });
        // The way into the palette that a phone can reach (Ctrl+P is the other).
        // Tap opens it for typing; hold opens it already listening. The timing is
        // the kit's, so a long press feels the same here as in aperture.
        let dt = ui.input(|i| i.stable_dt);
        if view.palette.hold.tick(dt) {
            view.palette.open_listening();
        }
        // Right to left first, so the button keeps its place at the edge and the
        // title truncates into whatever is left.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = ui.add(egui::Button::new(uikit_egui::icons::glyph_or(&ctx, '⌘', "cmd")).min_size(egui::vec2(40.0, 28.0)));
            let hold = &mut view.palette.hold;
            if b.is_pointer_button_down_on() && hold.progress() == 0.0 && !hold.is_open() {
                hold.press();
            }
            if hold.progress() > 0.0 || hold.is_open() {
                // Release is read from the pointer, not from `clicked()`: egui
                // does not report a click for a press held this long, and a hold
                // that is never released would block every later press.
                if !ui.input(|i| i.pointer.primary_down()) {
                    if let Some(uikit_egui::cmdstrip::Release::Tap) = hold.release() {
                        view.palette.open();
                    }
                } else {
                    ui.ctx().request_repaint();
                }
            }
            // A press and release inside ONE frame never shows up as "down on":
            // egui reports it only as a click. Under software GL every tap is like
            // that (seen on screen: hold worked, tap did nothing), and a slow
            // phone will do the same under load. `open` is idempotent, so a tap
            // the hold path also saw opens once.
            if b.clicked() {
                view.palette.open();
            }
            b.on_hover_text(if palette::HAS_VOICE { "commands — tap to type, hold to speak (Ctrl+P)" } else { "commands (Ctrl+P)" });
            if st.busy > 0 {
                ui.spinner();
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(job).truncate());
            });
        });
    });
    // Recents, when asked for: what you just ran is overwhelmingly what you run next.
    if view.palette.strip.visible {
        let entries: Vec<(String, String)> = view.palette.strip.entries().iter().map(|e| (e.id.clone(), e.label.clone())).collect();
        let mut run = None;
        egui::ScrollArea::horizontal().id_salt("stoat-strip").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (id, label) in &entries {
                    if ui.add(egui::Button::new(RichText::new(label).small()).min_size(egui::vec2(44.0, 28.0))).clicked() {
                        run = Some(id.clone());
                    }
                }
            });
        });
        if let Some(id) = run {
            view.palette.run_id(&id, st);
        }
    }

    ui.add_space(uikit_egui::tokens::SPACE_XS as f32);
    let w = ui.available_width();
    let cols = view.plan.decide(fit_cols(w, GUTTER, &view.metas), fit_cols(w * (1.0 - GROW_MARGIN), GUTTER, &view.metas), now).min(PAGES.len());
    let mut pages_mode = false;
    if cols < PAGES.len() {
        view.strip.select(st.page);
        match responsive_nav::nav_ui(ui, &view.strip, responsive_nav::DEFAULT_MIN_TAB_WIDTH, c) {
            (NavLayout::Tabs, Some(i)) => st.page = i,
            (NavLayout::Tabs, None) => {}
            (NavLayout::Pages, _) => {
                pages_mode = true;
                compact_nav(ui, st, c);
            }
        }
    }
    let visible: Vec<usize> = (0..cols).map(|i| (st.page + i) % PAGES.len()).collect();
    if visible.contains(&PAGE_STACK) {
        pages::stack::header(ui, st, c);
    }
    ui.add_space(uikit_egui::tokens::SPACE_SM as f32);

    // ---- body ----
    let body = ui.available_rect_before_wrap();
    if cols <= 1 {
        if pages_mode && st.page != PAGE_STACK {
            *view.page_swipe.config_mut() = uikit_gesture::GestureConfig::for_page_size(f64::from(body.width()));
            let g = view.page_swipe.update(ui, body, true);
            if let Some(dir) = g.page_jump {
                st.page = (st.page as i32 + dir.sign()).rem_euclid(PAGES.len() as i32) as usize;
            }
            if let Some(edge) = g.barrier_edge {
                uikit_egui::paint_barrier(ui, body, g.barrier_fraction, edge, c.accent);
            }
        }
        let page = st.page;
        ui.scope_builder(UiBuilder::new().max_rect(body), |ui| page_ui(ui, page, st, view, c));
    } else {
        let col_w = (body.width() - GUTTER * (cols as f32 - 1.0)) / cols as f32;
        for (i, page) in visible.into_iter().enumerate() {
            let x = body.min.x + i as f32 * (col_w + GUTTER);
            let r = Rect::from_min_size(egui::pos2(x, body.min.y), egui::vec2(col_w, body.height()));
            if i > 0 {
                // Columns are separate pages side by side; say so with a rule in the gutter.
                let gx = x - GUTTER * 0.5;
                ui.painter().vline(gx, body.y_range(), egui::Stroke::new(1.0_f32, uikit_egui::callout::mix(c.bg_base, c.border, 0.6)));
            }
            ui.scope_builder(UiBuilder::new().max_rect(r), |ui| {
                ui.push_id(("stoat-page", page), |ui| {
                    ui.label(RichText::new(PAGES[page]).small().strong().color(c.accent));
                    ui.add_space(uikit_egui::tokens::SPACE_XS as f32);
                    page_ui(ui, page, st, view, c);
                });
            });
        }
    }

    // ---- palette: over everything, under the toast ----
    palette::ui(&ctx, st, &mut view.palette, c);

    // ---- toast, and the one question it can ask ----
    if let Some(answer) = st.toasts.ui_actions(&ctx, 72.0) {
        st.toasts.clear();
        if answer == "Delete" {
            st.apply_prune();
        }
    }
}

fn page_ui(ui: &mut Ui, page: usize, st: &mut ShellState, view: &mut View, c: &PaletteColors) {
    match page {
        shell::PAGE_LAUNCH => pages::launch::ui(ui, st, c),
        shell::PAGE_STACK => {
            // Gestures from this frame's input, against what was drawn last frame.
            let (gestures, _) = view.touch.poll(ui.ctx(), &mut view.recogniser);
            pages::stack::ui(ui, st, c, &mut view.frame_swipe, &mut view.targets, &gestures);
        }
        shell::PAGE_VITALS => pages::vitals::ui(ui, st, c),
        shell::PAGE_WIRE => pages::wire::ui(ui, st, c),
        _ => pages::caches::ui(ui, st, c),
    }
}

/// Too narrow for the strip: previous / name / next, each a full-height target.
fn compact_nav(ui: &mut Ui, st: &mut ShellState, c: &PaletteColors) {
    let n = PAGES.len();
    ui.horizontal(|ui| {
        let side = 44.0;
        if ui.add_sized([side, 32.0], egui::Button::new("‹")).clicked() {
            st.page = (st.page + n - 1) % n;
        }
        let mid = (ui.available_width() - side - 8.0).max(60.0);
        ui.add_sized([mid, 32.0], egui::Label::new(RichText::new(format!("{}  ·  {}/{n}", PAGES[st.page], st.page + 1)).strong().color(c.accent)));
        if ui.add_sized([side, 32.0], egui::Button::new("›")).clicked() {
            st.page = (st.page + 1) % n;
        }
    });
}

/// gdb-frontend conventions, for the desktop half of the fleet. Inert while a
/// text field has the keyboard.
fn shortcuts(ctx: &egui::Context, st: &mut ShellState) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    // (The palette's own keys are handled in palette::ui, which consumes them.)
    use egui::Key;
    let (f5, f10, f11, shift, alt_l, alt_r) = ctx.input(|i| (i.key_pressed(Key::F5), i.key_pressed(Key::F10), i.key_pressed(Key::F11), i.modifiers.shift, i.modifiers.alt && i.key_pressed(Key::ArrowLeft), i.modifiers.alt && i.key_pressed(Key::ArrowRight)));
    if f5 {
        st.go_or_interrupt();
    }
    if f10 {
        st.step(StepKind::Over);
    }
    if f11 {
        st.step(if shift { StepKind::Out } else { StepKind::Into });
    }
    if alt_l {
        st.toward_callee();
    }
    if alt_r {
        st.toward_caller();
    }
}
