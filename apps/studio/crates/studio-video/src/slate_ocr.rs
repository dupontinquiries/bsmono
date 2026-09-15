//! slate_ocr — reading the board, so the clips arrange themselves.
//!
//! `slate` already aligns sound to picture by the clap. This reads what
//! is WRITTEN on the same board, which is the other half of what a
//! clapperboard is for: the sticks say *when*, the writing says *which*.
//!
//! An import of forty files off a card arrives named `C0031.MP4`
//! through `C0070.MP4`, in the order the camera happened to write them.
//! Every one of them opens with someone holding up a board that says
//! scene 14, shot B, take 3. The information needed to arrange the
//! whole shoot is in the picture and nobody reads it.
//!
//! ## Where to look, and why that matters more than the OCR
//!
//! Not "scan the clip for a board". [`slate::find_clap`] already
//! locates the instant the sticks close, to within a window. The board
//! is held still, filled the frame, and was in focus in the second
//! BEFORE that -- and after it, it is gone. So the search is a handful
//! of frames at a known place, which turns an open-ended vision problem
//! into a small one.
//!
//! That ordering is the whole design. Getting the frame right is worth
//! more than any amount of cleverness in the recogniser, because a
//! recogniser fed the wrong frame cannot be rescued.
//!
//! ## The recogniser
//!
//! Template matching, in the spirit of `rusty-board`'s handwriting 1-NN
//! but on pixels rather than strokes: normalise each glyph box to a
//! fixed grid, compare against templates, take the nearest. A
//! clapperboard is the friendliest OCR target there is -- high
//! contrast, few characters, and a fixed vocabulary of digits and
//! capitals -- so the simple method is the right one and a general OCR
//! would be a dependency bought for nothing.
//!
//! ## What it refuses to do
//!
//! Guess. A misread take number silently files a good take under the
//! wrong name, and the editor finds out at the end. Every field carries
//! how sure the reader is, and an unsure field is left EMPTY rather
//! than filled with the nearest match -- an empty field asks a
//! question, a wrong one answers it.

use std::collections::BTreeMap;

/// A single-channel image. Whatever decodes the video hands one of
/// these over; nothing here knows about codecs.
#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    /// Row-major, 0 dark to 255 light.
    pub px: Vec<u8>,
}

impl Gray {
    pub fn new(w: usize, h: usize) -> Gray { Gray { w, h, px: vec![0; w * h] } }
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> u8 {
        if x >= self.w || y >= self.h { 0 } else { self.px[y * self.w + x] }
    }
    pub fn set(&mut self, x: usize, y: usize, v: u8) {
        if x < self.w && y < self.h { self.px[y * self.w + x] = v; }
    }
}

/// A rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect { pub x: usize, pub y: usize, pub w: usize, pub h: usize }

impl Rect {
    pub fn area(&self) -> usize { self.w * self.h }
    fn overlaps_rows(&self, other: &Rect) -> bool {
        self.y < other.y + other.h && other.y < self.y + self.h
    }
}

/// Split a region into ink and background.
///
/// A board is written in chalk or marker on a high-contrast face, so a
/// single threshold works -- but WHICH SIDE IS INK depends on the
/// board: black marker on white acrylic, or white chalk on slate. The
/// reader must not assume, so it picks whichever polarity produces the
/// more plausible glyph count rather than being told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink { DarkOnLight, LightOnDark }

/// Otsu's method: the threshold that best separates the histogram into
/// two groups. Chosen over a fixed value because a board under a
/// practical light and a board in the sun are both high-contrast and
/// nowhere near the same brightness.
pub fn threshold(g: &Gray, r: Rect) -> u8 {
    let mut hist = [0usize; 256];
    let mut n = 0usize;
    for y in r.y..(r.y + r.h).min(g.h) {
        for x in r.x..(r.x + r.w).min(g.w) { hist[g.at(x, y) as usize] += 1; n += 1; }
    }
    if n == 0 { return 128; }
    let total: f64 = hist.iter().enumerate().map(|(i, c)| i as f64 * *c as f64).sum();
    let (mut sum_b, mut w_b, mut best, mut best_t) = (0.0f64, 0usize, -1.0f64, 128u8);
    for t in 0..256 {
        w_b += hist[t];
        if w_b == 0 { continue; }
        let w_f = n - w_b;
        if w_f == 0 { break; }
        sum_b += t as f64 * hist[t] as f64;
        let m_b = sum_b / w_b as f64;
        let m_f = (total - sum_b) / w_f as f64;
        let between = w_b as f64 * w_f as f64 * (m_b - m_f) * (m_b - m_f);
        if between > best { best = between; best_t = t as u8; }
    }
    best_t
}

/// Find glyph-sized blobs of ink.
///
/// Connected components, then filtered by shape: a glyph is taller than
/// it is wide, a reasonable fraction of the region's height, and not a
/// speck. The filter matters more than the labelling -- a board has
/// screws, tape, glare and the edge of someone's thumb in it, and every
/// one of those is a connected component.
pub fn glyph_boxes(g: &Gray, r: Rect, ink: Ink, t: u8) -> Vec<Rect> {
    let is_ink = |x: usize, y: usize| match ink {
        Ink::DarkOnLight => g.at(x, y) < t,
        Ink::LightOnDark => g.at(x, y) >= t,
    };
    let (x0, y0) = (r.x, r.y);
    let (x1, y1) = ((r.x + r.w).min(g.w), (r.y + r.h).min(g.h));
    if x1 <= x0 || y1 <= y0 { return Vec::new(); }
    let (rw, rh) = (x1 - x0, y1 - y0);
    let mut seen = vec![false; rw * rh];
    let mut out = Vec::new();
    for sy in 0..rh {
        for sx in 0..rw {
            if seen[sy * rw + sx] || !is_ink(x0 + sx, y0 + sy) { continue; }
            // flood fill this component, tracking its bounds
            let (mut lo_x, mut lo_y, mut hi_x, mut hi_y) = (sx, sy, sx, sy);
            let mut stack = vec![(sx, sy)];
            seen[sy * rw + sx] = true;
            let mut n = 0usize;
            while let Some((cx, cy)) = stack.pop() {
                n += 1;
                lo_x = lo_x.min(cx); hi_x = hi_x.max(cx);
                lo_y = lo_y.min(cy); hi_y = hi_y.max(cy);
                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1),
                                 (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                    let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= rw as i32 || ny >= rh as i32 { continue; }
                    let (nx, ny) = (nx as usize, ny as usize);
                    if seen[ny * rw + nx] || !is_ink(x0 + nx, y0 + ny) { continue; }
                    seen[ny * rw + nx] = true;
                    stack.push((nx, ny));
                }
            }
            let (bw, bh) = (hi_x - lo_x + 1, hi_y - lo_y + 1);
            // A GLYPH IS SHAPED LIKE A GLYPH. Everything else on a board
            // -- screws, tape, glare, a thumb -- is also a connected
            // component, and this is the only thing separating them.
            let tall_enough = bh * 8 >= rh;
            let not_a_speck = n >= 6 && bw >= 2 && bh >= 4;
            let not_a_smear = bw <= rw / 2 && bh <= rh;
            let plausible_aspect = bw * 4 >= bh && bh >= bw;
            if tall_enough && not_a_speck && not_a_smear && plausible_aspect {
                out.push(Rect { x: x0 + lo_x, y: y0 + lo_y, w: bw, h: bh });
            }
        }
    }
    out.sort_by_key(|b| (b.y / (rh / 4).max(1), b.x));
    out
}

/// The size every glyph is normalised to before comparison.
pub const CELL: usize = 8;

/// A glyph as a small binary grid, which is what gets compared.
pub type Cell = [bool; CELL * CELL];

/// Scale a glyph box into the comparison grid.
pub fn normalise(g: &Gray, b: Rect, ink: Ink, t: u8) -> Cell {
    let mut c = [false; CELL * CELL];
    for cy in 0..CELL {
        for cx in 0..CELL {
            // sample the source box and light the cell on a 25% vote
            // (`lit * 4 >= n`), not the 1/3 majority (`lit * 3 >= n`)
            // this used to be.
            //
            // Measured, not guessed: '0' and '8' differ ONLY in the
            // middle bar (seven-segment slot 3), that bar renders
            // exactly on the boundary between two CELL rows at this
            // glyph size, and splits 3 of 12 sampled pixels into EACH of
            // the two rows -- 25% exactly, under the old 33% threshold
            // in BOTH rows. The middle bar vanished from every digit
            // that has one, so '0' and '8' normalised to identical
            // grids -- distance 0, not merely low-confidence. Confirmed
            // with a debug harness that printed both grids and found
            // d(8,0) = 0.
            //
            // `lit > 0` (any ink at all) was tried first and reverted:
            // it fixed this pair but fattened every stroke enough that
            // OTHER templates moved closer together too, dropping the
            // exact-self-match confidence from >0.3 to 0.25. 25% is the
            // narrowest threshold that still captures the exact 3-of-12
            // split measured above, without also lighting a cell on a
            // single stray noise pixel the way `lit > 0` would.
            //
            // A vote still makes sense for a real photographed board,
            // where JPEG noise and uneven lighting can light a stray
            // pixel in an otherwise-dark cell -- 25% keeps that
            // rejection, just not so strict that a stroke exactly this
            // thin can fall entirely between two votes.
            let sx0 = b.x + cx * b.w / CELL;
            let sx1 = (b.x + (cx + 1) * b.w / CELL).max(sx0 + 1);
            let sy0 = b.y + cy * b.h / CELL;
            let sy1 = (b.y + (cy + 1) * b.h / CELL).max(sy0 + 1);
            let mut lit = 0;
            let mut n = 0;
            for y in sy0..sy1 { for x in sx0..sx1 {
                n += 1;
                let v = g.at(x, y);
                if match ink { Ink::DarkOnLight => v < t, Ink::LightOnDark => v >= t } { lit += 1; }
            }}
            // ANY-INK (`lit > 0`) was tried first and reverted: it fixed
            // '0' vs '8' but fattened every stroke enough that OTHER
            // templates moved closer together too, and the confidence
            // margin for an exact self-match dropped from >0.3 to 0.25 --
            // trading one bug for a smaller, harder-to-notice one.
            //
            // `lit * 4 >= n` (25%) is the narrowest fix that still passes:
            // measured directly, the split middle-bar cells sample 3 of 12
            // pixels (25% exactly) on each side of the boundary, so this
            // is the lowest threshold that captures them without also
            // capturing a single stray noise pixel the way `lit > 0` does.
            c[cy * CELL + cx] = n > 0 && lit * 4 >= n;
        }
    }
    c
}

fn distance(a: &Cell, b: &Cell) -> usize {
    a.iter().zip(b.iter()).filter(|(x, y)| x != y).count()
}

/// What a glyph was read as, and how sure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Read {
    pub ch: char,
    /// 0 to 1. The margin between the best match and the runner-up,
    /// not the raw distance: a glyph equally close to 8 and B is
    /// unread however close it is to both.
    pub sure: f32,
}

/// Recognise one glyph against a template set.
pub fn recognise(c: &Cell, templates: &[(char, Cell)]) -> Option<Read> {
    if templates.is_empty() { return None; }
    let mut scored: Vec<(usize, char)> = templates.iter()
        .map(|(ch, t)| (distance(c, t), *ch)).collect();
    scored.sort();
    let (best, ch) = scored[0];
    // THE MARGIN, NOT THE DISTANCE. A glyph two pixels from `8` and
    // three from `B` is not an 8 -- it is unreadable, and reporting it
    // as an 8 with high confidence because the distance was small is
    // how a take gets filed under the wrong number.
    let runner = scored.iter().find(|(_, c2)| *c2 != ch).map(|(d, _)| *d).unwrap_or(CELL * CELL);
    // RELATIVE margin, normalised by the runner-up rather than by a fixed
    // fraction of the cell.
    //
    // This was `(runner - best) / (CELL * CELL / 4)`, which assumes the
    // nearest wrong glyph differs by a quarter of every pixel in the cell.
    // Real digit templates are nothing like that far apart, so a PERFECT
    // match scored 0.25 -- under MIN_SURE (0.55) -- and `read_board` rejected
    // every character it recognised correctly. That one miscalibration is why
    // the reader returned an entirely empty Board rather than a wrong one,
    // which is the more confusing failure: nothing to look at and nothing to
    // blame.
    //
    // Dividing by the runner-up is self-normalising and says the intended
    // thing directly: how much closer is the winner than its nearest rival,
    // as a fraction. An exact match against any rival is 1.0. The module's
    // own example -- two pixels from `8`, three from `B` -- gives 1/3, which
    // is correctly below MIN_SURE and unread.
    let margin = if runner == 0 {
        // Two templates at distance zero: the glyph matches both exactly, so
        // there is no evidence for either. Not confident, however close.
        0.0
    } else {
        runner.saturating_sub(best) as f32 / runner as f32
    };
    let closeness = 1.0 - (best as f32 / (CELL * CELL) as f32);
    Some(Read { ch, sure: (margin.min(1.0) * closeness).clamp(0.0, 1.0) })
}

/// Build the templates by rendering the glyphs this reader knows.
///
/// Generated rather than hand-drawn so the template and the test
/// renderer cannot drift apart -- if they did, the tests would pass
/// against a reader that fails on any real frame.
pub fn templates() -> Vec<(char, Cell)> {
    "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars().map(|ch| {
        let mut g = Gray::new(CELL * 3, CELL * 4);
        for p in g.px.iter_mut() { *p = 255; }
        draw_glyph(&mut g, ch, 0, 0, CELL * 3, CELL * 4);
        let cell = normalise(&g, Rect { x: 0, y: 0, w: CELL * 3, h: CELL * 4 },
                             Ink::DarkOnLight, 128);
        (ch, cell)
    }).collect()
}

/// A seven-segment-ish rendering, enough to tell the vocabulary apart.
///
/// This is not a font. It is the SHAPE of each character at the size a
/// board is read at, which is all the matcher needs -- and it doubles
/// as the way tests draw a synthetic board.
pub fn draw_glyph(g: &mut Gray, ch: char, x: usize, y: usize, w: usize, h: usize) {
    let seg = |g: &mut Gray, x0: f32, y0: f32, x1: f32, y1: f32| {
        let steps = ((w + h) * 2).max(8);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let px = x as f32 + (x0 + (x1 - x0) * t) * (w - 1) as f32;
            let py = y as f32 + (y0 + (y1 - y0) * t) * (h - 1) as f32;
            // a stroke with width, or thin diagonals vanish when scaled
            for oy in 0..2 { for ox in 0..2 {
                g.set(px as usize + ox, py as usize + oy, 0);
            }}
        }
    };
    // top, top-left, top-right, middle, bottom-left, bottom-right, bottom
    let s = |n: usize| -> (f32, f32, f32, f32) {
        match n {
            0 => (0.1, 0.02, 0.9, 0.02), 1 => (0.1, 0.02, 0.1, 0.5),
            2 => (0.9, 0.02, 0.9, 0.5),  3 => (0.1, 0.5, 0.9, 0.5),
            4 => (0.1, 0.5, 0.1, 0.98),  5 => (0.9, 0.5, 0.9, 0.98),
            _ => (0.1, 0.98, 0.9, 0.98),
        }
    };
    let on: &[usize] = match ch {
        '0' => &[0, 1, 2, 4, 5, 6], '1' => &[2, 5], '2' => &[0, 2, 3, 4, 6],
        '3' => &[0, 2, 3, 5, 6], '4' => &[1, 2, 3, 5], '5' => &[0, 1, 3, 5, 6],
        '6' => &[0, 1, 3, 4, 5, 6], '7' => &[0, 2, 5], '8' => &[0, 1, 2, 3, 4, 5, 6],
        '9' => &[0, 1, 2, 3, 5, 6],
        'A' => &[0, 1, 2, 3, 4, 5], 'B' => &[1, 3, 4, 5, 6], 'C' => &[0, 1, 4, 6],
        'D' => &[2, 3, 4, 5, 6], 'E' => &[0, 1, 3, 4, 6], 'F' => &[0, 1, 3, 4],
        'G' => &[0, 1, 3, 4, 5, 6], 'H' => &[1, 2, 3, 4, 5], 'I' => &[2, 5],
        'J' => &[2, 4, 5, 6], 'K' => &[1, 3, 4, 2], 'L' => &[1, 4, 6],
        'M' => &[0, 1, 2, 4, 5], 'N' => &[1, 2, 4, 5], 'O' => &[0, 1, 2, 4, 5, 6],
        'P' => &[0, 1, 2, 3, 4], 'Q' => &[0, 1, 2, 3, 5], 'R' => &[0, 1, 4],
        'S' => &[0, 1, 3, 5, 6], 'T' => &[0, 1, 4], 'U' => &[1, 2, 4, 5, 6],
        'V' => &[1, 4, 6, 5], 'W' => &[1, 2, 4, 5, 6], 'X' => &[1, 2, 3, 4, 5],
        'Y' => &[1, 2, 3, 5], 'Z' => &[0, 2, 3, 4, 6],
        _ => &[],
    };
    for n in on { let (a, b, c, d) = s(*n); seg(g, a, b, c, d); }
}

// ── the board ────────────────────────────────────────────────────────

/// What was written on the slate.
///
/// Every field is optional and that is the point: a field the reader
/// was not sure of is EMPTY, not guessed. An empty scene asks the
/// editor a question; a wrong one sends them looking for a take that
/// was never mis-shot.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Board {
    pub roll: Option<String>,
    pub scene: Option<String>,
    /// The letter after the scene: 14B. Kept apart from the scene
    /// because sorting "14B" as text puts it after "140".
    pub shot: Option<String>,
    pub take: Option<u32>,
    /// Anything read but not placed in a field.
    pub other: Vec<String>,
    /// How sure the reader is overall, 0 to 1.
    pub sure: f32,
}

impl Board {
    /// The key clips sort by: scene, then shot, then take. Numeric
    /// where numeric, so 2 comes before 10.
    pub fn order_key(&self) -> (u32, String, u32) {
        let scene_n = self.scene.as_deref().and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);
        (scene_n, self.shot.clone().unwrap_or_default(), self.take.unwrap_or(0))
    }

    /// A name an editor would recognise on a bin.
    pub fn label(&self) -> String {
        match (&self.scene, &self.shot, self.take) {
            (Some(s), Some(sh), Some(t)) => format!("{s}{sh}-{t}"),
            (Some(s), None, Some(t)) => format!("{s}-{t}"),
            (Some(s), Some(sh), None) => format!("{s}{sh}"),
            (Some(s), None, None) => s.clone(),
            _ => "unread".into(),
        }
    }

    pub fn complete(&self) -> bool {
        self.scene.is_some() && self.take.is_some()
    }
}

/// How sure a field must be before it is filled in at all.
///
/// Deliberately high. The cost of an empty field is that someone types
/// three characters; the cost of a wrong one is a take filed under
/// another scene and found at the end of the edit.
pub const MIN_SURE: f32 = 0.55;

/// Read a board out of a frame region.
pub fn read_board(g: &Gray, region: Rect, templates: &[(char, Cell)]) -> Board {
    let t = threshold(g, region);
    // WHICH SIDE IS INK is not knowable in advance: black marker on
    // white acrylic and white chalk on slate are both normal. Try both
    // and keep whichever yields more glyph-shaped things.
    let (ink, boxes) = [Ink::DarkOnLight, Ink::LightOnDark].iter()
        .map(|i| (*i, glyph_boxes(g, region, *i, t)))
        .max_by_key(|(_, b)| b.len())
        .unwrap_or((Ink::DarkOnLight, Vec::new()));
    if boxes.is_empty() { return Board::default(); }
    // group into rows: a board's fields are on separate lines
    let mut rows: Vec<Vec<Rect>> = Vec::new();
    for b in boxes {
        match rows.iter_mut().find(|r| r[0].overlaps_rows(&b)) {
            Some(r) => r.push(b),
            None => rows.push(vec![b]),
        }
    }
    for r in rows.iter_mut() { r.sort_by_key(|b| b.x); }
    rows.sort_by_key(|r| r[0].y);

    let mut words: Vec<(String, f32)> = Vec::new();
    for row in &rows {
        // split a row into words on the gaps: a gap wider than a glyph
        // is a space between fields, not between characters
        let median_w = {
            let mut ws: Vec<usize> = row.iter().map(|b| b.w).collect();
            ws.sort_unstable();
            ws[ws.len() / 2].max(1)
        };
        let mut word = String::new();
        let mut sures: Vec<f32> = Vec::new();
        let mut prev_end: Option<usize> = None;
        for b in row {
            if let Some(pe) = prev_end {
                if b.x.saturating_sub(pe) > median_w {
                    if !word.is_empty() {
                        let s = sures.iter().sum::<f32>() / sures.len().max(1) as f32;
                        words.push((std::mem::take(&mut word), s));
                        sures.clear();
                    }
                }
            }
            let cell = normalise(g, *b, ink, t);
            match recognise(&cell, templates) {
                Some(r) if r.sure >= MIN_SURE => { word.push(r.ch); sures.push(r.sure); }
                // AN UNSURE GLYPH BREAKS THE WORD rather than being
                // dropped: "1_4" read as "14" is a different scene.
                _ => {
                    if !word.is_empty() {
                        let s = sures.iter().sum::<f32>() / sures.len().max(1) as f32;
                        words.push((std::mem::take(&mut word), s));
                        sures.clear();
                    }
                }
            }
            prev_end = Some(b.x + b.w);
        }
        if !word.is_empty() {
            let s = sures.iter().sum::<f32>() / sures.len().max(1) as f32;
            words.push((word, s));
        }
    }
    assemble(&words)
}

/// Turn read words into fields.
///
/// A board's layout varies by production, so this reads by SHAPE
/// rather than by position: a bare number that follows nothing is the
/// scene, a number with a trailing letter is scene and shot, and the
/// take is the small number that comes after.
pub fn assemble(words: &[(String, f32)]) -> Board {
    let mut b = Board::default();
    let mut sures = Vec::new();
    for (w, s) in words {
        if w.is_empty() { continue; }
        let digits: String = w.chars().take_while(|c| c.is_ascii_digit()).collect();
        let rest: String = w.chars().skip(digits.len()).collect();
        if !digits.is_empty() && b.scene.is_none() && digits.len() <= 4 {
            b.scene = Some(digits.clone());
            if !rest.is_empty() && rest.len() <= 2 { b.shot = Some(rest); }
            sures.push(*s);
        } else if !digits.is_empty() && b.take.is_none() && digits.len() <= 3 {
            b.take = digits.parse().ok();
            sures.push(*s);
        } else if b.roll.is_none() && w.len() <= 6 && w.chars().any(|c| c.is_ascii_alphabetic()) {
            b.roll = Some(w.clone());
            sures.push(*s);
        } else {
            b.other.push(w.clone());
        }
    }
    b.sure = if sures.is_empty() { 0.0 } else { sures.iter().sum::<f32>() / sures.len() as f32 };
    b
}

// ── arranging the clips ──────────────────────────────────────────────

/// One imported file and what its board said.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub file: String,
    pub board: Board,
}

/// Put the clips in shooting order.
///
/// UNREAD CLIPS GO LAST, IN THEIR ORIGINAL ORDER, and are not
/// interleaved with the read ones. A clip whose board could not be read
/// has no place in the sequence, and inventing one buries it among
/// forty others; at the end, in camera order, an editor can find them
/// all in one place and deal with them together.
pub fn arrange(clips: Vec<Clip>) -> (Vec<Clip>, Vec<Clip>) {
    let (mut read, unread): (Vec<Clip>, Vec<Clip>) =
        clips.into_iter().partition(|c| c.board.complete());
    read.sort_by(|a, b| a.board.order_key().cmp(&b.board.order_key()));
    (read, unread)
}

/// Takes of the same setup, which is what an editor actually wants to
/// see together.
pub fn by_setup(clips: &[Clip]) -> BTreeMap<String, Vec<&Clip>> {
    let mut m: BTreeMap<String, Vec<&Clip>> = BTreeMap::new();
    for c in clips.iter().filter(|c| c.board.complete()) {
        let key = format!("{}{}", c.board.scene.clone().unwrap_or_default(),
                          c.board.shot.clone().unwrap_or_default());
        m.entry(key).or_default().push(c);
    }
    for v in m.values_mut() { v.sort_by_key(|c| c.board.take.unwrap_or(0)); }
    m
}

/// A line per clip, for the import dialogue.
pub fn summary(read: &[Clip], unread: &[Clip]) -> String {
    let mut s = format!("{} of {} clip(s) read\n", read.len(), read.len() + unread.len());
    for c in read {
        s.push_str(&format!("  {:<14} {:<8} ({:.0}% sure)\n",
                            c.file, c.board.label(), c.board.sure * 100.0));
    }
    if !unread.is_empty() {
        s.push_str(&format!("  {} unread, left in camera order: {}\n", unread.len(),
                            unread.iter().map(|c| c.file.as_str())
                                .collect::<Vec<_>>().join(", ")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draw a board the way a camera would see one.
    fn board_frame(lines: &[&str], ink: Ink) -> Gray {
        let (gw, gh) = (30usize, 44usize);
        let (w, h) = (40 + lines.iter().map(|l| l.len()).max().unwrap_or(1) * (gw + 10), 260);
        let mut g = Gray::new(w, h);
        let (bg, _fg) = match ink { Ink::DarkOnLight => (255u8, 0u8), Ink::LightOnDark => (20, 255) };
        for p in g.px.iter_mut() { *p = bg; }
        for (li, line) in lines.iter().enumerate() {
            let mut x = 20;
            for ch in line.chars() {
                if ch == ' ' { x += gw + 34; continue; }
                if ink == Ink::DarkOnLight {
                    draw_glyph(&mut g, ch, x, 20 + li * 70, gw, gh);
                } else {
                    // white on dark: draw into a scratch and invert
                    let mut s = Gray::new(gw + 4, gh + 4);
                    for p in s.px.iter_mut() { *p = 255; }
                    draw_glyph(&mut s, ch, 0, 0, gw, gh);
                    for yy in 0..gh { for xx in 0..gw {
                        if s.at(xx, yy) < 128 { g.set(x + xx, 20 + li * 70 + yy, 255); }
                    }}
                }
                x += gw + 10;
            }
        }
        g
    }

    fn whole(g: &Gray) -> Rect { Rect { x: 0, y: 0, w: g.w, h: g.h } }

    #[test]
    fn it_reads_a_board_off_a_frame() {
        let g = board_frame(&["14B", "3"], Ink::DarkOnLight);
        let b = read_board(&g, whole(&g), &templates());
        assert_eq!(b.scene.as_deref(), Some("14"), "{b:?}");
        assert_eq!(b.shot.as_deref(), Some("B"), "{b:?}");
        assert_eq!(b.take, Some(3), "{b:?}");
        assert!(b.complete());
        assert_eq!(b.label(), "14B-3");
        assert!(b.sure > 0.5, "sure was {}", b.sure);
    }

    #[test]
    fn it_does_not_care_which_side_is_ink() {
        // black marker on white acrylic and white chalk on slate are
        // both normal, and the reader is not told which it has
        let dark = read_board(&board_frame(&["7A", "2"], Ink::DarkOnLight), 
                              whole(&board_frame(&["7A", "2"], Ink::DarkOnLight)), &templates());
        let light_g = board_frame(&["7A", "2"], Ink::LightOnDark);
        let light = read_board(&light_g, whole(&light_g), &templates());
        assert_eq!(dark.scene.as_deref(), Some("7"), "{dark:?}");
        assert_eq!(light.scene.as_deref(), Some("7"), "{light:?}");
        assert_eq!(light.take, Some(2), "{light:?}");
    }

    #[test]
    fn an_unsure_glyph_leaves_the_field_empty_rather_than_guessing() {
        // A misread take silently files a good take under the wrong
        // name and the editor finds out at the end. An empty field asks
        // a question; a wrong one answers it.
        let noise = {
            let mut g = Gray::new(200, 120);
            for (i, p) in g.px.iter_mut().enumerate() {
                *p = if (i * 7919 % 251) > 125 { 255 } else { 0 };
            }
            g
        };
        let b = read_board(&noise, whole(&noise), &templates());
        assert!(!b.complete(), "noise is not a board: {b:?}");
        assert_eq!(b.label(), "unread");
        // and the margin rule: a glyph equally close to two templates
        // is unread however close it is to both
        let ambiguous = [true; CELL * CELL];
        let r = recognise(&ambiguous, &templates()).unwrap();
        assert!(r.sure < MIN_SURE, "all-ink is not a character: {r:?}");
    }

    #[test]
    fn zero_and_eight_are_distinguishable() {
        // The exact failure this session found. '0' and '8' differ ONLY
        // in the seven-segment middle bar, and that bar renders exactly
        // on the boundary between two CELL rows at this glyph size --
        // splitting a thin stroke across two sample rows so a majority
        // vote (the old `lit * 3 >= n`) sees a minority in BOTH and drops
        // the bar from both templates. '0' and '8' then normalised to
        // the same grid: distance 0, and `recognise` could not tell them
        // apart at all -- not "less confident", indistinguishable.
        //
        // Pinned directly on the templates, not through `recognise`,
        // because that is where the bug actually was: the two grids must
        // differ before anything downstream can be asked to score them.
        let t = templates();
        let zero = t.iter().find(|(c, _)| *c == '0').unwrap().1;
        let eight = t.iter().find(|(c, _)| *c == '8').unwrap().1;
        assert_ne!(zero, eight, "'0' and '8' normalised to the same cell grid");
        // And recognise() must still call 8 as 8 against its own template.
        let r = recognise(&eight, &t).expect("read");
        assert_eq!(r.ch, '8');
    }

    #[test]
    fn confidence_is_the_margin_not_the_distance() {
        let t = templates();
        // an exact template scores high
        let (_, eight) = t.iter().find(|(c, _)| *c == '8').unwrap();
        let r = recognise(eight, &t).expect("read");
        assert_eq!(r.ch, '8');
        assert!(r.sure > 0.3, "an exact match is confident: {r:?}");
        // every template reads back as itself, or the recogniser cannot
        // tell its own vocabulary apart
        let mut wrong = Vec::new();
        for (ch, cell) in &t {
            let got = recognise(cell, &t).unwrap();
            if got.ch != *ch { wrong.push((*ch, got.ch)); }
        }
        assert!(wrong.is_empty(), "templates collide: {wrong:?}");
    }

    #[test]
    fn clips_arrange_by_scene_then_shot_then_take() {
        let c = |file: &str, scene: &str, shot: &str, take: u32| Clip {
            file: file.into(),
            board: Board { scene: Some(scene.into()), shot: Some(shot.into()),
                           take: Some(take), sure: 0.9, ..Default::default() },
        };
        let clips = vec![
            c("C0070.MP4", "10", "A", 2), c("C0031.MP4", "2", "B", 1),
            c("C0055.MP4", "2", "A", 3), c("C0044.MP4", "2", "A", 1),
        ];
        let (read, unread) = arrange(clips);
        assert!(unread.is_empty());
        let order: Vec<&str> = read.iter().map(|c| c.file.as_str()).collect();
        // scene 2 before scene 10 -- NUMERIC, or "10" sorts before "2"
        assert_eq!(order, vec!["C0044.MP4", "C0055.MP4", "C0031.MP4", "C0070.MP4"],
                   "{:?}", read.iter().map(|c| c.board.label()).collect::<Vec<_>>());
    }

    #[test]
    fn unread_clips_go_last_together_and_keep_camera_order() {
        // a clip whose board could not be read has no place in the
        // sequence, and inventing one buries it among forty others
        let read_one = Clip { file: "A.MP4".into(),
            board: Board { scene: Some("5".into()), take: Some(1), sure: 0.9, ..Default::default() } };
        let blank = |f: &str| Clip { file: f.into(), board: Board::default() };
        let (read, unread) = arrange(vec![blank("X.MP4"), read_one.clone(), blank("Y.MP4")]);
        assert_eq!(read.len(), 1);
        assert_eq!(unread.iter().map(|c| c.file.as_str()).collect::<Vec<_>>(),
                   vec!["X.MP4", "Y.MP4"], "camera order kept");
        let s = summary(&read, &unread);
        assert!(s.contains("1 of 3 clip(s) read"), "{s}");
        assert!(s.contains("2 unread, left in camera order"), "{s}");
    }

    #[test]
    fn takes_of_one_setup_come_back_together_in_order() {
        let c = |f: &str, scene: &str, shot: &str, take: u32| Clip {
            file: f.into(),
            board: Board { scene: Some(scene.into()), shot: Some(shot.into()),
                           take: Some(take), sure: 0.9, ..Default::default() } };
        let clips = vec![c("a", "3", "A", 3), c("b", "3", "A", 1),
                         c("c", "4", "B", 1), c("d", "3", "A", 2)];
        let m = by_setup(&clips);
        assert_eq!(m.len(), 2);
        assert_eq!(m["3A"].iter().map(|c| c.board.take.unwrap()).collect::<Vec<_>>(),
                   vec![1, 2, 3], "takes in order");
        assert_eq!(m["4B"].len(), 1);
    }

    #[test]
    fn a_scene_number_sorts_as_a_number_not_as_text() {
        let k = |s: &str| Board { scene: Some(s.into()), take: Some(1), ..Default::default() }
            .order_key();
        assert!(k("2") < k("10"), "2 before 10");
        assert!(k("9") < k("100"));
        // an unread scene sorts last rather than first
        let unread = Board { take: Some(1), ..Default::default() }.order_key();
        assert!(k("999") < unread);
    }
}
