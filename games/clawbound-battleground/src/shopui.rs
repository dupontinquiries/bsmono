//! shopui — the Hoard: the shop drawn as a clan's store of gifts.
//!
//! Every decision comes from `shopview`; this file only paints. The look:
//! a bark board with a vine along its rim, shelves as carved stones with a
//! glyph each (claw marks, whiskers, a star, a pelt, a fang, pounce
//! chevrons, a heart, a trophy bone), items as plaques with a leaf tab in
//! the shelf's colour and tier shown as one to three leaves. All vector,
//! drawn with the egui painter -- no textures, nothing to load, and it
//! scales with the screen.
//!
//! Two layouts (`shopview::layout_for`): side by side from 760 pt up, and a
//! single column on a phone, where the build sits behind a second tab
//! instead of being squeezed beside the shelf.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use crate::consts;
use crate::items;
use crate::shopview::{self, Glyph, Layout, Offer, Refusal};
use crate::sim::Game;
use crate::ui::{glow_hover, hover_t, pulse_get, pulse_set, UiState, GOLD};

// ── palette ─────────────────────────────────────────────────────────────
const BARK_DARK: Color32 = Color32::from_rgb(28, 20, 14);
const BARK: Color32 = Color32::from_rgb(46, 33, 22);
const BARK_LIGHT: Color32 = Color32::from_rgb(70, 52, 34);
const MOSS: Color32 = Color32::from_rgb(78, 118, 58);
const VINE: Color32 = Color32::from_rgb(52, 92, 40);
const INK: Color32 = Color32::from_rgb(236, 226, 204);
const INK_DIM: Color32 = Color32::from_rgb(160, 146, 120);
const EMBER: Color32 = Color32::from_rgb(214, 124, 92);

/// Which half a phone shows. Desktop shows both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PhoneTab {
    #[default]
    Shelves,
    Hoard,
}

fn c32(c: [f32; 3]) -> Color32 {
    Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8)
}

/// Cheap deterministic noise for grain, keyed on position so a board does
/// not shimmer from frame to frame.
fn hash(a: f32, b: f32) -> f32 {
    let x = (a * 12.9898 + b * 78.233).sin() * 43758.547;
    x - x.floor()
}

/// A bark board: dark fill, vertical grain, a lighter lip, rounded.
fn bark(p: &egui::Painter, r: Rect, rounding: f32, tone: f32) {
    let fill = BARK.linear_multiply(tone);
    p.rect_filled(r, rounding, fill);
    let step = 7.0;
    let mut x = r.left() + 3.0;
    while x < r.right() - 3.0 {
        let h = hash(x, r.top());
        if h > 0.35 {
            let wob = (h - 0.5) * 4.0;
            let a = Pos2::new(x, r.top() + 4.0 + h * 10.0);
            let b = Pos2::new(x + wob, r.bottom() - 4.0 - hash(r.bottom(), x) * 10.0);
            p.line_segment([a, b], Stroke::new(1.0, BARK_DARK.linear_multiply(0.55 * tone)));
        }
        x += step * (0.6 + h);
    }
    p.rect_stroke(r, rounding, Stroke::new(1.5, BARK_LIGHT.linear_multiply(tone)));
}

/// A leaf, pointing along `dir`, `len` long.
fn leaf(p: &egui::Painter, at: Pos2, dir: Vec2, len: f32, fill: Color32) {
    let d = dir.normalized() * len;
    let n = Vec2::new(-d.y, d.x) * 0.38;
    let tip = at + d;
    let pts = vec![at, at + d * 0.5 + n, tip, at + d * 0.5 - n];
    p.add(Shape::convex_polygon(pts, fill, Stroke::new(1.0, fill.linear_multiply(0.6))));
    p.line_segment([at, at + d * 0.8], Stroke::new(0.8, fill.linear_multiply(0.45)));
}

/// A vine along the top and bottom edges of `r`, with leaves.
fn vine_rim(p: &egui::Painter, r: Rect) {
    for (y, flip) in [(r.top() + 2.0, 1.0f32), (r.bottom() - 2.0, -1.0)] {
        let mut pts = Vec::new();
        let mut x = r.left() + 10.0;
        while x <= r.right() - 10.0 {
            pts.push(Pos2::new(x, y + flip * (3.0 + 2.5 * (x * 0.045).sin())));
            x += 6.0;
        }
        if pts.len() > 1 {
            p.add(Shape::line(pts.clone(), Stroke::new(2.0, VINE)));
            for (k, q) in pts.iter().enumerate().step_by(5) {
                let side = if k % 10 == 0 { 1.0 } else { -1.0 };
                leaf(p, *q, Vec2::new(0.6 * side, flip * 1.0), 9.0, MOSS);
            }
        }
    }
}

/// The picture on a shelf's stone. Everything is built from lines, circles
/// and convex polygons centred on `c`, `s` across.
pub fn glyph(p: &egui::Painter, c: Pos2, s: f32, g: Glyph, col: Color32) {
    let st = Stroke::new((s * 0.09).max(1.5), col);
    let h = s * 0.5;
    match g {
        Glyph::Claw => {
            for k in -1..=1 {
                let dx = k as f32 * s * 0.24;
                let pts: Vec<Pos2> = (0..=6)
                    .map(|i| {
                        let t = i as f32 / 6.0;
                        Pos2::new(c.x + dx + (t - 0.5) * s * 0.35 + (t * 3.1).sin() * s * 0.05, c.y - h * 0.8 + t * s * 0.8)
                    })
                    .collect();
                p.add(Shape::line(pts, st));
            }
        }
        Glyph::Whisker => {
            p.circle_filled(c, s * 0.08, col);
            for k in [-1.0f32, 0.0, 1.0] {
                for side in [-1.0f32, 1.0] {
                    let a = c + Vec2::new(side * s * 0.12, k * s * 0.06);
                    let b = c + Vec2::new(side * h * 0.95, k * s * 0.2 - s * 0.02);
                    p.line_segment([a, b], Stroke::new(st.width * 0.7, col));
                }
            }
        }
        Glyph::Star => {
            let pts: Vec<Pos2> = (0..10)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::PI / 5.0 - std::f32::consts::FRAC_PI_2;
                    let r = if i % 2 == 0 { h * 0.9 } else { h * 0.38 };
                    c + Vec2::new(a.cos(), a.sin()) * r
                })
                .collect();
            p.add(Shape::closed_line(pts, st));
        }
        Glyph::Pelt => {
            let pts = vec![
                c + Vec2::new(-h * 0.7, -h * 0.7),
                c + Vec2::new(h * 0.7, -h * 0.7),
                c + Vec2::new(h * 0.6, h * 0.1),
                c + Vec2::new(0.0, h * 0.85),
                c + Vec2::new(-h * 0.6, h * 0.1),
            ];
            p.add(Shape::closed_line(pts, st));
            for k in 0..3 {
                let y = c.y - h * 0.35 + k as f32 * h * 0.35;
                p.line_segment([Pos2::new(c.x - h * 0.3, y), Pos2::new(c.x + h * 0.3, y + h * 0.08)], Stroke::new(st.width * 0.6, col));
            }
        }
        Glyph::Fang => {
            for side in [-1.0f32, 1.0] {
                let x = c.x + side * h * 0.35;
                let pts = vec![Pos2::new(x - h * 0.22, c.y - h * 0.7), Pos2::new(x + h * 0.22, c.y - h * 0.7), Pos2::new(x + side * h * 0.05, c.y + h * 0.8)];
                p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
            }
        }
        Glyph::Pounce => {
            for k in 0..3 {
                let x = c.x - h * 0.6 + k as f32 * h * 0.5;
                p.add(Shape::line(vec![Pos2::new(x, c.y - h * 0.6), Pos2::new(x + h * 0.45, c.y), Pos2::new(x, c.y + h * 0.6)], st));
            }
        }
        Glyph::Heart => {
            let r = h * 0.42;
            p.circle_filled(c + Vec2::new(-r * 0.85, -r * 0.4), r, col);
            p.circle_filled(c + Vec2::new(r * 0.85, -r * 0.4), r, col);
            let pts = vec![c + Vec2::new(-r * 1.75, -r * 0.15), c + Vec2::new(r * 1.75, -r * 0.15), c + Vec2::new(0.0, h * 0.9)];
            p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
        }
        Glyph::Trophy => {
            let a = c + Vec2::new(-h * 0.7, h * 0.5);
            let b = c + Vec2::new(h * 0.7, -h * 0.5);
            p.line_segment([a, b], Stroke::new(st.width * 1.4, col));
            for e in [a, b] {
                let n = Vec2::new(1.0, 1.0).normalized() * h * 0.2;
                p.circle_filled(e + n, h * 0.18, col);
                p.circle_filled(e - n, h * 0.18, col);
            }
        }
    }
}

/// A gold nugget and an amount.
fn gold_mark(p: &egui::Painter, at: Pos2, amount: f32, size: f32, col: Color32, align: Align2) -> Rect {
    let text = format!("{amount:.0}");
    let galley = p.layout_no_wrap(text, FontId::proportional(size), col);
    let w = galley.size().x + size * 0.9;
    let r = align.anchor_size(at, Vec2::new(w, size));
    p.circle_filled(Pos2::new(r.left() + size * 0.3, r.center().y), size * 0.3, GOLD);
    p.circle_stroke(Pos2::new(r.left() + size * 0.3, r.center().y), size * 0.3, Stroke::new(1.0, GOLD.linear_multiply(0.5)));
    p.galley(Pos2::new(r.left() + size * 0.8, r.center().y - galley.size().y * 0.5), galley, col);
    r
}

/// The shop. Same signature the app has always called.
pub fn draw(ctx: &egui::Context, game: &mut Game, player: usize, st: &mut UiState) {
    let screen = ctx.screen_rect();
    let layout = shopview::layout_for(screen.width(), screen.height());
    let size = match layout {
        Layout::Wide | Layout::Short => Vec2::new(980.0f32.min(screen.width() - 24.0), 680.0f32.min(screen.height() - 16.0)),
        Layout::Narrow => Vec2::new(screen.width(), (screen.height() * 0.86).max(320.0)),
    };
    let rect = match layout {
        Layout::Wide | Layout::Short => Rect::from_center_size(screen.center(), size),
        Layout::Narrow => Rect::from_min_size(Pos2::new(screen.left(), screen.bottom() - size.y), size),
    };
    st.shop_pick.cols = if layout == Layout::Narrow { 1 } else { 2 };

    egui::Area::new(egui::Id::new("hoard"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_clip_rect(rect);
            let p = ui.painter().clone();
            // shade the world behind, then the board
            p.rect_filled(screen, 0.0, Color32::from_black_alpha(110));
            bark(&p, rect, if layout == Layout::Narrow { 0.0 } else { 14.0 }, 1.0);
            vine_rim(&p, rect);
            let inner = rect.shrink2(Vec2::new(16.0, 20.0));
            let inner = Rect::from_min_max(inner.min, Pos2::new(inner.right(), inner.bottom() - 6.0));
            let mut child = ui.child_ui(inner, egui::Layout::top_down(egui::Align::Min));
            header(&mut child, game, player, st, layout);
            shelves_row(&mut child, game, player, st, layout);
            match layout {
                Layout::Wide | Layout::Short => {
                    let rest = child.available_rect_before_wrap();
                    let split = rest.left() + rest.width() * 0.62;
                    let left = Rect::from_min_max(rest.min, Pos2::new(split - 8.0, rest.bottom()));
                    let right = Rect::from_min_max(Pos2::new(split + 8.0, rest.top()), rest.max);
                    let mut l = child.child_ui(left, egui::Layout::top_down(egui::Align::Min));
                    shelf(&mut l, game, player, st, layout);
                    let mut r = child.child_ui(right, egui::Layout::top_down(egui::Align::Min));
                    hoard(&mut r, game, player, st);
                }
                Layout::Narrow => {
                    phone_tabs(&mut child, st);
                    match st.shop_phone_tab {
                        PhoneTab::Shelves => shelf(&mut child, game, player, st, layout),
                        PhoneTab::Hoard => hoard(&mut child, game, player, st),
                    }
                }
            }
        });
}

fn header(ui: &mut egui::Ui, game: &Game, player: usize, st: &mut UiState, layout: Layout) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), if layout == Layout::Short { 34.0 } else { 46.0 }), Sense::hover());
    let p = ui.painter();
    p.text(r.left_top() + Vec2::new(2.0, 0.0), Align2::LEFT_TOP, "THE HOARD", FontId::proportional(24.0), GOLD);
    if layout == Layout::Wide {
        p.text(r.left_top() + Vec2::new(2.0, 28.0), Align2::LEFT_TOP, "gifts of the old clans, traded for gold", FontId::proportional(12.0), INK_DIM);
    }
    let g = &game.gods[player];
    let gr = gold_mark(p, r.right_top() + Vec2::new(-4.0, 2.0), g.gold, 20.0, INK, Align2::RIGHT_TOP);
    // What to buy next: a pointer, one tap to its shelf.
    if let Some(s) = shopview::suggestion(game, player) {
        let text = format!("next: {} ", s.name);
        let galley = p.layout_no_wrap(text, FontId::proportional(12.0), MOSS.linear_multiply(1.6));
        // Under the purse, except on a landscape phone, where the header is
        // one line and there is room beside it instead.
        let tr = if layout != Layout::Short {
            Align2::RIGHT_TOP.anchor_size(Pos2::new(gr.right(), gr.bottom() + 6.0), galley.size())
        } else {
            Align2::RIGHT_CENTER.anchor_size(Pos2::new(gr.left() - 14.0, gr.center().y), galley.size())
        };
        p.galley(tr.min, galley, MOSS.linear_multiply(1.6));
        let resp = ui.interact(tr.expand(3.0), egui::Id::new("hoard_next"), Sense::click());
        if resp.clicked() {
            st.shop_tab = s.cat;
            st.shop_pick.set_search(String::new());
            st.shop_phone_tab = PhoneTab::Shelves;
        }
    }
}

fn shelves_row(ui: &mut egui::Ui, game: &Game, player: usize, st: &mut UiState, layout: Layout) {
    let rec = shopview::recommended(game, player);
    let dens = shopview::dens();
    let w = ui.available_width();
    // A stone per shelf; on a phone, glyph-only stones so all eight fit one row.
    let named = layout == Layout::Wide;
    let gap = 6.0;
    let stone_w = ((w - gap * 7.0) / 8.0).max(34.0);
    let stone_h = if named { 58.0 } else { 44.0 };
    let (row, _) = ui.allocate_exact_size(Vec2::new(w, stone_h + 4.0), Sense::hover());
    for (k, d) in dens.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(row.left() + k as f32 * (stone_w + gap), row.top()), Vec2::new(stone_w, stone_h));
        let id = egui::Id::new("hoard_shelf").with(k);
        let resp = ui.interact(r, id, Sense::click());
        let sel = st.shop_tab == d.cat;
        let t = hover_t(ui.ctx(), id, resp.hovered());
        let col = c32(d.color);
        let p = ui.painter();
        let stone = if sel { Color32::from_rgb(92, 84, 70) } else { Color32::from_rgb(58, 54, 48) };
        p.rect_filled(r.expand(t), 10.0, stone.linear_multiply(1.0 + 0.2 * t));
        p.rect_stroke(r.expand(t), 10.0, Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { col } else { BARK_DARK }));
        let gy = if named { r.top() + 22.0 } else { r.center().y };
        glyph(p, Pos2::new(r.center().x, gy), 22.0, d.glyph, if sel { col } else { col.linear_multiply(0.75) });
        if named {
            p.text(Pos2::new(r.center().x, r.bottom() - 6.0), Align2::CENTER_BOTTOM, d.name, FontId::proportional(12.0), if sel { INK } else { INK_DIM });
        }
        if rec.contains(&d.cat) {
            leaf(p, r.right_top() + Vec2::new(-10.0, 4.0), Vec2::new(0.5, 1.0), 9.0, MOSS.linear_multiply(1.4));
        }
        if resp.clicked() {
            st.shop_tab = d.cat;
            st.shop_pick.cursor = 0;
        }
        resp.on_hover_text(format!("{} — {}", d.name, d.blurb));
    }
    let d = shopview::den(st.shop_tab);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(d.name).size(15.0).color(c32(d.color)).strong());
        ui.label(egui::RichText::new(d.blurb).size(12.0).color(INK_DIM));
        if rec.contains(&d.cat) {
            ui.label(egui::RichText::new("· your role builds this first").size(12.0).color(MOSS.linear_multiply(1.5)));
        }
    });
}

fn phone_tabs(ui: &mut egui::Ui, st: &mut UiState) {
    ui.horizontal(|ui| {
        for (tab, name) in [(PhoneTab::Shelves, "Shelves"), (PhoneTab::Hoard, "Your hoard")] {
            let sel = st.shop_phone_tab == tab;
            let b = egui::Button::new(egui::RichText::new(name).size(15.0).color(if sel { BARK_DARK } else { INK }))
                .fill(if sel { GOLD } else { BARK_LIGHT })
                .min_size(Vec2::new(120.0, 36.0));
            if ui.add(b).clicked() {
                st.shop_phone_tab = tab;
            }
        }
    });
    ui.add_space(4.0);
}

fn offer_text<'a>(o: &'a Offer) -> (&'a str, &'a str) {
    (o.item.name, o.item.desc)
}

fn shelf(ui: &mut egui::Ui, game: &mut Game, player: usize, st: &mut UiState, layout: Layout) {
    let offers = shopview::offers(game, player, st.shop_tab);
    let search_id = egui::Id::new("shop_search");
    ui.visuals_mut().extreme_bg_color = BARK_DARK;
    ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::new(1.0, BARK_LIGHT);
    let sresp = ui.add(
        egui::TextEdit::singleline(&mut st.shop_pick.search)
            .id(search_id)
            .hint_text("sniff out an item…")
            .desired_width(ui.available_width().min(260.0)),
    );
    if sresp.changed() {
        let s = st.shop_pick.search.clone();
        st.shop_pick.set_search(s);
    }
    let shown = st.shop_pick.shown(&offers, offer_text);
    if !sresp.has_focus() {
        let (dx, dy, enter) = ui.ctx().input(|i| {
            (
                i.key_pressed(egui::Key::ArrowRight) as i32 - i.key_pressed(egui::Key::ArrowLeft) as i32,
                i.key_pressed(egui::Key::ArrowDown) as i32 - i.key_pressed(egui::Key::ArrowUp) as i32,
                i.key_pressed(egui::Key::Enter),
            )
        });
        if dx != 0 || dy != 0 {
            st.shop_pick.move_cursor(dx, dy, shown.len());
        }
        if enter {
            if let Some(i) = st.shop_pick.hovered(&offers, offer_text) {
                buy(ui.ctx(), game, player, &offers[i]);
            }
        }
    }
    ui.add_space(4.0);
    let cols = st.shop_pick.cols.max(1);
    let card_w = ((ui.available_width() - 10.0 * (cols as f32 - 1.0)) / cols as f32).max(200.0);
    let card_h = match layout {
        Layout::Wide => 98.0,
        Layout::Short => 86.0,
        Layout::Narrow => 104.0,
    };
    let suggested = shopview::suggestion(game, player).map(|i| i.id);
    egui::ScrollArea::vertical().id_source("hoard_items").auto_shrink([false, false]).show(ui, |ui| {
        if shown.is_empty() {
            ui.add_space(16.0);
            ui.label(egui::RichText::new("nothing here smells like that").size(13.0).color(INK_DIM));
        }
        for (row, chunk) in shown.chunks(cols).enumerate() {
            ui.horizontal(|ui| {
                for (col, &i) in chunk.iter().enumerate() {
                    let cell = row * cols + col;
                    let o = offers[i];
                    let clicked = card(ui, &o, Vec2::new(card_w, card_h), st.shop_pick.cursor == cell, suggested == Some(o.item.id));
                    if clicked {
                        st.shop_pick.point(cell, shown.len());
                        buy(ui.ctx(), game, player, &o);
                    }
                }
            });
            ui.add_space(6.0);
        }
    });
}

/// Buy through the game (the only thing that changes state), flash on
/// success, shake the card on a refusal.
fn buy(ctx: &egui::Context, game: &mut Game, player: usize, o: &Offer) {
    if o.buyable() && game.buy_item(player, o.item.id) {
        pulse_set(ctx, egui::Id::new("item_bought").with(o.item.id));
    } else {
        pulse_set(ctx, egui::Id::new("item_refused").with(o.item.id));
    }
}

/// One plaque. Returns whether it was clicked.
fn card(ui: &mut egui::Ui, o: &Offer, size: Vec2, cursor: bool, suggested: bool) -> bool {
    let (base, resp) = ui.allocate_exact_size(size, Sense::click());
    let id = egui::Id::new("hoard_card").with(o.item.id);
    let t = hover_t(ui.ctx(), id, resp.hovered() || cursor);
    let bought = pulse_get(ui.ctx(), egui::Id::new("item_bought").with(o.item.id), 2.2);
    let refused = pulse_get(ui.ctx(), egui::Id::new("item_refused").with(o.item.id), 3.0);
    let shake = (refused * 40.0).sin() * refused * 5.0;
    let r = base.expand(t * 2.0).translate(Vec2::new(shake, 0.0));
    let d = shopview::den(o.item.cat);
    let col = c32(d.color);
    let p = ui.painter();
    let tone = if o.buyable() || o.owned.is_some() { 1.0 + 0.25 * t } else { 0.7 };
    bark(p, r, 8.0, tone);
    if bought > 0.01 {
        p.rect_filled(r, 8.0, MOSS.linear_multiply(0.5 * bought));
    }
    if suggested {
        p.rect_stroke(r.expand(2.0), 10.0, Stroke::new(2.0, GOLD.linear_multiply(0.8)));
    }
    if cursor {
        p.rect_stroke(r, 8.0, Stroke::new(1.5, INK.linear_multiply(0.7)));
    }
    // the leaf tab in the shelf colour, with the shelf glyph on it
    let tab = Rect::from_min_size(r.left_top(), Vec2::new(30.0, r.height()));
    p.rect_filled(tab, 8.0, col.linear_multiply(if o.buyable() { 0.55 } else { 0.3 }));
    glyph(p, tab.center() - Vec2::new(0.0, 14.0), 18.0, d.glyph, INK);
    // tier as leaves on the tab
    for k in 0..o.item.tier.min(3) {
        leaf(p, Pos2::new(tab.center().x - 7.0, tab.center().y + 8.0 + k as f32 * 11.0), Vec2::new(1.0, -0.45), 14.0, Color32::from_rgb(196, 232, 140));
    }
    let x0 = tab.right() + 10.0;
    let name_col = if o.buyable() || o.owned.is_some() { INK } else { INK_DIM };
    p.text(Pos2::new(x0, r.top() + 10.0), Align2::LEFT_TOP, o.item.name, FontId::proportional(15.0), name_col);
    // price or ownership, top right
    match o.owned {
        Some(slot) => {
            p.text(r.right_top() + Vec2::new(-10.0, 10.0), Align2::RIGHT_TOP, format!("held · slot {}", slot + 1), FontId::proportional(12.0), MOSS.linear_multiply(1.7));
        }
        None => {
            gold_mark(p, r.right_top() + Vec2::new(-10.0, 9.0), o.item.cost, 14.0, if o.buyable() { INK } else { INK_DIM }, Align2::RIGHT_TOP);
        }
    }
    p.text(
        Pos2::new(x0, r.top() + 31.0),
        Align2::LEFT_TOP,
        format!("{} · {}", shopview::tier_name(o.item.tier), shopview::stat_tokens(&o.item.stats).join("  ")),
        FontId::proportional(12.0),
        Color32::from_rgb(196, 206, 176),
    );
    // description, or why not
    let (line, lc) = match o.refusal {
        Some(r) if o.owned.is_none() => (r.text(), match r {
            Refusal::Full => EMBER,
            Refusal::Gold { .. } => EMBER.linear_multiply(0.85),
        }),
        _ => (o.item.desc.to_string(), INK_DIM),
    };
    let wrap = p.layout(line, FontId::proportional(11.0), lc, (r.right() - x0 - 10.0).max(40.0));
    p.galley(Pos2::new(x0, r.bottom() - 10.0 - wrap.size().y.min(28.0)), wrap, lc);
    if let Some(s) = &o.item.stack {
        p.text(r.right_bottom() + Vec2::new(-10.0, -8.0), Align2::RIGHT_BOTTOM, format!("{} (to {})", shopview::stack_kind_name(s.kind), s.max), FontId::proportional(10.0), GOLD.linear_multiply(0.8));
    }
    let clicked = resp.clicked();
    let tip = match o.refusal {
        None => format!("{} — tap to trade {} gold", o.item.name, o.item.cost),
        Some(r) => format!("{} — {}", o.item.name, r.text()),
    };
    resp.on_hover_text(tip);
    clicked
}

/// The player's six slots, as hollows in the bark, and the sell flow
/// (tap once to arm, again to trade back; closing the shop disarms).
fn hoard(ui: &mut egui::Ui, game: &mut Game, player: usize, st: &mut UiState) {
    // Scrolls: six slots and the stat block do not fit a landscape phone.
    egui::ScrollArea::vertical().id_source("hoard_slots").auto_shrink([false, false]).show(ui, |ui| hoard_inner(ui, game, player, st));
}

fn hoard_inner(ui: &mut egui::Ui, game: &mut Game, player: usize, st: &mut UiState) {
    ui.label(egui::RichText::new("YOUR HOARD").size(15.0).color(GOLD).strong());
    let stats = Game::item_stats(&game.gods[player]);
    ui.label(
        egui::RichText::new(format!(
            "STR {:.0} · INT {:.0} · PWR {:.0} · HP {:.0}\nPROT {:.0} · AS +{:.0}% · PEN {:.0}% / {:.0} · CDR {:.0}%",
            stats.strength,
            stats.intelligence,
            stats.power,
            stats.health,
            stats.protection,
            stats.attack_speed * 100.0,
            stats.pen_pct * 100.0,
            stats.pen_flat,
            stats.cdr * 100.0
        ))
        .size(12.0)
        .color(INK_DIM)
        .monospace(),
    );
    ui.add_space(6.0);
    let w = ui.available_width();
    for i in 0..shopview::SLOTS {
        let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 44.0), Sense::click());
        let p = ui.painter();
        let slot_id = egui::Id::new("build_slot").with(i);
        let t = hover_t(ui.ctx(), slot_id, resp.hovered());
        let held = game.gods[player].items.get(i).copied();
        // a hollow: darker, inset
        p.rect_filled(r, 12.0, BARK_DARK.linear_multiply(1.0 + 0.4 * t));
        p.rect_stroke(r.shrink(1.0), 12.0, Stroke::new(1.0, BARK_LIGHT.linear_multiply(0.6)));
        match held {
            None => {
                p.text(r.left_center() + Vec2::new(14.0, 0.0), Align2::LEFT_CENTER, format!("{} · empty hollow", i + 1), FontId::proportional(12.0), INK_DIM.linear_multiply(0.7));
            }
            Some(iid) => {
                let item = &items::all()[iid as usize];
                let d = shopview::den(item.cat);
                glyph(p, r.left_center() + Vec2::new(20.0, 0.0), 18.0, d.glyph, c32(d.color));
                let armed = st.sell_armed == Some(i);
                let refund = (item.cost * consts::SELL_REFUND).round();
                let label = if armed { format!("trade back for {refund:.0}? tap again") } else { item.name.to_string() };
                p.text(r.left_center() + Vec2::new(38.0, -6.0), Align2::LEFT_CENTER, label, FontId::proportional(13.5), if armed { EMBER } else { INK });
                let sub = match &item.stack {
                    Some(s) => format!("{} stacks of {}", game.gods[player].stacks.get(i).copied().unwrap_or(0.0), s.max),
                    None => shopview::tier_name(item.tier).to_string(),
                };
                p.text(r.left_center() + Vec2::new(38.0, 9.0), Align2::LEFT_CENTER, sub, FontId::proportional(10.5), INK_DIM);
                if !armed {
                    gold_mark(p, r.right_center() + Vec2::new(-12.0, 0.0), refund, 12.0, INK_DIM, Align2::RIGHT_CENTER);
                }
                if armed {
                    p.rect_stroke(r, 12.0, Stroke::new(2.0, EMBER));
                }
                glow_hover(ui, &resp, EMBER.linear_multiply(0.6), 12.0);
                if resp.clicked() {
                    if armed {
                        game.sell_item(player, i);
                        st.sell_armed = None;
                    } else {
                        st.sell_armed = Some(i);
                    }
                }
            }
        }
        ui.add_space(4.0);
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Tap a gift to trade gold for it. Tap a held one twice to trade it back.").size(11.5).color(INK_DIM));
}
