//! `sim::viewport_split` — local splitscreen layout solver for 1–4 players.
//!
//! Computes each player's viewport rectangle in normalised `[0,1]` space
//! (origin top-left). The rectangles tile the screen exactly — no gaps, no
//! overlaps, total area 1. No rendering; the bin scales these by the framebuffer
//! size to get pixel viewports/scissors.
//!
//! - **1** → full screen.
//! - **2** → split. `Columns` = side-by-side, `Rows` = stacked; `Auto` picks the
//!   orientation whose per-viewport pixel aspect is closest to the window aspect.
//! - **3** → one on top, two on the bottom (`Auto`); `Columns`/`Rows` give three
//!   equal strips.
//! - **4** → four equal quadrants.
//!
//! `players` is clamped to `1..=4`.
//!
//! Pure, deterministic, finite, panic-free. No `crate::render`, no `ash`/`vk`,
//! no IO, no clock, no RNG, no `.unwrap()`/`.expect()` in non-test code; compiles
//! clean under `cargo check --release` (no `cfg(test)`/`cfg(debug_assertions)`
//! gated helpers).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitPref {
    Auto,
    Columns,
    Rows,
}

/// A viewport rectangle in normalised `[0,1]` space, origin top-left.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Viewport {
    /// This viewport's pixel aspect (width/height in pixels), given the window's
    /// aspect. Since pixel size is `(w·W, h·H)`, the aspect is `(w/h)·window_aspect`.
    /// Returns 0 for a degenerate (zero-height) viewport rather than dividing by 0.
    pub fn aspect(&self, window_aspect: f32) -> f32 {
        if self.h <= 0.0 {
            0.0
        } else {
            (self.w / self.h) * window_aspect
        }
    }
}

/// `n` equal vertical strips (side-by-side columns) tiling `[0,1]` exactly.
fn cols(n: usize) -> Vec<Viewport> {
    let mut v = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let x0 = i as f32 / n as f32;
        // force the final boundary to exactly 1.0 so the strips never poke past
        // the unit square (e.g. thirds in f32).
        let x1 = if i + 1 == n { 1.0 } else { (i + 1) as f32 / n as f32 };
        v.push(Viewport { x: x0, y: 0.0, w: x1 - x0, h: 1.0 });
        i += 1;
    }
    v
}

/// `n` equal horizontal strips (stacked rows) tiling `[0,1]` exactly.
fn rows(n: usize) -> Vec<Viewport> {
    let mut v = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let y0 = i as f32 / n as f32;
        let y1 = if i + 1 == n { 1.0 } else { (i + 1) as f32 / n as f32 };
        v.push(Viewport { x: 0.0, y: y0, w: 1.0, h: y1 - y0 });
        i += 1;
    }
    v
}

fn quadrants() -> Vec<Viewport> {
    vec![
        Viewport { x: 0.0, y: 0.0, w: 0.5, h: 0.5 },
        Viewport { x: 0.5, y: 0.0, w: 0.5, h: 0.5 },
        Viewport { x: 0.0, y: 0.5, w: 0.5, h: 0.5 },
        Viewport { x: 0.5, y: 0.5, w: 0.5, h: 0.5 },
    ]
}

/// Compute the viewports for `players` (clamped `1..=4`) local players.
pub fn layout(players: u8, window_aspect: f32, pref: SplitPref) -> Vec<Viewport> {
    let p = players.clamp(1, 4);
    match p {
        1 => vec![Viewport { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }],
        2 => match pref {
            SplitPref::Columns => cols(2),
            SplitPref::Rows => rows(2),
            SplitPref::Auto => {
                let c = cols(2);
                let r = rows(2);
                let dc = (c[0].aspect(window_aspect) - window_aspect).abs();
                let dr = (r[0].aspect(window_aspect) - window_aspect).abs();
                if dc <= dr {
                    c
                } else {
                    r
                }
            }
        },
        3 => match pref {
            SplitPref::Columns => cols(3),
            SplitPref::Rows => rows(3),
            SplitPref::Auto => vec![
                Viewport { x: 0.0, y: 0.0, w: 1.0, h: 0.5 }, // top, full width
                Viewport { x: 0.0, y: 0.5, w: 0.5, h: 0.5 }, // bottom-left
                Viewport { x: 0.5, y: 0.5, w: 0.5, h: 0.5 }, // bottom-right
            ],
        },
        _ => quadrants(), // 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(v: &Viewport) -> f32 {
        v.w * v.h
    }
    fn overlap_area(a: &Viewport, b: &Viewport) -> f32 {
        let ox = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
        let oy = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
        ox.max(0.0) * oy.max(0.0)
    }
    fn in_unit(v: &Viewport) -> bool {
        v.x >= -1e-6
            && v.y >= -1e-6
            && v.w > 0.0
            && v.h > 0.0
            && v.x + v.w <= 1.0 + 1e-6
            && v.y + v.h <= 1.0 + 1e-6
    }

    const PREFS: [SplitPref; 3] = [SplitPref::Auto, SplitPref::Columns, SplitPref::Rows];

    #[test]
    fn length_equals_clamped_player_count() {
        for players in 0u8..=6 {
            let expected = players.clamp(1, 4) as usize;
            for &pref in &PREFS {
                assert_eq!(layout(players, 1.78, pref).len(), expected, "players={}", players);
            }
        }
    }

    #[test]
    fn rects_tile_the_unit_square() {
        for players in 1u8..=4 {
            for &pref in &PREFS {
                for &wa in &[0.5f32, 1.0, 1.6, 1.78, 2.37] {
                    let vps = layout(players, wa, pref);
                    // each within [0,1], positive size
                    for v in &vps {
                        assert!(in_unit(v), "out of unit square: {:?} (p={}, {:?})", v, players, pref);
                    }
                    // total area ~ 1
                    let total: f32 = vps.iter().map(area).sum();
                    assert!((total - 1.0).abs() < 1e-4, "area {} for p={} {:?}", total, players, pref);
                    // pairwise non-overlapping
                    for i in 0..vps.len() {
                        for j in (i + 1)..vps.len() {
                            assert!(
                                overlap_area(&vps[i], &vps[j]) < 1e-5,
                                "overlap {} & {} (p={}, {:?})", i, j, players, pref
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn one_player_is_full_screen() {
        for &pref in &PREFS {
            assert_eq!(
                layout(1, 1.78, pref),
                vec![Viewport { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }]
            );
        }
        // 0 clamps up to 1
        assert_eq!(layout(0, 1.78, SplitPref::Auto).len(), 1);
    }

    #[test]
    fn four_players_are_equal_quarters() {
        for &pref in &PREFS {
            let vps = layout(4, 1.78, pref);
            assert_eq!(vps.len(), 4);
            for v in &vps {
                assert!((area(v) - 0.25).abs() < 1e-6, "quarter area, got {}", area(v));
                assert!((v.w - 0.5).abs() < 1e-6 && (v.h - 0.5).abs() < 1e-6);
            }
        }
        // >4 clamps down to 4
        assert_eq!(layout(9, 1.78, SplitPref::Auto).len(), 4);
    }

    #[test]
    fn two_auto_picks_aspect_nearest_to_window() {
        let wa = 1.78; // wide window
        let auto = layout(2, wa, SplitPref::Auto);
        let c = layout(2, wa, SplitPref::Columns);
        let r = layout(2, wa, SplitPref::Rows);
        let da = (auto[0].aspect(wa) - wa).abs();
        let dc = (c[0].aspect(wa) - wa).abs();
        let dr = (r[0].aspect(wa) - wa).abs();
        // Auto's per-viewport aspect is at least as near the window aspect as
        // either explicit orientation.
        assert!(da <= dc + 1e-6 && da <= dr + 1e-6, "auto not nearest: da={} dc={} dr={}", da, dc, dr);
        // and Auto equals whichever explicit orientation is nearer
        if dc <= dr {
            assert_eq!(auto, c);
        } else {
            assert_eq!(auto, r);
        }
    }

    #[test]
    fn aspect_formula_is_correct() {
        let half_wide = Viewport { x: 0.0, y: 0.0, w: 0.5, h: 1.0 }; // half width, full height
        assert!((half_wide.aspect(2.0) - 1.0).abs() < 1e-6, "0.5/1 * 2 = 1");
        let half_tall = Viewport { x: 0.0, y: 0.0, w: 1.0, h: 0.5 };
        assert!((half_tall.aspect(2.0) - 4.0).abs() < 1e-6, "1/0.5 * 2 = 4");
        // degenerate height -> 0, no div by zero
        let degen = Viewport { x: 0.0, y: 0.0, w: 1.0, h: 0.0 };
        assert_eq!(degen.aspect(1.78), 0.0);
    }

    #[test]
    fn columns_and_rows_orientation() {
        let c = layout(2, 1.0, SplitPref::Columns);
        assert!((c[0].w - 0.5).abs() < 1e-6 && (c[0].h - 1.0).abs() < 1e-6, "columns are side-by-side");
        let r = layout(2, 1.0, SplitPref::Rows);
        assert!((r[0].w - 1.0).abs() < 1e-6 && (r[0].h - 0.5).abs() < 1e-6, "rows are stacked");
        // 3 columns tile exactly with the last reaching 1.0
        let c3 = layout(3, 1.0, SplitPref::Columns);
        let last = &c3[2];
        assert!((last.x + last.w - 1.0).abs() < 1e-6, "last column reaches exactly 1.0");
    }

    #[test]
    fn deterministic_and_finite() {
        for players in 0u8..=5 {
            for &pref in &PREFS {
                for &wa in &[0.0f32, 0.5, 1.0, 1.78, 100.0] {
                    let a = layout(players, wa, pref);
                    let b = layout(players, wa, pref);
                    assert_eq!(a, b);
                    for v in &a {
                        assert!(v.x.is_finite() && v.y.is_finite() && v.w.is_finite() && v.h.is_finite());
                    }
                }
            }
        }
    }
}
