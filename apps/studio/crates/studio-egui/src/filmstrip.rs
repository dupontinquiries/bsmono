//! filmstrip — picture previews along video clips.
//!
//! Where the tiles go is decided here, apart from any decoding, so it can be
//! tested:
//!
//! * tiles are laid from the clip's OWN start, so they move with the clip when
//!   the view scrolls instead of swimming under it, and only the visible ones
//!   are asked for (the same visible-part rule the waveform keeps);
//! * each tile shows the frame at its left edge, rounded to a fixed ladder of
//!   steps (1/4, 1/2, 1, 2, 5, 10, 30, 60 s) at least one tile wide, so small
//!   zoom changes and scrolls ask for frames already decoded rather than new
//!   ones;
//! * the timeline only READS decoded frames (`Thumbs::tex`) and lists what it
//!   wanted but did not have (`Thumbs::wanted`); decoding happens elsewhere, in
//!   the background, so drawing never waits on ffmpeg.

use std::cell::RefCell;
use std::collections::HashMap;
use studio_core::{SourceId, Ticks, TICKS_PER_SECOND};

/// A decoded frame's key: the source and the frame time in milliseconds.
pub type Key = (SourceId, u64);

/// What the timeline has to draw with, and where it reports what it lacked.
#[derive(Default)]
pub struct Thumbs {
    pub tex: HashMap<Key, egui::TextureId>,
    pub wanted: RefCell<Vec<Key>>,
}

const LADDER: [f64; 8] = [0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0];

/// The ladder step for tiles lasting `tile_secs`: the smallest step at least
/// that long, so neighbouring tiles never show the same frame.
pub fn step_for(tile_secs: f64) -> f64 {
    LADDER.iter().copied().find(|s| *s >= tile_secs).unwrap_or(*LADDER.last().unwrap())
}

/// One tile: its left edge on screen, its width, and the frame time it shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile { pub x: f32, pub w: f32, pub ms: u64 }

/// Tiles for a clip drawn from `clip_x0` to `clip_x1` on screen, of which
/// `vis_x0..vis_x1` is visible, starting at `src_in` in its source and
/// `len` long, at `px_per_sec`, tiles `tile_w` wide.
pub fn tiles(clip_x0: f32, clip_x1: f32, vis_x0: f32, vis_x1: f32, src_in: Ticks, len: Ticks,
             px_per_sec: f32, tile_w: f32) -> Vec<Tile> {
    if tile_w < 4.0 || px_per_sec <= 0.0 || vis_x1 <= vis_x0 || clip_x1 <= clip_x0 { return Vec::new(); }
    let step = step_for(tile_w as f64 / px_per_sec as f64);
    let src0 = src_in.0 as f64 / TICKS_PER_SECOND as f64;
    let end = src0 + len.0 as f64 / TICKS_PER_SECOND as f64;
    // the first tile that reaches into view, counted from the clip's own start
    let first = (((vis_x0 - clip_x0) / tile_w).floor().max(0.0)) as usize;
    let mut out = Vec::new();
    let mut i = first;
    loop {
        let x = clip_x0 + i as f32 * tile_w;
        if x >= vis_x1.min(clip_x1) { break; }
        let t = src0 + (x - clip_x0) as f64 / px_per_sec as f64;
        let q = ((t / step).round() * step).clamp(src0, (end - 0.001).max(src0));
        out.push(Tile { x, w: tile_w.min(clip_x1 - x), ms: (q * 1000.0).round() as u64 });
        i += 1;
        if out.len() > 400 { break; }   // a guard, never reached at sane zooms
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: f64) -> Ticks { Ticks::from_seconds(x) }

    #[test]
    fn tiles_move_with_the_clip_and_only_the_visible_are_asked_for() {
        // a 20 s clip at 50 px/s is 1000 px; tiles 80 px wide (1.6 s -> the 2 s step)
        let a = tiles(100.0, 1100.0, 100.0, 600.0, s(3.0), s(20.0), 50.0, 80.0);
        assert_eq!(a[0].x, 100.0, "the first tile at the clip's start");
        assert_eq!(a.len(), 7, "only up to the view's edge: {a:?}");
        // scroll 230 px left: the clip starts off-screen; tiles keep their places ON THE CLIP
        let b = tiles(-130.0, 870.0, 100.0, 600.0, s(3.0), s(20.0), 50.0, 80.0);
        assert!(b.iter().all(|t| ((t.x + 130.0) / 80.0).fract().abs() < 1e-4), "aligned to the clip, not the view: {b:?}");
        assert!(b[0].x <= 100.0 && b[0].x + b[0].w > 100.0, "the first one reaches into view");
        // the same place on the clip shows the same frame after the scroll
        let at = |v: &[Tile], clip_x: f32| v.iter().find(|t| (t.x - clip_x).abs() < 1e-3).map(|t| t.ms);
        assert_eq!(at(&a, 100.0 + 320.0), at(&b, -130.0 + 320.0));
    }

    #[test]
    fn frames_come_from_a_fixed_ladder_and_stay_inside_the_clip() {
        assert_eq!(step_for(0.1), 0.25);
        assert_eq!(step_for(1.6), 2.0);
        assert_eq!(step_for(500.0), 60.0);
        let t = tiles(0.0, 1000.0, 0.0, 1000.0, s(3.0), s(20.0), 50.0, 80.0);
        assert!(t.iter().all(|x| x.ms % 2000 == 0 || x.ms == 3000), "on the 2 s ladder: {:?}", t.iter().map(|x| x.ms).collect::<Vec<_>>());
        assert!(t.iter().all(|x| (3000..23_000).contains(&x.ms)), "never before src_in or past the end");
        assert!(t.windows(2).all(|w| w[1].ms >= w[0].ms), "in order");
        // a small zoom change asks for the same frames (the ladder does not move)
        let z: std::collections::HashSet<u64> = tiles(0.0, 1040.0, 0.0, 1000.0, s(3.0), s(20.0), 52.0, 80.0).iter().map(|x| x.ms).collect();
        let base: std::collections::HashSet<u64> = t.iter().map(|x| x.ms).collect();
        assert!(z.difference(&base).count() <= 1, "zooming 4% reuses decoded frames");
        // the last tile is trimmed to the clip's end
        let short = tiles(0.0, 200.0, 0.0, 1000.0, s(0.0), s(4.0), 50.0, 80.0);
        assert_eq!(short.last().unwrap().x + short.last().unwrap().w, 200.0);
        assert!(tiles(0.0, 200.0, 300.0, 400.0, s(0.0), s(4.0), 50.0, 80.0).is_empty(), "off-screen: nothing asked for");
    }
}
