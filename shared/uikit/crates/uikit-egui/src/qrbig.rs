//! qrbig — a REAL QR code (Nayuki's `qrcodegen`), drawn in the same hand as the identicon,
//! and a SEQUENCE of them for data that will not fit in one.
//!
//! `qr` is a 21×21 identicon that carries a 22-byte id. This carries data: up to 2,953
//! bytes per symbol at version 40 with low error correction, decodable by any phone camera.
//! For more than that — a zettelkasten — the payload is cut into frames with a small
//! header (`VZK`, part, count, a checksum of the whole), and the viewer cycles them; a
//! scanner that knows the header reassembles, one that does not gets readable text parts.
//!
//! The drawing is the identicon's: rounded finder eyes, and dots for the data modules when
//! they are large enough to survive it (a module under three pixels is drawn square —
//! rounded corners on a two-pixel dot are a blur the camera has to fight).

use crate::PaletteColors;
use egui::{Color32, CornerRadius, Pos2, Rect, Ui};

/// A rendered symbol: `size` modules a side, row-major `dark`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matrix {
    pub size: usize,
    pub dark: Vec<bool>,
}

impl Matrix {
    pub fn get(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.dark[y * self.size + x]
    }
}

/// The most bytes one frame carries at version 40, ECC low, binary mode.
pub const FRAME_BYTES: usize = 2953;
/// The frame header's size: `VZK` + part(2) + count(2) + checksum(4).
pub const HEADER_BYTES: usize = 11;
/// Bytes of payload per frame after the header.
pub const FRAME_PAYLOAD: usize = FRAME_BYTES - HEADER_BYTES;

/// Encode bytes into one symbol. `None` when it does not fit even at version 40.
pub fn encode(data: &[u8]) -> Option<Matrix> {
    let qr = qrcodegen::QrCode::encode_binary(data, qrcodegen::QrCodeEcc::Low).ok()?;
    let size = qr.size() as usize;
    let mut dark = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            dark.push(qr.get_module(x as i32, y as i32));
        }
    }
    Some(Matrix { size, dark })
}

/// FNV-1a over the whole payload, so every frame names the same set.
pub fn checksum(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in data {
        h ^= *b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// Cut a payload into frames, each with its header. One frame when it fits.
pub fn frames(data: &[u8]) -> Vec<Vec<u8>> {
    let sum = checksum(data);
    let count = data.len().div_ceil(FRAME_PAYLOAD).max(1);
    (0..count)
        .map(|i| {
            let start = i * FRAME_PAYLOAD;
            let end = (start + FRAME_PAYLOAD).min(data.len());
            let mut f = Vec::with_capacity(HEADER_BYTES + end.saturating_sub(start));
            f.extend_from_slice(b"VZK");
            f.extend_from_slice(&(i as u16).to_be_bytes());
            f.extend_from_slice(&(count as u16).to_be_bytes());
            f.extend_from_slice(&sum.to_be_bytes());
            f.extend_from_slice(&data[start.min(data.len())..end]);
            f
        })
        .collect()
}

/// Read a frame's header: (part, count, checksum) and the payload after it.
pub fn parse_frame(frame: &[u8]) -> Option<(u16, u16, u32, &[u8])> {
    if frame.len() < HEADER_BYTES || &frame[..3] != b"VZK" {
        return None;
    }
    let part = u16::from_be_bytes([frame[3], frame[4]]);
    let count = u16::from_be_bytes([frame[5], frame[6]]);
    let sum = u32::from_be_bytes([frame[7], frame[8], frame[9], frame[10]]);
    Some((part, count, sum, &frame[HEADER_BYTES..]))
}

/// Reassemble frames (any order, duplicates allowed). `None` until every part is in, or if
/// the parts disagree about the whole.
pub fn reassemble(frames: &[Vec<u8>]) -> Option<Vec<u8>> {
    let first = parse_frame(frames.first()?)?;
    let (count, sum) = (first.1 as usize, first.2);
    let mut parts: Vec<Option<&[u8]>> = vec![None; count];
    for f in frames {
        let (p, c, s, body) = parse_frame(f)?;
        if c as usize != count || s != sum || p as usize >= count {
            return None;
        }
        parts[p as usize] = Some(body);
    }
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(p?);
    }
    (checksum(&out) == sum).then_some(out)
}

/// How long the viewer shows each frame of a sequence.
pub const FRAME_DWELL_S: f64 = 1.2;
/// How long a symbol takes to appear, and how long one frame takes to become the next.
pub const APPEAR_S: f64 = 0.34;

/// Which frame to show at `t` seconds since the sequence started.
pub fn frame_at(t: f64, count: usize) -> usize {
    if count <= 1 {
        return 0;
    }
    ((t / FRAME_DWELL_S).max(0.0) as usize) % count
}

/// The appearance, eased out: 0..1 over [`APPEAR_S`] from `t`, on the kit's `out_cubic`.
/// A code that cuts in is a flash on a dark screen; one that eases in is a thing arriving.
pub fn appear(t: f64) -> f32 {
    crate::motion::ease::out_cubic((t / APPEAR_S).clamp(0.0, 1.0) as f32)
}

/// How far through the change from the previous frame to `frame_at(t)`: 1.0 once settled.
/// Each frame eases in over its first [`APPEAR_S`], so a sequence pulses rather than
/// stutters — and a scanner still gets a full still for most of the dwell.
pub fn frame_appear(t: f64, count: usize) -> f32 {
    if count <= 1 {
        return appear(t);
    }
    let into = (t.max(0.0)) % FRAME_DWELL_S;
    appear(into.min(t))
}

/// Draw a symbol into `rect` on the identicon's glass card. Quiet zone of four modules.
pub fn paint(painter: &egui::Painter, rect: Rect, m: &Matrix, pixels_per_point: f32, c: &PaletteColors) {
    paint_appearing(painter, rect, m, pixels_per_point, c, 1.0);
}

/// [`paint`] mid-appearance: `p` is 0..1 from [`appear`]. The card is there from the first
/// frame (it is the surface the code sits on); the modules fade up and settle from a
/// slightly smaller scale, so the code arrives rather than flashes.
pub fn paint_appearing(painter: &egui::Painter, rect: Rect, m: &Matrix, pixels_per_point: f32, c: &PaletteColors, p: f32) {
    let p = p.clamp(0.0, 1.0);
    let (bg, fg) = crate::qr::theme_colors(c);
    let fg = {
        let [r, g, b, a] = fg.to_srgba_unmultiplied();
        Color32::from_rgba_unmultiplied(r, g, b, (a as f32 * p) as u8)
    };
    painter.rect_filled(rect, CornerRadius::same(crate::qr::CARD_RADIUS), bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(crate::qr::CARD_RADIUS),
        egui::Stroke::new(1.0_f32, Color32::from_white_alpha(crate::glass::GlassAlphas::HAIRLINE)),
        egui::StrokeKind::Inside,
    );
    let quiet = 4.0;
    let ppp = pixels_per_point.max(0.1);
    let side = rect.width().min(rect.height());
    // an integer cell in physical pixels, converted back to points
    let cell_px = ((side * ppp) / (m.size as f32 + 2.0 * quiet)).floor().max(1.0);
    let cell = cell_px / ppp;
    // settles from 96% — small enough that a scanner reading mid-appearance still sees a
    // square grid, large enough that the arrival is visible
    let k = 0.96 + 0.04 * p;
    let cell = cell * k;
    let used = cell * m.size as f32;
    let origin = rect.center() - egui::vec2(used, used) * 0.5;
    let at = |x: usize, y: usize| origin + egui::vec2(x as f32 * cell, y as f32 * cell);
    let big = cell_px >= 3.0;
    let in_eye = |x: usize, y: usize| {
        let s = m.size;
        (x < 7 && y < 7) || (x >= s - 7 && y < 7) || (x < 7 && y >= s - 7)
    };
    // the three eyes, rounded as the identicon's
    for (ox, oy) in [(0usize, 0usize), (m.size - 7, 0), (0, m.size - 7)] {
        let sq = |o: usize, size: f32| Rect::from_min_size(at(ox + o, oy + o), egui::vec2(size * cell, size * cell));
        painter.rect_filled(sq(0, 7.0), CornerRadius::same((cell * 1.6) as u8), fg);
        painter.rect_filled(sq(1, 5.0), CornerRadius::same((cell * 1.0) as u8), bg);
        painter.rect_filled(sq(2, 3.0), CornerRadius::same((cell * 0.8) as u8), fg);
    }
    let r = cell * 0.5;
    for y in 0..m.size {
        let mut x = 0;
        while x < m.size {
            if in_eye(x, y) || !m.get(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x + 1 < m.size && m.get(x + 1, y) && !in_eye(x + 1, y) {
                x += 1;
            }
            let run = Rect::from_min_size(at(start, y), egui::vec2((x - start + 1) as f32 * cell, cell));
            if big {
                painter.rect_filled(run, CornerRadius::same(r as u8), fg);
            } else {
                painter.rect_filled(run, 0.0, fg);
            }
            x += 1;
        }
    }
}

/// A symbol as a widget: `side` points, on its card.
pub fn qr_ui(ui: &mut Ui, m: &Matrix, side: f32, c: &PaletteColors) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
    paint(ui.painter(), rect, m, ui.ctx().pixels_per_point(), c);
    resp
}

/// The huge view: a full-window scrim, the symbol at the largest square that fits, the
/// frame counter and the caption under it, Close. Cycles frames of a sequence on
/// [`FRAME_DWELL_S`]. `state` is the identicon's `Expanded` (same open guard and fade);
/// returns true on the frame it closes.
pub fn huge_ui(ctx: &egui::Context, state: &mut Option<crate::qr::Expanded>, frames: &[Matrix], caption: &str, c: &PaletteColors) -> bool {
    let Some(ex) = *state else { return false };
    let now = ctx.input(|i| i.time);
    let a = ex.alpha(now);
    if a < 1.0 || frames.len() > 1 {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
    // The USABLE area — a code under the status bar cannot be scanned.
    let window = crate::usable_rect(ctx);
    let margin = if window.width() < 420.0 { 12.0 } else { 24.0 };
    let side = (window.width() - margin * 2.0).min(window.height() - margin * 2.0 - 110.0).max(64.0);
    let idx = frame_at(now - ex.opened_at, frames.len());
    let mut close = false;
    egui::Area::new(egui::Id::new("uikit-qrbig-huge"))
        .order(egui::Order::Tooltip)
        .fixed_pos(window.min)
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_min_size(window.size());
            let scrim = ui.allocate_rect(window, egui::Sense::click());
            ui.painter().rect_filled(window, 0.0, Color32::from_rgba_unmultiplied(c.bg_base.r(), c.bg_base.g(), c.bg_base.b(), (242.0 * a) as u8));
            let code_rect = Rect::from_center_size(Pos2::new(window.center().x, window.top() + margin + side * 0.5), egui::vec2(side, side));
            if let Some(m) = frames.get(idx) {
                paint_appearing(ui.painter(), code_rect, m, ctx.pixels_per_point(), c, frame_appear(now - ex.opened_at, frames.len()));
            }
            if a < 1.0 {
                ui.painter().rect_filled(window, 0.0, Color32::from_rgba_unmultiplied(c.bg_base.r(), c.bg_base.g(), c.bg_base.b(), (255.0 * (1.0 - a)) as u8));
            }
            let mut y = code_rect.bottom() + 12.0;
            let line = if frames.len() > 1 { format!("frame {} of {} · {caption}", idx + 1, frames.len()) } else { caption.to_string() };
            ui.painter().text(Pos2::new(window.center().x, y), egui::Align2::CENTER_TOP, line, egui::FontId::proportional(15.0), c.text);
            y += 26.0;
            if frames.len() > 1 {
                // the progress ring for the sequence: which parts a scanner has had the chance to see
                let w = side.min(320.0);
                let bar = Rect::from_center_size(Pos2::new(window.center().x, y + 3.0), egui::vec2(w, 4.0));
                ui.painter().rect_filled(bar, 2.0, crate::glass::tint(c.border, crate::glass::GlassAlphas::BORDER));
                let done = Rect::from_min_size(bar.min, egui::vec2(w * (idx + 1) as f32 / frames.len() as f32, 4.0));
                ui.painter().rect_filled(done, 2.0, c.accent);
                y += 14.0;
            }
            let btn = Rect::from_center_size(Pos2::new(window.center().x, (y + 24.0).min(window.bottom() - margin - 24.0)), egui::vec2(160.0, crate::qr::CLOSE_HEIGHT));
            let mut brow = ui.new_child(egui::UiBuilder::new().max_rect(btn));
            if brow
                .add(
                    egui::Button::new("\u{2715}  Close")
                        .fill(crate::glass::key_fill(c.bg_card))
                        .stroke(crate::glass::key_border(c.border))
                        .corner_radius(CornerRadius::same(crate::qr::CARD_RADIUS))
                        .min_size(btn.size()),
                )
                .clicked()
            {
                close = true;
            }
            if scrim.clicked() && ex.can_close(now) {
                if let Some(p) = scrim.interact_pointer_pos() {
                    if !code_rect.contains(p) && !btn.contains(p) {
                        close = true;
                    }
                }
            }
        });
    // CONSUMED, not merely read: the huge code sits over a terminal, and an Escape that
    // closes it and ALSO reaches the pty types `^[` at the shell. `consume_key` takes it
    // off the event list for everyone below. Seen under Xvfb in voiceterm.
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        close = true;
    }
    if close {
        *state = None;
    }
    close
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_payload_is_one_symbol_and_a_long_one_is_a_sequence_that_reassembles() {
        let m = encode(b"hello zettelkasten").unwrap();
        assert!(m.size <= 25, "a short payload is a small version: {}", m.size);
        assert!(m.get(0, 0) && m.get(3, 3) && !m.get(1, 1), "finder eye at the origin");
        let data: Vec<u8> = (0..7000u32).map(|i| (i % 251) as u8).collect();
        let fr = frames(&data);
        assert_eq!(fr.len(), 3, "7000 bytes at {FRAME_PAYLOAD} per frame");
        for f in &fr {
            assert!(f.len() <= FRAME_BYTES);
            assert!(encode(f).is_some(), "every frame fits a version-40 symbol");
        }
        let mut shuffled = fr.clone();
        shuffled.rotate_left(1);
        shuffled.push(fr[0].clone()); // a duplicate, as a scanner would see
        assert_eq!(reassemble(&shuffled).as_deref(), Some(data.as_slice()));
        assert_eq!(reassemble(&fr[..2]), None, "a missing part is not a whole");
        let mut wrong = fr.clone();
        wrong[1][12] ^= 1;
        assert_eq!(reassemble(&wrong), None, "a corrupt part fails the checksum");
    }

    #[test]
    fn a_symbol_eases_in_and_every_frame_of_a_sequence_does_too() {
        assert_eq!(appear(0.0), 0.0);
        assert!((appear(APPEAR_S) - 1.0).abs() < 1e-6);
        assert!(appear(APPEAR_S * 0.5) > 0.8, "out_cubic: most of the way early");
        assert!(appear(APPEAR_S * 0.5) < 1.0);
        // a single frame eases once and stays
        assert!((frame_appear(5.0, 1) - 1.0).abs() < 1e-6);
        // in a sequence each frame eases over its own first moments
        assert_eq!(frame_appear(0.0, 3), 0.0);
        assert!((frame_appear(FRAME_DWELL_S - 0.01, 3) - 1.0).abs() < 1e-6, "settled by the end of a dwell");
        assert!(frame_appear(FRAME_DWELL_S + 0.001, 3) < 0.2, "the next frame starts from nothing");
        assert!((frame_appear(FRAME_DWELL_S + APPEAR_S, 3) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn frames_cycle_on_the_dwell_and_a_single_frame_never_moves() {
        assert_eq!(frame_at(0.0, 3), 0);
        assert_eq!(frame_at(FRAME_DWELL_S + 0.01, 3), 1);
        assert_eq!(frame_at(3.0 * FRAME_DWELL_S + 0.01, 3), 0, "wraps");
        assert_eq!(frame_at(99.0, 1), 0);
        assert_eq!(frame_at(-1.0, 3), 0, "before the start is the first frame");
    }

    #[test]
    fn the_symbol_and_the_huge_view_draw_headless() {
        let c = crate::default_palette();
        let m = encode(b"# note\nbody\n").unwrap();
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0)));
        let mut ex = Some(crate::qr::Expanded::open(0.0));
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                qr_ui(ui, &m, 200.0, &c);
            });
            assert!(!huge_ui(ctx, &mut ex, &[m.clone(), m.clone()], "2 notes", &c));
        });
        assert!(out.shapes.len() > 20);
    }
}
