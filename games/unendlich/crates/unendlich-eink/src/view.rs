//! A 2.5D heightfield view (the classic voxel-space renderer): for each
//! screen column, march a ray front to back over surface heights, project
//! each height to a screen row, and fill the span it adds above what is
//! already drawn (a y-buffer, so near ground occludes far). Caves and
//! overhangs are not shown: this is the SURFACE of unendlich's world, which
//! is what a low-power CPU can draw at interactive rates.
//!
//! Pure: a camera and a height lookup in, RGBA pixels out.

pub use crate::heights::LEVELS;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cam {
    pub x: f32,
    pub z: f32,
    /// Eye height, world y.
    pub y: f32,
    /// Heading, radians; 0 looks +x.
    pub yaw: f32,
    /// Horizon as a fraction of image height from the top (look up/down).
    pub horizon: f32,
    /// Horizontal field of view, radians.
    pub fov: f32,
}

impl Default for Cam {
    fn default() -> Self {
        Cam { x: 0.0, z: 0.0, y: 110.0, yaw: 0.3, horizon: 0.38, fov: 1.2 }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    pub samples: usize,
    pub misses: usize,
}

pub const SKY_TOP: [u8; 3] = [150, 190, 235];
pub const SKY_HORIZON: [u8; 3] = [215, 228, 242];
const FAR: f32 = 900.0;

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8)
}

/// Terrain colour by height above sea level.
pub fn terrain(h: f32, sea: f32) -> [u8; 3] {
    let a = h - sea;
    if a <= 0.0 {
        mix([40, 90, 170], [70, 130, 200], (a + 20.0) / 20.0)
    } else if a < 3.0 {
        [214, 200, 150]
    } else if a < 40.0 {
        mix([80, 150, 70], [50, 110, 50], (a - 3.0) / 37.0)
    } else if a < 80.0 {
        mix([120, 110, 100], [150, 145, 140], (a - 40.0) / 40.0)
    } else {
        [240, 244, 248]
    }
}

/// Level of detail for a march step: coarser cells as the step grows.
pub fn level_for(step: f32) -> u8 {
    let mut l = 0u8;
    while l + 1 < LEVELS && (1u32 << (l + 1)) as f32 <= step {
        l += 1;
    }
    l
}

/// Render `w`×`h` RGBA into `out` (len `w*h*4`). `lookup(level, cx, cz)`
/// returns the surface height of a cell, or None if it is not known yet.
pub fn render(cam: &Cam, w: usize, h: usize, sea: f32, lookup: &dyn Fn(u8, i32, i32) -> Option<i16>, out: &mut [u8]) -> Stats {
    assert_eq!(out.len(), w * h * 4);
    let mut st = Stats::default();
    let horizon = cam.horizon * h as f32;
    let scale = h as f32 * 0.9;
    // sky
    for y in 0..h {
        let c = mix(SKY_TOP, SKY_HORIZON, y as f32 / horizon.max(1.0));
        for x in 0..w {
            out[(y * w + x) * 4..][..4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    for col in 0..w {
        let a = cam.yaw + (col as f32 / w as f32 - 0.5) * cam.fov;
        let (dx, dz) = (a.cos(), a.sin());
        let mut ybuf = h as i32; // lowest row not yet drawn (exclusive)
        let mut d = 1.0f32;
        let mut prev_h: Option<f32> = None;
        while d < FAR && ybuf > 0 {
            let step = 0.5 + d * 0.012;
            let l = level_for(step);
            let s = (1i32 << l) as f32;
            let (wx, wz) = (cam.x + dx * d, cam.z + dz * d);
            st.samples += 1;
            let hv = match lookup(l, (wx / s).floor() as i32, (wz / s).floor() as i32) {
                Some(v) => v as f32,
                None => {
                    st.misses += 1;
                    match prev_h {
                        Some(p) => p, // continue the last known ground
                        None => {
                            d += step;
                            continue;
                        }
                    }
                }
            };
            let surf = hv.max(sea);
            let row = ((cam.y - surf) / d * scale + horizon) as i32;
            if row < ybuf {
                let mut c = terrain(hv, sea);
                // slope light: rising ground (toward the viewer's gaze) is lit
                if let Some(p) = prev_h {
                    let k = ((hv - p) * 0.8).clamp(-40.0, 40.0);
                    c = [0, 1, 2].map(|i| (c[i] as f32 + k).clamp(0.0, 255.0) as u8);
                }
                c = mix(c, SKY_HORIZON, (d / FAR).powf(1.3));
                for y in row.max(0)..ybuf {
                    out[(y as usize * w + col) * 4..][..4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                }
                ybuf = row.max(0);
            }
            prev_h = Some(hv);
            d += step;
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(out: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
        let o = (y * w + x) * 4;
        [out[o], out[o + 1], out[o + 2]]
    }

    #[test]
    fn a_flat_plane_puts_ground_below_the_horizon_and_sky_above() {
        let (w, h) = (80, 60);
        let mut out = vec![0; w * h * 4];
        let cam = Cam { y: 80.0, ..Cam::default() };
        render(&cam, w, h, 60.0, &|_, _, _| Some(70), &mut out);
        assert_eq!(px(&out, w, 40, 0), SKY_TOP, "top row is sky");
        let ground = px(&out, w, 40, h - 1);
        assert!(ground[1] > ground[2], "bottom row is grass: {ground:?}");
    }

    #[test]
    fn a_near_wall_hides_everything_behind_it() {
        let (w, h) = (40, 60);
        let mut out = vec![0; w * h * 4];
        let cam = Cam { x: 0.0, z: 0.0, y: 70.0, yaw: 0.0, fov: 0.2, horizon: 0.5 };
        // a tall ridge 20 blocks ahead (+x), low ground elsewhere
        render(&cam, w, h, 0.0, &|l, cx, _| {
            let x = cx * (1 << l);
            Some(if (20..24).contains(&x) { 200 } else { 10 })
        }, &mut out);
        // the ridge (snow at 200) fills the view down to its base
        let top = px(&out, w, 20, 1);
        assert!(top.iter().all(|c| *c > 200), "the snowy ridge fills the view to the top: {top:?}");
    }

    #[test]
    fn unknown_cells_do_not_block_and_are_counted() {
        let (w, h) = (20, 20);
        let mut out = vec![0; w * h * 4];
        let st = render(&Cam::default(), w, h, 60.0, &|_, _, _| None, &mut out);
        assert!(st.samples > 0 && st.misses == st.samples);
        assert_eq!(px(&out, w, 10, 19), mix(SKY_TOP, SKY_HORIZON, 19.0 / (0.38 * 20.0)), "nothing known: all sky");
    }

    #[test]
    fn distance_picks_coarser_levels() {
        assert_eq!(level_for(0.6), 0);
        assert_eq!(level_for(2.5), 1);
        assert_eq!(level_for(9.0), 3);
        assert_eq!(level_for(1000.0), LEVELS - 1);
    }
}
