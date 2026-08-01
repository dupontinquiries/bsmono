//! The reader screen itself: turns a `Pagination` + `BookContent` into
//! interactive egui UI, handles page-turn gestures, text selection,
//! in-book search, footnote popovers, and the raster (CBZ/PDF-image)
//! viewing path for image-first books.

use crate::content::{BookContent, Locus, LocusRange};
use crate::layout::{ApproxMeasurer, Page, Pagination, PaginationCache, Typography};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use uikit_egui::{motion, palette_colors, tokens, PaletteColors};
use uikit_egui::{Direction, GestureConfig};
use uikit_egui::GestureLayer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaderMode {
    Paged,
    Scroll,
    Spread,
}

impl ReaderMode {
    pub fn all() -> &'static [ReaderMode] {
        &[ReaderMode::Paged, ReaderMode::Scroll, ReaderMode::Spread]
    }
    pub fn label(&self) -> &'static str {
        match self {
            ReaderMode::Paged => "Paged",
            ReaderMode::Scroll => "Scroll",
            ReaderMode::Spread => "Two-page spread",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapZone {
    PrevPage,
    NextPage,
    Menu,
    None,
}

/// Classify a tap position within the reading surface into a navigation
/// zone: left third = previous page, right third = next page, centre =
/// bring up the reading menu. This mirrors the classic ereader tap map.
pub fn classify_tap(rect: Rect, pos: Pos2, rtl: bool) -> TapZone {
    if !rect.contains(pos) {
        return TapZone::None;
    }
    let frac = (pos.x - rect.left()) / rect.width().max(1.0);
    let (left_zone, right_zone) = if rtl {
        (TapZone::NextPage, TapZone::PrevPage)
    } else {
        (TapZone::PrevPage, TapZone::NextPage)
    };
    if frac < 0.28 {
        left_zone
    } else if frac > 0.72 {
        right_zone
    } else {
        TapZone::Menu
    }
}

/// A pending in-book text selection, tracked in locus space so it
/// survives repagination (font-size changes mid-selection, etc).
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub anchor: Option<Locus>,
    pub head: Option<Locus>,
}

impl Selection {
    pub fn range(&self) -> Option<LocusRange> {
        match (self.anchor, self.head) {
            (Some(a), Some(b)) => Some(LocusRange { from: a, to: b }.normalized()),
            _ => None,
        }
    }
    pub fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
    }
    pub fn is_active(&self) -> bool {
        self.anchor.is_some() && self.head.is_some()
    }
}

/// Search-in-book session state, decoupled from the drawing code so the
/// same results can drive both a results list panel and highlighted
/// occurrences on the current page.
#[derive(Clone, Debug, Default)]
pub struct SearchSession {
    pub query: String,
    pub results: Vec<LocusRange>,
    pub active: usize,
}

impl SearchSession {
    pub fn run(&mut self, content: &BookContent, cap: usize) {
        self.results = if self.query.trim().is_empty() {
            Vec::new()
        } else {
            content.search(&self.query, cap)
        };
        self.active = 0;
    }
    pub fn next(&mut self) {
        if !self.results.is_empty() {
            self.active = (self.active + 1) % self.results.len();
        }
    }
    pub fn prev(&mut self) {
        if !self.results.is_empty() {
            self.active = (self.active + self.results.len() - 1) % self.results.len();
        }
    }
    pub fn current(&self) -> Option<LocusRange> {
        self.results.get(self.active).copied()
    }
}

/// Page-turn animation state, driven by `uikit_egui::motion` easing.
pub struct PageTurnAnim {
    pub anim_id: egui::Id,
    pub direction: Option<Direction>,
}

impl Default for PageTurnAnim {
    fn default() -> Self {
        Self {
            anim_id: egui::Id::new("tomescope_page_turn"),
            direction: None,
        }
    }
}

/// Everything the reader screen needs across frames for one open book.
pub struct ReaderState {
    pub mode: ReaderMode,
    pub typo: Typography,
    pub cache: PaginationCache,
    pub current_page: usize,
    pub selection: Selection,
    pub search: SearchSession,
    pub gesture: GestureLayer,
    pub anim: PageTurnAnim,
    pub scroll_offset: f32,
    pub raster_zoom: f32,
    pub raster_pan: Vec2,
    pub raster_page: usize,
    pub show_footnote: Option<usize>,
    pub rtl: bool,
    pub pending_locus: Option<Locus>,
}

impl Default for ReaderState {
    fn default() -> Self {
        Self {
            mode: ReaderMode::Paged,
            typo: Typography::default(),
            cache: PaginationCache::default(),
            current_page: 0,
            selection: Selection::default(),
            search: SearchSession::default(),
            gesture: GestureLayer::new(uikit_egui::Axis::Horizontal, GestureConfig::default()),
            anim: PageTurnAnim::default(),
            scroll_offset: 0.0,
            raster_zoom: 1.0,
            raster_pan: Vec2::ZERO,
            raster_page: 0,
            show_footnote: None,
            rtl: false,
            pending_locus: None,
        }
    }
}

impl ReaderState {
    pub fn open_book(&mut self, content: &BookContent, at: Locus) {
        self.cache = PaginationCache::default();
        self.selection.clear();
        self.search = SearchSession::default();
        self.raster_page = 0;
        self.rtl = content
            .raster
            .as_ref()
            .map(|r| r.rtl)
            .unwrap_or(false);
        self.go_to_locus(content, at);
    }

    pub fn go_to_locus(&mut self, content: &BookContent, locus: Locus) {
        // We need a pagination to resolve the page; caller supplies one
        // via `paginate_now`. Here we just remember the target block so
        // the next paginate_now call can seek.
        self.pending_locus = Some(locus);
        let _ = content;
    }

    /// Returns an owned `Rc<Pagination>` (not `&Pagination`): the cache
    /// entry may need to be recomputed on a later call, and — more
    /// importantly — callers (the reader screen) need to call other
    /// `&mut self` methods on this same `ReaderState` (e.g. `turn`)
    /// while still holding the result of this call. A borrow tied to
    /// `&mut self` would make that impossible; an owned `Rc` clone does
    /// not.
    pub fn paginate_now(
        &mut self,
        book_id: u64,
        content: &BookContent,
        width: f32,
        height: f32,
    ) -> std::rc::Rc<Pagination> {
        let measurer = ApproxMeasurer::default();
        let pagination =
            self.cache
                .get_or_compute(book_id, content, &measurer, &self.typo, width, height);
        if let Some(locus) = self.pending_locus.take() {
            self.current_page = pagination.page_for_locus(locus);
        }
        self.current_page = self.current_page.min(pagination.pages.len().saturating_sub(1));
        pagination
    }

    pub fn percent(&self, content: &BookContent, pagination: &Pagination) -> f32 {
        pagination
            .pages
            .get(self.current_page)
            .map(|p| content.percent_at(p.start))
            .unwrap_or(0.0)
    }

    pub fn turn(&mut self, dir: Direction, pagination: &Pagination) {
        match dir {
            Direction::Next => {
                if self.current_page + 1 < pagination.pages.len() {
                    self.current_page += 1;
                    self.anim.direction = Some(Direction::Next);
                }
            }
            Direction::Prev => {
                if self.current_page > 0 {
                    self.current_page -= 1;
                    self.anim.direction = Some(Direction::Prev);
                }
            }
        }
    }
}

/// Draw the paged reading surface for the current page. Returns the tap
/// zone if the user tapped (so the caller can drive page turns / menu
/// toggling), and updates selection state on drag.
pub fn draw_page(
    ui: &mut egui::Ui,
    content: &BookContent,
    page: &Page,
    colors: &PaletteColors,
    typo: &Typography,
    selection: &mut Selection,
) -> Option<TapZone> {
    let rect = ui.available_rect_before_wrap();
    let response = ui.interact(rect, ui.id().with("reader_surface"), Sense::click_and_drag());

    let margin = rect.width() * typo.margin_frac;
    let text_rect = Rect::from_min_max(
        rect.min + Vec2::new(margin, tokens::SPACE_LG as f32),
        rect.max - Vec2::new(margin, tokens::SPACE_LG as f32),
    );

    let painter = ui.painter_at(rect);
    let mut cursor_y = text_rect.top();
    for line in &page.lines {
        let block = &content.blocks[line.block];
        let text = block.plain_text();
        let slice: String = text
            .chars()
            .skip(line.char_start)
            .take(line.char_end.saturating_sub(line.char_start))
            .collect();
        let is_heading = matches!(block, crate::content::Block::Heading { .. });
        let color = if is_heading { colors.text } else { colors.text };
        let size = if is_heading {
            typo.font_size * 1.25
        } else {
            typo.font_size
        };
        painter.text(
            Pos2::new(text_rect.left(), cursor_y),
            egui::Align2::LEFT_TOP,
            slice,
            egui::FontId::proportional(size),
            color,
        );
        cursor_y += line.height;
    }

    // Selection highlight (very approximate: highlights full lines within
    // the selection's block range — a real glyph-accurate highlight needs
    // galley cursor math wired to the real egui font measurer, which the
    // production draw path uses instead of ApproxMeasurer).
    if let Some(range) = selection.range() {
        let mut y = text_rect.top();
        for line in &page.lines {
            let in_range = line.block >= range.from.block && line.block <= range.to.block;
            if in_range {
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(text_rect.left(), y),
                        Vec2::new(text_rect.width(), line.height),
                    ),
                    egui::CornerRadius::same(0),
                    colors.accent_soft.gamma_multiply(0.35),
                );
            }
            y += line.height;
        }
    }

    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            selection.anchor = pick_locus_at(page, content, text_rect, pos);
        }
    }
    if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            selection.head = pick_locus_at(page, content, text_rect, pos);
        }
    }

    if response.clicked() && !response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            return Some(classify_tap(rect, pos, false));
        }
    }
    None
}

fn pick_locus_at(page: &Page, _content: &BookContent, text_rect: Rect, pos: Pos2) -> Option<Locus> {
    let mut y = text_rect.top();
    for line in &page.lines {
        if pos.y >= y && pos.y < y + line.height {
            // Approximate horizontal position -> char offset within line.
            let frac = ((pos.x - text_rect.left()) / text_rect.width().max(1.0)).clamp(0.0, 1.0);
            let span = line.char_end.saturating_sub(line.char_start);
            let ch = line.char_start + (frac * span as f32) as usize;
            return Some(Locus {
                block: line.block,
                ch,
            });
        }
        y += line.height;
    }
    None
}

/// Draw the footnote popover for the currently-open footnote, if any.
pub fn draw_footnote_popover(
    ctx: &egui::Context,
    content: &BookContent,
    open: &mut Option<usize>,
    colors: &PaletteColors,
) {
    let Some(idx) = *open else { return };
    let Some(fnote) = content.footnotes.get(idx) else {
        *open = None;
        return;
    };
    let mut still_open = true;
    egui::Window::new("Footnote")
        .id(egui::Id::new("tomescope_footnote_popover"))
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .show(ctx, |ui| {
            ui.set_max_width(360.0);
            ui.colored_label(colors.text_weak, format!("[{}]", fnote.label));
            ui.label(RichText::new(crate::content::join_spans(&fnote.spans)).color(colors.text));
        });
    if !still_open {
        *open = None;
    }
}

/// Draw the raster (image-page) viewer used for CBZ pages and rasterized
/// PDF fallback, with basic pinch/drag zoom-pan.
pub fn draw_raster_page(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    zoom: &mut f32,
    pan: &mut Vec2,
) {
    let rect = ui.available_rect_before_wrap();
    let response = ui.interact(rect, ui.id().with("raster_surface"), Sense::drag());
    if response.dragged() {
        *pan += response.drag_delta();
    }
    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
    if scroll != 0.0 {
        *zoom = (*zoom * (1.0 + scroll * 0.001)).clamp(0.2, 6.0);
    }
    if let Some(tex) = texture {
        let img_size = tex.size_vec2() * *zoom;
        let center = rect.center() + *pan;
        let img_rect = Rect::from_center_size(center, img_size);
        ui.painter().image(
            tex.id(),
            img_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        ui.painter().rect_stroke(
            rect.shrink(20.0),
            egui::CornerRadius::same(0),
            Stroke::new(1.0, Color32::GRAY),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Loading page…",
            egui::FontId::proportional(16.0),
            Color32::GRAY,
        );
    }
}

/// A thin progress/status bar drawn at the bottom of the reader.
pub fn draw_status_bar(
    ui: &mut egui::Ui,
    colors: &PaletteColors,
    page_label: &str,
    percent: f32,
    time_left_mins: Option<u32>,
) {
    ui.horizontal(|ui| {
        ui.colored_label(colors.text_weak, page_label);
        ui.add_space(tokens::SPACE_SM as f32);
        let bar_rect = ui.available_rect_before_wrap();
        let bar_h = 4.0;
        let bar = Rect::from_min_size(
            bar_rect.min,
            Vec2::new(bar_rect.width() - 90.0, bar_h),
        );
        ui.painter().rect_filled(bar, egui::CornerRadius::same(2), colors.border);
        let filled = Rect::from_min_size(
            bar.min,
            Vec2::new(bar.width() * percent.clamp(0.0, 1.0), bar_h),
        );
        ui.painter().rect_filled(filled, egui::CornerRadius::same(2), colors.accent);
        ui.add_space(bar.width() + 8.0);
        if let Some(mins) = time_left_mins {
            ui.colored_label(colors.text_weak, format!("{mins} min left"));
        } else {
            ui.colored_label(colors.text_weak, crate::util::percent_string(percent));
        }
    });
}

/// Apply an out-cubic easing offset for the page-turn slide animation.
/// `animate_eased` here animates a boolean "is turning" flag toward 0/1
/// with the given easing curve — we use the eased value to slide the new
/// page in from the turn direction and fade the old one out.
pub fn page_turn_offset(ctx: &egui::Context, anim: &PageTurnAnim, page_width: f32) -> f32 {
    let turning = anim.direction.is_some();
    let eased = motion::animate_eased(ctx, anim.anim_id, turning, (tokens::ANIM_NORMAL as f32) / 1000.0, motion::ease::out_cubic);
    (1.0 - eased) * page_width
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Pos2, Rect};

    #[test]
    fn tap_zones_split_thirds() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 400.0));
        assert_eq!(classify_tap(rect, Pos2::new(10.0, 200.0), false), TapZone::PrevPage);
        assert_eq!(classify_tap(rect, Pos2::new(150.0, 200.0), false), TapZone::Menu);
        assert_eq!(classify_tap(rect, Pos2::new(290.0, 200.0), false), TapZone::NextPage);
    }

    #[test]
    fn tap_zones_flip_for_rtl() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 400.0));
        assert_eq!(classify_tap(rect, Pos2::new(10.0, 200.0), true), TapZone::NextPage);
        assert_eq!(classify_tap(rect, Pos2::new(290.0, 200.0), true), TapZone::PrevPage);
    }

    #[test]
    fn search_session_wraps() {
        let mut s = SearchSession::default();
        s.results = vec![
            LocusRange { from: Locus::START, to: Locus::START },
            LocusRange { from: Locus::START, to: Locus::START },
        ];
        s.active = 1;
        s.next();
        assert_eq!(s.active, 0);
        s.prev();
        assert_eq!(s.active, 1);
    }
}
