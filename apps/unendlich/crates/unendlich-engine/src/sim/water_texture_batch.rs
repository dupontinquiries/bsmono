//! `sim::water_texture_batch` — generate the animated water caustic texture across threads.
//!
//! [`water_caustics::caustic`](crate::water_caustics::caustic) is a pure per-pixel
//! function (a sum of moving sine waves, `2π`-periodic in `t`). A `w*h` caustic
//! texture is therefore a **Class-P** parallel map over independent rows — each row
//! owns its output span — across the engine's rayon lane
//! ([`util::task_lanes::map_pure`](crate::util::task_lanes)). Rows come back in order,
//! so the concatenated texture is identical to computing it serially.

use crate::util::task_lanes::map_pure;
use crate::water_caustics::caustic;

/// Generate a `w*h` row-major caustic intensity texture (values in `[0,1]`) at time
/// `t` and spatial `scale`, computing the rows in parallel.
pub fn caustic_texture(w: usize, h: usize, t: f32, scale: f32) -> Vec<f32> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let rows: Vec<usize> = (0..h).collect();
    let computed: Vec<Vec<f32>> = map_pure(&rows, |&y| {
        let mut row = vec![0.0f32; w];
        for (x, cell) in row.iter_mut().enumerate() {
            let uv = [x as f32 / w as f32, y as f32 / h as f32];
            *cell = caustic(uv, t, scale);
        }
        row
    });
    let mut out = Vec::with_capacity(w * h);
    for row in computed {
        out.extend(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial(w: usize, h: usize, t: f32, scale: f32) -> Vec<f32> {
        let mut out = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                out[y * w + x] = caustic([x as f32 / w as f32, y as f32 / h as f32], t, scale);
            }
        }
        out
    }

    #[test]
    fn parallel_texture_equals_serial() {
        let (w, h, t, s) = (32, 24, 1.3, 4.0);
        assert_eq!(caustic_texture(w, h, t, s), serial(w, h, t, s));
    }

    #[test]
    fn texture_has_right_size_and_range() {
        let tex = caustic_texture(16, 16, 0.7, 3.0);
        assert_eq!(tex.len(), 16 * 16);
        assert!(tex.iter().all(|&v| (0.0..=1.0).contains(&v)), "caustic values stay in [0,1]");
    }

    #[test]
    fn deterministic_and_empty() {
        assert_eq!(caustic_texture(8, 8, 2.0, 2.0), caustic_texture(8, 8, 2.0, 2.0));
        assert!(caustic_texture(0, 8, 1.0, 1.0).is_empty());
        assert!(caustic_texture(8, 0, 1.0, 1.0).is_empty());
    }
}
