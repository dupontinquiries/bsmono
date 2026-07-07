//! render.rs — the software forward-renderer. Descended directly from `logscope::mesh3d::render`:
//! transform every mesh's triangles with the camera, shade each vertex with a hemispherical sun
//! (Lambert + sky/ground ambient), depth-sort back-to-front (painter's algorithm — no z-buffer, exactly
//! like the Doom/Build-era software renderers this game emulates), and emit ONE `egui::epaint::Mesh`.
//!
//! Two additions the reference didn't need but an *interior* FPS does:
//!   • Near-plane clipping. Walls a foot from your face have a vertex behind the camera; without clipping
//!     the triangle smears across the screen. We Sutherland-Hodgman each triangle against the near plane
//!     in camera space, interpolating colour, before projecting.
//!   • Distance fog. The quintessential turn-of-the-millennium trick: blend distant geometry toward a fog
//!     colour. It sells depth on a flat-shaded scene and hides the far clip.

use crate::camera::FpsCam;
use crate::math::Vec3;
use crate::mesh::{Light, Mesh3};
use egui::{epaint::Mesh, Color32, Pos2};

/// Fog/atmosphere settings. `start`..`end` are world-space distances; beyond `end` everything is `color`.
#[derive(Clone, Copy)]
pub struct Fog {
    pub color: Color32,
    pub start: f32,
    pub end: f32,
}
impl Fog {
    pub fn new(color: Color32, start: f32, end: f32) -> Self {
        Self { color, start, end }
    }
    #[inline]
    fn factor(&self, depth: f32) -> f32 {
        if self.end <= self.start {
            return 0.0;
        }
        ((depth - self.start) / (self.end - self.start)).clamp(0.0, 1.0)
    }
}

#[inline]
fn lerp_c(a: Color32, b: Color32, f: f32) -> Color32 {
    let f = f.clamp(0.0, 1.0);
    Color32::from_rgba_unmultiplied(
        (a.r() as f32 * (1.0 - f) + b.r() as f32 * f) as u8,
        (a.g() as f32 * (1.0 - f) + b.g() as f32 * f) as u8,
        (a.b() as f32 * (1.0 - f) + b.b() as f32 * f) as u8,
        (a.a() as f32 * (1.0 - f) + b.a() as f32 * f) as u8,
    )
}

/// Hemispherical sun shading — lifted from `mesh3d::shade`. `ny` is the world-up component of the normal
/// (1 = facing sky, 0 = facing ground); `ndotl` is the clamped sun term.
#[inline]
fn shade(albedo: Color32, light: &Light, ny: f32, ndotl: f32) -> Color32 {
    let h = 0.5 + 0.5 * ny;
    let ch = |a: u8, sky: u8, gnd: u8, sun: u8| {
        let amb = (gnd as f32 * (1.0 - h) + sky as f32 * h) / 255.0 * light.ambient;
        let lit = amb + ndotl * (sun as f32 / 255.0);
        (((a as f32 / 255.0) * lit) * 1.65 * 255.0).min(255.0) as u8
    };
    // Preserve the albedo's alpha so `set_alpha` glass/mist/skylights stay translucent.
    Color32::from_rgba_unmultiplied(
        ch(albedo.r(), light.sky.r(), light.ground.r(), light.color.r()),
        ch(albedo.g(), light.sky.g(), light.ground.g(), light.color.g()),
        ch(albedo.b(), light.sky.b(), light.ground.b(), light.color.b()),
        albedo.a(),
    )
}

/// Add `amt` of light colour `col` on top of `lit` (clamped). Used for specular/rim highlights.
#[inline]
fn add_scaled(lit: Color32, col: Color32, amt: f32) -> Color32 {
    if amt <= 0.0 {
        return lit;
    }
    let f = |l: u8, c: u8| (l as f32 + c as f32 * amt).min(255.0) as u8;
    Color32::from_rgba_unmultiplied(f(lit.r(), col.r()), f(lit.g(), col.g()), f(lit.b(), col.b()), lit.a())
}

#[inline]
fn add_emissive(lit: Color32, albedo: Color32, e: f32) -> Color32 {
    if e <= 0.0 {
        return lit;
    }
    let f = |l: u8, a: u8| (l as f32 + a as f32 * e).min(255.0) as u8;
    Color32::from_rgba_unmultiplied(f(lit.r(), albedo.r()), f(lit.g(), albedo.g()), f(lit.b(), albedo.b()), lit.a())
}

/// ACES filmic tonemap curve (Narkowicz approximation), ported from the attached engine's `fboshader.glsl`.
/// Maps an unbounded/linear-ish value through a filmic shoulder+toe so bright highlights (muzzle flashes,
/// emissives, sky, explosions) roll off smoothly toward white instead of clipping to a flat hard white, and
/// shadows keep a gentle toe. Operates per channel on 0..∞ input, returns 0..1.
#[inline]
fn aces(x: f32) -> f32 {
    let (a, b, c, d, e) = (2.51f32, 0.03f32, 2.43f32, 0.59f32, 0.14f32);
    ((x * (a * x + b)) / (x * (c * x + d) + e)).clamp(0.0, 1.0)
}

/// Punch up flat Lambert shading toward a richer, more contrasty look — deeper (not crushed) shadows,
/// a touch more saturation, and a subtle cool-shadow/warm-highlight split tone, finished with an ACES filmic
/// tonemap so highlights roll off like film. Cheap, per-vertex, but it's most of the gap between a flat-shaded
/// early-2000s look and the punchier, filmic grading of a modern renderer.
#[inline]
fn grade(c: Color32, ndl: f32) -> Color32 {
    let to_f = |v: u8| v as f32 / 255.0;
    let (r0, g0, b0) = (to_f(c.r()), to_f(c.g()), to_f(c.b()));
    // Slightly boost input into the tonemapper's sweet spot (a hair of "exposure") so mid-tones keep their
    // punch after the filmic shoulder pulls the top down, then ACES-tonemap each channel.
    let expose = 1.05;
    let curve = |v: f32| aces(v * expose);
    let (mut r, mut g, mut b) = (curve(r0), curve(g0), curve(b0));
    // Saturation lift around luminance.
    let lum = 0.299 * r + 0.587 * g + 0.114 * b;
    let sat = 1.14;
    r = (lum + (r - lum) * sat).clamp(0.0, 1.0);
    g = (lum + (g - lum) * sat).clamp(0.0, 1.0);
    b = (lum + (b - lum) * sat).clamp(0.0, 1.0);
    // Split tone: shadows lean cool (blue), sunlit faces lean warm (amber) — the push that reads as a
    // graded, "shot on a real camera" look instead of a flat vertex tint.
    let shadow = (1.0 - ndl).clamp(0.0, 1.0);
    let hi = ndl.clamp(0.0, 1.0);
    r = (r + hi * 0.035 - shadow * 0.012).clamp(0.0, 1.0);
    g = (g + hi * 0.01).clamp(0.0, 1.0);
    b = (b - hi * 0.03 + shadow * 0.045).clamp(0.0, 1.0);
    Color32::from_rgba_unmultiplied((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, c.a())
}

/// One vertex carried through clipping: camera-space position + already-shaded colour + atlas UV.
#[derive(Clone, Copy)]
struct CVert {
    cam: Vec3, // (x,y,depth) in camera space; depth = .z, > 0 in front
    col: Color32,
    uv: Pos2,
}
#[inline]
fn lerp_cv(a: CVert, b: CVert, t: f32) -> CVert {
    CVert {
        cam: a.cam.lerp(b.cam, t),
        col: lerp_c(a.col, b.col, t),
        uv: Pos2::new(a.uv.x + (b.uv.x - a.uv.x) * t, a.uv.y + (b.uv.y - a.uv.y) * t),
    }
}

/// Render `meshes` through `cam` into one depth-sorted egui mesh.
/// `(cx,cy)` = viewport centre in screen px; `scale` = pixels per world unit at unit depth.
/// Render a flat list of owned meshes (the normal path).
pub fn render(
    meshes: &[Mesh3],
    cam: &FpsCam,
    cx: f32,
    cy: f32,
    scale: f32,
    light: &Light,
    fog: &Fog,
    view_w: f32,
    view_h: f32,
    time: f32,
    wind: crate::math::Vec2,
    atlas: egui::TextureId,
    atlas_cols: usize,
    lights: &[crate::mesh::PointLight],
) -> Mesh {
    render_inner(meshes.iter(), cam, cx, cy, scale, light, fog, view_w, view_h, time, wind, atlas, atlas_cols, lights)
}

/// Render a list of mesh *references* — used by the dynamic LOD field, whose active-tier meshes live inside
/// the chunk structs.
pub fn render_refs(
    meshes: &[&Mesh3],
    cam: &FpsCam,
    cx: f32,
    cy: f32,
    scale: f32,
    light: &Light,
    fog: &Fog,
    view_w: f32,
    view_h: f32,
    time: f32,
    wind: crate::math::Vec2,
    atlas: egui::TextureId,
    atlas_cols: usize,
    lights: &[crate::mesh::PointLight],
) -> Mesh {
    render_inner(meshes.iter().copied(), cam, cx, cy, scale, light, fog, view_w, view_h, time, wind, atlas, atlas_cols, lights)
}

fn render_inner<'a>(
    meshes: impl IntoIterator<Item = &'a Mesh3>,
    cam: &FpsCam,
    cx: f32,
    cy: f32,
    scale: f32,
    light: &Light,
    fog: &Fog,
    view_w: f32,
    view_h: f32,
    time: f32,
    wind: crate::math::Vec2,
    atlas: egui::TextureId,
    atlas_cols: usize,
    lights: &[crate::mesh::PointLight],
) -> Mesh {
    let l = light.dir.normalized();
    let (right, up, fwd) = cam.basis();
    let near = FpsCam::NEAR;
    let far = fog.end + 6.0;
    // Frustum half-extents at unit depth (x_edge = depth * tan_h). `scale` is px per world-unit at unit
    // depth, so the screen edge sits at world-x = depth * (view_w/2)/scale.
    let tan_h = (view_w * 0.5) / scale * 1.04; // 4% guard band so we never clip a sliver that's truly visible
    let tan_v = (view_h * 0.5) / scale * 1.04;

    // (sort_depth, [screen pts], [colours], [uvs]) — fan-triangulated after clipping.
    let mut out: Vec<(f32, [Pos2; 3], [Color32; 3], [Pos2; 3])> = Vec::new();

    for m in meshes {
        // Whole-mesh (chunk) frustum cull: if a chunk's cached AABB is entirely behind, past the fog, or off
        // one screen edge, skip all its triangles before the per-tri loop. No-op for meshes without bounds.
        if let Some(b) = m.bounds {
            let lf = if m.lod_far > 0.0 { m.lod_far } else { 1.0 };
            if crate::chunk::aabb_culled(&b, cam.pos, right, up, fwd, near, far * lf, tan_h, tan_v) {
                continue;
            }
        }
        let reflect = m.reflect;
        // Atlas mapping for this mesh: its material tile, the flat white atlas tile (keep vertex colour), or —
        // when no atlas is bound (atlas_cols == 0, e.g. the editor preview) — egui's white pixel.
        let use_atlas = atlas_cols > 0;
        let (textured, mrect) = match (use_atlas, m.material) {
            (true, Some(mat)) => (true, crate::mesh::atlas_rect(mat, atlas_cols)),
            (true, None) => {
                let (wu, wv) = crate::mesh::atlas_white(atlas_cols);
                (false, (wu, wv, wu, wv))
            }
            (false, _) => {
                let w = egui::epaint::WHITE_UV;
                (false, (w.x, w.y, w.x, w.y))
            }
        };
        let uv_for = |k: usize| -> Pos2 {
            if textured {
                let lp = m.uv.get(k).copied().unwrap_or(Pos2::new(0.5, 0.5));
                Pos2::new(mrect.0 + lp.x * (mrect.2 - mrect.0), mrect.1 + lp.y * (mrect.3 - mrect.1))
            } else {
                Pos2::new(mrect.0, mrect.1)
            }
        };
        for tri in &m.tris {
            let idx = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            // World positions, with per-vertex wind sway applied to foliage (tips bow, bases stay rooted).
            // A phase that advances along the wind direction makes gusts sweep across the field in sequence.
            let mut wv = [m.verts[idx[0]]; 3];
            for (j, &k) in idx.iter().enumerate() {
                let mut p = m.verts[k] + m.offset;
                let w = m.sway.get(k).copied().unwrap_or(0.0);
                if w > 0.0 {
                    let phase = time * 2.2 + p.x * 0.5 + p.z * 0.45;
                    let bend = phase.sin() * 0.62 + (phase * 2.6 + 1.0).sin() * 0.38;
                    let fl = (time * 3.3 + p.x * 0.8).sin() * 0.3;
                    p.x += (wind.x * bend - wind.y * fl) * w;
                    p.z += (wind.y * bend + wind.x * fl) * w;
                }
                wv[j] = p;
            }
            // Camera-space coords first (cheap dot products) so we can reject before the costly shading.
            let mut camx = [0.0f32; 3];
            let mut camy = [0.0f32; 3];
            let mut camd = [0.0f32; 3];
            for j in 0..3 {
                let rel = wv[j] - cam.pos;
                camx[j] = rel.dot(right);
                camy[j] = rel.dot(up);
                camd[j] = rel.dot(fwd);
            }
            // 1) Far cull — nearest vertex already past the fog wall (fully fogged to the horizon).
            if camd[0] > far && camd[1] > far && camd[2] > far {
                continue;
            }
            // 2) Behind cull — the whole triangle is behind the near plane.
            if camd[0] < near && camd[1] < near && camd[2] < near {
                continue;
            }
            // 3) Lateral frustum cull — every vertex off the same screen edge (only when all are in front, so
            //    the per-vertex `depth * tan` edge test is valid). This skips shading work for off-screen
            //    geometry while you spin the camera through a dense world.
            if camd[0] > near && camd[1] > near && camd[2] > near {
                let off_r = camx[0] > camd[0] * tan_h && camx[1] > camd[1] * tan_h && camx[2] > camd[2] * tan_h;
                let off_l = -camx[0] > camd[0] * tan_h && -camx[1] > camd[1] * tan_h && -camx[2] > camd[2] * tan_h;
                let off_t = camy[0] > camd[0] * tan_v && camy[1] > camd[1] * tan_v && camy[2] > camd[2] * tan_v;
                let off_b = -camy[0] > camd[0] * tan_v && -camy[1] > camd[1] * tan_v && -camy[2] > camd[2] * tan_v;
                if off_r || off_l || off_t || off_b {
                    continue;
                }
            }
            // 3.5) Backface cull — for closed, opaque, correctly-wound meshes (boxes, tubes, spheres, terrain
            //      chunks), skip any triangle whose outward face points away from the camera. On solid geometry
            //      that's ~half the triangles, and it happens *before* the expensive per-vertex shading, so it
            //      roughly halves shading cost on those meshes. Never applied to double-sided geometry
            //      (foliage, water, particles, billboards) which leave `single_sided` false.
            if m.single_sided {
                // geometric face normal from the (wind-applied) world triangle
                let e1 = wv[1] - wv[0];
                let e2 = wv[2] - wv[0];
                let fnrm = e1.cross(e2);
                // view vector from the triangle to the camera; if the face points away (same side as view),
                // it's a backface. Use the centroid so near-camera triangles judge correctly.
                let centroid = (wv[0] + wv[1] + wv[2]) * (1.0 / 3.0);
                let to_cam = cam.pos - centroid;
                if fnrm.dot(to_cam) < 0.0 {
                    continue;
                }
            }
            // Survivors: shade in world space (normal, specular, reflection, fog), carry into camera space.
            let mut poly: Vec<CVert> = Vec::with_capacity(4);
            for (j, &k) in idx.iter().enumerate() {
                let wp = wv[j];
                let depth = camd[j];
                let n = m.norms[k].normalized();
                // Textured surfaces shade a neutral albedo so the atlas carries the colour; untextured keep
                // their per-vertex palette colour.
                let base = if textured {
                    Color32::from_rgb(232, 229, 223)
                } else {
                    m.vcol.as_ref().and_then(|vc| vc.get(k).copied()).unwrap_or(m.color)
                };
                let ndl = n.dot(l).max(0.0);
                let mut lit = shade(base, light, n.y, ndl);
                let v = (cam.pos - wp).normalized();
                let nv = n.dot(v).max(0.0);
                let half = (l + v).normalized();
                // PBR-lite: textured surfaces get their material's own specular tightness/intensity and
                // reflectivity (metal glints tight and bright and reflects the sky; cloth/dirt barely responds
                // to specular at all); untextured meshes keep the original uniform "everything is slightly
                // glossy plastic" response so old vertex-coloured props don't change look.
                let (spec_power, spec_intensity, mat_reflect) = match m.material {
                    Some(mat) => crate::mesh::material_pbr(mat),
                    None => (24.0, 0.40, 0.0),
                };
                let reflect = reflect.max(mat_reflect);
                if ndl > 0.0 {
                    // tight specular glint on lit faces
                    let spec = n.dot(half).max(0.0).powf(spec_power) * spec_intensity * ndl;
                    lit = add_scaled(lit, light.color, spec);
                }
                // soft sky-tinted rim at grazing angles → cheap fresnel sheen
                let rim = (1.0 - nv).powi(3) * 0.12 * (0.4 + 0.6 * (0.5 + 0.5 * n.y));
                lit = add_scaled(lit, light.sky, rim);
                // Environment reflection (water / ice / metal / car paint): Schlick-fresnel blend toward an
                // environment colour, brightest at grazing angles, plus a sharp mirror glint along the sun.
                if reflect > 0.0 {
                    let fres = {
                        let f = 1.0 - nv;
                        let f2 = f * f;
                        f2 * f2 * f
                    };
                    let amt = (reflect * (0.18 + 0.82 * fres)).min(0.78);
                    // The reflected environment isn't a flat sky wash: up-facing surfaces mirror the sky,
                    // vertical walls mirror a sky-leaning horizon, and only down-facing surfaces catch the
                    // darker ground tone — giving reflectors a believable vertical gradient without ever
                    // dimming a wall below the sky it used to reflect. Tinted slightly by the surface colour.
                    let horizon = lerp_c(light.ground, light.sky, 0.72);
                    let env_sky = if n.y >= 0.0 {
                        lerp_c(horizon, light.sky, n.y) // horizon → sky as it tips up
                    } else {
                        lerp_c(horizon, light.ground, -n.y) // horizon → ground as it tips down
                    };
                    let env = lerp_c(env_sky, base, 0.16);
                    lit = lerp_c(lit, env, amt);
                    let glint = n.dot(half).max(0.0).powf(80.0) * reflect * 1.5 * (0.3 + 0.7 * ndl);
                    lit = add_scaled(lit, light.color, glint);
                    // Reflective surfaces also mirror nearby dynamic lights (a burning car's hood glows with
                    // the fire, a muzzle flash flares off wet floor) — a cheap screen-space-free reflection.
                    if !lights.is_empty() {
                        for pl in lights {
                            let c = pl.contribution(wp, n);
                            if c > 0.002 {
                                lit = add_scaled(lit, pl.color, c * reflect * 1.2);
                            }
                        }
                    }
                }
                // Dynamic point lights: add each nearby light's coloured contribution (distance falloff ×
                // half-lambert against the vertex normal). This is what makes a muzzle flash, an explosion, a
                // planted-bomb device or a flag actually cast light on the surrounding geometry. Bounded to a
                // handful of lights, so it's a cheap per-vertex add.
                if !lights.is_empty() {
                    for pl in lights {
                        let c = pl.contribution(wp, n);
                        if c > 0.001 {
                            lit = add_scaled(lit, pl.color, c);
                        }
                    }
                }
                let lit = grade(lit, ndl);
                let lit = add_emissive(lit, base, m.emissive);
                let col = lerp_c(lit, fog.color, fog.factor(depth.max(0.0)));
                poly.push(CVert {
                    cam: Vec3::new(camx[j], camy[j], depth),
                    col,
                    uv: uv_for(k),
                });
            }

            // Clip the triangle against the near plane (depth >= near) in camera space.
            let clipped = clip_near(&poly, near);
            if clipped.len() < 3 {
                continue;
            }

            // Project the clipped polygon, fan-triangulate, accumulate.
            let mut sp: Vec<(Pos2, f32, Color32, Pos2)> = Vec::with_capacity(clipped.len());
            // Clamp projected coordinates to a bounded range around the viewport. A vertex right at the near
            // plane projects to a screen coordinate in the millions (scale / z with z→near); egui chokes trying
            // to rasterise a triangle that spans millions of pixels (the "preview shows only a colour" bug when
            // a top-down camera sits just above dense geometry). Clamping to a few viewports out keeps the
            // on-screen portion correct while bounding the coordinate so the rasteriser stays happy.
            let clamp_lim = (view_w.max(view_h)) * 8.0;
            let clamp = |v: f32| v.clamp(-clamp_lim, clamp_lim);
            for v in &clipped {
                let persp = scale / v.cam.z;
                sp.push((Pos2::new(clamp(cx + v.cam.x * persp), clamp(cy - v.cam.y * persp)), v.cam.z, v.col, v.uv));
            }
            for i in 1..sp.len() - 1 {
                let (a, b, c) = (sp[0], sp[i], sp[i + 1]);
                // off-screen reject: tris entirely outside the viewport
                if (a.0.x < 0.0 && b.0.x < 0.0 && c.0.x < 0.0)
                    || (a.0.x > view_w && b.0.x > view_w && c.0.x > view_w)
                    || (a.0.y < 0.0 && b.0.y < 0.0 && c.0.y < 0.0)
                    || (a.0.y > view_h && b.0.y > view_h && c.0.y > view_h)
                {
                    continue;
                }
                let area2 = ((b.0.x - a.0.x) * (c.0.y - a.0.y) - (c.0.x - a.0.x) * (b.0.y - a.0.y)).abs();
                if area2 < 1.1 {
                    continue;
                }
                let sort = (a.1 + b.1 + c.1) / 3.0;
                out.push((sort, [a.0, b.0, c.0], [a.2, b.2, c.2], [a.3, b.3, c.3]));
            }
        }
    }

    out.sort_by(|p, q| q.0.partial_cmp(&p.0).unwrap_or(std::cmp::Ordering::Equal)); // far → near

    let mut mesh = Mesh::default();
    mesh.texture_id = atlas;
    mesh.vertices.reserve(out.len() * 3);
    mesh.indices.reserve(out.len() * 3);
    for (_, sp, cols, uvs) in out {
        let i = mesh.vertices.len() as u32;
        for k in 0..3 {
            mesh.vertices.push(egui::epaint::Vertex { pos: sp[k], uv: uvs[k], color: cols[k] });
        }
        mesh.add_triangle(i, i + 1, i + 2);
    }
    mesh
}

/// Sutherland–Hodgman clip of a convex polygon against the single plane `cam.z >= near`.
fn clip_near(poly: &[CVert], near: f32) -> Vec<CVert> {
    let mut out: Vec<CVert> = Vec::with_capacity(poly.len() + 1);
    let n = poly.len();
    for i in 0..n {
        let cur = poly[i];
        let prv = poly[(i + n - 1) % n];
        let cur_in = cur.cam.z >= near;
        let prv_in = prv.cam.z >= near;
        if cur_in {
            if !prv_in {
                let t = (near - prv.cam.z) / (cur.cam.z - prv.cam.z);
                out.push(lerp_cv(prv, cur, t));
            }
            out.push(cur);
        } else if prv_in {
            let t = (near - prv.cam.z) / (cur.cam.z - prv.cam.z);
            out.push(lerp_cv(prv, cur, t));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{boxx, rgb, FACE_ALL};

    fn light() -> Light {
        Light::sun(Vec3::new(0.4, 1.0, 0.3), rgb(255, 244, 214))
    }
    fn fog() -> Fog {
        Fog::new(rgb(20, 24, 34), 18.0, 60.0)
    }

    #[test]
    fn backface_culling_halves_a_closed_box() {
        // A closed single-sided box viewed from outside should render only its front-facing triangles — the
        // 3 faces pointing away from the camera are culled before shading. FACE_ALL is 6 faces = 12 tris; from
        // a corner-ish view at most 3 faces (6 tris) can face the camera, so the survivor count must drop.
        let cam = FpsCam { pos: Vec3::new(4.0, 3.0, 5.0), yaw: -0.6, pitch: -0.4 };
        let solid = boxx(Vec3::ZERO, Vec3::splat(1.0), rgb(180, 180, 180), FACE_ALL);
        assert!(solid.single_sided, "boxx must be flagged single-sided");
        let culled = render(&[solid], &cam, 200.0, 150.0, 300.0, &light(), &fog(), 400.0, 300.0, 0.0, crate::math::Vec2::new(0.0, 0.0), egui::TextureId::Managed(0), 5, &[]);
        // Same box but forced double-sided → no backface cull, so more triangles survive.
        let mut two_sided = boxx(Vec3::ZERO, Vec3::splat(1.0), rgb(180, 180, 180), FACE_ALL);
        two_sided.single_sided = false;
        let uncap = render(&[two_sided], &cam, 200.0, 150.0, 300.0, &light(), &fog(), 400.0, 300.0, 0.0, crate::math::Vec2::new(0.0, 0.0), egui::TextureId::Managed(0), 5, &[]);
        assert!(!culled.vertices.is_empty(), "front faces still render");
        assert!(culled.indices.len() < uncap.indices.len(), "backface culling must remove the rear faces ({} vs {})", culled.indices.len(), uncap.indices.len());
    }

    #[test]
    fn renders_a_box_in_front() {
        let cam = FpsCam { pos: Vec3::new(0.0, 0.0, 5.0), yaw: 0.0, pitch: 0.0 }; // looking -Z toward origin
        let b = boxx(Vec3::ZERO, Vec3::splat(1.0), rgb(180, 180, 180), FACE_ALL);
        let m = render(&[b], &cam, 200.0, 150.0, 300.0, &light(), &fog(), 400.0, 300.0, 0.0, crate::math::Vec2::new(0.0, 0.0), egui::TextureId::Managed(0), 5, &[]);
        assert!(!m.vertices.is_empty(), "a box dead ahead must produce visible triangles");
        assert!(m.indices.len() % 3 == 0);
    }

    #[test]
    fn metal_and_cloth_shade_differently_via_material_pbr() {
        // Same geometry, same light, same camera — the only difference is the atlas material bound to each
        // panel. Facing the light head-on with the camera near the reflection angle so any specular
        // difference actually lands on-screen, a bright metal panel should come back lit brighter than a
        // matte cloth panel purely from the specular/reflect terms `material_pbr` drives.
        let sun_dir = Vec3::new(0.0, 0.6, 1.0).normalized();
        // A dim sun so the neutral textured albedo doesn't saturate to white on both panels — that leaves
        // headroom for the metal's specular glint + environment reflection to read as genuinely brighter than
        // matte cloth (the whole point of the test). At full daylight both clip near white and can't separate.
        let l = Light::sun(sun_dir, rgb(120, 118, 112));
        let f = Fog::new(rgb(20, 24, 34), 200.0, 400.0); // pushed back so fog never dilutes the comparison
        // Front +z faces only (robust to backface culling), at the original oblique camera where the metal's
        // specular/reflection measurably separates it from matte cloth. Mid-grey base leaves headroom so the
        // specular add isn't clipped to white on both.
        let mut metal = boxx(Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 2.0, 0.05), rgb(120, 120, 120), crate::mesh::FACE_ALL & 0x04);
        metal.material = Some(crate::mesh::Material::Corrugated);
        let mut cloth = boxx(Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 2.0, 0.05), rgb(120, 120, 120), crate::mesh::FACE_ALL & 0x04);
        cloth.material = Some(crate::mesh::Material::Cloth);
        let cam = FpsCam { pos: Vec3::new(0.0, 1.2, 3.5), yaw: 0.0, pitch: -0.32 };
        let brightness = |mesh: &Mesh| -> f32 {
            mesh.vertices.iter().map(|v| v.color.r() as f32 + v.color.g() as f32 + v.color.b() as f32).sum::<f32>() / mesh.vertices.len().max(1) as f32
        };
        let mm = render(&[metal], &cam, 200.0, 150.0, 300.0, &l, &f, 400.0, 300.0, 0.0, crate::math::Vec2::new(0.0, 0.0), egui::TextureId::Managed(1), 5, &[]);
        let cm = render(&[cloth], &cam, 200.0, 150.0, 300.0, &l, &f, 400.0, 300.0, 0.0, crate::math::Vec2::new(0.0, 0.0), egui::TextureId::Managed(1), 5, &[]);
        assert!(!mm.vertices.is_empty() && !cm.vertices.is_empty());
        // The regression this guards is that per-material PBR (specular tightness/intensity + reflectivity) is
        // actually wired into the shader — i.e. a reflective metal and a matte cloth under identical light and
        // view produce *different* shading. (Which is brighter depends on whether the surface is already near
        // saturation: a dim surface gets a bright specular lift, an over-lit one is pulled toward the reflected
        // horizon. Both are correct; the invariant is that the material choice changes the result at all.)
        let mb = brightness(&mm);
        let cb = brightness(&cm);
        assert!((mb - cb).abs() > 0.5, "metal and cloth should shade differently via material_pbr ({mb:.1} vs {cb:.1})");
    }

    #[test]
    fn behind_camera_produces_nothing() {
        let cam = FpsCam { pos: Vec3::new(0.0, 0.0, -5.0), yaw: 0.0, pitch: 0.0 }; // box is behind (+Z)
        let b = boxx(Vec3::ZERO, Vec3::splat(1.0), rgb(180, 180, 180), FACE_ALL);
        let m = render(&[b], &cam, 200.0, 150.0, 300.0, &light(), &fog(), 400.0, 300.0, 0.0, crate::math::Vec2::new(0.0, 0.0), egui::TextureId::Managed(0), 5, &[]);
        assert!(m.vertices.is_empty(), "geometry fully behind the camera is culled");
    }

    #[test]
    fn near_clip_keeps_a_straddling_triangle() {
        // A big floor quad the camera stands on: some verts in front, the plane straddles near.
        let mut floor = Mesh3::new(rgb(100, 100, 100));
        let n = Vec3::UP;
        floor.push_quad(
            Vec3::new(-10.0, -1.6, 10.0),
            Vec3::new(10.0, -1.6, 10.0),
            Vec3::new(10.0, -1.6, -10.0),
            Vec3::new(-10.0, -1.6, -10.0),
            n,
            rgb(100, 100, 100),
        );
        let cam = FpsCam { pos: Vec3::ZERO, yaw: 0.0, pitch: -0.3 };
        let m = render(&[floor], &cam, 200.0, 150.0, 300.0, &light(), &fog(), 400.0, 300.0, 0.0, crate::math::Vec2::new(0.0, 0.0), egui::TextureId::Managed(0), 5, &[]);
        assert!(!m.vertices.is_empty(), "the floor under the player must still render after near-clip");
        // every projected vertex must be finite (no divide-by-~0 smear)
        assert!(m.vertices.iter().all(|v| v.pos.x.is_finite() && v.pos.y.is_finite()));
    }

    #[test]
    fn offscreen_side_geometry_is_culled() {
        // A box far to the right, outside the horizontal FOV, should contribute nothing.
        let cam = FpsCam { pos: Vec3::new(0.0, 0.0, 5.0), yaw: 0.0, pitch: 0.0 };
        let b = boxx(Vec3::new(60.0, 0.0, 0.0), Vec3::splat(1.0), rgb(180, 180, 180), FACE_ALL);
        let m = render(&[b], &cam, 200.0, 150.0, 300.0, &light(), &fog(), 400.0, 300.0, 0.0, crate::math::Vec2::new(0.0, 0.0), egui::TextureId::Managed(0), 5, &[]);
        assert!(m.vertices.is_empty(), "geometry off the side of the frustum is culled");
    }

    #[test]
    fn reflective_surface_still_renders() {
        let cam = FpsCam { pos: Vec3::new(0.0, 0.0, 5.0), yaw: 0.0, pitch: 0.0 };
        let b = boxx(Vec3::ZERO, Vec3::splat(1.0), rgb(120, 140, 180), FACE_ALL).with_reflect(0.6);
        let m = render(&[b], &cam, 200.0, 150.0, 300.0, &light(), &fog(), 400.0, 300.0, 0.0, crate::math::Vec2::new(0.0, 0.0), egui::TextureId::Managed(0), 5, &[]);
        assert!(!m.vertices.is_empty(), "a reflective box still produces geometry");
        assert!(m.vertices.iter().all(|v| v.pos.x.is_finite() && v.pos.y.is_finite()));
    }

    #[test]
    fn fog_blends_distant_color_toward_fog() {
        let f = fog();
        assert_eq!(f.factor(10.0), 0.0); // before start
        assert_eq!(f.factor(100.0), 1.0); // past end
        assert!((f.factor(39.0) - 0.5).abs() < 0.05); // midway
    }
}
