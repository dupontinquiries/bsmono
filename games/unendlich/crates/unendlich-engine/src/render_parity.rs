//! CPU reference renderer + image-parity harness.
//!
//! Goal (from the 2026-05-29 session): know whether the rendered world is
//! "still broken" — missing geometry, swiss-cheesed terrain, holes — WITHOUT
//! loading a world and eyeballing it. The approach the user asked for: a basic
//! CPU renderer that *approximates* the scene from the same world + camera, and
//! a fail-fast comparison against a GPU framebuffer capture. If the difference
//! exceeds a threshold, the test fails and points you at a regression.
//!
//! Two layers of signal, in robustness order:
//!   1. **Coverage** (terrain-vs-sky per pixel). This is the primary "is the
//!      mesh showing" check and is robust to shading/lighting differences — a
//!      hole or missing chunk reads as sky where the reference says terrain.
//!   2. **Luminance** (coarse height/depth shading). Secondary; tolerant
//!      threshold, catches gross mis-shading but not lighting nuance.
//!
//! The CPU reference raymarches the SAME occupancy predicate the chunk
//! generator uses (`density > DENSITY_THRESHOLD && !is_cave`), so it tracks
//! worldgen exactly. It does NOT model the GPU's lighting — by design: the
//! coverage mask is what catches the bugs we care about.
//!
//! GPU side (runs on the user's hardware, not here): capture the swapchain
//! image to a row-major RGB8 buffer, build a `RefImage` from it via
//! [`RefImage::from_rgb8_coverage`], and call [`assert_parity`] against
//! [`cpu_reference`] for the same camera. `during/after stabilize` = capture
//! once the chunk stream has settled (e.g. N frames after the last upload).

use crate::world::cpu_generator::WorldGenerator;
use crate::world::world_model::anchors;

/// Pinhole camera for the reference raymarcher. `yaw`/`pitch` in radians;
/// yaw 0 looks +X, pitch <0 looks down.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub pos: [f64; 3],
    pub yaw: f64,
    pub pitch: f64,
    pub fov_deg: f64,
}

impl Default for Camera {
    fn default() -> Self {
        // Slightly downward over origin terrain (surface ~64): catches the
        // ground in the lower frame, sky in the upper.
        Camera { pos: [0.0, 95.0, 0.0], yaw: 0.3, pitch: -0.45, fov_deg: 70.0 }
    }
}

/// A small rendered image: per-pixel coverage (true = terrain hit) + luminance.
/// Row-major, origin top-left.
#[derive(Clone, Debug)]
pub struct RefImage {
    pub w: usize,
    pub h: usize,
    pub cover: Vec<bool>,
    pub lum: Vec<u8>,
}

impl RefImage {
    /// Build a coverage+luma image from a row-major RGB8 GPU capture. A pixel
    /// counts as "covered" (terrain) when its luminance differs from the known
    /// flat `sky_lum` by more than `sky_tol` — i.e. it isn't sky. This lets a
    /// GPU capture be compared structurally against the CPU reference without
    /// matching exact colors.
    pub fn from_rgb8_coverage(rgb: &[u8], w: usize, h: usize, sky_lum: u8, sky_tol: u8) -> Self {
        assert_eq!(rgb.len(), w * h * 3, "rgb buffer size mismatch");
        let mut cover = vec![false; w * h];
        let mut lum = vec![0u8; w * h];
        for i in 0..w * h {
            let r = rgb[i * 3] as u32;
            let g = rgb[i * 3 + 1] as u32;
            let b = rgb[i * 3 + 2] as u32;
            let l = ((r * 30 + g * 59 + b * 11) / 100) as u8;
            lum[i] = l;
            cover[i] = (l as i32 - sky_lum as i32).unsigned_abs() as u8 > sky_tol;
        }
        Self { w, h, cover, lum }
    }

    /// Fraction of pixels marked as terrain.
    pub fn coverage(&self) -> f64 {
        let c = self.cover.iter().filter(|&&b| b).count();
        c as f64 / (self.w * self.h) as f64
    }
}

#[inline]
fn solid(g: &WorldGenerator, x: f64, y: f64, z: f64) -> bool {
    let (xi, yi, zi) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    g.density(xi, yi, zi) > anchors::DENSITY_THRESHOLD && !g.is_cave(xi, yi, zi)
}

fn normalize3(v: [f64; 3]) -> [f64; 3] {
    let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if m == 0.0 { v } else { [v[0] / m, v[1] / m, v[2] / m] }
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Render a CPU reference image by raymarching the world occupancy. Pure +
/// deterministic for a given `(seed, camera, w, h, max_dist, step)`.
pub fn cpu_reference(
    g: &WorldGenerator,
    cam: &Camera,
    w: usize,
    h: usize,
    max_dist: f64,
    step: f64,
) -> RefImage {
    let (cp, sp) = (cam.pitch.cos(), cam.pitch.sin());
    let (cy, sy) = (cam.yaw.cos(), cam.yaw.sin());
    let forward = normalize3([cp * cy, sp, cp * sy]);
    let world_up = [0.0, 1.0, 0.0];
    let right = normalize3(cross(forward, world_up));
    let up = cross(right, forward);
    let aspect = w as f64 / h as f64;
    let tan_half = (cam.fov_deg.to_radians() * 0.5).tan();

    let mut cover = vec![false; w * h];
    let mut lum = vec![0u8; w * h];
    let sea = g.sea_level_at(cam.pos[0] as i32, cam.pos[2] as i32);

    for py in 0..h {
        // ndc y: +1 top, -1 bottom
        let ndc_y = 1.0 - 2.0 * (py as f64 + 0.5) / h as f64;
        for px in 0..w {
            let ndc_x = (2.0 * (px as f64 + 0.5) / w as f64 - 1.0) * aspect;
            let dir = normalize3([
                forward[0] + ndc_x * tan_half * right[0] + ndc_y * tan_half * up[0],
                forward[1] + ndc_x * tan_half * right[1] + ndc_y * tan_half * up[1],
                forward[2] + ndc_x * tan_half * right[2] + ndc_y * tan_half * up[2],
            ]);
            let i = py * w + px;
            let mut t = 1.0;
            let mut hit = false;
            while t < max_dist {
                let p = [
                    cam.pos[0] + dir[0] * t,
                    cam.pos[1] + dir[1] * t,
                    cam.pos[2] + dir[2] * t,
                ];
                if p[1] < -8.0 || p[1] > 400.0 {
                    // ray left the vertical world band; stop early if heading away
                    if (p[1] < -8.0 && dir[1] < 0.0) || (p[1] > 400.0 && dir[1] > 0.0) {
                        break;
                    }
                }
                if solid(g, p[0], p[1], p[2]) {
                    hit = true;
                    // Coarse shade: brighter when higher than sea + nearer.
                    let depth_fade = (1.0 - (t / max_dist)).clamp(0.0, 1.0);
                    let height_term = ((p[1] - sea) * 1.5).clamp(-60.0, 60.0);
                    let l = (120.0 + height_term + depth_fade * 60.0).clamp(20.0, 255.0);
                    lum[i] = l as u8;
                    cover[i] = true;
                    break;
                }
                t += step;
            }
            if !hit {
                lum[i] = SKY_LUM; // flat sky
            }
        }
    }
    RefImage { w, h, cover, lum }
}

/// The flat luminance the CPU reference paints for sky pixels. Exposed so the
/// GPU-capture coverage extraction can use a matching sky baseline.
pub const SKY_LUM: u8 = 200;

/// Result of comparing two images. `coverage_mismatch_frac` is the primary
/// signal (fraction of pixels where one says terrain and the other sky).
#[derive(Clone, Copy, Debug)]
pub struct ParityReport {
    pub pixels: usize,
    pub coverage_mismatch_frac: f64,
    pub lum_mean_abs: f64,
    pub lum_max_abs: u16,
}

/// Compare two equally-sized images. Luminance diff is computed only over
/// pixels both consider terrain (so sky-vs-terrain shows up in coverage, not
/// as a huge luma delta that drowns the signal).
pub fn image_diff(a: &RefImage, b: &RefImage) -> ParityReport {
    assert_eq!((a.w, a.h), (b.w, b.h), "image dimensions differ");
    let n = a.w * a.h;
    let mut cover_mismatch = 0usize;
    let mut lum_sum = 0u64;
    let mut lum_max = 0u16;
    let mut both_terrain = 0usize;
    for i in 0..n {
        if a.cover[i] != b.cover[i] {
            cover_mismatch += 1;
        } else if a.cover[i] {
            let d = (a.lum[i] as i32 - b.lum[i] as i32).unsigned_abs() as u16;
            lum_sum += d as u64;
            if d > lum_max { lum_max = d; }
            both_terrain += 1;
        }
    }
    ParityReport {
        pixels: n,
        coverage_mismatch_frac: cover_mismatch as f64 / n as f64,
        lum_mean_abs: if both_terrain > 0 { lum_sum as f64 / both_terrain as f64 } else { 0.0 },
        lum_max_abs: lum_max,
    }
}

/// Fail-fast parity assertion. Returns `Err(report)` if the coverage mismatch
/// or mean luminance error exceeds the thresholds — the caller turns that into
/// a test failure. This is the "partial test": passing doesn't prove pixel
/// perfection, but failing means something structural diverged and is worth a
/// look.
pub fn assert_parity(
    reference: &RefImage,
    captured: &RefImage,
    max_coverage_mismatch: f64,
    max_lum_mean: f64,
) -> Result<ParityReport, ParityReport> {
    let r = image_diff(reference, captured);
    if r.coverage_mismatch_frac > max_coverage_mismatch || r.lum_mean_abs > max_lum_mean {
        Err(r)
    } else {
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen() -> WorldGenerator { WorldGenerator::new(42) }

    #[test]
    fn reference_is_deterministic() {
        let g = gen();
        let cam = Camera::default();
        let a = cpu_reference(&g, &cam, 48, 32, 320.0, 1.0);
        let b = cpu_reference(&g, &cam, 48, 32, 320.0, 1.0);
        assert_eq!(a.cover, b.cover, "coverage not deterministic");
        assert_eq!(a.lum, b.lum, "luminance not deterministic");
    }

    #[test]
    fn terrain_camera_sees_substantial_ground() {
        // A downward-ish camera over origin terrain must hit a healthy amount
        // of ground. This guards against GROSS worldgen breakage — inverted
        // density, an all-air world, the surface generating far below the
        // camera — where coverage collapses toward sky. NOTE (measured): it
        // does NOT catch cave over-carving, because a top-down ray still hits
        // the first solid per column regardless of internal cave volume
        // (coverage was 0.84 at both the correct 0.70 and the buggy 0.58 cave
        // thresholds). Caves are guarded by `cave_volume_fraction_is_sane` and
        // `surface_y_finds_terrain`, not here.
        let g = gen();
        let cov = cpu_reference(&g, &Camera::default(), 64, 48, 360.0, 1.0).coverage();
        assert!(
            cov > 0.30,
            "terrain coverage {:.1}% too low — worldgen grossly broken (inverted density / all-air?)",
            cov * 100.0
        );
    }

    #[test]
    fn identical_images_have_zero_mismatch() {
        let g = gen();
        let img = cpu_reference(&g, &Camera::default(), 48, 32, 320.0, 1.0);
        let r = image_diff(&img, &img);
        assert_eq!(r.coverage_mismatch_frac, 0.0);
        assert_eq!(r.lum_mean_abs, 0.0);
        assert!(assert_parity(&img, &img, 0.02, 8.0).is_ok());
    }

    #[test]
    fn a_hole_is_detected_and_fails_fast() {
        // Punch a block of terrain pixels to sky (simulating a missing-mesh
        // hole in a GPU capture) and confirm parity fails.
        let g = gen();
        let reference = cpu_reference(&g, &Camera::default(), 48, 32, 320.0, 1.0);
        let mut holed = reference.clone();
        let mut punched = 0;
        for i in 0..holed.cover.len() {
            if holed.cover[i] && punched < (holed.w * holed.h) / 10 {
                holed.cover[i] = false;
                holed.lum[i] = SKY_LUM;
                punched += 1;
            }
        }
        assert!(punched > 0, "test setup: no terrain to punch");
        let res = assert_parity(&reference, &holed, 0.02, 8.0);
        assert!(res.is_err(), "a 10% hole should trip the parity gate");
    }

    #[test]
    fn rgb8_coverage_extraction_roundtrips() {
        // A buffer that is half sky-colored, half dark should extract ~50%
        // coverage.
        let (w, h) = (10usize, 10usize);
        let mut rgb = vec![0u8; w * h * 3];
        for i in 0..w * h {
            let v = if i < w * h / 2 { SKY_LUM } else { 40 };
            rgb[i * 3] = v; rgb[i * 3 + 1] = v; rgb[i * 3 + 2] = v;
        }
        let img = RefImage::from_rgb8_coverage(&rgb, w, h, SKY_LUM, 16);
        assert!((img.coverage() - 0.5).abs() < 0.01, "coverage {:.2}", img.coverage());
    }

    /// The GPU octree mesh shader colors blocks with its own `color_for_block`
    /// switch; any id it doesn't handle renders bright magenta (a debug
    /// fallback). This regressed visibly: BirchLeaves/Flower fell through and
    /// whole canopies rendered magenta under GPU_MESHER=2. This test parses the
    /// shader's handled ids and asserts every block the CPU mesher gives a
    /// natural color also has a shader entry, so foliage can't silently go
    /// magenta again. (It checks coverage, not exact rgb — the shader values
    /// are the flat/no-climate approximations the CPU mesher tints at runtime.)
    #[test]
    fn gpu_shader_colors_every_cpu_colored_block() {
        const SHADER: &str = include_str!("shaders/chunk_mesh_octree.comp");
        // Collect ids the shader handles: `if (block_id == Nu) return ...;`.
        let mut handled = std::collections::HashSet::new();
        for line in SHADER.lines() {
            if let Some(pos) = line.find("block_id == ") {
                let rest = &line[pos + "block_id == ".len()..];
                let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(id) = num.parse::<u32>() {
                    handled.insert(id);
                }
            }
        }
        assert!(handled.len() >= 12, "shader parse found too few ids: {}", handled.len());

        // Every BlockId that the CPU mesher colors must be in the shader too.
        // Air(0)/Water(9) are excluded: air isn't meshed, water has a dedicated
        // translucent path, neither flows through color_for_block.
        use crate::world::blocks::BlockId;
        let cpu_colored: &[(u32, BlockId)] = &[
            (1, BlockId::Stone), (2, BlockId::Grass), (3, BlockId::Dirt),
            (4, BlockId::Cobblestone), (7, BlockId::Bedrock), (11, BlockId::Lava),
            (12, BlockId::Sand), (13, BlockId::Gravel), (14, BlockId::GoldOre),
            (15, BlockId::IronOre), (16, BlockId::CoalOre), (17, BlockId::OakLog),
            (18, BlockId::OakLeaves), (19, BlockId::BirchLog), (20, BlockId::BirchLeaves),
            (21, BlockId::Cactus), (22, BlockId::Clay), (23, BlockId::Moss),
            (24, BlockId::RedSandstone), (25, BlockId::Flower), (26, BlockId::TallGrass),
            (27, BlockId::Fern), (28, BlockId::DeadBush), (29, BlockId::Vine),
            (30, BlockId::Dripstone), (31, BlockId::Ice), (32, BlockId::Snow),
            (33, BlockId::Mushroom), (34, BlockId::Cobweb), (35, BlockId::Lichen),
            (36, BlockId::Limestone), (37, BlockId::Marble), (38, BlockId::Basalt),
            (39, BlockId::Tuff), (40, BlockId::Granite), (41, BlockId::Slate),
            (42, BlockId::Calcite), (43, BlockId::Andesite), (44, BlockId::Schist),
            (45, BlockId::Bone), (46, BlockId::Amber), (47, BlockId::Crystal),
            (56, BlockId::DiamondOre), (80, BlockId::SnowBlock),
        ];
        for &(id, block) in cpu_colored {
            // Guard the id↔BlockId mapping stays truthful as the enum evolves.
            assert_eq!(block as u32, id, "BlockId discriminant for {block:?} drifted from {id}");
            assert!(
                handled.contains(&id),
                "GPU shader has no color for {block:?} (id {id}); it would render \
                 magenta under GPU_MESHER=2. Add it to color_for_block in \
                 chunk_mesh_octree.comp.",
            );
        }
    }
}
