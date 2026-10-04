//! Drawing the lock screen's colour wash from a handful of numbers.
//!
//! The pane shows the room as colour, not as a picture. A 32x40 frame
//! uploaded as a texture and stretched is already cheap, but it is still ~5
//! kB crossing the bus every window and a texture upload that invalidates a
//! cache line per row. The wash does not need any of that.
//!
//! [`crate::camera::glass`] has already reduced the frame to a coarse grid —
//! 4x5 is 60 bytes. Those twenty cells are drawn as a lattice of
//! gradient-interpolated quads, so the "upscaling" is the interpolation the
//! GPU does between vertices for free. Nothing is resampled, nothing large
//! is uploaded, and the data that moves per window fits in a cache line or
//! two.
//!
//! Two details make it look like glass rather than like twenty squares:
//!
//! **Lattice, not cells.** Colours live at the CORNERS of the lattice, each
//! the average of the cells touching it, so adjacent quads share vertices
//! and the gradient is continuous. Colouring whole quads would show the grid
//! as flat facets with visible seams.
//!
//! **Noise.** A gradient that smooth across a phone screen bands, because
//! 8-bit steps are wider than the eye's threshold at low brightness — and
//! the lock screen is dim by definition. A fixed, tiling noise texture at
//! low alpha breaks the bands. It is uploaded ONCE and reused: per-frame
//! noise would reintroduce exactly the data movement this avoids, and would
//! also shimmer, which reads as a fault rather than as grain.

/// Colour at each lattice corner, row-major, `(cols + 1) * (rows + 1)` of
/// them. Corner colours are the mean of the cells that touch them, which is
/// what makes neighbouring quads agree along their shared edge.
pub fn lattice(cells: &[[u8; 3]], cols: usize, rows: usize) -> Vec<[u8; 3]> {
    if cols == 0 || rows == 0 || cells.len() < cols * rows {
        return vec![[0, 0, 0]];
    }
    let at = |cx: isize, cy: isize| -> Option<[u8; 3]> {
        if cx < 0 || cy < 0 || cx >= cols as isize || cy >= rows as isize {
            return None;
        }
        Some(cells[cy as usize * cols + cx as usize])
    };
    let mut out = Vec::with_capacity((cols + 1) * (rows + 1));
    for gy in 0..=rows as isize {
        for gx in 0..=cols as isize {
            let mut sum = [0u32; 3];
            let mut n = 0u32;
            for (dx, dy) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
                if let Some(c) = at(gx + dx, gy + dy) {
                    for i in 0..3 {
                        sum[i] += c[i] as u32;
                    }
                    n += 1;
                }
            }
            let n = n.max(1);
            out.push([(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]);
        }
    }
    out
}

/// Triangle indices for a `cols` x `rows` lattice of quads.
pub fn indices(cols: usize, rows: usize) -> Vec<u32> {
    let stride = (cols + 1) as u32;
    let mut idx = Vec::with_capacity(cols * rows * 6);
    for y in 0..rows as u32 {
        for x in 0..cols as u32 {
            let tl = y * stride + x;
            let (tr, bl, br) = (tl + 1, tl + stride, tl + stride + 1);
            idx.extend_from_slice(&[tl, tr, br, tl, br, bl]);
        }
    }
    idx
}

/// A square tile of grain, as alpha values.
///
/// Deterministic from `seed` so it is identical every run, and TILEABLE:
/// generated on a torus so the right edge continues into the left. A tile
/// with seams would draw a grid of its own over the grid it is hiding.
pub fn noise_tile(size: usize, seed: u32, amount: u8) -> Vec<u8> {
    let size = size.max(1);
    let mut out = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            // A cheap integer hash of the wrapped coordinates. Wrapping is
            // what makes the tile seamless: (size, y) hashes as (0, y).
            let (hx, hy) = ((x % size) as u32, (y % size) as u32);
            let mut h = hx.wrapping_mul(0x9E37_79B9) ^ hy.wrapping_mul(0x85EB_CA6B) ^ seed;
            h ^= h >> 15;
            h = h.wrapping_mul(0x2545_F491);
            h ^= h >> 13;
            out.push(((h & 0xFF) as u16 * amount as u16 / 255) as u8);
        }
    }
    out
}

/// A black point for an OLED panel: everything at or under `floor` becomes exactly 0 (an OLED pixel at
/// (20, 20, 22) is a lit grey, at 0 it is off), and the rest is stretched back over 0..255 so
/// highlights keep their full value. Per channel, so hue survives: a dark red stays red, a dark neutral
/// goes black. The lock screen's wash is the room's colour, which is usually dark and desaturated,
/// and without this it reads as a flat mid-grey.
pub fn black_point(c: [u8; 3], floor: u8) -> [u8; 3] {
    let f = floor.min(254) as f32;
    let m = |v: u8| ((v as f32 - f).max(0.0) * 255.0 / (255.0 - f)).round() as u8;
    [m(c[0]), m(c[1]), m(c[2])]
}

/// How night it is, 0 (a lit room) to 1 (dark), from the smoothed ambient lux; `None` (no reading) is day, so a phone
/// with no light sensor keeps the daytime wash. Smooth in log lux, so it never steps: full night at 3 lux and below,
/// none at 40 and above.
pub fn nightness(lux: Option<f32>) -> f32 {
    let Some(l) = lux else { return 0.0 };
    let t = ((40f32.ln() - l.max(0.01).ln()) / (40f32.ln() - 3f32.ln())).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The OLED wash at night: a gate that takes the dark cells to exactly 0 (a camera frame of a dark room is never
/// black, it is a lifted grey, and a lit OLED pixel at (20,20,22) is the glow you see on the ceiling), and a
/// compressor that stops a bright patch (a lamp in frame) from lighting the whole panel. `night` is
/// [`nightness`]. At 0 it is close to the daytime `black_point(c, 24)` (the same floor, eased in rather than a hard clamp).
///
/// Per channel so hue survives. The gate is a smooth knee rather than a hard clamp: a hard clamp turns a
/// gradient into a flat black region with a visible edge. The compressor is a soft limit (tanh) above a knee, so
/// ordering of brightness is kept and nothing clips flat.
pub fn night_wash(c: [u8; 3], night: f32) -> [u8; 3] {
    let n = night.clamp(0.0, 1.0);
    let gate = 24.0 + 36.0 * n; // 24 by day, 60 deep at night
    let cap = 255.0 - 150.0 * n; // 255 by day, 105 at night
    // the knee sits at the top of the range by day (the compressor does nothing) and drops to 55% of the cap at night
    let knee = cap - (cap * 0.45) * n;
    let one = |v: u8| -> u8 {
        let v = v as f32;
        // gate: 0 below `gate`, eased up to the identity by 1.6x gate
        let g = if v <= gate { 0.0 } else {
            let t = ((v - gate) / (0.6 * gate)).min(1.0);
            let ease = t * t * (3.0 - 2.0 * t);
            (v - gate) * (255.0 / (255.0 - gate)) * ease
        };
        // compressor
        let o = if g <= knee || cap - knee < 1e-3 { g.min(cap) } else { knee + (cap - knee) * ((g - knee) / (cap - knee)).tanh() };
        o.round().clamp(0.0, 255.0) as u8
    };
    [one(c[0]), one(c[1]), one(c[2])]
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_black_point_turns_a_dark_neutral_black_and_keeps_hue_and_highlights() {
        assert_eq!(black_point([20, 20, 22], 24), [0, 0, 0], "the lifted grey of a dark room is off");
        assert_eq!(black_point([0, 0, 0], 24), [0, 0, 0]);
        assert_eq!(black_point([255, 255, 255], 24), [255, 255, 255], "white stays white");
        let red = black_point([60, 20, 20], 24);
        assert!(red[0] > 0 && red[1] == 0 && red[2] == 0, "a dark red is still red: {red:?}");
        // monotonic: a brighter input never comes out darker
        for v in 0..255u8 {
            assert!(black_point([v + 1, 0, 0], 24)[0] >= black_point([v, 0, 0], 24)[0], "at {v}");
        }
        // a floor of zero changes nothing
        assert_eq!(black_point([20, 40, 90], 0), [20, 40, 90]);
    }

    use super::*;

    #[test]
    fn corners_are_shared_so_the_gradient_is_continuous() {
        // Two cells, red then blue. The shared corner must be the average of
        // both: that is the whole difference between a gradient and facets.
        let cells = [[200u8, 0, 0], [0, 0, 200]];
        let l = lattice(&cells, 2, 1);
        assert_eq!(l.len(), 3 * 2, "(cols+1) x (rows+1)");
        assert_eq!(l[0], [200, 0, 0], "outer corner takes the one cell it touches");
        assert_eq!(l[1], [100, 0, 100], "shared corner is the mean");
        assert_eq!(l[2], [0, 0, 200]);
        // The bottom row repeats it: one row of cells, two rows of corners.
        assert_eq!(&l[3..], &l[..3]);
    }

    #[test]
    fn a_flat_scene_produces_one_flat_colour() {
        let cells = vec![[40u8, 50, 60]; 20];
        let l = lattice(&cells, 4, 5);
        assert_eq!(l.len(), 5 * 6);
        assert!(l.iter().all(|c| *c == [40, 50, 60]), "averaging must not darken the edges");
    }

    #[test]
    fn the_mesh_covers_every_quad_exactly_once() {
        let idx = indices(4, 5);
        assert_eq!(idx.len(), 4 * 5 * 6, "two triangles per quad");
        assert!(idx.iter().all(|i| (*i as usize) < 5 * 6), "no index past the lattice");
        // Every quad contributes its four corners.
        for q in idx.chunks(6) {
            let mut c: Vec<u32> = q.to_vec();
            c.sort_unstable();
            c.dedup();
            assert_eq!(c.len(), 4, "a degenerate triangle would leave a hole");
        }
    }

    #[test]
    fn the_grain_tiles_without_a_seam() {
        let t = noise_tile(8, 7, 255);
        assert_eq!(t.len(), 64);
        // Column 0 is what continues past column 7, by construction.
        for y in 0..8 {
            let wrapped = noise_tile(8, 7, 255)[y * 8];
            assert_eq!(t[y * 8], wrapped);
        }
        // Deterministic across runs, and the amount scales it.
        assert_eq!(noise_tile(8, 7, 255), t);
        let faint = noise_tile(8, 7, 16);
        assert!(faint.iter().max().unwrap() <= &16, "amount is a ceiling");
        assert!(faint.iter().any(|v| *v > 0), "but not silence");
        // Different seeds differ, so two surfaces do not share a pattern.
        assert_ne!(noise_tile(8, 8, 255), t);
    }

    #[test]
    fn degenerate_grids_do_not_panic() {
        assert_eq!(lattice(&[], 0, 0).len(), 1);
        assert_eq!(lattice(&[[1, 2, 3]], 4, 5).len(), 1, "too few cells is refused, not read past");
        assert!(indices(0, 0).is_empty());
        assert_eq!(noise_tile(0, 1, 255).len(), 1);
    }

    #[test]
    fn at_night_a_dark_room_is_exactly_off_and_a_lamp_cannot_light_the_panel() {
        use super::*;
        assert_eq!(night_wash([50, 50, 56], 1.0), [0, 0, 0], "a lifted grey of a dark room is off at night");
        assert_ne!(night_wash([50, 50, 56], 0.0), [0, 0, 0], "but the day wash keeps more of it");
        let lamp = night_wash([255, 255, 255], 1.0);
        assert!(lamp[0] <= 110 && lamp[0] > 60, "a lamp is held down but is still the brightest thing: {lamp:?}");
        assert_eq!(night_wash([255, 255, 255], 0.0), [255, 255, 255], "by day white is white");
        // order is preserved: brighter in, brighter (or equal) out
        let mut last = 0;
        for v in 0..=255u8 { let o = night_wash([v, v, v], 1.0)[0]; assert!(o >= last, "monotone at {v}"); last = o; }
        // and nothing steps: neighbouring inputs differ by a few levels at most (no cliff at the gate)
        for v in 0..255u8 { let (a, b) = (night_wash([v; 3], 1.0)[0] as i32, night_wash([v + 1; 3], 1.0)[0] as i32); assert!((b - a).abs() <= 4, "cliff at {v}: {a} -> {b}"); }
    }

    #[test]
    fn nightness_follows_the_room_in_log_lux_and_a_missing_sensor_is_day() {
        use super::*;
        assert_eq!(nightness(None), 0.0);
        assert_eq!(nightness(Some(1.0)), 1.0);
        assert_eq!(nightness(Some(300.0)), 0.0);
        let (a, b, c) = (nightness(Some(5.0)), nightness(Some(12.0)), nightness(Some(30.0)));
        assert!(a > b && b > c && a < 1.0 && c > 0.0, "{a} {b} {c}");
    }
}
