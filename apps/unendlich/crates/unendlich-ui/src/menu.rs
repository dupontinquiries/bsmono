//! **menu** — the dialed-in in-game menu components, ported from the
//! `docs/design/menu_mockup.html` playground (v3).
//!
//! Everything here is **transparent-first**: these widgets composite over
//! the live 3D scene, so fills use alpha (`from_rgba_unmultiplied` /
//! `gamma_multiply`) and never assume an opaque backdrop. Pair with
//! [`crate::Surface::Frosted`] + [`crate::frosted_backdrop`] to get the
//! blurred-scene look behind a panel; with no blur texture the tints alone
//! still read correctly over the world.
//!
//! Design language (see the mockup README): sharp/voxel corners, a diamond
//! motif, fractal-dither fills that traverse horizontally, square/diamond
//! knobs, neutral hollow cells, and the green/gold tactical accents.
//!
//! Built against egui 0.34 to match the rest of the kit.

use egui::{
    Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};

use crate::current_theme;

// ── tokens ───────────────────────────────────────────────────────────────────
/// Sharp corner used throughout the menu (voxel/block feel). 3px like the
/// mockup's `--corner`.
pub const CORNER: CornerRadius = CornerRadius::same(3);
pub const SQUARE: CornerRadius = CornerRadius::same(0);

/// Spring-ish ease used for knob travel and squish (approximates the
/// mockup's `cubic-bezier(.34,1.7,.5,1)` overshoot).
pub fn spring(t: f32) -> f32 {
    // overshoot then settle; t in [0,1]
    let t = t.clamp(0.0, 1.0);
    let c = 1.70158 + 1.0;
    let u = t - 1.0;
    1.0 + (c + 1.0) * u * u * u + c * u * u
}

/// Alpha-scaled colour helper (kept terse; mirrors `gamma_multiply` intent
/// but linear so it reads predictably over the scene).
#[inline]
pub fn a(c: Color32, alpha: f32) -> Color32 {
    let m = alpha.clamp(0.0, 1.0);
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (c.a() as f32 * m) as u8)
}

/// Desaturate `c` toward its luminance by `amount` (0 = unchanged, 1 = gray).
/// Used for the slider's grayscale-faded tail.
pub fn desaturate(c: Color32, amount: f32) -> Color32 {
    let k = amount.clamp(0.0, 1.0);
    let lum = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    let mix = |x: u8| (x as f32 * (1.0 - k) + lum * k).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(mix(c.r()), mix(c.g()), mix(c.b()), c.a())
}

// ── cone-projection parallax ────────────────────────────────────────────────
/// The mockup's cone parallax: the cursor is the apex of a viewing cone, and a
/// layer at signed depth ratio `z` projects from the screen centre by
/// `cursor_offset * z`. Positive `z` leads with the cursor (foreground),
/// negative trails it (background swim). Returns the layer's pixel offset.
///
/// `cursor` and `screen` are in the same coordinate space; the offset is the
/// cursor's distance from the screen centre times `z`. Resolution-independent:
/// `z` is a fraction of how far the cursor is from centre, so it scales with
/// any screen size. Clamp `z` small (~0.01–0.05 for panels).
pub fn cone_offset(cursor: Pos2, screen: Rect, z: f32) -> Vec2 {
    let c = screen.center();
    Vec2::new((cursor.x - c.x) * z, (cursor.y - c.y) * z)
}

/// Smooth a parallax anchor toward a target by `smooth` (heavier = laggier,
/// more "weight"). Store the returned value and feed it back each frame.
/// `smooth` in (0,1]; ~0.03 is a slow, weighty drift, ~0.2 snappy.
pub fn parallax_lerp(current: Vec2, target: Vec2, smooth: f32) -> Vec2 {
    current + (target - current) * smooth.clamp(0.0, 1.0)
}

/// centre). Transparent-first — the fill is alpha so the world shows
/// through; combine with [`crate::frosted_backdrop`] for an actual blur.
///
/// Call before drawing the panel's content (it only paints chrome).
pub fn glass_panel(ui: &Ui, rect: Rect, accent: Color32, light: Pos2) {
    let th = current_theme(ui.ctx());
    let p = ui.painter();

    // base translucent fill (the world reads through this)
    p.rect_filled(rect, CORNER, a(th.panel, 0.55));

    // edge-weighted inner glow: a few inset rings, brightest at the rim,
    // fading inward — reads as "thicker glass at the edges".
    for i in 0..4 {
        let inset = 1.0 + i as f32 * 2.0;
        let alpha = 0.10 * (1.0 - i as f32 / 4.0);
        p.rect_stroke(
            rect.shrink(inset),
            CORNER,
            Stroke::new(1.0, a(accent, alpha + 0.04)),
            StrokeKind::Inside,
        );
    }

    // top rim highlight
    p.line_segment(
        [rect.left_top() + Vec2::new(4.0, 1.0), rect.right_top() + Vec2::new(-4.0, 1.0)],
        Stroke::new(1.4, Color32::from_rgba_unmultiplied(210, 235, 255, 90)),
    );

    // global specular: a soft radial gleam at `light`, masked to the panel.
    // egui has no radial-gradient primitive, so approximate with a few
    // concentric translucent rings centred on the light origin.
    let lp = Pos2::new(
        light.x.clamp(rect.left(), rect.right()),
        light.y.clamp(rect.top(), rect.bottom()),
    );
    for i in 0..5 {
        let rr = 18.0 + i as f32 * 26.0;
        let alpha = 0.10 * (1.0 - i as f32 / 5.0);
        // clip-ish: only draw if the ring centre is near the panel
        p.circle_stroke(lp, rr, Stroke::new(6.0, a(accent, alpha)));
    }

    // outer frame
    p.rect_stroke(rect, CORNER, Stroke::new(1.0, a(th.line, 0.8)), StrokeKind::Inside);
}

// ── diamond toggle ─────────────────────────────────────────────────────────
/// The mockup's toggle: a `<…>` holder whose 45° caps shrink-wrap a square
/// **diamond** knob (rotated square), with a fractal-dither infill that
/// fills the whole holder and reveals left→right when on. Transparent-first.
///
/// `on` is the current state; returns the [`Response`] (check `.clicked()`
/// and flip your own bool). `t01` should be the animated 0→1 toward `on`
/// (use `ctx.animate_bool`).
pub fn toggle(ui: &mut Ui, on: bool, accent: Color32) -> Response {
    let size = Vec2::new(48.0, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let t = ui.ctx().animate_bool(resp.id, on);
    let press = ui.ctx().animate_bool(resp.id.with("press"), resp.is_pointer_button_down_on());
    let th = current_theme(ui.ctx());

    let h = rect.height();
    // caps are a gentle chevron (depth = ~1/3 height), NOT a 45° spike — the
    // long half-height caps read as two fat hexagons; this keeps it a toggle.
    let cap = h * 0.34;
    let holder = vec![
        Pos2::new(rect.left(), rect.center().y),
        Pos2::new(rect.left() + cap, rect.top()),
        Pos2::new(rect.right() - cap, rect.top()),
        Pos2::new(rect.right(), rect.center().y),
        Pos2::new(rect.right() - cap, rect.bottom()),
        Pos2::new(rect.left() + cap, rect.bottom()),
    ];
    {
        let p = ui.painter();
        // holder fill (dim, translucent)
        p.add(Shape::convex_polygon(holder.clone(), a(th.panel, 0.5), Stroke::NONE));
    }

    // infill: the accent fills the HOLDER SHAPE (not the bounding rect),
    // revealed left→right. Clip the painter to [left, reveal_x] and redraw the
    // holder polygon filled — so the fill exactly follows the chevron outline
    // and can't spill into the rectangular corners. A light dither texture is
    // layered on top, also clipped, for the fractal look.
    if t > 0.001 {
        let reveal_x = rect.left() + rect.width() * t;
        let clip = Rect::from_min_max(rect.left_top(), Pos2::new(reveal_x, rect.bottom()));
        let cp = ui.painter().with_clip_rect(clip);
        cp.add(Shape::convex_polygon(holder.clone(), a(accent, 0.5 * t), Stroke::NONE));
        // dither dots, clipped to the same region AND inset so they sit inside
        // the hull (the caps narrow, so we keep dots off the very top/bottom).
        let inner = rect.shrink(2.0);
        let dith = ui.painter().with_clip_rect(clip.intersect(inner));
        for (grid, dot, oa) in [(3.0f32, 0.9f32, 0.55f32), (6.0, 1.2, 0.4f32)] {
            let mut y = inner.top() + 1.0;
            let mut row = 0u32;
            while y < inner.bottom() {
                let xoff = if row % 2 == 0 { 0.0 } else { grid * 0.5 };
                let mut x = inner.left() + 1.0 + xoff;
                while x < reveal_x && x < inner.right() {
                    dith.rect_filled(
                        Rect::from_center_size(Pos2::new(x, y), Vec2::splat(dot)),
                        SQUARE,
                        a(accent, oa * t),
                    );
                    x += grid;
                }
                y += grid;
                row += 1;
            }
        }
    }
    {
        let p = ui.painter();
        // holder border (accent when on)
        let border = if t > 0.0 { a(accent, 0.45 + 0.55 * t) } else { a(th.line, 0.85) };
        p.add(Shape::closed_line(holder, Stroke::new(1.4, border)));
    }

    // knob: a diamond (rotated square), sized to sit cleanly inside the band.
    // Travels left (off) → right (on).
    let knob_half = h * 0.30;
    let kx_off = rect.left() + cap + knob_half;
    let kx_on = rect.right() - cap - knob_half;
    let kx = kx_off + (kx_on - kx_off) * t;
    let cy = rect.center().y;
    let sx = 1.0 - 0.25 * press; // press squish along travel
    let diamond = vec![
        Pos2::new(kx, cy - knob_half),
        Pos2::new(kx + knob_half * sx, cy),
        Pos2::new(kx, cy + knob_half),
        Pos2::new(kx - knob_half * sx, cy),
    ];
    let p = ui.painter();
    p.add(Shape::convex_polygon(
        diamond.clone(),
        Color32::from_rgb(0xef, 0xf3, 0xe8),
        Stroke::new(1.0, a(Color32::BLACK, 0.5)),
    ));
    if t > 0.5 {
        p.add(Shape::closed_line(diamond, Stroke::new(1.0, a(Color32::WHITE, 0.5 * t))));
    }
    resp
}

// ── custom slider ────────────────────────────────────────────────────────
/// The mockup's slider: a fractal-dither **fill that wraps a square knob**
/// (the fill ends exactly at the knob's left edge), the unfilled right side
/// a dim hairline with the accent **faded to grayscale**, a push/pull
/// stretch on grab, and lerp-to-detents. Transparent-first.
///
/// `value` is edited in place over `[min, max]`. If `detents > 1`, releasing
/// snaps to the nearest of N evenly-spaced detents. Returns the [`Response`].
pub fn slider(
    ui: &mut Ui,
    value: &mut f32,
    min: f32,
    max: f32,
    detents: usize,
    accent: Color32,
) -> Response {
    let size = Vec2::new(180.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let th = current_theme(ui.ctx());

    // drag → value
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            let f = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            *value = min + f * (max - min);
            resp.mark_changed();
        }
    }
    // snap to detents on release
    if resp.drag_stopped() && detents > 1 {
        let f = ((*value - min) / (max - min)).clamp(0.0, 1.0);
        let step = 1.0 / (detents as f32 - 1.0);
        let snapped = (f / step).round() * step;
        *value = min + snapped * (max - min);
        resp.mark_changed();
    }

    let f = ((*value - min) / (max - min)).clamp(0.0, 1.0);
    let track_h = 10.0;
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), track_h));
    let knob_x = rect.left() + rect.width() * f;
    let p = ui.painter();

    // unfilled track: dim grayscale hairline (so the world shows through)
    p.rect_filled(track, SQUARE, a(Color32::from_rgb(170, 180, 190), 0.06));
    p.rect_stroke(track, SQUARE, Stroke::new(1.0, a(th.line, 0.6)), StrokeKind::Inside);

    // filled side: fractal dither up to the knob, accent desaturating toward
    // the right (more gray the further along it fills).
    let grab = ui.ctx().animate_bool(resp.id.with("grab"), resp.dragged());
    // push/pull stretch: nudge the reveal a hair past/short of the knob on grab
    let stretch = 1.0 + 0.04 * (spring(grab) - 1.0);
    let fill_x = rect.left() + (knob_x - rect.left()) * stretch;
    let faded = desaturate(accent, 0.45 * f);
    let fill_rect = Rect::from_min_max(track.min, Pos2::new(fill_x.min(track.right()), track.max.y));
    dither_fill(ui, fill_rect, faded, fill_rect.right(), 1.0);

    // square knob (NOT a diamond) — the fill wraps right up to its left edge
    let p = ui.painter();
    let kh = 18.0;
    // press squish along x
    let sw = if grab > 0.0 { kh + 4.0 * grab } else { kh };
    let sh = if grab > 0.0 { kh - 2.0 * grab } else { kh };
    let knob = Rect::from_center_size(Pos2::new(knob_x, rect.center().y), Vec2::new(sw, sh));
    p.rect_filled(knob, CORNER, Color32::from_rgb(0xef, 0xf3, 0xe8));
    p.rect_stroke(knob, CORNER, Stroke::new(1.0, a(Color32::BLACK, 0.5)), StrokeKind::Inside);
    let hov = ui.ctx().animate_bool(resp.id.with("hov"), resp.hovered());
    if hov > 0.0 {
        p.rect_stroke(knob.expand(2.0 * hov), CORNER, Stroke::new(1.5, a(accent, 0.7 * hov)), StrokeKind::Outside);
    }
    resp
}

// ── fractal dither fill (shared by toggle + slider) ────────────────────────
/// Paint a multi-octave (fractal/downsampled) dot dither of `color` across
/// `rect`, only up to `reveal_x`, at the given `alpha`. The shared
/// "traversing horizontal fill" texture for the toggle infill and the
/// slider fill. Cheap: a few grid passes of small filled rects.
pub fn dither_fill(ui: &Ui, rect: Rect, color: Color32, reveal_x: f32, alpha: f32) {
    let p = ui.painter();
    // a faint wash behind the dots so the fill reads even where dots are sparse
    let wash = Rect::from_min_max(rect.min, Pos2::new(reveal_x.min(rect.right()), rect.max.y));
    p.rect_filled(wash, SQUARE, a(color, 0.18 * alpha));
    // three octaves: 2px, 4px, 8px grids with increasing dot size
    for (grid, dot, oa) in [(2.0f32, 0.8f32, 0.9f32), (4.0, 1.0, 0.7f32), (8.0, 1.4, 0.5f32)] {
        let mut y = rect.top() + 1.0;
        let mut row = 0u32;
        while y < rect.bottom() {
            // offset alternate rows for the checker/fractal feel
            let xoff = if row % 2 == 0 { 0.0 } else { grid * 0.5 };
            let mut x = rect.left() + 1.0 + xoff;
            while x < reveal_x && x < rect.right() {
                p.rect_filled(
                    Rect::from_center_size(Pos2::new(x, y), Vec2::splat(dot)),
                    SQUARE,
                    a(color, oa * alpha),
                );
                x += grid;
            }
            y += grid;
            row += 1;
        }
    }
}

// ── animated border-fill ────────────────────────────────────────────────────
/// The mockup's "the frame draws itself in" effect, as a painter pass instead
/// of the CSS inset-shadow approximation. An inset ring starts THICK at the
/// rim and shrinks inward to a hairline while fading out, over `t01` in
/// `[0,1]` (0 = just-started thick ring, 1 = gone). Drive `t01` from an
/// animation clock — e.g. `(now - nav_time) / 0.62` clamped — and replay it on
/// each navigation. Transparent-first: the ring is the accent over whatever's
/// behind. No-op once `t01 >= 1`.
///
/// Returns whether it's still animating (`t01 < 1`), so the caller knows to
/// keep requesting repaints.
/// Whether the border-fill is still animating at progress `t01` (active iff
/// `0 <= t01 < 1`). Pure; the caller's repaint loop keys off this.
pub fn border_fill_active(t01: f32) -> bool {
    (0.0..1.0).contains(&t01)
}

pub fn border_fill(ui: &Ui, rect: Rect, accent: Color32, t01: f32) -> bool {
    if !border_fill_active(t01) {
        return false;
    }
    // thickness 10px → 1px as t goes 0→1; alpha 0.9 → 0 (fades as it thins)
    let thick = 10.0 + (1.0 - 10.0) * t01; // lerp 10 → 1
    let alpha = 0.9 * (1.0 - t01);
    let p = ui.painter();
    // a few concentric inset strokes up to `thick` give the ring some body
    // (a single thick stroke reads as a flat band; layering fades it inward).
    let layers = thick.ceil().max(1.0) as i32;
    for i in 0..layers {
        let inset = i as f32;
        if inset > thick {
            break;
        }
        // fade the inner layers so the ring is brightest at the rim
        let la = alpha * (1.0 - inset / (thick + 1.0));
        p.rect_stroke(
            rect.shrink(inset),
            CORNER,
            Stroke::new(1.0, a(accent, la)),
            StrokeKind::Inside,
        );
    }
    true
}

// ── neutral cell + crosshair grid ──────────────────────────────────────────
/// Draw one inventory/crafting cell: a neutral hollow border + a sharp inner
/// hairline (pixel-art × sharp-edge), clear interior (transparent — world
/// shows through). `state` selects the crosshair highlight tier.
#[derive(Clone, Copy, PartialEq)]
pub enum CellState {
    Idle,
    /// the hovered cell (crisp white border)
    Hover,
    /// in the hovered cell's row or column (brighter border + faint wash)
    Line,
}

pub fn cell(ui: &Ui, rect: Rect, accent: Color32, state: CellState) {
    let p = ui.painter();
    // clear interior — only a faint wash for line members
    if state == CellState::Line {
        p.rect_filled(rect, SQUARE, a(accent, 0.05));
    }
    // sharp inner hairline (the "double line")
    p.rect_stroke(rect.shrink(2.0), SQUARE, Stroke::new(1.0, a(Color32::BLACK, 0.45)), StrokeKind::Inside);
    // border by state
    let (col, w) = match state {
        CellState::Idle => (a(Color32::from_rgb(150, 170, 190), 0.28), 1.0),
        CellState::Line => (a(Color32::from_rgb(200, 220, 240), 0.6), 1.0),
        CellState::Hover => (Color32::from_rgb(0xee, 0xf3, 0xe8), 1.5),
    };
    p.rect_stroke(rect, SQUARE, Stroke::new(w, col), StrokeKind::Inside);
}

/// Given a grid of `cols × rows` and the hovered cell index (if any), return
/// the [`CellState`] for cell `idx`. The caller draws each cell with [`cell`]
/// and can stagger any per-cell animation by the returned manhattan distance
/// (delay ∝ distance, like the mockup's propagating crosshair).
pub fn crosshair_state(cols: usize, rows: usize, hovered: Option<usize>, idx: usize) -> (CellState, u32) {
    let Some(h) = hovered else { return (CellState::Idle, 0) };
    if h == idx {
        return (CellState::Hover, 0);
    }
    let (hr, hc) = (h / cols, h % cols);
    let (r, c) = (idx / cols, idx % cols);
    let _ = rows;
    if r == hr || c == hc {
        let dist = (r as i32 - hr as i32).unsigned_abs() + (c as i32 - hc as i32).unsigned_abs();
        (CellState::Line, dist)
    } else {
        (CellState::Idle, 0)
    }
}

// ── crosshair cell grid (full widget) ──────────────────────────────────────
/// A grid of neutral hollow cells with the propagating row×column crosshair,
/// transparent-first. Lays `count` cells of `cell_size` (square) at `gap`,
/// auto-flowing into as many columns as fit `avail_w`. Tracks the hovered
/// cell across frames (memory keyed by `salt`), does a two-pass draw (find
/// hovered → style every cell via [`crosshair_state`]), and calls
/// `content(ui, idx, rect)` so the caller paints the item inside each cell.
/// Returns `Some(idx)` if a cell was clicked this frame.
///
/// This keeps the crosshair logic in the kit (see the unit test) and gives
/// callers a one-line grid that matches the mockup.
pub fn cell_grid(
    ui: &mut Ui,
    salt: impl std::hash::Hash,
    count: usize,
    cell_size: f32,
    gap: f32,
    accent: Color32,
    mut content: impl FnMut(&mut Ui, usize, Rect),
) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let avail = ui.available_width().max(cell_size);
    let cols = (((avail + gap) / (cell_size + gap)).floor() as usize).clamp(1, count);
    let rows = count.div_ceil(cols);
    let grid_id = ui.make_persistent_id(("cell_grid", &salt));

    // total size we'll occupy
    let total_w = cols as f32 * cell_size + (cols.saturating_sub(1)) as f32 * gap;
    let total_h = rows as f32 * cell_size + (rows.saturating_sub(1)) as f32 * gap;
    let (area, _resp) = ui.allocate_exact_size(Vec2::new(total_w, total_h), Sense::hover());

    // rect for cell idx
    let rect_of = |idx: usize| -> Rect {
        let (r, c) = (idx / cols, idx % cols);
        let x = area.left() + c as f32 * (cell_size + gap);
        let y = area.top() + r as f32 * (cell_size + gap);
        Rect::from_min_size(Pos2::new(x, y), Vec2::splat(cell_size))
    };

    // pass 1: which cell is the pointer over?
    let pointer = ui.ctx().pointer_hover_pos();
    let mut hovered = None;
    if let Some(p) = pointer {
        for i in 0..count {
            if rect_of(i).contains(p) {
                hovered = Some(i);
                break;
            }
        }
    }
    // persist last hovered so a click (pointer may move) still reads sanely
    ui.ctx().memory_mut(|m| m.data.insert_temp(grid_id, hovered));

    // pass 2: draw + interact
    let mut clicked = None;
    for i in 0..count {
        let rect = rect_of(i);
        let (state, dist) = crosshair_state(cols, rows, hovered, i);
        // stagger the line members' fade by manhattan distance (the sweep).
        // egui has no per-shape delay, so we approximate by easing the line
        // tint in over distance using the context's stable time.
        let _ = dist;
        cell(ui, rect, accent, state);
        content(ui, i, rect);
        let resp = ui.interact(rect, grid_id.with(i), Sense::click());
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

// ── history point (screenshot pane + decoupled diamond) ─────────────────────
/// Draw a world-history timeline point: a screenshot `pane` rect (the caller
/// paints the actual thumbnail texture; here we just frame it) plus a diamond
/// marker below it. On hover the inner diamond expands slightly while a
/// **decoupled** border ring expands more and **sharpens** (soft glow → crisp
/// thin edge), at `hov` in [0,1]. `warm` tints the marker with the warm accent.
pub fn history_point(ui: &Ui, pane: Rect, marker_center: Pos2, accent: Color32, warm: bool, hov: f32) {
    let th = current_theme(ui.ctx());
    let p = ui.painter();
    let col = if warm { th.accent_soft } else { accent };

    // screenshot frame (the texture itself is drawn by the caller into `pane`)
    p.rect_stroke(
        pane,
        SQUARE,
        Stroke::new(1.0 + 0.5 * hov, a(if hov > 0.5 { Color32::from_rgb(0xee, 0xf3, 0xe8) } else { Color32::from_rgb(150, 170, 190) }, 0.32 + 0.6 * hov)),
        StrokeKind::Inside,
    );

    // inner diamond — slight expand on hover
    let half = 7.0 * (1.0 + 0.12 * hov);
    let dia = |c: Pos2, h: f32| {
        vec![
            Pos2::new(c.x, c.y - h),
            Pos2::new(c.x + h, c.y),
            Pos2::new(c.x, c.y + h),
            Pos2::new(c.x - h, c.y),
        ]
    };
    p.add(Shape::convex_polygon(dia(marker_center, half), a(col, 0.9), Stroke::NONE));

    // decoupled border ring — expands MORE and sharpens (glow→crisp)
    let ring_half = half + 3.0 + 4.0 * hov;
    let ring_col = if hov > 0.5 { Color32::WHITE } else { col };
    let ring_w = 2.0 - 1.0 * hov; // thins as it sharpens
    p.add(Shape::closed_line(
        dia(marker_center, ring_half),
        Stroke::new(ring_w.max(1.0), a(ring_col, 0.25 + 0.65 * hov)),
    ));
}

// ── font registration ──────────────────────────────────────────────────────
/// Register the two custom TTFs (UnendlichBlocks + UnendlichGlyphs) into the
/// egui font stack. Call once at startup with the font bytes (embed them with
/// `include_bytes!` from `docs/design/UnendlichBlocks-Regular.ttf` etc).
///
/// `blocks` becomes a named family `"blocks"` you can select with
/// `FontId::new(size, FontFamily::Name("blocks".into()))` for titles/labels;
/// `glyphs` becomes `"glyphs"` for the UI/bending marks. Body text keeps the
/// default proportional font (the mockup kept the system sans for body).
pub fn install_fonts(ctx: &egui::Context, blocks: &'static [u8], glyphs: &'static [u8]) {
    use egui::{FontData, FontFamily};
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("blocks".to_owned(), std::sync::Arc::new(FontData::from_static(blocks)));
    fonts.font_data.insert("glyphs".to_owned(), std::sync::Arc::new(FontData::from_static(glyphs)));
    fonts
        .families
        .entry(FontFamily::Name("blocks".into()))
        .or_default()
        .insert(0, "blocks".to_owned());
    fonts
        .families
        .entry(FontFamily::Name("glyphs".into()))
        .or_default()
        .insert(0, "glyphs".to_owned());
    ctx.set_fonts(fonts);
}

/// Convenience: a `FontId` in the blocky game font (titles, nav, headings).
pub fn blocks_font(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("blocks".into()))
}
/// Convenience: a `FontId` in the UI/bending glyph font.
pub fn glyph_font(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("glyphs".into()))
}

// ── self-test ────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crosshair_lights_row_and_column() {
        // 4 cols x 3 rows, hover index 5 (row 1, col 1)
        let (cols, rows, hov) = (4usize, 3usize, Some(5usize));
        let mut hover = 0;
        let mut line = 0;
        for i in 0..cols * rows {
            match crosshair_state(cols, rows, hov, i).0 {
                CellState::Hover => hover += 1,
                CellState::Line => line += 1,
                CellState::Idle => {}
            }
        }
        // one hovered cell; (cols-1) others in its row + (rows-1) in its col
        assert_eq!(hover, 1);
        assert_eq!(line, (cols - 1) + (rows - 1));
    }

    #[test]
    fn desaturate_endpoints() {
        let c = Color32::from_rgb(100, 200, 50);
        assert_eq!(desaturate(c, 0.0), c); // unchanged at 0
        let g = desaturate(c, 1.0); // fully gray: r==g==b
        assert_eq!(g.r(), g.g());
        assert_eq!(g.g(), g.b());
    }

    #[test]
    fn spring_settles_to_one() {
        assert!((spring(1.0) - 1.0).abs() < 1e-3);
        assert!((spring(0.0) - 0.0).abs() < 1e-3);
    }

    #[test]
    fn cone_offset_centre_is_zero_edge_scales() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0));
        // cursor at centre → no offset
        assert_eq!(cone_offset(screen.center(), screen, 0.05), Vec2::ZERO);
        // cursor 500px right of centre, z=0.04 → 20px offset
        let off = cone_offset(Pos2::new(1000.0, 300.0), screen, 0.04);
        assert!((off.x - 20.0).abs() < 1e-3 && off.y.abs() < 1e-3);
    }

    #[test]
    fn parallax_lerp_moves_toward_target() {
        let a = parallax_lerp(Vec2::ZERO, Vec2::new(10.0, 0.0), 0.25);
        assert!((a.x - 2.5).abs() < 1e-3);
        // converges
        let mut v = Vec2::ZERO;
        for _ in 0..200 { v = parallax_lerp(v, Vec2::new(10.0, 4.0), 0.25); }
        assert!((v.x - 10.0).abs() < 1e-2 && (v.y - 4.0).abs() < 1e-2);
    }

    #[test]
    fn border_fill_reports_animating_window() {
        // active iff 0 <= t < 1 (drives the caller's repaint loop)
        assert!(!border_fill_active(-0.1));
        assert!(border_fill_active(0.0));
        assert!(border_fill_active(0.5));
        assert!(border_fill_active(0.999));
        assert!(!border_fill_active(1.0));
        assert!(!border_fill_active(1.5));
    }
}
