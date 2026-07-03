//! C FFI for snapclone-composer.
//!
//! The Qt group view holds the tile list and a scrub position (driven by
//! snapclone-gesture). Each time the position or voice state changes it calls
//! sc_composer_view(...) with the tiles + active index + voice flag and a
//! caller-owned output array; the function fills it with up to 3 VisibleTile
//! descriptors and returns the count. No allocation crosses the boundary.

use crate::{Composer, PlaybackState, Tile};
use std::os::raw::c_int;
use std::panic::catch_unwind;

/// C mirror of Tile.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScTile {
    pub id: u64,
    pub ts_ms: i64,
    /// 1 if the tile has an audio track (video), 0 if a still image.
    pub has_audio_track: c_int,
}

/// C playback-state codes (mirror PlaybackState).
pub const SC_PB_PLAYING_WITH_AUDIO: c_int = 0;
pub const SC_PB_PLAYING_MUTED: c_int = 1;
pub const SC_PB_DIMMED: c_int = 2; // dimmed + time-warped + muted (voice gating)
pub const SC_PB_STILL: c_int = 3;

/// C mirror of VisibleTile.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScVisibleTile {
    pub id: u64,
    pub playback: c_int, // SC_PB_*
    pub slot: i32,       // -1 prev, 0 active, +1 next
    /// Playback rate as a percent of normal. 100 for normal-speed states;
    /// for SC_PB_DIMMED this is the time-warp rate (e.g. 35 = 0.35x).
    pub rate_pct: i32,
}

fn pb_to_c(p: PlaybackState) -> (c_int, i32) {
    match p {
        PlaybackState::PlayingWithAudio => (SC_PB_PLAYING_WITH_AUDIO, 100),
        PlaybackState::PlayingMuted => (SC_PB_PLAYING_MUTED, 100),
        PlaybackState::Dimmed { rate_pct } => (SC_PB_DIMMED, rate_pct as i32),
        PlaybackState::Still => (SC_PB_STILL, 100),
    }
}

/// Build the tiles vec from a C array (null-tolerant).
///
/// # Safety
/// `tiles` must point to `count` `ScTile`s (or be null with count 0).
unsafe fn tiles_from_c(tiles: *const ScTile, count: c_int) -> Vec<Tile> {
    if tiles.is_null() || count <= 0 {
        return Vec::new();
    }
    let slice = unsafe { std::slice::from_raw_parts(tiles, count as usize) };
    slice.iter().map(|t| Tile {
        id: t.id,
        ts_ms: t.ts_ms,
        has_audio_track: t.has_audio_track != 0,
    }).collect()
}

/// Fill `out` (capacity >= 3) with the visible tiles for the given scrub.
/// `base_index` + `steps` select the active tile (clamped); `active_has_voice`
/// gates the neighbors. Returns the number of visible tiles written (0..=3).
/// Writes `out_active_index` (>=0, or -1 if the feed is empty) and
/// `out_window_len` if those pointers are non-null.
///
/// # Safety
/// `tiles` points to `tile_count` ScTiles; `out` points to >= 3 ScVisibleTile.
#[no_mangle]
pub unsafe extern "C" fn sc_composer_view(
    tiles: *const ScTile,
    tile_count: c_int,
    base_index: c_int,
    steps: i64,
    active_has_voice: c_int,
    now_ms: i64,
    out: *mut ScVisibleTile,
    out_active_index: *mut c_int,
    out_window_len: *mut c_int,
) -> c_int {
    let res = catch_unwind(|| {
        if out.is_null() {
            return 0;
        }
        let tv = unsafe { tiles_from_c(tiles, tile_count) };
        let c = Composer::new(&tv, now_ms);
        let base = if base_index < 0 { 0usize } else { base_index as usize };
        let view = c.scrub(base, steps, active_has_voice != 0);

        let n = view.tiles.len().min(3);
        for (i, vt) in view.tiles.iter().take(n).enumerate() {
            unsafe {
                let slot = out.add(i);
                let (pb, rate) = pb_to_c(vt.playback);
                (*slot).id = vt.id;
                (*slot).playback = pb;
                (*slot).slot = vt.slot as i32;
                (*slot).rate_pct = rate;
            }
        }
        if !out_active_index.is_null() {
            unsafe {
                *out_active_index = match view.active_index {
                    Some(i) => i as c_int,
                    None => -1,
                };
            }
        }
        if !out_window_len.is_null() {
            unsafe { *out_window_len = view.window_len as c_int; }
        }
        n as c_int
    });
    res.unwrap_or(0)
}

/// Tap-to-activate by id (replay). Same outputs as sc_composer_view.
///
/// # Safety
/// Same contract as sc_composer_view.
#[no_mangle]
pub unsafe extern "C" fn sc_composer_activate_id(
    tiles: *const ScTile,
    tile_count: c_int,
    tap_id: u64,
    active_has_voice: c_int,
    now_ms: i64,
    out: *mut ScVisibleTile,
    out_active_index: *mut c_int,
    out_window_len: *mut c_int,
) -> c_int {
    let res = catch_unwind(|| {
        if out.is_null() {
            return 0;
        }
        let tv = unsafe { tiles_from_c(tiles, tile_count) };
        let c = Composer::new(&tv, now_ms);
        let view = c.activate_id(tap_id, active_has_voice != 0);
        let n = view.tiles.len().min(3);
        for (i, vt) in view.tiles.iter().take(n).enumerate() {
            unsafe {
                let slot = out.add(i);
                let (pb, rate) = pb_to_c(vt.playback);
                (*slot).id = vt.id;
                (*slot).playback = pb;
                (*slot).slot = vt.slot as i32;
                (*slot).rate_pct = rate;
            }
        }
        if !out_active_index.is_null() {
            unsafe {
                *out_active_index = view.active_index.map(|i| i as c_int).unwrap_or(-1);
            }
        }
        if !out_window_len.is_null() {
            unsafe { *out_window_len = view.window_len as c_int; }
        }
        n as c_int
    });
    res.unwrap_or(0)
}

/// C mirror of a TilePlacement.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScTilePlacement {
    pub slot: i32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub is_big: c_int,
}

pub const SC_EDGE_RIGHT: c_int = 0;
pub const SC_EDGE_BOTTOM: c_int = 1;
pub const SC_EDGE_AUTO: c_int = 2;

/// Compute the 2D tiling layout (two-small-one-big) for the present slots.
/// `slots` is an array of slot ids (-1/0/+1); `big_slot` gets the big pane.
/// `edge`: SC_EDGE_RIGHT/BOTTOM/AUTO (auto picks by viewport aspect). Fills
/// `out` (capacity >= slot_count) and returns the count written.
///
/// # Safety
/// `slots` points to `slot_count` i32; `out` to >= `slot_count` ScTilePlacement.
#[no_mangle]
pub unsafe extern "C" fn sc_composer_layout(
    slots: *const i32, slot_count: c_int, big_slot: i32,
    vp_x: f32, vp_y: f32, vp_w: f32, vp_h: f32,
    edge: c_int, big_fraction: f32, gap: f32,
    out: *mut ScTilePlacement, out_cap: c_int,
) -> c_int {
    let res = catch_unwind(|| {
        if slots.is_null() || out.is_null() || slot_count <= 0 || out_cap <= 0 {
            return 0;
        }
        let slot_slice = unsafe { std::slice::from_raw_parts(slots, slot_count as usize) };
        let sv: Vec<i8> = slot_slice.iter().map(|&s| s as i8).collect();
        let vp = crate::Rect { x: vp_x, y: vp_y, w: vp_w, h: vp_h };
        let e = match edge {
            SC_EDGE_RIGHT => crate::SmallEdge::Right,
            SC_EDGE_BOTTOM => crate::SmallEdge::Bottom,
            _ => crate::TileLayout::auto_edge(vp),
        };
        let mut l = crate::TileLayout::new(vp, e);
        if big_fraction > 0.0 && big_fraction < 1.0 { l.big_fraction = big_fraction; }
        if gap >= 0.0 { l.gap = gap; }
        let placements = l.layout(&sv, big_slot as i8);
        let n = (placements.len() as c_int).min(out_cap);
        for (i, p) in placements.iter().take(n as usize).enumerate() {
            unsafe {
                let o = out.add(i);
                (*o).slot = p.slot as i32;
                (*o).x = p.rect.x; (*o).y = p.rect.y;
                (*o).w = p.rect.w; (*o).h = p.rect.h;
                (*o).is_big = if p.is_big { 1 } else { 0 };
            }
        }
        n
    });
    res.unwrap_or(0)
}

/// Re-tile step: next slot to promote to big (the rotate gesture).
/// `forward` != 0 cycles forward. Returns the new big slot (or current_big if
/// slots empty).
///
/// # Safety
/// `slots` points to `slot_count` i32.
#[no_mangle]
pub unsafe extern "C" fn sc_composer_next_big(
    slots: *const i32, slot_count: c_int, current_big: i32, forward: c_int,
) -> i32 {
    let res = catch_unwind(|| {
        if slots.is_null() || slot_count <= 0 { return current_big; }
        let slot_slice = unsafe { std::slice::from_raw_parts(slots, slot_count as usize) };
        let sv: Vec<i8> = slot_slice.iter().map(|&s| s as i8).collect();
        let vp = crate::Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };
        let l = crate::TileLayout::new(vp, crate::SmallEdge::Bottom);
        l.next_big(&sv, current_big as i8, forward != 0) as i32
    });
    res.unwrap_or(current_big)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000_000_000;

    fn t(id: u64, ts: i64, audio: bool) -> ScTile {
        ScTile { id, ts_ms: ts, has_audio_track: if audio { 1 } else { 0 } }
    }

    #[test]
    fn ffi_view_fills_and_reports() {
        let tiles = [t(1, NOW - 3000, true), t(2, NOW - 2000, true), t(3, NOW - 1000, true)];
        let mut out = [ScVisibleTile { id: 0, playback: 0, slot: 0, rate_pct: 0 }; 3];
        let mut active = -9;
        let mut wlen = -9;
        // base = newest (2), steps -1 → active id 2 (index 1)
        let n = unsafe {
            sc_composer_view(tiles.as_ptr(), 3, 2, -1, 0, NOW,
                out.as_mut_ptr(), &mut active, &mut wlen)
        };
        assert_eq!(n, 3);
        assert_eq!(active, 1);
        assert_eq!(wlen, 3);
        assert_eq!(out[1].slot, 0); // active in the middle
        assert_eq!(out[1].playback, SC_PB_PLAYING_WITH_AUDIO);
    }

    #[test]
    fn ffi_voice_dims_neighbors() {
        let tiles = [t(1, NOW - 3000, true), t(2, NOW - 2000, true), t(3, NOW - 1000, true)];
        let mut out = [ScVisibleTile { id: 0, playback: 0, slot: 0, rate_pct: 0 }; 3];
        let n = unsafe {
            sc_composer_view(tiles.as_ptr(), 3, 1, 0, 1, NOW,
                out.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut())
        };
        assert_eq!(n, 3);
        assert_eq!(out[0].playback, SC_PB_DIMMED);
        assert_eq!(out[2].playback, SC_PB_DIMMED);
    }

    #[test]
    fn ffi_activate_id() {
        let tiles = [t(10, NOW - 2000, true), t(20, NOW - 1000, true)];
        let mut out = [ScVisibleTile { id: 0, playback: 0, slot: 0, rate_pct: 0 }; 3];
        let mut active = -9;
        let n = unsafe {
            sc_composer_activate_id(tiles.as_ptr(), 2, 10, 0, NOW,
                out.as_mut_ptr(), &mut active, std::ptr::null_mut())
        };
        assert!(n >= 1);
        assert_eq!(active, 0);
    }

    #[test]
    fn ffi_null_safe() {
        let mut out = [ScVisibleTile { id: 0, playback: 0, slot: 0, rate_pct: 0 }; 3];
        // null tiles → empty feed → 0
        assert_eq!(unsafe {
            sc_composer_view(std::ptr::null(), 0, 0, 0, 0, NOW,
                out.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut())
        }, 0);
        // null out → 0
        assert_eq!(unsafe {
            sc_composer_view(std::ptr::null(), 0, 0, 0, 0, NOW,
                std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
        }, 0);
    }
}
