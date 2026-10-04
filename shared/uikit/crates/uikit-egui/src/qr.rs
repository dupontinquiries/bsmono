//! qr — a theme-aware, custom-drawn QR-shaped identicon, and the expanded overlay for it.
//! `QrCodeRenderer.cpp` ported from snapclone into the kit, so any app that shows an id
//! can show it the same way.
//!
//! ## This is a QR-shaped identicon, and calling it that matters
//!
//! The C++ draws finder patterns, timing rows and a data field derived from the id — a
//! **Version 1 layout** at 21×21 — but there is no Reed-Solomon encoder, no mask
//! evaluation and no format information. A real QR reader will not decode it. It is a
//! recognisable, deterministic visual for an id, and this keeps that honestly:
//! [`Layout::modules`] is the drawing, and the caption in the app says what it is not.
//!
//! ## Three things that are load-bearing (from the C++)
//!
//! * **Cell size is an integer, floored, with a floor of 2.** A fractional module means the
//!   renderer resamples and the edges blur — the C++ names this as *"visible as the soft
//!   edges on the FP5 QR"*. Two is the minimum where a module survives at all.
//! * **Rendered at device pixel ratio.** A phone is 2–3x; drawing at logical size and
//!   letting the toolkit upscale is exactly the blur above. Here the cell is floored in
//!   physical pixels from `ctx.pixels_per_point()` and converted back to points.
//! * **A WCAG 4.5:1 contrast guard.** Unusual for a QR and completely right: a camera has
//!   to separate the modules, and a themed palette can easily put foreground and
//!   background close together. When the theme fails the check it falls back to pure
//!   black or white rather than to something nearer.
//!
//! ## The drawing is the app's, not a printer's
//!
//! The C++ fills squares. This draws the three finder eyes as rounded squares with a
//! rounded core, and the data modules as dots — horizontal runs of them joined into
//! pills — on a glass card with the hairline the rest of the chrome has. Module centres
//! are where a scanner samples, and every centre is exactly where the square version put
//! it; only the corners changed. Theme colours: the card colour behind, the text colour
//! for the modules (the accent on a pale card, as the C++ does), then the guard.
//!
//! ## The expanded overlay, and the tap that opens it
//!
//! Tapping the small code opens it full-screen: a 95% scrim of the base colour, the code
//! at 80% of the width (capped at 70% of the height) on its card, the id in mono at 24,
//! a hint, a Close button at 48. Two guards from the C++ ("V32.30", "flash open and
//! shut"): releases are ignored for 300 ms after opening, so the opening tap's own
//! release cannot close it; and only a tap on the SCRIM closes — not on the code, which
//! is where a finger naturally rests. Escape closes. It fades in over 220 ms, OutCubic,
//! one fade — the nested second fade the C++ removed for flickering is not reproduced.

use crate::PaletteColors;
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Ui};

/// Version 1 QR: 21×21 modules.
pub const MODULES: usize = 21;
/// Divisor used to pick a cell size, leaving room for a quiet border.
pub const CELL_DIVISOR: u32 = 22;
/// Smallest cell that still renders as a module.
pub const MIN_CELL: u32 = 2;
/// Contrast floor, WCAG AA for normal text.
pub const MIN_CONTRAST: f32 = 4.5;

/// Pixel size of one module, and the resulting image size.
///
/// Integer and floored, because a fractional module resamples and blurs the
/// edges — the FP5 symptom the C++ calls out.
pub fn cell_size(px: u32, dpr: f32) -> u32 {
    let scaled = (px as f32 * dpr.max(0.1)) as u32;
    (scaled / CELL_DIVISOR).max(MIN_CELL)
}

pub fn image_size(px: u32, dpr: f32) -> u32 { cell_size(px, dpr) * MODULES as u32 }

/// Relative luminance, sRGB.
pub fn luminance(c: [u8; 3]) -> f32 {
    let ch = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])
}

/// WCAG contrast ratio between two colours.
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (l1, l2) = (luminance(a), luminance(b));
    (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
}

/// Pick a foreground that a camera can separate from `bg`.
///
/// Falls back to pure black or white rather than to something merely better: a
/// QR that *nearly* scans is worse than one that obviously does, because the
/// user retries instead of moving the phone.
pub fn readable_fg(bg: [u8; 3], preferred: [u8; 3]) -> [u8; 3] {
    if contrast(bg, preferred) >= MIN_CONTRAST { return preferred; }
    if luminance(bg) > 0.5 { [0, 0, 0] } else { [255, 255, 255] }
}

/// The module grid, row-major, `true` where a module is dark.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub modules: Vec<bool>,
}

impl Layout {
    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= MODULES || y >= MODULES { return false; }
        self.modules[y * MODULES + x]
    }

    /// Is this coordinate inside a finder pattern or its separator?
    ///
    /// Data must never be written here: the finders are what a scanner locates
    /// first, and a stray dark module beside one breaks detection outright
    /// rather than corrupting a byte.
    pub fn is_reserved(x: usize, y: usize) -> bool {
        let corner = |ox: usize, oy: usize| x >= ox && x < ox + 8 && y >= oy && y < oy + 8;
        corner(0, 0)
            || (x >= MODULES - 8 && y < 8)
            || (x < 8 && y >= MODULES - 8)
            // Timing rows.
            || x == 6 || y == 6
    }
}

/// Build the layout for an id.
///
/// Deterministic: one id always renders the same square, which is the property
/// that makes it usable as an identicon at all.
pub fn layout_for(id: &str) -> Layout {
    let mut m = vec![false; MODULES * MODULES];
    let mut set = |x: usize, y: usize, on: bool| {
        if x < MODULES && y < MODULES { m[y * MODULES + x] = on; }
    };

    // Three finder patterns: 7×7 dark, 5×5 light, 3×3 dark.
    for (ox, oy) in [(0, 0), (MODULES - 7, 0), (0, MODULES - 7)] {
        for dy in 0..7 {
            for dx in 0..7 {
                let ring = dx == 0 || dy == 0 || dx == 6 || dy == 6;
                let core = (2..=4).contains(&dx) && (2..=4).contains(&dy);
                set(ox + dx, oy + dy, ring || core);
            }
        }
    }

    // Timing patterns: alternating modules along row and column 6.
    for i in 8..MODULES - 8 {
        let on = i % 2 == 0;
        set(i, 6, on);
        set(6, i, on);
    }

    // THE DATA FIELD ENCODES THE ID — the C++ `makeIdQr` bit scheme, so its scanner
    // (`QrScanDialog`) and `decode_layout` below both read it back: the id's bytes, LSB
    // first, one bit per data cell in `data_cells()` order; past the id, the C++'s
    // `(row ^ col) & 1` parity texture. The first port hashed the id here, which drew a
    // nice square that nothing could ever decode.
    let bytes = id.as_bytes();
    for (i, (x, y)) in data_cells().enumerate() {
        let on = if i / 8 < bytes.len() {
            (bytes[i / 8] >> (i % 8)) & 1 == 1
        } else {
            (y ^ x) & 1 == 1
        };
        set(x, y, on);
    }
    Layout { modules: m }
}


// ── theme ──────────────────────────────────────────────────────────────────────────────

fn rgb(c: Color32) -> [u8; 3] {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    [r, g, b]
}

/// The code's colours from the palette: the card behind, the text colour for the modules
/// (the accent on a pale card, as the C++'s `lightness() > 200` rule), then the guard.
pub fn theme_colors(c: &PaletteColors) -> (Color32, Color32) {
    let bg = rgb(c.bg_card);
    let pale = luminance(bg) > 0.55;
    let preferred = rgb(if pale { c.accent } else { c.text });
    let fg = readable_fg(bg, preferred);
    (Color32::from_rgb(bg[0], bg[1], bg[2]), Color32::from_rgb(fg[0], fg[1], fg[2]))
}

// ── drawing ────────────────────────────────────────────────────────────────────────────

/// Card padding around the modules, in points (the C++ `padding: 8px`).
pub const CARD_PAD: f32 = 8.0;
/// Card corner radius (the C++ `border-radius: 12px`).
pub const CARD_RADIUS: u8 = 12;

// ── the reveal ─────────────────────────────────────────────────────────────────────────
//
// The code does not appear, it DRAWS: the three eyes first, then every pill and dot fades
// and grows in on its own offset, so the whole thing resolves over about a second the way
// an SVG with staggered `<animate begin>`s would (and `svg()` below writes exactly that).
// Offsets are deterministic from the element's place, so the same code always draws the
// same way — a small signature, like the code itself.

/// How long one element takes to come in.
pub const REVEAL_ELEMENT_S: f64 = 0.42;
/// The spread of offsets across the elements: the last one starts this late.
pub const REVEAL_SPREAD_S: f64 = 0.70;
/// The eyes lead by this much before the first data element starts.
pub const REVEAL_LEAD_S: f64 = 0.12;
/// The whole reveal, first eye to last dot.
pub const REVEAL_TOTAL_S: f64 = REVEAL_LEAD_S + REVEAL_SPREAD_S + REVEAL_ELEMENT_S;

/// The offset of a data element at (`x`,`y`): its place on a FRACTAL guide.
///
/// A hash gave every element a different offset but clumped them — three neighbours in a
/// row would come in together and a corner would wait. This orders the grid by the
/// bit-reversed Morton (Z-order) index of the cell: the first elements to appear are one
/// per quadrant, then one per sub-quadrant, then finer, so at every moment of the reveal
/// what is on screen is an even sample of the whole code — the quadtree filling in level by
/// level, the way a progressive image loads. Every cell has its own index, so no two
/// elements share an offset, and the same cell always gets the same one.
pub fn reveal_delay(x: usize, y: usize) -> f64 {
    let m = morton(x as u32, y as u32); // 10 bits: 5 per axis, covering 0..32
    let rev = reverse_bits(m, 10);
    REVEAL_LEAD_S + REVEAL_SPREAD_S * (rev as f64 / 1023.0)
}

/// Interleave the low 5 bits of `x` and `y` (x in the even bits).
fn morton(x: u32, y: u32) -> u32 {
    let mut m = 0;
    for b in 0..5 {
        m |= ((x >> b) & 1) << (2 * b);
        m |= ((y >> b) & 1) << (2 * b + 1);
    }
    m
}

fn reverse_bits(v: u32, bits: u32) -> u32 {
    let mut r = 0;
    for b in 0..bits {
        r |= ((v >> b) & 1) << (bits - 1 - b);
    }
    r
}

/// The offset of eye `i` (0..3): they lead.
pub fn eye_delay(i: usize) -> f64 {
    i as f64 * 0.06
}

/// 0..1 progress of an element that started at `delay` after the reveal began `t` ago,
/// eased out (fast in, gentle settle).
pub fn reveal_progress(t: f64, delay: f64) -> f32 {
    let u = ((t - delay) / REVEAL_ELEMENT_S).clamp(0.0, 1.0) as f32;
    // the kit's curve, not a hand-rolled cubic: one place decides what "eased out" means
    // here, in `motion`, alongside every other animation in the fleet.
    crate::motion::ease::out_cubic(u)
}

/// Draw the code for `layout` at `side` points on a glass card. Returns the response of
/// the whole card (`Sense::click`, so the caller can open the expanded view on a tap).
/// `revealed_at` is when the code came on screen; while the reveal runs the elements draw
/// in and a repaint is requested. `None` draws it complete.
pub fn qr_ui(ui: &mut Ui, layout: &Layout, side: f32, c: &PaletteColors, revealed_at: Option<f64>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), Sense::click());
    let t = revealed_at.map(|t0| ui.input(|i| i.time) - t0);
    if matches!(t, Some(t) if t < REVEAL_TOTAL_S) {
        ui.ctx().request_repaint();
    }
    paint(ui.painter(), rect, layout, ui.ctx().pixels_per_point(), c, t);
    resp
}

/// The painter half of [`qr_ui`], for callers that place the rect themselves. `reveal_t` is
/// seconds since the reveal began, or `None` for the finished code.
pub fn paint(painter: &egui::Painter, rect: Rect, layout: &Layout, pixels_per_point: f32, c: &PaletteColors, reveal_t: Option<f64>) {
    let (bg, _fg) = theme_colors(c);
    // the card: the rest of the chrome's glass, with the hairline along the top
    painter.rect_filled(rect, CornerRadius::same(CARD_RADIUS), bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(CARD_RADIUS),
        egui::Stroke::new(1.0_f32, Color32::from_white_alpha(crate::glass::GlassAlphas::HAIRLINE)),
        egui::StrokeKind::Inside,
    );
    paint_modules(painter, rect.shrink(CARD_PAD), layout, pixels_per_point, c, reveal_t);
}

/// The modules alone — eyes, dots, pills, with the reveal — inside `inner`.
fn paint_modules(painter: &egui::Painter, inner: Rect, layout: &Layout, pixels_per_point: f32, c: &PaletteColors, reveal_t: Option<f64>) {
    let (bg, fg) = theme_colors(c);
    // an integer cell in PHYSICAL pixels, back to points: the modules land on pixels
    let ppp = pixels_per_point.max(0.1);
    let cell_px = cell_size(inner.width().min(inner.height()) as u32, ppp);
    let cell = cell_px as f32 / ppp;
    let used = cell * MODULES as f32;
    // centred on the leftover, so flooring does not push the pattern into a corner
    let origin = inner.min + egui::vec2((inner.width() - used) * 0.5, (inner.height() - used) * 0.5);
    let at = |x: usize, y: usize| origin + egui::vec2(x as f32 * cell, y as f32 * cell);

    let progress = |delay: f64| match reveal_t {
        Some(t) => reveal_progress(t, delay),
        None => 1.0,
    };
    let faded = |col: Color32, p: f32| {
        let [r, g, b, a] = col.to_srgba_unmultiplied();
        Color32::from_rgba_unmultiplied(r, g, b, (a as f32 * p) as u8)
    };
    // finder eyes: rounded outer ring, the light ring, a rounded core — each eye scales in
    // from its centre as it fades, so it settles rather than pops
    for (i, (ox, oy)) in [(0usize, 0usize), (MODULES - 7, 0), (0, MODULES - 7)].into_iter().enumerate() {
        let p = progress(eye_delay(i));
        if p <= 0.0 {
            continue;
        }
        let k = 0.7 + 0.3 * p;
        let centre = at(ox, oy) + egui::vec2(3.5 * cell, 3.5 * cell);
        let sq = |size: f32| Rect::from_center_size(centre, egui::vec2(size * cell * k, size * cell * k));
        painter.rect_filled(sq(7.0), CornerRadius::same((cell * 1.6 * k) as u8), faded(fg, p));
        painter.rect_filled(sq(5.0), CornerRadius::same((cell * 1.0 * k) as u8), faded(bg, p));
        painter.rect_filled(sq(3.0), CornerRadius::same((cell * 0.8 * k) as u8), faded(fg, p));
    }
    // data + timing modules: dots, with horizontal runs joined into pills. A pill grows
    // from its CENTRE outward as it fades in, a dot from its centre — the appearance is
    // centred, not a stroke from the left. The offset is the run's centre cell's.
    let r = cell * 0.44;
    for y in 0..MODULES {
        let mut x = 0;
        while x < MODULES {
            if in_finder(x, y) || !layout.get(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x + 1 < MODULES && layout.get(x + 1, y) && !in_finder(x + 1, y) {
                x += 1;
            }
            let p = progress(reveal_delay((start + x) / 2, y));
            if p > 0.0 {
                let col = faded(fg, p);
                let a_full = at(start, y) + egui::vec2(cell * 0.5, cell * 0.5);
                let b_full = at(x, y) + egui::vec2(cell * 0.5, cell * 0.5);
                let mid = (a_full + b_full.to_vec2()) * 0.5;
                if start == x {
                    painter.circle_filled(a_full, r * (0.5 + 0.5 * p), col);
                } else {
                    let a = mid + (a_full - mid) * p;
                    let b = mid + (b_full - mid) * p;
                    let rr = r * (0.5 + 0.5 * p);
                    let pill = Rect::from_two_pos(a - egui::vec2(rr, rr), b + egui::vec2(rr, rr));
                    painter.rect_filled(pill, CornerRadius::same(rr as u8), col);
                }
            }
            x += 1;
        }
    }
}

// ── the data field's order, and reading it back ───────────────────────────────────────

/// The data cells in write/read order — the C++ loop: rows top to bottom, columns 8..21,
/// skipping the two right-hand finder blocks and the left-hand separator column — and,
/// unlike the C++, skipping the timing row: it let data bits overwrite row 6, which is the
/// row the scanner checks before it trusts a grid. 180 cells: 22 bytes, room for `sc-` and
/// 19 hex digits.
pub fn data_cells() -> impl Iterator<Item = (usize, usize)> {
    (0..MODULES).flat_map(|row| {
        (8..MODULES).filter_map(move |col| {
            let skip = row == 6 || (row < 9 && col < 9) || (row < 9 && col > 12) || (row > 12 && col < 9);
            (!skip).then_some((col, row))
        })
    })
}

/// Read the id back out of a layout: the data bits as bytes until the first byte that is
/// not printable ASCII, then the same `sc-…` check the payload parser makes. `None` for a
/// layout that does not carry one — including one that was hashed rather than encoded.
pub fn decode_layout(l: &Layout) -> Option<String> {
    let mut bytes = Vec::new();
    let mut cur = 0u8;
    for (i, (x, y)) in data_cells().enumerate() {
        if l.get(x, y) {
            cur |= 1 << (i % 8);
        }
        if i % 8 == 7 {
            if !(0x20..0x7f).contains(&cur) {
                break;
            }
            bytes.push(cur);
            cur = 0;
        }
    }
    let s = String::from_utf8(bytes).ok()?;
    // the same shape parse_payload accepts: `sc-` and a hex run
    let start = s.find("sc-")?;
    let after = &s[start + 3..];
    let hex_len = after.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(after.len());
    (hex_len >= 4).then(|| format!("sc-{}", &after[..hex_len]))
}

// ── the scanner: from camera pixels to an id ───────────────────────────────────────────
//
// The C++ has `QrScanDialog` doing this over the camera; the egui port had the status line
// "Point at a SnapClone QR code" over a viewfinder that decoded nothing. This is the
// decoder: threshold the frame, find the three finder eyes by the 1:1:3:1:1 run the
// standard uses, take the grid from their centres, sample each module's centre, decode.
// Handles rotation and scale; not perspective — a phone held roughly square to the code.

/// A finder centre, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Finder {
    pub x: f32,
    pub y: f32,
    /// The module size it implies (the run's total / 7).
    pub module: f32,
}

/// Luminance of an RGBA frame, one byte per pixel.
fn gray(rgba: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut g = vec![0u8; w * h];
    for i in 0..w * h {
        let p = &rgba[i * 4..i * 4 + 4];
        g[i] = ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8;
    }
    g
}

/// Does a run of five alternating stretches look like a finder (1:1:3:1:1)?
fn finder_ratio(runs: &[usize; 5]) -> Option<f32> {
    let total: usize = runs.iter().sum();
    if total < 7 {
        return None;
    }
    let m = total as f32 / 7.0;
    let tol = (m * 0.7).max(1.0);
    let ok = (runs[0] as f32 - m).abs() < tol
        && (runs[1] as f32 - m).abs() < tol
        && (runs[2] as f32 - 3.0 * m).abs() < 3.0 * tol
        && (runs[3] as f32 - m).abs() < tol
        && (runs[4] as f32 - m).abs() < tol;
    ok.then_some(m)
}

/// Find finder candidates by scanning rows for the 1:1:3:1:1 pattern and confirming each
/// with the same pattern down its column. `dark` is the thresholded frame (true = dark).
pub fn find_finders(dark: &[bool], w: usize, h: usize) -> Vec<Finder> {
    let mut found: Vec<Finder> = Vec::new();
    let at = |x: usize, y: usize| dark[y * w + x];
    for y in 0..h {
        // run-length encode the row
        let mut runs: Vec<(usize, bool, usize)> = Vec::new(); // (start, dark, len)
        let mut x = 0;
        while x < w {
            let d = at(x, y);
            let start = x;
            while x < w && at(x, y) == d {
                x += 1;
            }
            runs.push((start, d, x - start));
        }
        for i in 0..runs.len().saturating_sub(4) {
            if !runs[i].1 {
                continue;
            }
            let r = [runs[i].2, runs[i + 1].2, runs[i + 2].2, runs[i + 3].2, runs[i + 4].2];
            let Some(m) = finder_ratio(&r) else { continue };
            let cx = runs[i + 2].0 as f32 + runs[i + 2].2 as f32 / 2.0;
            // confirm down the column through the centre
            let cxi = cx as usize;
            let mut up = 0;
            let mut yy = y;
            while yy > 0 && at(cxi, yy) { yy -= 1; up += 1; }
            let mut down = 0;
            let mut yy = y;
            while yy + 1 < h && at(cxi, yy) { yy += 1; down += 1; }
            let core = (up + down).max(1) as f32;
            if (core - 3.0 * m).abs() > 2.0 * m.max(1.0) {
                continue;
            }
            let cy = (y as f32 - up as f32 + y as f32 + down as f32) / 2.0;
            // merge with an existing finder near this centre
            if let Some(f) = found.iter_mut().find(|f| (f.x - cx).abs() < 3.0 * m && (f.y - cy).abs() < 3.0 * m) {
                f.x = (f.x + cx) / 2.0;
                f.y = (f.y + cy) / 2.0;
                f.module = (f.module + m) / 2.0;
            } else {
                found.push(Finder { x: cx, y: cy, module: m });
            }
        }
    }
    found
}

/// Order three finders as (top-left, top-right, bottom-left) by the right angle at the
/// corner one, with the handedness that puts top-right clockwise from top-left.
fn orient(f: &[Finder]) -> Option<(Finder, Finder, Finder)> {
    if f.len() < 3 {
        return None;
    }
    // the corner finder is the one whose two edges are most nearly perpendicular
    let mut best: Option<(f32, usize)> = None;
    for i in 0..3 {
        let (a, b) = ((i + 1) % 3, (i + 2) % 3);
        let (ux, uy) = (f[a].x - f[i].x, f[a].y - f[i].y);
        let (vx, vy) = (f[b].x - f[i].x, f[b].y - f[i].y);
        let cos = (ux * vx + uy * vy).abs() / ((ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt()).max(1e-6);
        if best.map(|(c, _)| cos < c).unwrap_or(true) {
            best = Some((cos, i));
        }
    }
    let (_, tl) = best?;
    let (a, b) = ((tl + 1) % 3, (tl + 2) % 3);
    let (ux, uy) = (f[a].x - f[tl].x, f[a].y - f[tl].y);
    let (vx, vy) = (f[b].x - f[tl].x, f[b].y - f[tl].y);
    // cross > 0 → b is clockwise from a (y down): a is top-right, b bottom-left
    let cross = ux * vy - uy * vx;
    if cross > 0.0 {
        Some((f[tl], f[a], f[b]))
    } else {
        Some((f[tl], f[b], f[a]))
    }
}

/// Decode an RGBA frame: the id it shows, if a code is in view.
pub fn decode_frame(rgba: &[u8], w: usize, h: usize) -> Option<String> {
    if rgba.len() < w * h * 4 || w < 21 || h < 21 {
        return None;
    }
    let g = gray(rgba, w, h);
    // adaptive-ish threshold: the frame's mean, which separates a code from its card
    let mean = (g.iter().map(|&v| v as u64).sum::<u64>() / (w * h) as u64) as u8;
    let dark: Vec<bool> = g.iter().map(|&v| v < mean).collect();
    let mut finders = find_finders(&dark, w, h);
    finders.sort_by(|a, b| b.module.partial_cmp(&a.module).unwrap());
    // try the largest three first, then any three among the first six
    let n = finders.len().min(6);
    for i in 0..n {
        for j in i + 1..n {
            for k in j + 1..n {
                let Some((tl, tr, bl)) = orient(&[finders[i], finders[j], finders[k]]) else { continue };
                // the eye centres are modules (3,3), (17,3), (3,17): 14 modules apart
                let ux = (tr.x - tl.x) / 14.0;
                let uy = (tr.y - tl.y) / 14.0;
                let vx = (bl.x - tl.x) / 14.0;
                let vy = (bl.y - tl.y) / 14.0;
                let mut modules = vec![false; MODULES * MODULES];
                let mut ok = true;
                for my in 0..MODULES {
                    for mx in 0..MODULES {
                        let fx = mx as f32 - 3.0;
                        let fy = my as f32 - 3.0;
                        let px = tl.x + fx * ux + fy * vx;
                        let py = tl.y + fx * uy + fy * vy;
                        if px < 0.0 || py < 0.0 || px >= w as f32 || py >= h as f32 {
                            ok = false;
                            break;
                        }
                        modules[my * MODULES + mx] = dark[py as usize * w + px as usize];
                    }
                    if !ok {
                        break;
                    }
                }
                if !ok {
                    continue;
                }
                let l = Layout { modules };
                // the timing row is the cheap sanity check before decoding
                let timing_ok = (8..MODULES - 8).all(|i| l.get(i, 6) == (i % 2 == 0));
                if !timing_ok {
                    continue;
                }
                if let Some(id) = decode_layout(&l) {
                    return Some(id);
                }
            }
        }
    }
    None
}

/// Draw a layout into an RGBA buffer as plain squares — the scanner's own test fixture,
/// and a fallback renderer for a host without egui. `origin` and `cell` in pixels;
/// `rot` in radians about the code's centre.
pub fn rasterize(l: &Layout, rgba: &mut [u8], w: usize, h: usize, origin: (f32, f32), cell: f32, rot: f32, fg: [u8; 3]) {
    let side = cell * MODULES as f32;
    let (cx, cy) = (origin.0 + side / 2.0, origin.1 + side / 2.0);
    let (cs, sn) = (rot.cos(), rot.sin());
    for py in 0..h {
        for px in 0..w {
            // inverse-rotate the pixel into code space
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            let lx = cs * dx + sn * dy + side / 2.0;
            let ly = -sn * dx + cs * dy + side / 2.0;
            if lx < 0.0 || ly < 0.0 || lx >= side || ly >= side {
                continue;
            }
            let (mx, my) = ((lx / cell) as usize, (ly / cell) as usize);
            if l.get(mx, my) {
                let i = (py * w + px) * 4;
                rgba[i] = fg[0];
                rgba[i + 1] = fg[1];
                rgba[i + 2] = fg[2];
                rgba[i + 3] = 255;
            }
        }
    }
}

// ── the tilt ───────────────────────────────────────────────────────────────────────────
//
// The card is glass, and glass catches the light differently as it moves. `Tilt` is how
// far the phone leans, from the accelerometer's gravity vector — or, on a machine with
// none, from the pointer over the card, so a desktop gets the same effect under the mouse.
// The painter uses it three ways: the modules shift a little against the card (parallax,
// the eyes deeper than the dots), a highlight slides across the card toward the raised
// edge, and the edge away from the light darkens. Smoothed, so a jittery sensor does not
// shiver the code.

/// A tilt, −1..1 on each axis: `x` positive leaning right, `y` positive leaning toward you.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tilt {
    pub x: f32,
    pub y: f32,
}

impl Tilt {
    /// From an accelerometer reading (any units — the vector is normalised): the gravity
    /// components along the screen's axes. A face-up phone is (0, 0).
    pub fn from_accel(ax: f32, ay: f32, az: f32) -> Tilt {
        let n = (ax * ax + ay * ay + az * az).sqrt();
        if n < 1e-3 {
            return Tilt::default();
        }
        Tilt { x: (ax / n).clamp(-1.0, 1.0), y: (ay / n).clamp(-1.0, 1.0) }
    }
    /// From the pointer's place over `rect`: the centre is flat, the edges full tilt.
    pub fn from_pointer(p: Pos2, rect: Rect) -> Tilt {
        let x = ((p.x - rect.center().x) / (rect.width() * 0.5)).clamp(-1.0, 1.0);
        let y = ((p.y - rect.center().y) / (rect.height() * 0.5)).clamp(-1.0, 1.0);
        Tilt { x, y }
    }
    /// Low-pass toward `target`: `k` is the fraction moved per call (0.15 at 60 Hz settles in
    /// about a third of a second).
    pub fn ease_toward(self, target: Tilt, k: f32) -> Tilt {
        let k = k.clamp(0.0, 1.0);
        Tilt { x: self.x + (target.x - self.x) * k, y: self.y + (target.y - self.y) * k }
    }
    pub fn magnitude(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt().min(1.0)
    }
}

/// The parallax, in points, that a layer at `depth` (0 = the card face, 1 = deepest)
/// shifts under `tilt`: the deeper the layer the more it moves against the frame.
pub fn parallax(tilt: Tilt, depth: f32, max_pt: f32) -> egui::Vec2 {
    egui::vec2(-tilt.x, -tilt.y) * max_pt * depth.clamp(0.0, 1.0)
}

/// [`qr_ui`] with a tilt: the card catches the light. Pass the smoothed tilt from the
/// sensor, or `Tilt::from_pointer` of the hover position over the card.
pub fn qr_ui_tilted(ui: &mut Ui, layout: &Layout, side: f32, c: &PaletteColors, revealed_at: Option<f64>, tilt: Tilt) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), Sense::click());
    let t = revealed_at.map(|t0| ui.input(|i| i.time) - t0);
    if matches!(t, Some(t) if t < REVEAL_TOTAL_S) {
        ui.ctx().request_repaint();
    }
    paint_tilted(ui.painter(), rect, layout, ui.ctx().pixels_per_point(), c, t, tilt);
    resp
}

/// [`paint`] with a tilt.
pub fn paint_tilted(painter: &egui::Painter, rect: Rect, layout: &Layout, pixels_per_point: f32, c: &PaletteColors, reveal_t: Option<f64>, tilt: Tilt) {
    let (bg, _fg) = theme_colors(c);
    // the card, drawn where it is; the modules parallax within it by depth
    painter.rect_filled(rect, CornerRadius::same(CARD_RADIUS), bg);
    let max_pt = rect.width() * 0.012;
    let inner = rect.shrink(CARD_PAD + max_pt);
    // the modules, shifted as one layer (the eyes a touch deeper via the second pass)
    let shift = parallax(tilt, 1.0, max_pt);
    let clip = painter.with_clip_rect(rect);
    paint_modules(&clip, inner.translate(shift), layout, pixels_per_point, c, reveal_t);
    // the light: a soft highlight sliding toward the raised edge, and a shade on the far one
    let m = tilt.magnitude();
    if m > 0.01 {
        let dir = egui::vec2(tilt.x, tilt.y) / m;
        let hi_c = rect.center() - dir * rect.width() * 0.35;
        soft_light(&clip, hi_c, rect.width() * 0.55, Color32::from_white_alpha((28.0 * m) as u8));
        let lo_c = rect.center() + dir * rect.width() * 0.45;
        soft_light(&clip, lo_c, rect.width() * 0.6, Color32::from_black_alpha((40.0 * m) as u8));
    }
    painter.rect_stroke(
        rect,
        CornerRadius::same(CARD_RADIUS),
        egui::Stroke::new(1.0_f32, Color32::from_white_alpha(crate::glass::GlassAlphas::HAIRLINE)),
        egui::StrokeKind::Inside,
    );
}

/// A feathered disc of light or shade (the same mesh the live backgrounds use).
fn soft_light(painter: &egui::Painter, centre: Pos2, r: f32, tint: Color32) {
    let a = tint.a();
    if a == 0 {
        return;
    }
    let [cr, cg, cb, _] = tint.to_srgba_unmultiplied();
    crate::livebg::soft_disc(painter, centre, r, Color32::from_rgb(cr, cg, cb), a);
}

// ── the SVG ────────────────────────────────────────────────────────────────────────────

/// The code as an SVG with the same eyes and pills, and the same reveal as SMIL: every
/// element has its own `<animate>` on opacity with the offset `reveal_delay` gives it, and
/// pills grow from their left end. Opens in a browser drawing itself; dependency-free; for
/// sharing or saving a code. `side` is the viewBox in user units; colours as `#rrggbb`.
pub fn svg(layout: &Layout, side: f32, fg: Color32, bg: Color32) -> String {
    let hex = |c: Color32| {
        let [r, g, b, _] = c.to_srgba_unmultiplied();
        format!("#{r:02x}{g:02x}{b:02x}")
    };
    let (fg_s, bg_s) = (hex(fg), hex(bg));
    let inner = side - 2.0 * CARD_PAD;
    let cell = (inner / (MODULES as f32 + 1.0)).max(1.0);
    let used = cell * MODULES as f32;
    let o = CARD_PAD + (inner - used) * 0.5;
    let at = |x: usize, y: usize| (o + x as f32 * cell, o + y as f32 * cell);
    let dur = REVEAL_ELEMENT_S;
    let mut out = String::new();
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\" width=\"{side}\" height=\"{side}\">\n<rect width=\"{side}\" height=\"{side}\" rx=\"{CARD_RADIUS}\" fill=\"{bg_s}\"/>\n"
    ));
    for (i, (ox, oy)) in [(0usize, 0usize), (MODULES - 7, 0), (0, MODULES - 7)].into_iter().enumerate() {
        let d = eye_delay(i);
        let (x, y) = at(ox, oy);
        out.push_str(&format!("<g opacity=\"0\"><animate attributeName=\"opacity\" from=\"0\" to=\"1\" begin=\"{d:.3}s\" dur=\"{dur:.2}s\" fill=\"freeze\"/>\n"));
        out.push_str(&format!("<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{:.2}\" height=\"{:.2}\" rx=\"{:.2}\" fill=\"{fg_s}\"/>\n", 7.0 * cell, 7.0 * cell, 1.6 * cell));
        out.push_str(&format!("<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" rx=\"{:.2}\" fill=\"{bg_s}\"/>\n", x + cell, y + cell, 5.0 * cell, 5.0 * cell, 1.0 * cell));
        out.push_str(&format!("<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" rx=\"{:.2}\" fill=\"{fg_s}\"/>\n</g>\n", x + 2.0 * cell, y + 2.0 * cell, 3.0 * cell, 3.0 * cell, 0.8 * cell));
    }
    let r = cell * 0.44;
    for y in 0..MODULES {
        let mut x = 0;
        while x < MODULES {
            if in_finder(x, y) || !layout.get(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x + 1 < MODULES && layout.get(x + 1, y) && !in_finder(x + 1, y) {
                x += 1;
            }
            let d = reveal_delay((start + x) / 2, y);
            let (ax, ay) = at(start, y);
            let (cy, ) = (ay + cell * 0.5,);
            let w_full = (x - start) as f32 * cell + 2.0 * r;
            let x_left = ax + cell * 0.5 - r;
            let x_mid = x_left + w_full * 0.5;
            // grows from its centre: x slides left as width grows, so the middle stays put
            out.push_str(&format!(
                "<rect x=\"{x_left:.2}\" y=\"{:.2}\" width=\"{w_full:.2}\" height=\"{:.2}\" rx=\"{r:.2}\" fill=\"{fg_s}\" opacity=\"0\"><animate attributeName=\"opacity\" from=\"0\" to=\"1\" begin=\"{d:.3}s\" dur=\"{dur:.2}s\" fill=\"freeze\"/><animate attributeName=\"x\" from=\"{x_mid:.2}\" to=\"{x_left:.2}\" begin=\"{d:.3}s\" dur=\"{dur:.2}s\" fill=\"freeze\"/><animate attributeName=\"width\" from=\"0\" to=\"{w_full:.2}\" begin=\"{d:.3}s\" dur=\"{dur:.2}s\" fill=\"freeze\"/></rect>\n",
                cy - r, 2.0 * r
            ));
            x += 1;
        }
    }
    out.push_str("</svg>\n");
    out
}

/// Inside one of the three 7×7 finder eyes (drawn as shapes, not as modules).
fn in_finder(x: usize, y: usize) -> bool {
    let eye = |ox: usize, oy: usize| x >= ox && x < ox + 7 && y >= oy && y < oy + 7;
    eye(0, 0) || eye(MODULES - 7, 0) || eye(0, MODULES - 7)
}

// ── the expanded overlay ───────────────────────────────────────────────────────────────

/// Releases are ignored this long after opening — the opening tap's own release must not
/// close it (the C++ `m_openGuardActive`).
pub const OPEN_GUARD_S: f64 = 0.30;
/// The fade-in (the C++ 220 ms, OutCubic).
pub const FADE_S: f64 = 0.22;
/// The Close button's height (the C++ `setMinimumHeight(48)`).
pub const CLOSE_HEIGHT: f32 = 48.0;

/// The expanded view's state: when it opened. Pure; the timing rules are tested.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Expanded {
    pub opened_at: f64,
}

impl Expanded {
    pub fn open(now: f64) -> Expanded {
        Expanded { opened_at: now }
    }
    /// 0..1, OutCubic over the fade.
    pub fn alpha(&self, now: f64) -> f32 {
        let t = ((now - self.opened_at) / FADE_S).clamp(0.0, 1.0) as f32;
        crate::motion::ease::out_cubic(t)
    }
    /// May a release close it yet?
    pub fn can_close(&self, now: f64) -> bool {
        now - self.opened_at >= OPEN_GUARD_S
    }
    /// The code's side for a window: 80% of the usable width, capped at 70% of the usable
    /// height (the C++ `qMin(width - 2m, (height - 2m) * 7 / 10)`), margin 16 under 420 wide.
    pub fn qr_side(window: Rect) -> (f32, f32) {
        let margin = if window.width() < 420.0 { 16.0 } else { 32.0 };
        let side = (window.width() - margin * 2.0).min((window.height() - margin * 2.0) * 0.7);
        (side.max(64.0), margin)
    }
}

/// What the expanded overlay asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExpandedEvent {
    /// Closed this frame (a tap on the scrim after the guard, Escape, or Close).
    pub closed: bool,
    /// "Save SVG" pressed: the caller writes [`svg`] of this code somewhere shareable.
    pub save_svg: bool,
}

/// Draw the expanded overlay. `state` is `Some` while open; it is set to `None` on the
/// frame it closes. Returns whether it closed — see [`expanded_ui_with`] for the SVG button.
pub fn expanded_ui(ctx: &egui::Context, state: &mut Option<Expanded>, id: &str, layout: &Layout, c: &PaletteColors) -> bool {
    expanded_ui_with(ctx, state, id, layout, c, None).closed
}

/// [`expanded_ui`], reporting the Save SVG press as well, with a tilt: the sensor's when
/// the caller has one, else the pointer's over the code (a desktop's mouse tilts it).
pub fn expanded_ui_with(ctx: &egui::Context, state: &mut Option<Expanded>, id: &str, layout: &Layout, c: &PaletteColors, tilt: Option<Tilt>) -> ExpandedEvent {
    let mut ev = ExpandedEvent::default();
    let Some(ex) = *state else { return ev };
    let now = ctx.input(|i| i.time);
    let a = ex.alpha(now);
    if a < 1.0 {
        ctx.request_repaint();
    }
    // The USABLE area — a code under the status bar cannot be scanned.
    let window = crate::usable_rect(ctx);
    let (side, margin) = Expanded::qr_side(window);
    let mut close = false;
    // ABOVE EVERYTHING, including the dialog that opened it: a Foreground
    // Area painted before the dialog's own Foreground layer lost to it, so
    // the scrim came up and the code stayed hidden behind the small one.
    let area = egui::Area::new(egui::Id::new("uikit-qr-expanded"))
        .order(egui::Order::Tooltip)
        .fixed_pos(window.min)
        .interactable(true);
    area.show(ctx, |ui| {
        ui.set_min_size(window.size());
        let scrim = ui.allocate_rect(window, Sense::click());
        ui.painter().rect_filled(window, 0.0, Color32::from_rgba_unmultiplied(c.bg_base.r(), c.bg_base.g(), c.bg_base.b(), (242.0 * a) as u8));
        // the column: code, id, hint, close — centred, margin above and below
        let code_rect = Rect::from_center_size(
            Pos2::new(window.center().x, window.top() + margin + side * 0.5 + (window.height() - margin * 2.0 - side - 140.0).max(0.0) * 0.5),
            egui::vec2(side, side),
        );
        let mut card = ui.new_child(egui::UiBuilder::new().max_rect(code_rect).sense(Sense::click()));
        let tilt = tilt.unwrap_or_else(|| {
            ui.input(|i| i.pointer.hover_pos()).map(|p| Tilt::from_pointer(p, code_rect)).unwrap_or_default()
        });
        let _code = qr_ui_tilted(&mut card, layout, side, c, Some(ex.opened_at), tilt);
        // fade: paint the base over the top at 1-a, so the whole thing comes up together
        if a < 1.0 {
            ui.painter().rect_filled(window, 0.0, Color32::from_rgba_unmultiplied(c.bg_base.r(), c.bg_base.g(), c.bg_base.b(), (255.0 * (1.0 - a)) as u8));
        }
        let mut y = code_rect.bottom() + 16.0;
        let id_font = egui::FontId::monospace(24.0);
        ui.painter().text(Pos2::new(window.center().x, y), egui::Align2::CENTER_TOP, id, id_font, c.text);
        y += 40.0;
        ui.painter().text(Pos2::new(window.center().x, y), egui::Align2::CENTER_TOP, "Tap outside the code or press Close", egui::FontId::proportional(13.0), c.text_weak);
        y += 30.0;
        // two glass keys side by side, 48 high (the C++ `padding: 12px 24px`): Save SVG, Close
        let row_y = (y + CLOSE_HEIGHT * 0.5).min(window.bottom() - margin - CLOSE_HEIGHT * 0.5);
        let btn_rect = Rect::from_center_size(Pos2::new(window.center().x, row_y), egui::vec2(300.0, CLOSE_HEIGHT));
        let mut brow = ui.new_child(egui::UiBuilder::new().max_rect(btn_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let key = |label: &str| {
            egui::Button::new(label)
                .fill(crate::glass::key_fill(c.bg_card))
                .stroke(crate::glass::key_border(c.border))
                .corner_radius(CornerRadius::same(CARD_RADIUS))
                .min_size(egui::vec2(140.0, CLOSE_HEIGHT))
        };
        if brow.add(key("Save SVG")).on_hover_text("the code as an SVG that draws itself in").clicked() {
            ev.save_svg = true;
        }
        brow.add_space(12.0);
        if brow.add(key("\u{2715}  Close")).clicked() {
            close = true;
        }
        // a tap on the scrim, after the guard, and not on the code or the buttons
        if scrim.clicked() && ex.can_close(now) {
            if let Some(p) = scrim.interact_pointer_pos() {
                if !code_rect.contains(p) && !btn_rect.contains(p) {
                    close = true;
                }
            }
        }
    });
    // CONSUMED, not merely read: the expanded code sits over a terminal, and an Escape that
    // closes it and ALSO reaches the pty types `^[` at the shell. `consume_key` takes it
    // off the event list for everyone below. Seen under Xvfb in voiceterm.
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        close = true;
    }
    if close {
        *state = None;
    }
    ev.closed = close;
    ev
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlay_ignores_the_opening_taps_release_and_fades_out_cubic() {
        let ex = Expanded::open(10.0);
        assert!(!ex.can_close(10.0) && !ex.can_close(10.29), "inside the guard");
        assert!(ex.can_close(10.30));
        assert_eq!(ex.alpha(10.0), 0.0);
        assert!((ex.alpha(10.0 + FADE_S) - 1.0).abs() < 1e-6);
        let mid = ex.alpha(10.0 + FADE_S * 0.5);
        assert!(mid > 0.8, "OutCubic is front-loaded: {mid}");
    }

    #[test]
    fn the_expanded_code_is_eighty_percent_wide_capped_by_the_height() {
        let (side, m) = Expanded::qr_side(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0)));
        assert_eq!(m, 16.0, "a phone-width window gets the tighter margin");
        assert_eq!(side, 400.0 - 32.0, "width-bound");
        let (side, m) = Expanded::qr_side(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 500.0)));
        assert_eq!(m, 32.0);
        assert!((side - (500.0 - 64.0) * 0.7).abs() < 1e-3, "height-bound");
    }

    #[test]
    fn the_theme_picks_the_text_on_a_dark_card_and_the_accent_on_a_pale_one_then_guards() {
        let c = crate::default_palette();
        let (bg, fg) = theme_colors(&c);
        assert!(contrast(rgb(bg), rgb(fg)) >= MIN_CONTRAST, "a themed code still scans");
        let mut pale = c;
        pale.bg_card = Color32::from_rgb(245, 240, 230);
        pale.accent = Color32::from_rgb(40, 60, 120);
        let (_, fg) = theme_colors(&pale);
        assert_eq!(rgb(fg), [40, 60, 120], "the accent on a pale card, as the C++ does");
        pale.accent = Color32::from_rgb(230, 230, 220);
        let (_, fg) = theme_colors(&pale);
        assert_eq!(rgb(fg), [0, 0, 0], "an accent that would not scan falls back to black");
    }

    #[test]
    fn the_reveal_leads_with_the_eyes_and_every_cell_has_its_own_offset_by_the_total() {
        assert!(eye_delay(0) < eye_delay(2) && eye_delay(2) < reveal_delay(8, 8), "eyes first");
        let mut delays: Vec<f64> = Vec::new();
        for y in 0..MODULES {
            for x in 0..MODULES {
                delays.push(reveal_delay(x, y));
            }
        }
        let hi = delays.iter().cloned().fold(f64::MIN, f64::max);
        assert!(hi + REVEAL_ELEMENT_S <= REVEAL_TOTAL_S + 1e-9, "everything is in by the total");
        let mut distinct: Vec<i64> = delays.iter().map(|d| (d * 1e6) as i64).collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), MODULES * MODULES, "no two cells share an offset");
        assert_eq!(reveal_delay(3, 9), reveal_delay(3, 9), "deterministic");
        assert_eq!(reveal_progress(0.0, 0.5), 0.0);
        assert!((reveal_progress(0.5 + REVEAL_ELEMENT_S, 0.5) - 1.0).abs() < 1e-6);
        assert!(reveal_progress(0.5 + REVEAL_ELEMENT_S * 0.5, 0.5) > 0.8, "ease-out: most of the way early");
    }

    /// THE FRACTAL GUIDE. At any moment the cells already started are an even sample of the
    /// grid: the earliest tenth lands in every quadrant in near-equal numbers, and so does
    /// the earliest quarter — the quadtree filling in level by level, not a hash's clumps.
    #[test]
    fn the_earliest_cells_are_an_even_sample_of_every_quadrant() {
        let mut cells: Vec<(usize, usize, f64)> = Vec::new();
        for y in 0..MODULES {
            for x in 0..MODULES {
                cells.push((x, y, reveal_delay(x, y)));
            }
        }
        cells.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());
        for frac in [0.10f32, 0.25, 0.5] {
            let n = (cells.len() as f32 * frac) as usize;
            let mut quad = [0usize; 4];
            for (x, y, _) in &cells[..n] {
                quad[(*x >= 11) as usize + 2 * (*y >= 11) as usize] += 1;
            }
            let (lo, hi) = (*quad.iter().min().unwrap(), *quad.iter().max().unwrap());
            assert!(hi - lo <= n / 8 + 2, "first {frac}: quadrants {quad:?} — not even");
        }
        // the very first few are far apart: the coarsest level of the tree
        let (x0, y0, _) = cells[0];
        let (x1, y1, _) = cells[1];
        assert!((x0 as i64 - x1 as i64).abs() + (y0 as i64 - y1 as i64).abs() >= 8, "the first two cells are not neighbours");
    }

    #[test]
    fn the_svg_is_the_same_code_with_a_staggered_smil_reveal() {
        let layout = layout_for("sc-abcdef01");
        let s = svg(&layout, 256.0, Color32::from_rgb(230, 225, 215), Color32::from_rgb(28, 27, 25));
        assert!(s.starts_with("<svg") && s.trim_end().ends_with("</svg>"));
        assert_eq!(s.matches("<g opacity=\"0\">").count(), 3, "three eyes");
        let animates = s.matches("<animate ").count();
        let pills = s.matches("<rect ").count() - 1 - 9; // minus the card and the eyes' nine rects
        assert_eq!(animates, 3 + 3 * pills, "every pill animates opacity, x and width (it grows from its centre); every eye its opacity");
        assert!(s.contains("fill=\"#e6e1d7\"") && s.contains("fill=\"#1c1b19\""));
        let begins: std::collections::HashSet<&str> = s.split("begin=\"").skip(1).map(|t| t.split('s').next().unwrap()).collect();
        assert!(begins.len() > 40, "{} distinct begin offsets", begins.len());
    }

    #[test]
    fn the_id_round_trips_through_the_layout_and_a_hashed_square_does_not_decode() {
        for id in ["sc-b25a8b59", "sc-0001", "sc-deadbeefcafe"] {
            let l = layout_for(id);
            assert_eq!(decode_layout(&l).as_deref(), Some(id), "{id}");
        }
        assert_eq!(data_cells().count(), 180);
        // a square with no id in it — the old hashed data field, or noise
        let mut noise = layout_for("");
        for (i, (x, y)) in data_cells().enumerate() {
            noise.modules[y * MODULES + x] = (i * 7919) % 3 == 0;
        }
        assert_eq!(decode_layout(&noise), None);
    }

    #[test]
    fn a_rendered_code_is_scanned_back_straight_scaled_and_rotated() {
        let id = "sc-b25a8b59";
        let l = layout_for(id);
        for (cell, rot) in [(6.0f32, 0.0f32), (4.0, 0.0), (7.0, 0.35), (5.0, -1.2), (6.0, 1.57)] {
            let (w, h) = (320usize, 320usize);
            let mut rgba = vec![0u8; w * h * 4];
            // a mid-grey backdrop and a light card, so the threshold has something to split
            for i in 0..w * h {
                rgba[i * 4..i * 4 + 4].copy_from_slice(&[150, 150, 150, 255]);
            }
            let side = cell * MODULES as f32;
            let o = ((w as f32 - side) / 2.0, (h as f32 - side) / 2.0);
            // the card: light, a quiet zone around the modules
            for py in 0..h {
                for px in 0..w {
                    let dx = px as f32 - w as f32 / 2.0;
                    let dy = py as f32 - h as f32 / 2.0;
                    if dx.abs() < side * 0.8 && dy.abs() < side * 0.8 {
                        rgba[(py * w + px) * 4..(py * w + px) * 4 + 4].copy_from_slice(&[235, 232, 226, 255]);
                    }
                }
            }
            rasterize(&l, &mut rgba, w, h, o, cell, rot, [20, 20, 22]);
            assert_eq!(decode_frame(&rgba, w, h).as_deref(), Some(id), "cell {cell} rot {rot}");
        }
        // an empty frame decodes to nothing
        let blank = vec![120u8; 64 * 64 * 4];
        assert_eq!(decode_frame(&blank, 64, 64), None);
    }

    #[test]
    fn a_tilt_comes_from_gravity_or_the_pointer_and_settles() {
        assert_eq!(Tilt::from_accel(0.0, 0.0, 9.8), Tilt::default(), "face up is flat");
        let t = Tilt::from_accel(4.9, 0.0, 8.5);
        assert!(t.x > 0.4 && t.x < 0.6 && t.y.abs() < 1e-6);
        assert_eq!(Tilt::from_accel(0.0, 0.0, 0.0), Tilt::default(), "no reading is flat, not NaN");
        let r = Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0));
        assert_eq!(Tilt::from_pointer(Pos2::new(50.0, 50.0), r), Tilt::default());
        assert_eq!(Tilt::from_pointer(Pos2::new(100.0, 0.0), r), Tilt { x: 1.0, y: -1.0 });
        assert_eq!(Tilt::from_pointer(Pos2::new(500.0, 50.0), r).x, 1.0, "clamped");
        let mut s = Tilt::default();
        for _ in 0..30 {
            s = s.ease_toward(Tilt { x: 1.0, y: 0.0 }, 0.15);
        }
        assert!(s.x > 0.98, "settled in half a second at 60 Hz: {}", s.x);
        assert_eq!(parallax(Tilt { x: 1.0, y: 0.0 }, 1.0, 4.0), egui::vec2(-4.0, 0.0));
        assert_eq!(parallax(Tilt { x: 1.0, y: 0.0 }, 0.0, 4.0), egui::vec2(0.0, 0.0), "the card face does not move");
    }

    #[test]
    fn the_small_code_and_the_overlay_draw_headless() {
        let c = crate::default_palette();
        let layout = layout_for("sc-abcdef01");
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0)));
        let mut ex = Some(Expanded::open(0.0));
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = qr_ui(ui, &layout, 168.0, &c, None);
                assert!(!r.clicked());
                // and a reveal that has run its course draws everything too, tilted
                let _ = qr_ui_tilted(ui, &layout, 120.0, &c, Some(-REVEAL_TOTAL_S), Tilt { x: 0.4, y: -0.3 });
            });
            let closed = expanded_ui(ctx, &mut ex, "sc-abcdef01", &layout, &c);
            assert!(!closed, "nothing closes it on a frame with no input");
        });
        assert!(ex.is_some());
        assert!(out.shapes.len() > 40, "{} shapes — the modules were not drawn", out.shapes.len());
    }

    #[test]
    fn a_cell_is_a_whole_number_of_pixels() {
        // A fractional module resamples and blurs the edges — "visible as the
        // soft edges on the FP5 QR".
        for px in [64u32, 100, 220, 333] {
            for dpr in [1.0f32, 1.5, 2.0, 2.625, 3.0] {
                let c = cell_size(px, dpr);
                assert!(c >= MIN_CELL);
                assert_eq!(image_size(px, dpr) % c, 0, "image is a whole number of cells");
            }
        }
    }

    #[test]
    fn a_higher_pixel_ratio_renders_more_pixels() {
        // Drawing at logical size and letting the toolkit upscale is exactly
        // the blur the C++ names.
        assert!(cell_size(200, 3.0) > cell_size(200, 1.0));
    }

    #[test]
    fn a_tiny_request_still_produces_visible_modules() {
        // Below two pixels a module stops rendering as anything.
        assert_eq!(cell_size(10, 1.0), MIN_CELL);
        assert_eq!(cell_size(1, 1.0), MIN_CELL);
        assert_eq!(cell_size(0, 1.0), MIN_CELL, "zero does not divide to zero");
    }

    #[test]
    fn a_zero_or_negative_pixel_ratio_does_not_collapse_the_render() {
        // A platform reporting no DPR yet is reachable during startup.
        assert!(cell_size(200, 0.0) >= MIN_CELL);
        assert!(cell_size(200, -1.0) >= MIN_CELL);
    }

    #[test]
    fn a_low_contrast_theme_falls_back_to_black_or_white() {
        // A camera has to separate the modules, and a QR that NEARLY scans is
        // worse than one that obviously does — the user retries instead of
        // moving the phone.
        let dark_bg = [30, 32, 38];
        let dim_fg = [60, 64, 70];
        assert!(contrast(dark_bg, dim_fg) < MIN_CONTRAST);
        assert_eq!(readable_fg(dark_bg, dim_fg), [255, 255, 255]);

        let light_bg = [245, 245, 245];
        assert_eq!(readable_fg(light_bg, [200, 200, 200]), [0, 0, 0]);
    }

    #[test]
    fn a_theme_that_already_passes_keeps_its_own_colour() {
        // Overriding a palette that works would make every QR look foreign to
        // the app around it.
        let bg = [255, 255, 255];
        let fg = [20, 24, 30];
        assert!(contrast(bg, fg) >= MIN_CONTRAST);
        assert_eq!(readable_fg(bg, fg), fg);
    }

    #[test]
    fn the_contrast_ratio_matches_the_wcag_definition() {
        // Black on white is 21:1 exactly; a formula that is off here is off
        // everywhere.
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 0.01);
        assert!((contrast([255, 255, 255], [255, 255, 255]) - 1.0).abs() < 0.01);
    }

    #[test]
    fn contrast_is_symmetric() {
        let a = [12, 90, 200];
        let b = [240, 240, 230];
        assert!((contrast(a, b) - contrast(b, a)).abs() < 1e-4);
    }

    #[test]
    fn all_three_finder_patterns_are_present_and_correctly_shaped() {
        // These are what a scanner locates first; getting the ring-and-core
        // shape wrong means nothing detects the code at all.
        let l = layout_for("sc-abc12345");
        for (ox, oy) in [(0, 0), (MODULES - 7, 0), (0, MODULES - 7)] {
            assert!(l.get(ox, oy), "outer ring at {ox},{oy}");
            assert!(!l.get(ox + 1, oy + 1), "light ring");
            assert!(l.get(ox + 3, oy + 3), "dark core");
        }
    }

    #[test]
    fn the_fourth_corner_is_not_a_finder() {
        // Four finders is not a QR; a scanner uses the missing one to work out
        // the orientation.
        let l = layout_for("sc-abc12345");
        let (ox, oy) = (MODULES - 7, MODULES - 7);
        let ring_complete = (0..7).all(|d| l.get(ox + d, oy));
        assert!(!ring_complete, "bottom-right must not look like a finder");
    }

    #[test]
    fn data_never_lands_in_a_reserved_area() {
        // A stray dark module beside a finder breaks detection outright rather
        // than corrupting a byte.
        for id in ["sc-00000000", "sc-ffffffff", "sc-abc12345"] {
            let l = layout_for(id);
            for (ox, oy) in [(0usize, 0usize), (MODULES - 7, 0), (0, MODULES - 7)] {
                // The separator ring just outside each finder stays light.
                if ox + 7 < MODULES { assert!(!l.get(ox + 7, oy), "{id}: separator"); }
                if oy + 7 < MODULES { assert!(!l.get(ox, oy + 7), "{id}: separator"); }
            }
        }
    }

    #[test]
    fn timing_patterns_alternate() {
        // The row a scanner uses to measure module size.
        let l = layout_for("sc-abc12345");
        for i in 8..MODULES - 8 {
            assert_eq!(l.get(i, 6), i % 2 == 0, "timing row at {i}");
            assert_eq!(l.get(6, i), i % 2 == 0, "timing column at {i}");
        }
    }

    #[test]
    fn one_id_always_renders_the_same_square() {
        // The property that makes it usable as an identicon at all.
        assert_eq!(layout_for("sc-abc12345"), layout_for("sc-abc12345"));
    }

    #[test]
    fn different_ids_render_differently() {
        // Two contacts sharing a square is worse than no square.
        let a = layout_for("sc-abc12345");
        let b = layout_for("sc-abc12346");
        assert_ne!(a, b, "one character apart must differ");
    }

    #[test]
    fn an_empty_id_still_produces_a_valid_frame() {
        // Reachable before an identity exists; the finders should render so the
        // placeholder looks like the real thing rather than an empty box.
        let l = layout_for("");
        assert!(l.get(0, 0));
        assert!(l.get(3, 3));
    }

    #[test]
    fn reading_outside_the_grid_returns_light_rather_than_panicking() {
        let l = layout_for("sc-abc12345");
        assert!(!l.get(MODULES, 0));
        assert!(!l.get(0, MODULES + 100));
    }
}
