//! share -- a note or an offer as QR codes, and many codes as one MOSAIC on a screen.
//!
//! A QR carries at most ~2.9 KB. More than that is cut into frames, each with a small header (magic, kind, part,
//! count, a checksum of the whole), so a scanner that sees them in any order puts them back. An e-ink panel
//! shows a still picture well and cycles animated codes badly, so instead of flashing frames one after another
//! the [`mosaic`] lays a dozen of them out at once, each as large as the screen allows (but never smaller than a
//! module a phone camera can resolve), and only starts a second screen when the first is full.

use qrcodegen::{QrCode, QrCodeEcc};

pub const FRAME_BYTES: usize = 2953; // version 40, ECC low, binary mode
pub const MAGIC: &[u8; 3] = b"JTR";
pub const HEADER: usize = 12; // magic 3 + kind 1 + part 2 + count 2 + checksum 4
pub const PAYLOAD: usize = FRAME_BYTES - HEADER;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Offer = 1, Note = 2 }

pub fn checksum(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in data { h ^= *b as u32; h = h.wrapping_mul(0x01000193); }
    h
}

/// Cut `data` into frames (one when it fits).
pub fn frames(kind: Kind, data: &[u8]) -> Vec<Vec<u8>> {
    let sum = checksum(data);
    let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(PAYLOAD).collect() };
    let count = chunks.len().min(u16::MAX as usize) as u16;
    chunks.iter().enumerate().take(count as usize).map(|(i, c)| {
        let mut f = Vec::with_capacity(HEADER + c.len());
        f.extend_from_slice(MAGIC);
        f.push(kind as u8);
        f.extend_from_slice(&(i as u16).to_be_bytes());
        f.extend_from_slice(&count.to_be_bytes());
        f.extend_from_slice(&sum.to_be_bytes());
        f.extend_from_slice(c);
        f
    }).collect()
}

/// `(kind, part, count, checksum, payload)` of one frame, or `None` if it is not ours.
pub fn parse_frame(f: &[u8]) -> Option<(Kind, u16, u16, u32, &[u8])> {
    if f.len() < HEADER || &f[..3] != MAGIC { return None; }
    let kind = match f[3] { 1 => Kind::Offer, 2 => Kind::Note, _ => return None };
    let part = u16::from_be_bytes([f[4], f[5]]);
    let count = u16::from_be_bytes([f[6], f[7]]);
    let sum = u32::from_be_bytes([f[8], f[9], f[10], f[11]]);
    (count > 0 && part < count).then_some((kind, part, count, sum, &f[HEADER..]))
}

/// Put the frames back together, in any order, with repeats allowed. `None` until every part has arrived and the
/// whole matches its checksum.
pub fn reassemble(got: &[Vec<u8>]) -> Option<(Kind, Vec<u8>)> {
    let mut parts: std::collections::BTreeMap<u16, &[u8]> = Default::default();
    let (mut count, mut sum, mut kind) = (0u16, 0u32, None);
    for f in got {
        let (k, p, c, s, pl) = parse_frame(f)?;
        if kind.is_some() && (kind != Some(k) || count != c || sum != s) { return None; } // frames of different sets
        (kind, count, sum) = (Some(k), c, s);
        parts.insert(p, pl);
    }
    if parts.len() != count as usize { return None; }
    let data: Vec<u8> = parts.values().flat_map(|p| p.iter().copied()).collect();
    (checksum(&data) == sum).then(|| (kind.unwrap(), data))
}

/// A rendered symbol: `size` modules a side, row-major, `true` = dark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matrix { pub size: usize, pub dark: Vec<bool> }

impl Matrix {
    pub fn get(&self, x: usize, y: usize) -> bool { x < self.size && y < self.size && self.dark[y * self.size + x] }

    /// Two rows per text line, for a terminal.
    pub fn ascii(&self) -> String {
        let mut s = String::new();
        let q = 2usize;
        let n = self.size as i64;
        let at = |x: i64, y: i64| x >= 0 && y >= 0 && x < n && y < n && self.dark[(y * n + x) as usize];
        let mut y = -(q as i64);
        while y < n + q as i64 {
            for x in -(q as i64)..n + q as i64 {
                s.push(match (at(x, y), at(x, y + 1)) { (true, true) => '█', (true, false) => '▀', (false, true) => '▄', _ => ' ' });
            }
            s.push('\n');
            y += 2;
        }
        s
    }
}

pub fn encode(data: &[u8]) -> Option<Matrix> {
    let qr = QrCode::encode_binary(data, QrCodeEcc::Low).ok()?;
    let size = qr.size() as usize;
    let mut dark = Vec::with_capacity(size * size);
    for y in 0..size { for x in 0..size { dark.push(qr.get_module(x as i32, y as i32)); } }
    Some(Matrix { size, dark })
}

/// One code on a screen: which frame, its top-left corner, how many pixels a module is, and the side in pixels
/// (including its quiet border).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell { pub frame: usize, pub x: u32, pub y: u32, pub module_px: u32, pub side_px: u32 }

/// Quiet-zone modules around each code.
pub const QUIET: u32 = 4;
/// The smallest module a phone camera resolves from a normal distance on an e-ink screen.
pub const MIN_MODULE_PX: u32 = 3;
/// Most codes on one screen.
pub const MAX_PER_SCREEN: usize = 12;

/// Lay `count` codes of `modules` modules a side out on screens of `w` x `h` pixels: as many per screen as keep
/// every module at least `min_module` pixels, in a grid, each code the same size and the grid centred. Returns the
/// screens, each a list of cells in reading order.
pub fn mosaic(count: usize, modules: usize, w: u32, h: u32, min_module: u32) -> Vec<Vec<Cell>> {
    if count == 0 || w == 0 || h == 0 { return vec![]; }
    let span = modules as u32 + 2 * QUIET;
    // the best grid for n per screen: the one with the biggest module
    let best_for = |n: usize| -> Option<(u32, u32, u32)> {
        (1..=n as u32).filter_map(|cols| {
            let rows = (n as u32).div_ceil(cols);
            let m = (w / cols / span).min(h / rows / span);
            (m >= min_module.max(1)).then_some((m, cols, rows))
        }).max_by_key(|(m, _, _)| *m)
    };
    // the most per screen that still reads
    let n = (1..=MAX_PER_SCREEN.min(count)).rev().find(|n| best_for(*n).is_some()).unwrap_or(1);
    let (m, cols, rows) = best_for(n).unwrap_or_else(|| {
        let m = (w.min(h) / span).max(1);
        (m, 1, 1)
    });
    let side = m * span;
    let mut screens = Vec::new();
    let mut i = 0;
    while i < count {
        let here = n.min(count - i);
        let used_rows = (here as u32).div_ceil(cols).min(rows);
        let (gw, gh) = (cols.min(here as u32) * side, used_rows * side);
        let (ox, oy) = ((w.saturating_sub(gw)) / 2, (h.saturating_sub(gh)) / 2);
        screens.push((0..here).map(|k| {
            let (cx, cy) = (k as u32 % cols, k as u32 / cols);
            Cell { frame: i + k, x: ox + cx * side, y: oy + cy * side, module_px: m, side_px: side }
        }).collect());
        i += here;
    }
    screens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(n: usize) -> Vec<u8> { (0..n).map(|i| (i.wrapping_mul(31) ^ (i >> 3)) as u8).collect() }

    #[test]
    fn data_survives_being_cut_into_frames_and_shuffled() {
        for n in [0usize, 1, 100, PAYLOAD, PAYLOAD + 1, PAYLOAD * 3 + 17] {
            let d = data(n);
            let f = frames(Kind::Note, &d);
            assert_eq!(f.len(), if n == 0 { 1 } else { n.div_ceil(PAYLOAD) }, "n={n}");
            assert!(f.iter().all(|x| x.len() <= FRAME_BYTES));
            assert_eq!(reassemble(&f), Some((Kind::Note, d.clone())), "n={n}");
            let mut shuffled = f.clone();
            shuffled.reverse();
            shuffled.push(f[0].clone()); // a repeat
            assert_eq!(reassemble(&shuffled), Some((Kind::Note, d)), "any order, repeats allowed (n={n})");
        }
    }

    #[test]
    fn a_missing_damaged_or_foreign_frame_never_assembles() {
        let d = data(PAYLOAD * 2 + 5);
        let f = frames(Kind::Note, &d);
        assert_eq!(reassemble(&f[..2]), None, "a part is missing");
        let mut bad = f.clone();
        let last = bad[1].len() - 1;
        bad[1][last] ^= 1;
        assert_eq!(reassemble(&bad), None, "the checksum catches a damaged frame");
        let other = frames(Kind::Note, &data(PAYLOAD * 2 + 9));
        assert_eq!(reassemble(&[f[0].clone(), other[1].clone(), f[2].clone()]), None, "frames of two different sets");
        assert_eq!(reassemble(&[b"hello".to_vec()]), None);
        assert!(parse_frame(&f[0]).is_some() && parse_frame(&f[0][..5]).is_none());
        let mixed = frames(Kind::Offer, &d);
        assert_eq!(reassemble(&[f[0].clone(), mixed[1].clone()]), None, "an offer frame does not join a note's");
    }

    #[test]
    fn a_frame_is_a_real_qr_symbol_of_the_right_size() {
        let m = encode(b"hello").expect("fits");
        assert_eq!(m.size, 21, "version 1");
        let big = encode(&frames(Kind::Note, &data(PAYLOAD))[0]).expect("a full frame fits at version 40");
        assert_eq!(big.size, 177);
        assert!(encode(&data(FRAME_BYTES + 1)).is_none(), "more than a symbol holds is refused, not truncated");
        // finder pattern: the top-left 7x7 has a dark border and a dark centre
        assert!((0..7).all(|i| m.get(i, 0) && m.get(0, i) && m.get(6, i) && m.get(i, 6)) && m.get(3, 3) && !m.get(1, 1));
        assert!(m.ascii().lines().count() >= 11 && m.ascii().contains('█'));
    }

    #[test]
    fn the_mosaic_puts_as_many_codes_on_a_screen_as_still_scan() {
        // the Paper Pro in portrait: 1620 x 2160, full-size codes (177 modules)
        let one = mosaic(1, 177, 1620, 2160, MIN_MODULE_PX);
        assert_eq!(one.len(), 1);
        assert!(one[0][0].module_px >= 8, "a single code is drawn big: {}", one[0][0].module_px);
        let six = mosaic(6, 177, 1620, 2160, MIN_MODULE_PX);
        let per_screen = six[0].len();
        assert!(per_screen >= 2 && six.iter().map(Vec::len).sum::<usize>() == 6);
        for screen in &mosaic(40, 177, 1620, 2160, MIN_MODULE_PX) {
            assert!(screen.len() <= MAX_PER_SCREEN);
            for c in screen {
                assert!(c.module_px >= MIN_MODULE_PX, "never below a module a camera resolves: {c:?}");
                assert!(c.x + c.side_px <= 1620 && c.y + c.side_px <= 2160, "inside the screen: {c:?}");
            }
            // no two overlap
            for (i, a) in screen.iter().enumerate() {
                for b in &screen[i + 1..] {
                    assert!(a.x + a.side_px <= b.x || b.x + b.side_px <= a.x || a.y + a.side_px <= b.y || b.y + b.side_px <= a.y, "{a:?} overlaps {b:?}");
                }
            }
        }
        let all: Vec<usize> = mosaic(40, 177, 1620, 2160, MIN_MODULE_PX).iter().flatten().map(|c| c.frame).collect();
        assert_eq!(all, (0..40).collect::<Vec<_>>(), "every frame appears once, in order");
        assert!(mosaic(0, 21, 100, 100, 3).is_empty());
        // a screen too small for the minimum still shows ONE code rather than nothing
        let tiny = mosaic(3, 177, 300, 300, MIN_MODULE_PX);
        assert_eq!(tiny.iter().map(Vec::len).sum::<usize>(), 3);
        assert!(tiny.iter().all(|s| s.len() == 1));
        // small codes (a short note) fit more on a screen than full-size ones
        assert!(mosaic(12, 41, 1620, 2160, MIN_MODULE_PX)[0].len() > per_screen);
    }
}
