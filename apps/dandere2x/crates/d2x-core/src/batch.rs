//! Pack several small images into one canvas so one GPU call sees a full
//! tile's worth of work. A residual of a few cells is a handful of
//! workgroups; a modern GPU wants thousands in flight, so on exactly the
//! frames where block reuse is doing its job the network would otherwise
//! run at a few percent of the device.
//!
//! Shelf packing (rows of decreasing height) with a replicate-padded gutter
//! of `GUTTER` pixels around every item, so a same-pad network sees each
//! item's own edge for the first `GUTTER` pixels rather than its neighbour.
//! Items that don't fit a canvas run alone.

use crate::frame::Frame;
use crate::upscale::Upscaler;

pub const GUTTER: u32 = 8;

#[derive(Debug, Clone, Copy)]
struct Slot {
    item: usize,
    x: u32,
    y: u32,
}

/// Shelf-pack `sizes` (w, h) into canvases of width `canvas` and height ≤
/// `canvas`. Returns, per canvas, its (w, h) and the slots. Items larger
/// than the canvas are returned in the second vector.
fn pack(sizes: &[(u32, u32)], canvas: u32) -> (Vec<((u32, u32), Vec<Slot>)>, Vec<usize>) {
    let g = 2 * GUTTER;
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(sizes[i].1));
    let mut canvases: Vec<((u32, u32), Vec<Slot>)> = Vec::new();
    let mut alone = Vec::new();
    // Current canvas state.
    let (mut cw, mut ch, mut slots): (u32, u32, Vec<Slot>) = (0, 0, Vec::new());
    let (mut shelf_y, mut shelf_h, mut shelf_x) = (0u32, 0u32, 0u32);
    let flush = |cw: &mut u32, ch: &mut u32, slots: &mut Vec<Slot>, canvases: &mut Vec<((u32, u32), Vec<Slot>)>| {
        if !slots.is_empty() {
            canvases.push(((*cw, *ch), std::mem::take(slots)));
        }
        *cw = 0;
        *ch = 0;
    };
    // Only items small relative to the canvas benefit: a redraw-sized input
    // already fills a tile, and the gutter would be pure overhead.
    let small_limit = (canvas as u64) * (canvas as u64) / 8;
    for &i in &order {
        let (w, h) = (sizes[i].0 + g, sizes[i].1 + g);
        if w > canvas || h > canvas || (sizes[i].0 as u64) * (sizes[i].1 as u64) > small_limit {
            alone.push(i);
            continue;
        }
        // Fits on the current shelf?
        if slots.is_empty() || shelf_x + w > canvas {
            // New shelf; does it fit in this canvas?
            let new_y = if slots.is_empty() { 0 } else { shelf_y + shelf_h };
            if new_y + h > canvas {
                flush(&mut cw, &mut ch, &mut slots, &mut canvases);
                shelf_y = 0;
            } else {
                shelf_y = new_y;
            }
            shelf_x = 0;
            shelf_h = h;
        }
        slots.push(Slot { item: i, x: shelf_x + GUTTER, y: shelf_y + GUTTER });
        shelf_x += w;
        cw = cw.max(shelf_x);
        ch = ch.max(shelf_y + h);
    }
    flush(&mut cw, &mut ch, &mut slots, &mut canvases);
    (canvases, alone)
}

/// [`upscale_packed`] for a trait object, honouring its `canvas()`.
pub fn upscale_many_dyn(up: &mut dyn Upscaler, frames: &[&Frame]) -> anyhow::Result<Vec<Frame>> {
    let c = up.canvas();
    if c == 0 || frames.len() < 2 {
        return frames.iter().map(|f| up.upscale(f)).collect();
    }
    upscale_packed(up, frames, c)
}

/// Upscale `frames` through `up` with as few calls as packing allows.
/// Output order matches input order.
pub fn upscale_packed(up: &mut dyn Upscaler, frames: &[&Frame], canvas: u32) -> anyhow::Result<Vec<Frame>> {
    let s = up.scale();
    let sizes: Vec<(u32, u32)> = frames.iter().map(|f| (f.width, f.height)).collect();
    let (canvases, alone) = pack(&sizes, canvas);
    let mut out: Vec<Option<Frame>> = (0..frames.len()).map(|_| None).collect();
    for i in alone {
        out[i] = Some(up.upscale(frames[i])?);
    }
    for ((cw, ch), slots) in canvases {
        if slots.len() == 1 {
            let sl = slots[0];
            out[sl.item] = Some(up.upscale(frames[sl.item])?);
            continue;
        }
        let mut cv = Frame::new(cw, ch);
        for sl in &slots {
            let f = frames[sl.item];
            // Item plus its replicate gutter ring: one padded copy, blitted
            // row by row.
            let ring = f.padded(GUTTER, crate::frame::Pad::Replicate);
            let x0 = sl.x - GUTTER;
            let y0 = sl.y - GUTTER;
            let rb = (ring.width as usize) * 3;
            for y in 0..ring.height {
                let d = cv.idx(x0, y0 + y);
                cv.data[d..d + rb].copy_from_slice(ring.row(y, 0, ring.width));
            }
        }
        let big = up.upscale(&cv)?;
        anyhow::ensure!((big.width, big.height) == (cw * s, ch * s), "packed upscale size");
        for sl in &slots {
            let f = frames[sl.item];
            out[sl.item] = Some(big.crop(sl.x * s, sl.y * s, f.width * s, f.height * s));
        }
    }
    Ok(out.into_iter().map(|o| o.expect("every item upscaled")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upscale::NearestUpscaler;

    #[test]
    fn packing_covers_all_items_without_overlap() {
        let sizes: Vec<(u32, u32)> = (0..40).map(|i| (20 + (i * 37) % 200, 20 + (i * 53) % 150)).collect();
        let (canvases, alone) = pack(&sizes, 512);
        assert!(alone.is_empty());
        let mut seen = vec![false; sizes.len()];
        for ((cw, ch), slots) in &canvases {
            assert!(*cw <= 512 && *ch <= 512);
            for (a, sa) in slots.iter().enumerate() {
                assert!(!seen[sa.item]);
                seen[sa.item] = true;
                let (aw, ah) = sizes[sa.item];
                assert!(sa.x + aw + GUTTER <= *cw && sa.y + ah + GUTTER <= *ch);
                for sb in &slots[a + 1..] {
                    let (bw, bh) = sizes[sb.item];
                    let disjoint = sa.x + aw + GUTTER <= sb.x - GUTTER || sb.x + bw + GUTTER <= sa.x - GUTTER || sa.y + ah + GUTTER <= sb.y - GUTTER || sb.y + bh + GUTTER <= sa.y - GUTTER;
                    assert!(disjoint, "{sa:?} overlaps {sb:?}");
                }
            }
        }
        assert!(seen.iter().all(|&s| s));
        assert!(canvases.len() < 12, "{} canvases for 40 items", canvases.len());
    }

    #[test]
    fn packed_nearest_is_exact_and_ordered() {
        let mut up = NearestUpscaler { scale: 2 };
        let frames: Vec<Frame> = (0..7)
            .map(|i| {
                let mut f = Frame::new(30 + i * 9, 25 + (i * 13) % 40);
                for (k, b) in f.data.iter_mut().enumerate() {
                    *b = ((k * 31 + i as usize * 7) % 256) as u8;
                }
                f
            })
            .collect();
        let refs: Vec<&Frame> = frames.iter().collect();
        let got = upscale_packed(&mut up, &refs, 256).unwrap();
        for (g, f) in got.iter().zip(&frames) {
            assert_eq!(*g, up.upscale(f).unwrap());
        }
        // One that cannot fit, and one that is too large to be worth packing, run alone.
        let huge = Frame::new(300, 40);
        let big = Frame::new(100, 100);
        let got = upscale_packed(&mut up, &[&huge, &frames[0], &big], 256).unwrap();
        assert_eq!(got[0], up.upscale(&huge).unwrap());
        assert_eq!(got[2], up.upscale(&big).unwrap());
        let (canvases, alone) = pack(&[(300, 40), (100, 100), (40, 40), (50, 30)], 256);
        assert_eq!(alone.len(), 2);
        assert_eq!(canvases.len(), 1);
        assert_eq!(canvases[0].1.len(), 2);
    }
}
