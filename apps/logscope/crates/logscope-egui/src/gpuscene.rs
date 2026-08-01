//! gpuscene.rs — the GPU-ready core of the real-renderer framework, with NO GL/wgpu types so it can be
//! unit-tested. It turns the `mesh3d` scene into exactly what a shader pass needs:
//!   • `Camera` — a true 4×4 view-projection matrix that REPRODUCES `OrbitCam::project` pixel-for-pixel, so
//!     the GPU scene stays aligned with the 2D dots / runes / labels drawn through that projection.
//!   • `pack_sorted` — the scene flattened to an interleaved, back-to-front-sorted triangle soup
//!     (pos, normal, colour) ready to upload as a vertex buffer (painter's-algorithm order → no depth
//!     buffer needed for the convex shapes here).
//!   • `SHADER_VS` / `SHADER_FS` — the GLSL (per-fragment hemispherical sun + fresnel rim + distance fog),
//!     matching `mesh3d::shade` so CPU and GPU paths look the same. The version header is prepended by the
//!     GL backend (300 es vs 330) — see `glow_scene`.
//! The actual GL program/buffers/callback live in `glow_scene` (feature `gpu-scene`); this is its data.

use egui::Color32;
use crate::mesh3d::Mesh3;
use crate::spatial::{OrbitCam, Vec3};

// ── tiny column-major mat4 (what GLSL expects by default) ──────────────────────
type M4 = [f32; 16];
#[inline] fn idx(c: usize, r: usize) -> usize { c * 4 + r }
fn mul(a: &M4, b: &M4) -> M4 {
    let mut o = [0.0f32; 16];
    for c in 0..4 { for r in 0..4 {
        let mut s = 0.0;
        for k in 0..4 { s += a[idx(k, r)] * b[idx(c, k)]; }
        o[idx(c, r)] = s;
    }}
    o
}
fn translate(x: f32, y: f32, z: f32) -> M4 { let mut m = ident(); m[idx(3,0)] = x; m[idx(3,1)] = y; m[idx(3,2)] = z; m }
fn ident() -> M4 { let mut m = [0.0; 16]; m[0] = 1.0; m[5] = 1.0; m[10] = 1.0; m[15] = 1.0; m }
fn rot_y(a: f32) -> M4 { // matches OrbitCam yaw
    let (c, s) = (a.cos(), a.sin());
    let mut m = ident();
    m[idx(0,0)] = c; m[idx(0,2)] = -s; m[idx(2,0)] = s; m[idx(2,2)] = c;
    m
}
fn rot_x(a: f32) -> M4 { // matches OrbitCam pitch
    let (c, s) = (a.cos(), a.sin());
    let mut m = ident();
    m[idx(1,1)] = c; m[idx(1,2)] = s; m[idx(2,1)] = -s; m[idx(2,2)] = c;
    m
}
fn rot_z(a: f32) -> M4 { // Dutch-angle roll around the view axis
    let (c, s) = (a.cos(), a.sin());
    let mut m = ident();
    m[idx(0,0)] = c; m[idx(0,1)] = s; m[idx(1,0)] = -s; m[idx(1,1)] = c;
    m
}
fn mul_point(m: &M4, p: Vec3) -> [f32; 4] {
    [
        m[idx(0,0)]*p.x + m[idx(1,0)]*p.y + m[idx(2,0)]*p.z + m[idx(3,0)],
        m[idx(0,1)]*p.x + m[idx(1,1)]*p.y + m[idx(2,1)]*p.z + m[idx(3,1)],
        m[idx(0,2)]*p.x + m[idx(1,2)]*p.y + m[idx(2,2)]*p.z + m[idx(3,2)],
        m[idx(0,3)]*p.x + m[idx(1,3)]*p.y + m[idx(2,3)]*p.z + m[idx(3,3)],
    ]
}

// ── light-space matrices for the shadow map ────────────────────────────────────
fn look_at(eye: [f32; 3], ctr: [f32; 3], up: [f32; 3]) -> M4 {
    let sub = |a: [f32;3], b: [f32;3]| [a[0]-b[0], a[1]-b[1], a[2]-b[2]];
    let nrm = |v: [f32;3]| { let l = (v[0]*v[0]+v[1]*v[1]+v[2]*v[2]).sqrt().max(1e-6); [v[0]/l, v[1]/l, v[2]/l] };
    let cross = |a: [f32;3], b: [f32;3]| [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]];
    let dot = |a: [f32;3], b: [f32;3]| a[0]*b[0]+a[1]*b[1]+a[2]*b[2];
    let f = nrm(sub(ctr, eye));
    let s = nrm(cross(f, up));
    let u = cross(s, f);
    let mut m = ident();
    m[idx(0,0)] = s[0]; m[idx(1,0)] = s[1]; m[idx(2,0)] = s[2];
    m[idx(0,1)] = u[0]; m[idx(1,1)] = u[1]; m[idx(2,1)] = u[2];
    m[idx(0,2)] = -f[0]; m[idx(1,2)] = -f[1]; m[idx(2,2)] = -f[2];
    m[idx(3,0)] = -dot(s, eye); m[idx(3,1)] = -dot(u, eye); m[idx(3,2)] = dot(f, eye);
    m
}
fn ortho(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> M4 {
    let mut m = [0.0f32; 16];
    m[idx(0,0)] = 2.0/(r-l); m[idx(1,1)] = 2.0/(t-b); m[idx(2,2)] = -2.0/(f-n);
    m[idx(3,0)] = -(r+l)/(r-l); m[idx(3,1)] = -(t+b)/(t-b); m[idx(3,2)] = -(f+n)/(f-n); m[idx(3,3)] = 1.0;
    m
}
/// Orthographic light view-projection along `sun_dir` (the shader's L = surface→sun), covering a sphere of
/// `radius` about `center`. Column-major, ready for `u_light_viewproj`. Used to render the shadow depth map.
pub fn light_view_proj(sun_dir: [f32; 3], center: [f32; 3], radius: f32) -> M4 {
    let l = { let v = sun_dir; let m = (v[0]*v[0]+v[1]*v[1]+v[2]*v[2]).sqrt().max(1e-6); [v[0]/m, v[1]/m, v[2]/m] };
    let d = radius * 2.5;
    let eye = [center[0] + l[0]*d, center[1] + l[1]*d, center[2] + l[2]*d];
    let up = if l[1].abs() > 0.95 { [0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0] };
    mul(&ortho(-radius, radius, -radius, radius, 0.1, d + radius * 1.5), &look_at(eye, center, up))
}

/// Depth-only vertex shader for the shadow pre-pass (re-uses the scene VBO; a_pos + a_emissive).
pub const SHADOW_VS: &str = r#"
layout(location=0) in vec3 a_pos;
layout(location=4) in float a_emissive;
uniform mat4 u_light_viewproj;
void main() {
    if (a_emissive > 0.8) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; } // bright emitters (skylight/lanterns/pearls) don't cast
    gl_Position = u_light_viewproj * vec4(a_pos, 1.0);
}
"#;
/// Empty fragment shader — only the depth buffer is written.
pub const SHADOW_FS: &str = r#"
void main() {}
"#;

/// The orbit camera as real matrices. Built so `view_proj` reproduces `OrbitCam::project` exactly: the
/// implied vertical focal length is `2·scale/height` and the aspect is `width/height`, with `clip_w =
/// -view_z = depth`. Map NDC→screen the usual GL way (viewport = the callback rect, y flipped by egui_glow)
/// and you land on the same pixels as `project`.
pub struct Camera { view: M4, proj: M4 }
impl Camera {
    pub fn from_orbit(o: &OrbitCam, width: f32, height: f32, scale: f32, znear: f32, zfar: f32) -> Self {
        // view = Rz(roll) · T(0,0,-dist) · Rx(pitch) · Ry(yaw) · T(-target)   (roll = Dutch tilt around view axis)
        let view = mul(&mul(&mul(&mul(&rot_z(o.roll), &translate(0.0, 0.0, -o.dist)), &rot_x(o.pitch)), &rot_y(o.yaw)),
                       &translate(-o.target.x, -o.target.y, -o.target.z));
        let fy = 2.0 * scale / height.max(1.0);
        let fx = 2.0 * scale / width.max(1.0);
        let (n, f) = (znear, zfar);
        let mut proj = [0.0f32; 16];
        proj[idx(0,0)] = fx;
        proj[idx(1,1)] = fy;
        proj[idx(2,2)] = (f + n) / (n - f);
        proj[idx(2,3)] = -1.0;
        proj[idx(3,2)] = 2.0 * f * n / (n - f);
        Camera { view, proj }
    }
    /// Column-major view-projection for the vertex shader (`gl_Position = u_viewproj * vec4(pos,1)`).
    pub fn view_proj(&self) -> M4 { mul(&self.proj, &self.view) }
    /// Eye position in world space (for fresnel + fog in the fragment shader).
    pub fn eye(&self) -> [f32; 3] {
        // eye is the world point that maps to view origin → inverse of the view translation/rotation.
        // Recover it cheaply: eye = target + dist · (camera forward in world). Camera looks down -view_z;
        // world-forward = Ry(-yaw)·Rx(-pitch)·(0,0,1). Compute directly from the orbit angles isn't stored
        // here, so derive from the view matrix: eye = -Rᵀ·t where view = [R|t].
        let r = &self.view;
        let t = [r[idx(3,0)], r[idx(3,1)], r[idx(3,2)]];
        // Rᵀ·t (R is the upper-left 3×3, rows are the basis): eye = -(Rᵀ t)
        [
            -(r[idx(0,0)]*t[0] + r[idx(0,1)]*t[1] + r[idx(0,2)]*t[2]),
            -(r[idx(1,0)]*t[0] + r[idx(1,1)]*t[1] + r[idx(1,2)]*t[2]),
            -(r[idx(2,0)]*t[0] + r[idx(2,1)]*t[1] + r[idx(2,2)]*t[2]),
        ]
    }
    /// View-space depth (distance in front of the camera) of a world point — used to sort triangles.
    fn depth(&self, p: Vec3) -> f32 { -mul_point(&self.view, p)[2] }

    /// Screen-space uv (in [0,1], matching `ndc*0.5+0.5`) of a point at infinity along `dir` — i.e. where
    /// the sun sits on screen, for the god-ray pass. `None` if that direction is behind the camera.
    pub fn dir_uv(&self, dir: [f32; 3]) -> Option<[f32; 2]> {
        let vp = self.view_proj();
        let d = Vec3::new(dir[0], dir[1], dir[2]);
        // clip = VP * (dir, 0)  (a direction → vanishing point)
        let cx = vp[idx(0,0)]*d.x + vp[idx(1,0)]*d.y + vp[idx(2,0)]*d.z;
        let cy = vp[idx(0,1)]*d.x + vp[idx(1,1)]*d.y + vp[idx(2,1)]*d.z;
        let cw = vp[idx(0,3)]*d.x + vp[idx(1,3)]*d.y + vp[idx(2,3)]*d.z;
        if cw <= 1e-4 { return None; }
        Some([(cx / cw) * 0.5 + 0.5, (cy / cw) * 0.5 + 0.5])
    }
}

#[inline] fn col4(c: Color32) -> [f32; 4] { [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0] }

/// Flatten the scene to a back-to-front-sorted triangle soup: 12 floats per vertex (pos×3, normal×3,
/// colour×4, material×1), 3 vertices per triangle, applying each mesh's `offset` (floating). Draw with
/// `glDrawArrays`. The material id selects a `texgen` layer; the shader samples it triplanar.
pub fn pack_sorted(meshes: &[Mesh3], cam: &Camera, min_ndc: f32) -> Vec<f32> {
    // (depth, mesh index, tri index)
    let mut order: Vec<(f32, usize, usize)> = Vec::new();
    // FRUSTUM CULLING — drop triangles whose centroid falls well outside the view frustum before they reach the
    // GPU (the scene is one big draw call, so this is the only place to cull). Large triangles (walls/floor/dome
    // that span the screen even when their centroid is off-frame) are exempt by a size test so we never punch
    // holes. Tighter margin under battery-saver POWER so a phone culls harder. LOGSCOPE_NO_CULL=1 disables it.
    let cull_on = std::env::var("LOGSCOPE_NO_CULL").map(|v| v != "1").unwrap_or(true);
    let vp = cam.view_proj();
    let fy = cam.proj[idx(1, 1)].abs(); // vertical focal term → projected NDC size = radius * fy / depth
    let margin = 1.05 + 0.45 * crate::theme::power_mode(); // full 1.5 · half 1.275 · quarter 1.16
    let clipw = |p: Vec3| -> (f32, f32, f32) {
        (vp[idx(0, 0)] * p.x + vp[idx(1, 0)] * p.y + vp[idx(2, 0)] * p.z + vp[idx(3, 0)],
         vp[idx(0, 1)] * p.x + vp[idx(1, 1)] * p.y + vp[idx(2, 1)] * p.z + vp[idx(3, 1)],
         vp[idx(0, 3)] * p.x + vp[idx(1, 3)] * p.y + vp[idx(2, 3)] * p.z + vp[idx(3, 3)])
    };
    for (mi, m) in meshes.iter().enumerate() {
        let off = m.offset;
        // DYNAMIC LOD — drop an entire scenery mesh whose projected screen size is below the threshold (far + small).
        // Hole-free: we only remove meshes too small to read, and it re-evaluates every frame as the camera moves,
        // so distant foliage/props dissolve out and resolve back in. min_ndc==0 (the graph) keeps everything.
        if min_ndc > 0.0 && !m.verts.is_empty() {
            let inv = 1.0 / m.verts.len() as f32;
            let (mut sx, mut sy, mut sz) = (0.0f32, 0.0f32, 0.0f32);
            for v in &m.verts { sx += v.x; sy += v.y; sz += v.z; }
            let cen = Vec3::new(sx * inv + off.x, sy * inv + off.y, sz * inv + off.z);
            let mut rad2 = 0.0f32;
            for v in &m.verts { let (dx, dy, dz) = (v.x + off.x - cen.x, v.y + off.y - cen.y, v.z + off.z - cen.z); rad2 = rad2.max(dx * dx + dy * dy + dz * dz); }
            let depth = cam.depth(cen).max(0.25);
            if rad2.sqrt() * fy / depth < min_ndc { continue; } // too small on screen → skip the whole mesh
        }
        for (ti, tri) in m.tris.iter().enumerate() {
            let (a, b, d) = (m.verts[tri[0] as usize], m.verts[tri[1] as usize], m.verts[tri[2] as usize]);
            let c = Vec3::new((a.x + b.x + d.x) / 3.0 + off.x, (a.y + b.y + d.y) / 3.0 + off.y, (a.z + b.z + d.z) / 3.0 + off.z);
            if cull_on {
                let r2 = { // triangle size proxy: keep big tris regardless of where their centroid lands
                    let da = (a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y) + (a.z - b.z) * (a.z - b.z);
                    let db = (a.x - d.x) * (a.x - d.x) + (a.y - d.y) * (a.y - d.y) + (a.z - d.z) * (a.z - d.z);
                    da.max(db)
                };
                if r2 < 64.0 { // only cull triangles smaller than ~8 units across
                    let (cx, cy, cw) = clipw(c);
                    if cw <= 0.05 || cx.abs() > cw * margin || cy.abs() > cw * margin { continue; }
                }
            }
            order.push((cam.depth(c) - m.overlay, mi, ti));
        }
    }
    order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)); // far → near
    // Overlay (physical-UI) meshes are pulled toward the camera in WORLD space by `overlay` units
    // before packing, so their fragments win the depth test against scenery/foliage they float over —
    // fixing "the panel is occluded by leaves" without a new vertex attribute or shader change. The
    // pull is along the eye→point direction, a constant per-mesh shift, so the slab keeps its shape
    // and internal layering; only its distance to the eye shrinks. Scenery (overlay==0) is untouched.
    let eye = cam.eye();
    let eye = Vec3::new(eye[0], eye[1], eye[2]);
    let mut out = Vec::with_capacity(order.len() * 36);
    for (_, mi, ti) in order {
        let m = &meshes[mi];
        let tri = m.tris[ti];
        for &vi in &tri {
            let vi = vi as usize;
            let mut p = m.verts[vi];
            p = Vec3::new(p.x + m.offset.x, p.y + m.offset.y, p.z + m.offset.z);
            if m.overlay > 0.0 {
                // shift toward the eye by min(overlay, most of the eye-distance) so it never crosses behind
                let dx = eye.x - p.x; let dy = eye.y - p.y; let dz = eye.z - p.z;
                let d = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-3);
                let s = m.overlay.min(d * 0.6) / d;
                p = Vec3::new(p.x + dx * s, p.y + dy * s, p.z + dz * s);
            }
            let n = m.norms[vi];
            let c = m.vcol.as_ref().map_or(m.color, |vc| vc[vi]);
            let cc = col4(c);
            out.extend_from_slice(&[p.x, p.y, p.z, n.x, n.y, n.z, cc[0], cc[1], cc[2], cc[3], m.mat, m.emissive]);
        }
    }
    out
}

/// Vertex shader body (prepend a `#version` header). Attributes: 0 pos, 1 normal, 2 colour, 3 material id, 4 emissive.
pub const SHADER_VS: &str = r#"
layout(location=0) in vec3 a_pos;
layout(location=1) in vec3 a_normal;
layout(location=2) in vec4 a_color;
layout(location=3) in float a_mat;
layout(location=4) in float a_emissive;
uniform mat4 u_viewproj;
uniform float u_time;
out vec3 v_normal; out vec4 v_color; out vec3 v_world; flat out float v_mat; flat out float v_emissive;
void main() {
    vec3 wp = a_pos;
    if (a_emissive > 6.5 && a_emissive < 7.5) {          // GOLDEN RAINDROP: OCCASIONAL showers (off most of the time)
        float rw = fract(u_time / 55.0);
        if (rw < 0.62) { wp.y = a_pos.y + 80.0; }        // dry spell → park far above (culled)
        else {
            float cell = dot(floor(a_pos.xz * 0.5), vec2(1.0, 57.3));
            float ph = fract(sin(cell) * 43758.5);
            float H = 18.0;
            wp.y = a_pos.y - mod(u_time * 6.0 + ph * H, H);
        }
    }
    v_normal = a_normal; v_color = a_color; v_world = wp; v_mat = a_mat; v_emissive = a_emissive;
    gl_Position = u_viewproj * vec4(wp, 1.0);
}
"#;

/// Fragment shader body: TRIPLANAR procedural-texture sampling (no UVs) for both ALBEDO and a NORMAL+
/// roughness map (the normal perturbs the lighting normal per-fragment → real surface relief), modulated by
/// the per-vertex tint, then hemispherical sun (matching `mesh3d::shade`) + a roughness-driven specular
/// glint + fresnel rim + distance fog. Water (layer 3) scrolls with time. Prepend a `#version` header +
/// `precision highp float;` (and `precision highp sampler2DArray;` on GL ES).
pub const SHADER_FS: &str = r#"
in vec3 v_normal; in vec4 v_color; in vec3 v_world; flat in float v_mat; flat in float v_emissive;
uniform sampler2DArray u_mat; uniform sampler2DArray u_nrm;
uniform float u_texscale; uniform float u_time; uniform float u_bump; uniform float u_exposure;
uniform vec3 u_sun_dir; uniform vec3 u_sun_col; uniform vec3 u_sky; uniform vec3 u_ground;
uniform float u_ambient; uniform vec3 u_eye; uniform vec3 u_fog_col; uniform float u_fog;
uniform float u_caustic; uniform vec3 u_caustic_col;   // >0 on aquatic scenes: animated underwater light dapple
uniform float u_nacre;                                 // thin-film iridescence scale (1 = normal, 0 = off — a tint diagnostic)
uniform mat4 u_light_viewproj; uniform sampler2D u_shadow; uniform float u_shadow_on;  // directional shadow map
uniform vec3 u_fill_dir; uniform vec3 u_fill_col;                                       // colored secondary fill light
out vec4 frag;
vec3 tri(sampler2DArray t, float layer, vec3 wp, vec3 bw) {
    return texture(t, vec3(wp.yz, layer)).rgb * bw.x + texture(t, vec3(wp.zx, layer)).rgb * bw.y + texture(t, vec3(wp.xy, layer)).rgb * bw.z;
}
// TRIPLANAR surface coordinates. Projecting world XZ onto a face is correct for a floor and wrong
// for everything else: on a vertical face the pattern is constant along Y, so it stretches and
// SLIDES ACROSS THE FACE as the camera moves. That is the "textures slide over faces" symptom, and
// it is not a texture bug — there are no textures, only patterns sampled in the wrong space.
//
// The fix is to choose the projection plane per-fragment from the dominant axis of the normal, and
// blend at the seams so a 45-degree face does not pop between planes. The blend exponent controls
// how sharp the transition is: too low and every face looks smeared, too high and the seam is a
// visible line.
vec2 tri_uv(vec3 wp, vec3 n) {
    vec3 a = abs(normalize(n));
    a = pow(a, vec3(4.0));                       // sharpen toward the dominant axis
    a /= max(a.x + a.y + a.z, 1e-5);
    // Each plane drops the axis it faces along, so the coordinate always lies IN the surface.
    return a.x * wp.zy + a.y * wp.xz + a.z * wp.xy;
}

// value noise (smooth, cheap) — used to break the caustic network into discontinuous, irregular bits.
float vhash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float vnoise(vec2 p) {
    vec2 i = floor(p), f = fract(p); f = f * f * (3.0 - 2.0 * f);
    return mix(mix(vhash(i), vhash(i + vec2(1, 0)), f.x), mix(vhash(i + vec2(0, 1)), vhash(i + vec2(1, 1)), f.x), f.y);
}
// Animated caustics: layered moving cells folded to bright filament cores, plus a finer high-frequency sparkle
// network and a slow large-scale density field — then BROKEN UP by drifting noise so the dapple is patchy,
// discontinuous and irregular, the way real refracted sunlight pools, thins and tears, instead of a clean mesh.
float caustic(vec2 p, float t) {
    vec2 q = p;
    q += 0.45 * vec2(sin(p.y * 1.3 + t * 0.6), cos(p.x * 1.1 - t * 0.5));
    q += 0.22 * vec2(cos(p.y * 2.7 - t * 0.4), sin(p.x * 2.3 + t * 0.3));
    float c = 0.0;
    for (int k = 0; k < 5; k++) {
        float a = float(k) * 2.39996;
        vec2 d = vec2(cos(a), sin(a));
        c += sin(dot(q, d) * (1.1 + float(k) * 0.42) + t * (0.35 + float(k) * 0.16));
    }
    c = abs(c) / 5.0;
    float net = pow(max(0.0, 1.0 - c), 7.0);              // tight bright filament cores
    float c2 = 0.0; vec2 q2 = q * 2.4 + 3.1;
    for (int k = 0; k < 3; k++) {
        float a = float(k) * 2.39996 + 1.0;
        vec2 d = vec2(cos(a), sin(a));
        c2 += sin(dot(q2, d) * (2.1 + float(k) * 0.6) + t * (0.7 + float(k) * 0.25));
    }
    c2 = abs(c2) / 3.0;
    net += 0.45 * pow(max(0.0, 1.0 - c2), 11.0);
    // BREAK-UP: two noise fields at different scales tear the network into uneven, drifting fragments + glints
    float tear = smoothstep(0.30, 0.72, vnoise(p * 0.85 + vec2(t * 0.05, -t * 0.04)));
    float grain = 0.55 + 0.85 * vnoise(p * 2.3 - vec2(t * 0.11, t * 0.07));
    net *= tear * grain;
    // large-scale density field → bright pools and calm gaps drifting slowly across the surface
    float dens = 0.42 + 0.58 * (0.5 + 0.5 * sin(p.x * 0.21 + t * 0.13) * cos(p.y * 0.17 - t * 0.11));
    return net * dens;
}
void main() {
    vec3 n = normalize(v_normal);
    vec3 bw = pow(abs(n), vec3(4.0));
    bw /= max(bw.x + bw.y + bw.z, 1e-4);
    vec3 wp = v_world;
    if (v_mat > 2.5 && v_mat < 3.5) { wp.x += u_time * 0.03; wp.z += u_time * 0.02; } // water drift
    vec3 swp = wp * u_texscale;

    vec3 albedo = v_color.rgb * tri(u_mat, v_mat, swp, bw) * 1.7;

    // triplanar normal map: per-plane tangent slope → world axes, blended; A = roughness
    vec4 nmX = texture(u_nrm, vec3(swp.yz, v_mat));
    vec4 nmY = texture(u_nrm, vec3(swp.zx, v_mat));
    vec4 nmZ = texture(u_nrm, vec3(swp.xy, v_mat));
    vec3 tX = nmX.xyz * 2.0 - 1.0, tY = nmY.xyz * 2.0 - 1.0, tZ = nmZ.xyz * 2.0 - 1.0;
    vec3 pert = vec3(0.0, tX.x, tX.y) * bw.x + vec3(tY.y, 0.0, tY.x) * bw.y + vec3(tZ.x, tZ.y, 0.0) * bw.z;
    vec3 N = normalize(n + pert * u_bump);
    float rough = nmX.a * bw.x + nmY.a * bw.y + nmZ.a * bw.z;

    // WATER (layer 3) is the boundary to the world OUTSIDE the palace — the glass dome, the upper windows, the
    // outer wall, the floor sheen. Give it a refraction WARP: an animated surface wobble so reflections and
    // highlights shimmer like light through moving water. The dry AIR interior (stone, layer 2) is left crisp.
    bool is_water = (v_mat > 2.5 && v_mat < 3.5);
    if (is_water) {
        // Triplanar, not XZ. A surface wobble must follow the surface: projected on XZ it is
        // constant along Y, so on the dome and the outer wall the highlight slid across the face
        // instead of moving with it. Caustics and the rain-stain below keep XZ deliberately —
        // both fall from above, so a top-down projection is what they physically are.
        vec2 w = tri_uv(v_world, N) * 0.7 + u_time * 0.18;
        vec3 wob = vec3(sin(w.x * 1.7) + cos(w.y * 1.3), 0.0, cos(w.x * 1.1) + sin(w.y * 1.9));
        N = normalize(N + wob * 0.16);
    }

    float h = 0.5 + 0.5 * N.y;
    vec3 amb = mix(u_ground, u_sky, h) * u_ambient;
    vec3 L = normalize(u_sun_dir);
    float ndl = max(dot(N, L), 0.0);
    // SHADOW MAP: project the world point into the sun's ortho space, PCF-compare against the depth pre-pass.
    float sh = 1.0;
    if (u_shadow_on > 0.5) {
        vec4 lp = u_light_viewproj * vec4(v_world, 1.0);
        vec3 pc = lp.xyz / lp.w * 0.5 + 0.5;
        if (pc.x > 0.0 && pc.x < 1.0 && pc.y > 0.0 && pc.y < 1.0 && pc.z < 1.0) {
            float bias = max(0.0030 * (1.0 - ndl), 0.0010);
            vec2 tx = 1.0 / vec2(textureSize(u_shadow, 0));
            float acc = 0.0;
            for (int i = -1; i <= 1; i++) for (int j = -1; j <= 1; j++)
                acc += (pc.z - bias > texture(u_shadow, pc.xy + vec2(float(i), float(j)) * tx).r) ? 0.0 : 1.0;
            sh = mix(1.0, acc / 9.0, 0.72); // soften — shadowed areas keep some ambient/bounce, not pure black
        }
    }
    // ── PBR MICROSURFACE (photoreal core) ─────────────────────────────────────────────────────────────
    // Each material gets its own base reflectance F0 + a roughness FLOOR, so its highlight has the correct
    // CHARACTER before the normal-map roughness adds surface relief: water/glass smooth & mirror-tight,
    // stone broad & soft, paper matte, wood semi-gloss, moss rough. The texture roughness then modulates
    // WITHIN that band (relief), rather than every surface sharing one plastic sheen.
    vec3 view = normalize(u_eye - v_world);
    float rough_floor = 0.66; float f0s = 0.04;                        // stone / marble (default) — a touch more matte
    if (is_water)                        { rough_floor = 0.11; f0s = 0.02; } // water / glass / crystal (a touch less mirror-perfect)
    else if (v_mat < 0.5)                { rough_floor = 0.86; f0s = 0.03; } // paper (matte)
    else if (v_mat > 3.5)                { rough_floor = 0.74; f0s = 0.03; } // moss / leaf
    else if (v_mat > 0.5 && v_mat < 1.5) { rough_floor = 0.46; f0s = 0.05; } // wood (semi-gloss)
    float rgh = clamp(rough_floor * (0.6 + 0.6 * rough), 0.03, 1.0);
    // SPECULAR ANTIALIASING (geometric normal filtering): where the shading normal changes fast across a
    // pixel — curved silhouettes, column edges, the pearl rim — a tight GGX highlight would flicker/sparkle
    // under motion. Widen roughness by the screen-space normal variance so those sub-pixel microfacets
    // average out, added in quadrature so flat surfaces are untouched. Keeps the new sharp highlights STABLE.
    vec3 dNx = dFdx(N), dNy = dFdy(N);
    float nvar = dot(dNx, dNx) + dot(dNy, dNy);
    rgh = clamp(sqrt(rgh * rgh + min(2.0 * nvar, 0.25)), 0.03, 1.0);
    float gloss = 1.0 - rgh;
    float NdV = max(dot(N, view), 1e-3);
    // Schlick Fresnel with the per-material F0 — drives rim reflection strength + the spec/diffuse energy split.
    vec3  F0 = vec3(f0s);
    vec3  Fv = F0 + (1.0 - F0) * pow(1.0 - NdV, 5.0);
    float fres = Fv.g;
    // ENERGY CONSERVATION: energy reflected as fresnel isn't available to diffuse (grazing angles → less
    // diffuse, more mirror) — the subtle cue that separates a real dielectric from flat shading.
    float kd = 1.0 - clamp(max(Fv.r, max(Fv.g, Fv.b)), 0.0, 1.0);

    vec3 col = albedo * (amb + ndl * sh * u_sun_col * kd) * 1.65;
    // colored secondary fill from the off-sun side — tints shadowed faces with the scene's hue (not shadowed)
    col += albedo * max(dot(N, normalize(u_fill_dir)), 0.0) * u_fill_col;

    // Fresnel env reflection: a cheap procedural sky/ground gradient sampled along the reflection vector.
    // Uses Fresnel-Schlick-ROUGHNESS (Lagarde) so a ROUGH surface gets a broad, dim grazing reflection
    // instead of the too-bright sharp rim a plain fresnel gives it — smooth glass/water still mirror sharply,
    // rough stone/wood barely reflect. A HORIZON fade kills reflection rays that dip below the real surface
    // (no sky leaking through the underside), the way a real reflectance response occludes at the horizon.
    vec3 R = reflect(-view, N);
    vec3 Fr = F0 + (max(vec3(1.0 - rgh), F0) - F0) * pow(1.0 - NdV, 5.0);
    vec3 envc = mix(u_ground, u_sky, clamp(R.y * 0.5 + 0.5, 0.0, 1.0));
    float horiz = clamp(1.0 + dot(R, n), 0.0, 1.0); horiz *= horiz; // n = geometric normal
    col = mix(col, envc, clamp(Fr.g, 0.0, 1.0) * gloss * 0.6 * horiz);

    // BASIC RAY-BOUNCE / REFLECTIONS: a single analytic bounce toward the bright skylight overhead — glossy
    // surfaces (water, polished floor, glass) catch a sharp reflection of the sky-light disc, and pick up an
    // indirect fill the warmer/brighter the bounce direction points up. Cheap stand-in for a reflection trace.
    float skyhit = pow(clamp(R.y, 0.0, 1.0), mix(2.0, 30.0, gloss)); // tighter the glossier → mirror-like
    col += u_sun_col * skyhit * gloss * (0.25 + 0.6 * fres) * (0.6 + 0.4 * u_ambient);
    vec3 bounce = u_ground * clamp(-R.y * 0.5 + 0.5, 0.0, 1.0);      // light kicked back up off the floor/ground
    col += albedo * bounce * 0.12 * gloss;

    // COOK-TORRANCE GGX specular (microfacet D·G·F / 4·NdV·NdL) — a physically-based highlight that BROADENS
    // and DIMS correctly with roughness (replaces the old Blinn-Phong sheen). Water/crystal read as a tight
    // bright glint, stone as a broad soft sheen, paper almost none: the single biggest photoreal gain here.
    vec3 hv = normalize(L + view);
    float NdH = max(dot(N, hv), 0.0);
    float a2 = rgh * rgh * rgh * rgh;                               // alpha^2, with alpha = rgh^2
    float dnm = NdH * NdH * (a2 - 1.0) + 1.0;
    float D = a2 / (3.14159265 * dnm * dnm + 1e-6);                 // GGX normal distribution
    float kg = (rgh * rgh) * 0.5;                                   // Smith geometry k (direct lighting)
    float G = (NdV / (NdV * (1.0 - kg) + kg)) * (ndl / (ndl * (1.0 - kg) + kg));
    vec3  Fh = F0 + (1.0 - F0) * pow(1.0 - max(dot(hv, view), 0.0), 5.0);
    vec3  ggx = min((D * G) * Fh / (4.0 * NdV * max(ndl, 1e-3) + 1e-4) * ndl, vec3(8.0)); // clamp firefly spikes
    col += ggx * u_sun_col * sh * 2.6;                              // scaled to read under the bloom/exposure chain

    // NACRE: thin-film iridescence on glass/water — a view-dependent hue shimmer (the pearl/mother-of-pearl
    // sheen). Strongest at grazing angles (fresnel) and on glossy surfaces, so glass dome, crystal, clear
    // windows and water-material pearls shimmer with a slow rainbow rim without washing out the base colour.
    if (is_water) {
        float ph = fres * 6.2831 + dot(N, view) * 3.5 + u_time * 0.25 + v_world.y * 0.15;
        vec3 nacre = 0.5 + 0.5 * cos(ph + vec3(0.0, 2.094, 4.188)); // R,G,B offset 120° → spectral sweep
        col += u_nacre * nacre * fres * gloss * (0.22 + 0.18 * u_ambient);
        // view-INDEPENDENT mother-of-pearl sheen: a slow rainbow that flows across the surface by world position
        // + time, so even flat, head-on facets (the in-scene UI panel) shimmer and "play with light".
        float ph2 = dot(v_world.xyz, vec3(0.7, 1.1, 0.5)) * 0.85 + u_time * 0.45;
        vec3 sheen = 0.5 + 0.5 * cos(ph2 + vec3(0.0, 2.094, 4.188));
        col += u_nacre * sheen * gloss * 0.075 * (0.5 + 0.5 * u_ambient);
    }

    // Caustics: refracted sunlight through the water + glass dome dapples the scene — FULL on the water itself
    // (the dome / windows / floor sheen) and as a gentler skylight dapple on the dry interior above-facing stone.
    if (u_caustic > 0.0) {
        float up = clamp(N.y, 0.0, 1.0);
        vec2 cp = v_world.xz * 0.6;
        if (is_water) cp += 0.34 * vec2(sin(v_world.z * 0.7 + u_time * 0.5), cos(v_world.x * 0.6 - u_time * 0.4));
        float caus = caustic(cp, u_time);
        float wgt = is_water ? 1.0 : 0.45;           // air interior gets a softer dapple than the water surfaces
        col += u_caustic_col * caus * (0.18 + 0.82 * up) * u_caustic * wgt;
    }

    // Water depth murk: the water deepens to a murky blue with distance, so the world OUTSIDE the glass reads as
    // water while the AIR interior (handled by the lighter global fog) stays clear — the separation the palace wants.
    if (is_water) {
        float wd = clamp(length(u_eye - v_world) * 0.015, 0.0, 1.0);
        col = mix(col, u_fog_col * 1.08 + u_caustic_col * 0.04, wd * 0.42); // lighter murk → the hall reads clearly
    }

    // ── MATERIAL VARIATIONS ────────────────────────────────────────────────────────────────────────
    // LEAVES (MOSS, layer 4): leaves forward-scatter, so a backlit leaf GLOWS on the viewer's side (Habel
    // 2007). Cheap transmission: brightest where the eye looks toward the sun THROUGH the leaf; a soft wrap
    // keeps the terminator translucent too. Green-biased. This is the dappled, glowing-silhouette canopy —
    // and it varies per scene for free via the per-scene sun colour/ambient (warm Forest vs dim Canopy).
    if (v_mat > 3.5) {
        float back = pow(clamp(dot(view, -L), 0.0, 1.0), 3.0);
        float wrap = clamp((dot(N, L) + 0.5) / 1.5, 0.0, 1.0);
        col += albedo * vec3(0.72, 1.0, 0.42) * u_sun_col * (back * 0.85 + wrap * 0.22) * 1.15;
    }
    // PAGES (PAPER, layer 0): thin sheets glow softly when backlit + a warm matte sheen, so paper reads as
    // paper (warm parchment in Archive, cooler cream in Reading Room — again via the per-scene light).
    if (v_mat < 0.5) {
        float back = pow(clamp(dot(view, -L), 0.0, 1.0), 2.0);
        col += albedo * u_sun_col * vec3(1.0, 0.95, 0.82) * back * 0.45;
    }
    // GLASS (transparent WATER surfaces — the dome/windows): a crisp fresnel RIM so edges catch light like
    // real glass, on top of the nacre/refraction it already has.
    if (is_water && v_color.a < 0.85) {
        col += u_sun_col * pow(fres, 2.0) * gloss * 0.5;
    }

    col *= u_exposure;                                   // overall brightness
    float emis = v_emissive;
    // animated emissive for MOSS motes: gentle firefly TWINKLE, or — for the very bright lightning bolts — a
    // sharp, intermittent STROBE (mostly dark with occasional bright flashes, like a real storm strike).
    if (v_mat > 3.5 && v_mat < 4.5 && v_emissive > 1.6) {
        float ph = dot(v_world, vec3(1.7, 2.3, 0.9));
        if (v_emissive > 2.0) {
            float s = sin(u_time * 0.85 + ph * 0.4);
            float strike = pow(max(s, 0.0), 16.0);                 // rare, brief appearances
            float flick = 0.55 + 0.45 * sin(u_time * 42.0 + ph);   // rapid stutter during a strike
            emis = v_emissive * (0.02 + 1.9 * strike * flick);     // ~0 between strikes → flickers in and out
        } else {
            emis *= 0.18 + 1.05 * pow(0.5 + 0.5 * sin(u_time * 2.3 + ph), 4.0); // firefly: continuous twinkle
        }
    }
    if (v_emissive > 4.5 && v_emissive < 6.0) { // GOLD VEIN — a light-pulse sweeps ALONG the marble veining
        float along = v_world.x * 0.55 + v_world.z * 0.55;          // diagonal sweep axis
        float pulse = pow(max(sin(along * 0.7 - u_time * 2.2), 0.0), 6.0); // sharp travelling band
        emis = 0.30 + 6.0 * pulse;                                  // dim gold base + bright moving pulse
    } else if (v_emissive > 6.5) {              // GOLDEN RAINDROP — bright falling gold streak
        emis = 2.2;
    }
    // GARDEN "MEMORY MARBLE": golden raindrops restain the marble in patches that bloom in then slowly fade back
    // to white over a long cycle — a material memory that reforms and resolves.
    if (v_mat > 1.5 && v_mat < 2.5 && abs(v_emissive - 0.22) < 0.03) {
        vec2 cell = floor(v_world.xz * 0.55);
        float rnd = fract(sin(dot(cell, vec2(12.9, 78.2))) * 43758.5);
        float stainable = step(0.45, fract(rnd * 7.31));            // only some cells ever stain → mostly white
        float ph = fract(u_time / 26.0 + rnd);
        float gold = stainable * exp(-ph * 6.0) * smoothstep(0.0, 0.03, ph); // brief gold bloom, long return to white
        vec2 fl = fract(v_world.xz * 0.55) - 0.5;
        float patch = gold * smoothstep(0.55, 0.05, length(fl));
        col = mix(col, vec3(0.80, 0.58, 0.24), clamp(patch, 0.0, 1.0) * 0.8);
        emis = 0.0;                                                 // emissive value is just the flag, no self-glow
    }
    col += v_color.rgb * emis * 2.5;               // self-illumination → blooms (runes, crystal, lit text)

    float d = length(u_eye - v_world);
    float f = clamp(1.0 - exp(-u_fog * d), 0.0, 1.0);
    col = mix(col, u_fog_col, f);
    frag = vec4(col, v_color.a);
}
"#;

// ── post-processing chain (depth-resolved scene → bloom + god-rays → composite) ────────────────────────
// All post passes share this vertex shader: a single fullscreen triangle generated from gl_VertexID (no
// vertex buffer needed), with `v_uv` in [0,1] matching `ndc*0.5+0.5`.
pub const SHADER_POST_VS: &str = r#"
out vec2 v_uv;
void main() {
    vec2 p = vec2((gl_VertexID == 1) ? 3.0 : -1.0, (gl_VertexID == 2) ? 3.0 : -1.0);
    v_uv = 0.5 * p + 0.5;
    gl_Position = vec4(p, 0.0, 1.0);
}
"#;

/// Bright-pass: keep only the part of each pixel above `u_threshold` (the bloom source).
pub const SHADER_BRIGHT_FS: &str = r#"
in vec2 v_uv; uniform sampler2D u_tex; uniform float u_threshold; out vec4 frag;
void main() {
    vec3 c = texture(u_tex, v_uv).rgb;
    float l = dot(c, vec3(0.299, 0.587, 0.114));
    frag = vec4(c * (max(l - u_threshold, 0.0) / max(l, 1e-4)), 1.0);
}
"#;

/// Separable Gaussian blur; `u_dir` is the per-tap step in uv (set to (texel,0) then (0,texel)).
pub const SHADER_BLUR_FS: &str = r#"
in vec2 v_uv; uniform sampler2D u_tex; uniform vec2 u_dir; out vec4 frag;
void main() {
    vec3 c = texture(u_tex, v_uv).rgb * 0.2270;
    c += texture(u_tex, v_uv + u_dir * 1.0).rgb * 0.3162;
    c += texture(u_tex, v_uv - u_dir * 1.0).rgb * 0.3162;
    c += texture(u_tex, v_uv + u_dir * 2.0).rgb * 0.0702;
    c += texture(u_tex, v_uv - u_dir * 2.0).rgb * 0.0702;
    frag = vec4(c, 1.0);
}
"#;

/// Radial "god-ray" accumulation toward the sun's screen position `u_sun` (uv). `u_weight`/`u_decay` shape
/// the falloff. Cheap 24-step march along the ray; runs on the bright texture.
pub const SHADER_GODRAY_FS: &str = r#"
in vec2 v_uv; uniform sampler2D u_tex; uniform vec2 u_sun; uniform float u_weight; uniform float u_decay; out vec4 frag;
void main() {
    vec2 delta = (u_sun - v_uv) / 24.0;
    vec2 uv = v_uv; vec3 acc = vec3(0.0); float illum = 1.0;
    for (int i = 0; i < 24; i++) {
        uv += delta;
        acc += texture(u_tex, uv).rgb * illum * u_weight;
        illum *= u_decay;
    }
    frag = vec4(acc, 1.0);
}
"#;

/// Final composite into egui's framebuffer: scene + additive soft bloom + god-ray streaks. Alpha = scene
/// coverage plus a little from the glow so halos bleed past geometry edges over the egui content.
pub const SHADER_COMPOSITE_FS: &str = r#"
in vec2 v_uv;
uniform sampler2D u_scene; uniform sampler2D u_bloom; uniform sampler2D u_godray;
uniform float u_bloom_amt; uniform float u_godray_amt;
uniform float u_saturation;   // chroma gain at the very end: <1 grey, 1 neutral, >1 vibrant (counters fog/haze grey)
uniform float u_shoulder;      // luminance highlight rolloff strength: 0 = off (linear), 1 = full
uniform float u_pearl; uniform vec2 u_pearl_pos; uniform float u_pearl_r; uniform float u_aspect; uniform float u_time;
uniform float u_underwater;   // 0 in the air dome; →1 when the camera is OUTSIDE in open water (view warps/tints)
uniform vec2  u_cam_warp;     // camera (yaw,pitch) — anchors the underwater ripple/caustic field to the 3D view
uniform float u_pearl_variant; // composer-pearl optical preset (0 balanced · 1 fine-crackle · 2 milky · 3 clear)
uniform float u_defocus;       // >0 frosts the scene (disk blur) — the backdrop uses it for the glass UI
uniform float u_frost_a;       // frost band thickness at the v_uv.y≈0 screen edge (UV)
uniform float u_frost_b;       // frost band thickness at the v_uv.y≈1 screen edge (UV)
uniform float u_opaque;        // 1 = full-window backdrop: force opaque + fill empty sky with u_bg (no see-through
                               // to undefined framebuffer memory, which some drivers surface as garbage)
uniform vec3  u_bg;            // background fill colour for empty regions when u_opaque
uniform sampler2D u_depth;     // scene depth (sampleable texture) — drives depth-of-field
uniform float u_dof;           // depth-of-field strength (0 = off)
uniform float u_focus;         // focal distance in eye units (≈ orbit distance to the content)

// frosted-glass disk blur of the scene: a 13-tap sunflower kernel, radius scaled by `def`
vec4 scene_frosted(vec2 uv, float def) {
    if (def < 0.001) return texture(u_scene, uv);
    float r = 0.006 * def;
    vec4 s = texture(u_scene, uv) * 0.18;
    float w = 0.18;
    for (int i = 0; i < 12; i++) {
        float a = float(i) * 2.3998277;                 // golden-angle spiral
        float rad = r * sqrt((float(i) + 0.5) / 12.0);
        vec2 o = vec2(cos(a), sin(a)) * rad;
        float wi = 1.0 - 0.5 * (float(i) / 12.0);
        s += texture(u_scene, clamp(uv + o, vec2(0.0), vec2(1.0))) * wi; w += wi;
    }
    return s / w;
}
out vec4 frag;

vec2 h2(vec2 p){ p = vec2(dot(p, vec2(127.1, 311.7)), dot(p, vec2(269.5, 183.3))); return fract(sin(p) * 43758.5453); }
// worley edge distance (F2-F1): ~0 along cell borders → the crack seams of the pearl
float crackdist(vec2 uv){
    vec2 g = floor(uv), f = fract(uv); float f1 = 9.0, f2 = 9.0;
    for (int y = -1; y <= 1; y++) { for (int x = -1; x <= 1; x++) {
        vec2 o = vec2(float(x), float(y)); vec2 r = o + h2(g + o) - f; float d = dot(r, r);
        if (d < f1) { f2 = f1; f1 = d; } else if (d < f2) { f2 = d; }
    }}
    return sqrt(f2) - sqrt(f1);
}
vec3 nacre(float ph){ return 0.5 + 0.5 * cos(ph + vec3(0.0, 2.094, 4.188)); } // thin-film spectral sweep

void main() {
    // UNDERWATER WARP: when the camera is outside the air dome (in the water material) the whole view refracts —
    // a layered sinusoidal ripple on the sample coords + an aqua absorption tint, caustic shimmer and vignette.
    vec2 uv = v_uv;
    if (u_underwater > 0.001) {
        float w = u_underwater, tt = u_time;
        // anchor the field to the look direction: yaw pans it horizontally, pitch vertically, so ROTATING the
        // camera flows the refraction through the water (world-locked) instead of sliding it across the screen.
        vec2 cw = u_cam_warp;
        float ex = uv.x + cw.x * 0.35, ey = uv.y + cw.y * 0.5;
        vec2 ripple = vec2(
            sin(ey * 19.0 + tt * 1.3) + 0.5 * sin(ey * 37.0 - tt * 0.7 + cw.x),
            cos(ex * 16.0 + tt * 1.1) + 0.5 * cos(ex * 31.0 + tt * 0.9 - cw.y));
        // parallax: distortion strengthens toward the frame edges (more water column off-axis)
        float edge = 0.6 + 0.8 * length(v_uv - 0.5);
        uv += ripple * 0.0065 * w * edge;
    }
    // frost ONLY behind the chrome bands (top + bottom of the window); the middle viewport stays sharp.
    float frost_amt = 0.0;
    if (u_defocus > 0.001) {
        float band = max(
            1.0 - smoothstep(u_frost_a - 0.03, u_frost_a, v_uv.y),               // band hugging the y≈0 edge
            smoothstep(1.0 - u_frost_b, 1.0 - u_frost_b + 0.03, v_uv.y));        // band hugging the y≈1 edge
        frost_amt = u_defocus * clamp(band, 0.0, 1.0);
    }
    // DEPTH-OF-FIELD: blur grows for surfaces FARTHER than the focal plane (background defocus / bokeh),
    // leaving the in-focus content + the near UI panel sharp. Reuses the frosted disk-blur. NEAR/FAR match
    // Camera::from_orbit at the render site (0.3 / 400). Cleared (empty) depth = 1.0 → far → fully blurred.
    float dofc = 0.0;
    if (u_dof > 0.001) {
        float d = texture(u_depth, v_uv).r;            // window depth [0,1] at this pixel
        float ndc = d * 2.0 - 1.0;
        float lin = (2.0 * 0.3 * 400.0) / (400.0 + 0.3 - ndc * (400.0 - 0.3)); // eye-space distance (znear=0.3)
        float coc = smoothstep(u_focus * 1.3, u_focus * 2.4, lin); // wider sharp field, gentler ramp
        dofc = coc * u_dof * 1.5;                                   // lessened blur strength
    }
    float def = max(frost_amt, dofc);
    vec4 sc = scene_frosted(uv, def);
    vec3 bl = texture(u_bloom, uv).rgb * u_bloom_amt + texture(u_godray, uv).rgb * u_godray_amt;
    vec3 col = sc.rgb + bl;
    float a = clamp(sc.a + dot(bl, vec3(0.333)), 0.0, 1.0);
    if (u_underwater > 0.001) {
        float w = u_underwater, tt = u_time;
        col *= mix(vec3(1.0), vec3(0.62, 0.86, 1.06), w * 0.6);                 // water absorbs red → aqua cast
        float caus = sin((uv.x + u_cam_warp.x * 0.35) * 40.0 + tt) * sin((uv.y + u_cam_warp.y * 0.5) * 34.0 - tt * 1.2);
        col += vec3(0.10, 0.18, 0.22) * max(caus, 0.0) * w * 0.25;              // faint caustic shimmer
        float vig = smoothstep(1.25, 0.35, length(v_uv - 0.5) * 1.5);
        col *= mix(1.0, vig, w * 0.45);                                         // depth vignette
    }

    // ---- COMPOSER PEARL: a pale, physically-shaded pearl whose substance REFRACTS the scene behind it ----
    if (u_pearl > 0.001) {
        // VARIANT presets (dropdown): 0 balanced · 1 fine-crackle · 2 milky · 3 clear-glass
        int pv = int(u_pearl_variant + 0.5);
        float whiteMix = pv == 2 ? 0.62 : (pv == 3 ? 0.14 : 0.34);   // how opaque/pearly vs. clear
        float crackFreq = pv == 1 ? 19.0 : (pv == 3 ? 7.5 : 12.5);   // seam density
        float dispScale = pv == 1 ? 1.8 : (pv == 3 ? 1.5 : 1.0);     // chromatic spread
        float refrScale = pv == 2 ? 0.45 : (pv == 3 ? 1.25 : 1.0);   // how much the scene bends through
        vec2 d = v_uv - u_pearl_pos; d.x *= u_aspect;           // aspect-correct so it reads round
        float rr = length(d) / u_pearl_r;
        float halo = smoothstep(1.7, 1.0, rr) * (1.0 - smoothstep(1.0, 0.97, rr));
        col += vec3(0.62, 0.80, 1.0) * halo * 0.22 * u_pearl;   // soft outer light bleed
        if (rr < 1.0) {
            float nz = sqrt(max(0.0, 1.0 - rr * rr));           // sphere facing camera
            vec3 nrm = normalize(vec3(d.x, d.y, nz * 0.9));
            float fres = pow(1.0 - nz, 3.0);                    // fresnel: bright at grazing rim
            vec2 cuv = nrm.xy / (1.0 + nrm.z) * crackFreq + u_time * 0.02; // stereographic seam coords
            float ce = crackdist(cuv);
            float crack = smoothstep(0.055, 0.0, ce);           // tight seams → fine fractures
            crack += 0.5 * smoothstep(0.04, 0.0, crackdist(cuv * 2.3 + 7.0)); // a finer second layer
            crack = clamp(crack, 0.0, 1.0);
            // REFRACTION through the body, with chromatic DISPERSION + extra bend along the cracks
            vec2 ro = nrm.xy * u_pearl_r * (0.62 + 0.95 * crack) * refrScale;
            vec3 refr;
            refr.r = texture(u_scene, v_uv - ro * (1.0 + 0.06 * dispScale)).r;
            refr.g = texture(u_scene, v_uv - ro).g;
            refr.b = texture(u_scene, v_uv - ro * (1.0 - 0.06 * dispScale)).b;
            vec3 irid = nacre(nz * 7.0 + crack * 6.0 + u_time * 0.4); // thin-film interference
            vec3 pearl = mix(refr, vec3(0.90, 0.95, 1.0), whiteMix);  // PALE substance, scene scattering through
            pearl += irid * (0.16 + 0.55 * fres);               // iridescent sheen, strong at the rim
            pearl += vec3(1.0) * fres * 0.55;                   // fresnel white rim
            pearl += irid * crack * 0.9;                        // rainbow refraction along the seams
            pearl += vec3(0.95, 0.99, 1.0) * crack * 0.22;      // seam glints
            float spec = pow(max(dot(nrm, normalize(vec3(-0.35, 0.45, 0.82))), 0.0), 48.0);
            pearl += vec3(1.0) * spec * 0.9;                    // specular hotspot
            float edge = smoothstep(1.0, 0.93, rr);             // soft anti-aliased rim
            col = mix(col, pearl, edge * u_pearl);
            a = max(a, edge * u_pearl);
        }
    }
    // BACKDROP: fill empty/sky regions (low alpha) with the dark background and force fully opaque, so no
    // undefined framebuffer memory can show through behind the Background layer (garbage on some drivers).
    if (u_opaque > 0.5) {
        col = mix(u_bg, col, clamp(a * 1.5, 0.0, 1.0));
        a = 1.0;
    }
    // NaN guard (a NaN anywhere — e.g. a 0/0 or pow of a negative on some drivers — otherwise smears as garbage
    // colour) + clamp to displayable range so no out-of-range channel can manifest as a tint.
    if (!(dot(col, col) >= 0.0)) col = vec3(0.0);
    // VIBRANCE: pull chroma back up at the end so distance-fog + flat ambient don't leave everything grey.
    // Works around a luma-preserving grey; u_saturation 1 = neutral, >1 = more colourful.
    if (u_saturation > 0.0) { float l = dot(col, vec3(0.2126, 0.7152, 0.0722)); col = max(mix(vec3(l), col, u_saturation), 0.0); }
    // HIGHLIGHT SHOULDER — luminance-based so it DOESN'T desaturate. The old per-channel rolloff compressed a
    // bright colour's big channel more than its small ones, pulling them together → washing bright areas to
    // grey/white (the "everything's grey" the clamp caused). Here only LUMA rolls off and RGB is scaled to
    // match, preserving hue/saturation. u_shoulder scales it (0 = off/linear → set LOGSCOPE_SHOULDER=0 to A/B).
    // FILMIC TONEMAP (ACES approximation, Narkowicz) on LUMINANCE only — hue-preserving (RGB scaled to the new
    // luma, so it never desaturates the way a per-channel curve does). Rolls highlights off smoothly from the
    // mid-tones so bright materials (marble, water sheen, sky) keep detail instead of clipping to a flat white,
    // and reads more photographic than the old rational shoulder. Scaled by u_shoulder (0 = off/linear → A/B
    // via LOGSCOPE_SHOULDER=0); the highlight-heavy scenes are the ones this rescues.
    if (u_shoulder > 0.001) {
        float l = dot(col, vec3(0.2126, 0.7152, 0.0722));
        float aces = clamp((l * (2.51 * l + 0.03)) / (l * (2.43 * l + 0.59) + 0.14), 0.0, 1.0);
        float lt = mix(l, aces, u_shoulder);
        col *= lt / max(l, 1e-4);
    }
    frag = vec4(clamp(col, 0.0, 1.0), clamp(a, 0.0, 1.0));
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh3d;

    // THE alignment guarantee: the GPU view-projection must land on the same pixels as OrbitCam::project,
    // or the GPU scene and the 2D dots/runes/labels would drift apart.
    #[test]
    fn view_proj_matches_orbitcam_project() {
        let o = OrbitCam { yaw: 0.6, pitch: 0.32, dist: 15.0, target: Vec3::new(0.0, -3.0, 0.0), roll: 0.0 };
        let (w, h, scale) = (820.0f32, 610.0f32, 400.0f32);
        let cam = Camera::from_orbit(&o, w, h, scale, 0.1, 300.0);
        let vp = cam.view_proj();
        for p in [Vec3::new(1.0, 0.0, 2.0), Vec3::new(-3.0, -2.0, 1.5), Vec3::new(0.5, 4.0, -3.0), Vec3::new(2.2, -1.0, 0.0)] {
            let (sx, sy, depth) = o.project(p, w / 2.0, h / 2.0, scale).unwrap();
            let clip = mul_point(&vp, p);
            assert!((clip[3] - depth).abs() < 1e-2, "clip.w should equal project depth: {} vs {}", clip[3], depth);
            let (nx, ny) = (clip[0] / clip[3], clip[1] / clip[3]);
            let gx = (nx * 0.5 + 0.5) * w;            // viewport = rect at origin
            let gy = (1.0 - (ny * 0.5 + 0.5)) * h;     // GL y-up → egui y-down (egui_glow flips the viewport)
            assert!((gx - sx).abs() < 0.5 && (gy - sy).abs() < 0.5, "pixel match: ({gx},{gy}) vs ({sx},{sy})");
        }
    }

    #[test]
    fn eye_is_in_front_of_the_scene() {
        let o = OrbitCam::default();
        let cam = Camera::from_orbit(&o, 800.0, 600.0, 400.0, 0.1, 300.0);
        // the eye should be ~dist away from the target
        let e = cam.eye();
        let d = ((e[0] - o.target.x).powi(2) + (e[1] - o.target.y).powi(2) + (e[2] - o.target.z).powi(2)).sqrt();
        assert!((d - o.dist).abs() < 0.05, "eye sits ~dist from target: {d} vs {}", o.dist);
    }

    #[test]
    fn pack_is_sorted_and_shaped() {
        let o = OrbitCam::default();
        let cam = Camera::from_orbit(&o, 800.0, 600.0, 400.0, 0.1, 300.0);
        let scene = vec![mesh3d::uv_sphere(o.target, 1.0, 8, 6, Color32::WHITE)];
        let v = pack_sorted(&scene, &cam, 0.0);
        assert_eq!(v.len() % 36, 0, "12 floats × 3 verts per triangle");
        assert_eq!(v.len() / 36, 8 * 6 * 2, "all sphere triangles packed");
        // recompute per-triangle depth from the packed positions; must be non-increasing (far → near).
        // stride is 12 floats/vertex (pos3, normal3, color4, mat, emissive) → 36 per triangle.
        let mut last = f32::INFINITY;
        for t in v.chunks(36) {
            let c = Vec3::new((t[0] + t[12] + t[24]) / 3.0, (t[1] + t[13] + t[25]) / 3.0, (t[2] + t[14] + t[26]) / 3.0);
            let d = cam.depth(c);
            assert!(d <= last + 1e-3, "triangles are back-to-front");
            last = d;
        }
    }
}
