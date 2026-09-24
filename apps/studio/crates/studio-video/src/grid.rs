//! grid — where each angle goes in a multiview, without cutting pixels by accident.
//!
//! A multiview is several angles on one canvas. The canvas is the problem.
//! Three sizes are in play and conflating them is how a multiview quietly
//! loses picture:
//!
//! * **the source**, e.g. 3840×2160 per angle;
//! * **the monitor canvas**, what the grid is drawn on — a preview pane, a
//!   window, a second screen. This is where a multiview belongs: it is a way
//!   of LOOKING, so it should be drawn at the size you are looking at, not at
//!   the delivery size;
//! * **the delivery canvas**, 3840×2160, which a multiview must never be
//!   confused with: a 2×2 grid of 4K angles on a 4K canvas gives each angle
//!   1920×1080 of real pixels, and a cut taken from it is a quarter-resolution
//!   cut. [`Use::Deliverable`] exists to say out loud which one is meant.
//!
//! What this module returns is [`Tile`]s: integer rectangles that tile the
//! canvas exactly, plus the scale each angle is shown at. Nothing is rounded
//! away silently:
//!
//! * **Leftover pixels are distributed, not dropped.** 1920 across 7 columns
//!   is 274.28…; the tiles are 275, 275, 274, 274, 274, 274, 274 and they sum
//!   to 1920. Flooring every tile would leave a 2 px seam down the right edge.
//! * **Aspect is kept.** A 16:9 angle in a squarer cell is letterboxed inside
//!   its tile ([`Fit::Letterbox`], the default) rather than stretched; it can
//!   also fill the cell and lose the edges ([`Fit::Crop`]) — which is a real
//!   choice on this film's wide compositions, so it is a parameter, not a
//!   default.
//! * **Chroma alignment is optional and explicit.** An encoded multiview in
//!   4:2:0 wants tiles on even pixel boundaries ([`Align::Even`]); an on-screen
//!   monitor does not care ([`Align::Pixel`]).
//! * **Too small to read is reported, not rendered.** Nine angles in a 480 px
//!   pane gives 160 px wide tiles; [`Layout::too_small`] says so, so the caller
//!   can page the grid instead of drawing nine unreadable thumbnails.
//!
//! Pure integer geometry: no frames, no ffmpeg. The preview draws it by
//! handing each angle's frame to `studio_fx::compositor` as a layer with the
//! matching transform; an encoded multiview would be the same rectangles as
//! ffmpeg `xstack` inputs.

/// What the grid is for — which decides what the canvas may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    /// On-screen monitoring. Draw at the size of the pane.
    Monitor,
    /// A rendered multiview file. Each angle only gets its tile's pixels, so
    /// the canvas should be chosen for what the tiles need, not the other way
    /// round: see [`canvas_for`].
    Deliverable,
}

/// How an angle sits in its cell when the aspects differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Whole picture, bars where it does not fill.
    Letterbox,
    /// Fill the cell, lose what hangs over the edges.
    Crop,
}

/// Where tile edges may fall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Any pixel.
    Pixel,
    /// Even pixels only — for 4:2:0 encoding, where an odd edge shifts chroma.
    Even,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub fit: Fit,
    pub align: Align,
    /// Below this tile width, a tile is reported in [`Layout::too_small`].
    pub min_tile_w: u32,
    /// Gap between tiles, in canvas pixels (0 is the usual).
    pub gutter: u32,
}

impl Default for Options {
    fn default() -> Options {
        // 192 px: about the width at which a face in a wide shot is still
        // identifiable on a laptop screen. It is a default, not a law.
        Options { fit: Fit::Letterbox, align: Align::Pixel, min_tile_w: 192, gutter: 0 }
    }
}

/// One angle's rectangle on the canvas, and the picture inside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// The angle's picture within the tile (letterboxed or cropped): its
    /// on-canvas rectangle, clipped to the tile for [`Fit::Crop`].
    pub picture: (u32, u32, u32, u32),
    /// Source pixels per canvas pixel. 4.0 means the angle is shown at a
    /// quarter of its width — how much detail the viewer is NOT seeing.
    pub source_scale: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub cols: u32,
    pub rows: u32,
    pub tiles: Vec<Tile>,
    /// Indices of tiles narrower than `min_tile_w`.
    pub too_small: Vec<usize>,
}

/// Split `total` into `n` integer parts that sum to exactly `total`, the
/// remainder spread one pixel at a time over the first parts (and kept even
/// when asked). Returns the parts and their starts.
fn split(total: u32, n: u32, align: Align) -> (Vec<u32>, Vec<u32>) {
    let n = n.max(1);
    let mut parts = Vec::with_capacity(n as usize);
    let mut starts = Vec::with_capacity(n as usize);
    match align {
        Align::Pixel => {
            let base = total / n;
            let extra = total % n;
            let mut at = 0;
            for i in 0..n {
                let w = base + if i < extra { 1 } else { 0 };
                starts.push(at);
                parts.push(w);
                at += w;
            }
        }
        Align::Even => {
            // Every start and every size even: hand out even chunks, then
            // give the (even) remainder to the last tile, so no edge lands
            // on an odd pixel and the parts still sum to `total`.
            let units = total / 2;
            let base = (units / n) * 2;
            let mut at = 0;
            for i in 0..n {
                let w = if i + 1 == n { total - at } else { base };
                starts.push(at);
                parts.push(w);
                at += w;
            }
        }
    }
    (parts, starts)
}

/// Rows and columns for `n` angles on a `w`×`h` canvas: the arrangement whose
/// cells waste the least, given the angles' aspect.
pub fn shape(n: u32, w: u32, h: u32, src_aspect: f32) -> (u32, u32) {
    if n <= 1 { return (1, 1); }
    let (mut best, mut cols) = (f32::MAX, 1);
    for c in 1..=n {
        let r = n.div_ceil(c);
        let (cw, ch) = (w as f32 / c as f32, h as f32 / r as f32);
        if cw <= 0.0 || ch <= 0.0 { continue; }
        // used area if each cell letterboxes the source aspect, minus the
        // cells that would be empty (a 3-up in a 2×2 wastes a quarter)
        let used = (cw * ch).min(cw * cw / src_aspect).min(ch * src_aspect * ch);
        let waste = (c * r - n) as f32 * cw * ch;
        let score = -(used * n as f32 - waste);
        if score < best { best = score; cols = c; }
    }
    (cols, n.div_ceil(cols))
}

/// Lay `n` angles of `src_w`×`src_h` out on a `w`×`h` canvas.
pub fn layout(n: u32, w: u32, h: u32, src_w: u32, src_h: u32, opts: Options) -> Layout {
    if n == 0 || w == 0 || h == 0 {
        return Layout { cols: 0, rows: 0, tiles: Vec::new(), too_small: Vec::new() };
    }
    let aspect = src_w.max(1) as f32 / src_h.max(1) as f32;
    let (cols, rows) = shape(n, w, h, aspect);
    let g = opts.gutter;
    let inner_w = w.saturating_sub(g * (cols.saturating_sub(1)));
    let inner_h = h.saturating_sub(g * (rows.saturating_sub(1)));
    let (cw, cx) = split(inner_w, cols, opts.align);
    let (ch, cy) = split(inner_h, rows, opts.align);
    let mut tiles = Vec::with_capacity(n as usize);
    let mut too_small = Vec::new();
    for i in 0..n as usize {
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        let (tw, th) = (cw[c as usize], ch[r as usize]);
        let (x, y) = (cx[c as usize] + c * g, cy[r as usize] + r * g);
        // Letterbox: the largest source-aspect rectangle inside the tile.
        // Crop: the smallest that covers it, clipped to the tile.
        let scale = match opts.fit {
            Fit::Letterbox => (tw as f32 / src_w as f32).min(th as f32 / src_h as f32),
            Fit::Crop => (tw as f32 / src_w as f32).max(th as f32 / src_h as f32),
        };
        let (pw, ph) = ((src_w as f32 * scale).round() as u32, (src_h as f32 * scale).round() as u32);
        let picture = match opts.fit {
            Fit::Letterbox => (x + (tw.saturating_sub(pw)) / 2, y + (th.saturating_sub(ph)) / 2, pw.min(tw), ph.min(th)),
            Fit::Crop => (x, y, tw, th),
        };
        if tw < opts.min_tile_w { too_small.push(i); }
        tiles.push(Tile { x, y, w: tw, h: th, picture, source_scale: if scale > 0.0 { 1.0 / scale } else { f32::INFINITY } });
    }
    Layout { cols, rows, tiles, too_small }
}

/// The canvas a DELIVERABLE multiview should use so each of `n` angles keeps
/// `keep` of its own pixels (1.0 = full resolution, 0.5 = half).
///
/// This is the answer to "a 2×2 of 4K angles on a 4K canvas is four 1080p
/// angles": if you want four 4K angles you need a 7680×4320 canvas, and if
/// you want a 4K canvas you are choosing 1080p angles. Saying which is the
/// point — the trade is fine, it just must not be a surprise.
pub fn canvas_for(n: u32, src_w: u32, src_h: u32, keep: f32, align: Align) -> (u32, u32) {
    // Squarest arrangement: a deliverable has no pane to fit, so the canvas
    // follows the angles rather than the other way round.
    let cols = (n.max(1) as f32).sqrt().ceil() as u32;
    let rows = n.max(1).div_ceil(cols);
    let (mut w, mut h) = (((src_w as f32 * keep).round() as u32 * cols).max(1),
                          ((src_h as f32 * keep).round() as u32 * rows).max(1));
    if align == Align::Even {
        w += w % 2;
        h += h % 2;
    }
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covers(l: &Layout, w: u32, h: u32, gutter: u32) {
        // every canvas pixel belongs to exactly one tile (no seam, no overlap)
        let mut painted = vec![0u8; (w * h) as usize];
        for t in &l.tiles {
            for y in t.y..t.y + t.h {
                for x in t.x..t.x + t.w {
                    let i = (y * w + x) as usize;
                    assert_eq!(painted[i], 0, "tiles overlap at ({x}, {y})");
                    painted[i] = 1;
                }
            }
        }
        if gutter == 0 && l.tiles.len() as u32 == l.cols * l.rows {
            assert!(painted.iter().all(|&p| p == 1), "the tiles must cover the canvas with no seam");
        }
    }

    #[test]
    fn leftover_pixels_are_spread_not_dropped() {
        // 1920 / 7 = 274.28…: flooring every part leaves a 2 px seam.
        // Asserted on `split` itself: going through `layout` hid this,
        // because `shape` picks 4 columns for 7 angles and 1920/4 is exact.
        for (total, n) in [(1920u32, 7u32), (1080, 7), (1000, 3), (7, 4), (5, 5), (3, 8)] {
            let (parts, starts) = split(total, n, Align::Pixel);
            assert_eq!(parts.iter().sum::<u32>(), total, "{total}/{n}: {parts:?}");
            assert!(parts.iter().max().unwrap() - parts.iter().min().unwrap() <= 1, "{parts:?}");
            for i in 0..n as usize {
                assert_eq!(starts[i], parts[..i].iter().sum::<u32>(), "{starts:?} vs {parts:?}");
            }
        }
        let l = layout(7, 1920, 1080, 3840, 2160, Options { min_tile_w: 0, ..Default::default() });
        let row: u32 = l.tiles.iter().take(l.cols as usize).map(|t| t.w).sum();
        assert_eq!(row, 1920, "a row of tiles spans the canvas");
        covers(&l, 1920, 1080, 0);
    }

    #[test]
    fn every_edge_is_even_when_the_encoder_needs_it() {
        for n in [2u32, 3, 5, 7, 9] {
            let l = layout(n, 1918, 1080, 3840, 2160, Options { align: Align::Even, min_tile_w: 0, ..Default::default() });
            for t in &l.tiles {
                assert_eq!((t.x % 2, t.y % 2, t.w % 2, t.h % 2), (0, 0, 0, 0), "n={n}: {t:?}");
            }
            covers(&l, 1918, 1080, 0);
        }
    }

    #[test]
    fn a_wide_angle_in_a_square_cell_is_letterboxed_or_cropped_but_never_stretched() {
        let opts = Options { min_tile_w: 0, ..Default::default() };
        let l = layout(4, 1000, 1000, 3840, 2160, opts);           // 500×500 cells, 16:9 source
        let t = l.tiles[0];
        assert_eq!((t.picture.2, t.picture.3), (500, 281), "whole width, bars top and bottom: {t:?}");
        assert!((t.picture.2 as f32 / t.picture.3 as f32 - 16.0 / 9.0).abs() < 0.02);
        assert!(t.picture.1 > t.y && t.picture.1 + t.picture.3 <= t.y + t.h, "centred inside the tile: {t:?}");

        let c = layout(4, 1000, 1000, 3840, 2160, Options { fit: Fit::Crop, ..opts });
        let t = c.tiles[0];
        assert_eq!((t.picture.2, t.picture.3), (t.w, t.h), "crop fills the cell: {t:?}");
    }

    #[test]
    fn source_scale_says_how_much_detail_is_lost() {
        let l = layout(4, 3840, 2160, 3840, 2160, Options { min_tile_w: 0, ..Default::default() });
        assert!((l.tiles[0].source_scale - 2.0).abs() < 0.01,
                "a 2x2 of 4K angles on a 4K canvas shows each at half width: {:?}", l.tiles[0]);
        let one = layout(1, 3840, 2160, 3840, 2160, Options::default());
        assert!((one.tiles[0].source_scale - 1.0).abs() < 0.01);
    }

    #[test]
    fn a_deliverable_multiview_sizes_the_canvas_to_the_angles() {
        // four 4K angles kept whole need a 8K canvas; asking for 4K is asking for 1080p angles
        let (w, h) = canvas_for(4, 3840, 2160, 1.0, Align::Even);
        assert_eq!((w, h), (7680, 4320));
        let (w, h) = canvas_for(4, 3840, 2160, 0.5, Align::Even);
        assert_eq!((w, h), (3840, 2160));
        let l = layout(4, w, h, 3840, 2160, Options { min_tile_w: 0, ..Default::default() });
        assert!((l.tiles[0].source_scale - 2.0).abs() < 0.01, "half of each angle, as asked");
    }

    #[test]
    fn unreadable_tiles_are_reported_rather_than_drawn_tiny() {
        let l = layout(9, 480, 270, 3840, 2160, Options::default());
        assert_eq!(l.too_small.len(), 9, "160 px tiles in a 480 px pane: {:?}", l.tiles[0]);
        let roomy = layout(4, 1920, 1080, 3840, 2160, Options::default());
        assert!(roomy.too_small.is_empty());
    }

    #[test]
    fn the_shape_suits_the_canvas_and_the_angles() {
        assert_eq!(shape(4, 1920, 1080, 16.0 / 9.0), (2, 2));
        assert_eq!(shape(2, 1920, 1080, 16.0 / 9.0), (1, 2), "two 16:9 angles stack on a 16:9 canvas");
        assert_eq!(shape(3, 3840, 720, 16.0 / 9.0), (3, 1), "a wide strip puts them in a row");
        let l = layout(3, 1920, 1080, 3840, 2160, Options { min_tile_w: 0, ..Default::default() });
        assert_eq!((l.cols, l.rows, l.tiles.len()), (2, 2, 3), "3-up leaves the fourth cell empty");
        covers(&l, 1920, 1080, 0);  // and the three tiles still do not overlap
    }

    #[test]
    fn gutters_and_degenerate_sizes_do_not_panic_or_overlap() {
        let l = layout(4, 1920, 1080, 3840, 2160, Options { gutter: 8, min_tile_w: 0, ..Default::default() });
        covers(&l, 1920, 1080, 8);
        assert!(l.tiles[1].x >= l.tiles[0].x + l.tiles[0].w + 8, "gutter between columns: {:?}", l.tiles);
        assert!(layout(0, 100, 100, 3840, 2160, Options::default()).tiles.is_empty());
        assert!(layout(3, 0, 100, 3840, 2160, Options::default()).tiles.is_empty());
        let tiny = layout(4, 7, 5, 3840, 2160, Options { min_tile_w: 0, ..Default::default() });
        covers(&tiny, 7, 5, 0);
    }
}
