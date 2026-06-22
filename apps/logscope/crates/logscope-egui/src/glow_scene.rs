//! glow_scene.rs — the real GL renderer (feature `gpu-scene`), with the polish rung: the 3D scene is drawn
//! into an OFFSCREEN FBO with a real DEPTH BUFFER (correct overlaps for interpenetrating geometry), then a
//! post chain extracts highlights, blurs them (soft BLOOM), streaks them toward the sun (GOD-RAYS), and
//! composites the result into egui's framebuffer — pixel-aligned with the 2D overlays (the camera matrix
//! matches `OrbitCam::project`, proven in gpuscene).
//!
//! VALIDATED on Mesa/llvmpipe: every shader (scene + bright/blur/godray/composite) compiles in both
//! `#version 300 es` and `#version 330`; the whole GUI compiles in-crate against eframe 0.34/egui_glow/glow;
//! the offscreen-FBO + depth + multi-pass + readback pipeline was exercised headless. The egui_glow
//! `PaintCallback`/`CallbackFn`/`painter.gl()`/`painter.intermediate_fbo()` usage follows egui's own docs +
//! `custom_3d_glow` example. Numbers (threshold, bloom/godray amounts, blur radius) are tunable on hardware.

use std::sync::Arc;
use eframe::glow::{self, HasContext as _};
use eframe::egui_glow;
use crate::gpuscene;

/// All per-frame inputs: the scene lighting, the sun's screen position (for god-rays), and the post knobs.
#[derive(Clone)]
pub struct Uniforms {
    pub viewproj: [f32; 16], pub eye: [f32; 3],
    pub sun_dir: [f32; 3], pub sun_col: [f32; 3], pub sky: [f32; 3], pub ground: [f32; 3], pub ambient: f32,
    pub fog_col: [f32; 3], pub fog: f32,
    pub sun_uv: [f32; 2],            // sun position on screen in [0,1]; god-rays stream from here
    pub time: f32,                   // seconds, for animated materials (water)
    pub bump: f32,                   // normal-map relief strength
    pub exposure: f32,               // overall brightness
    pub texscale: f32,              // world units → texture tile frequency (triplanar)
    pub threshold: f32,              // bloom bright-pass cutoff
    pub bloom_amt: f32, pub godray_amt: f32, pub godray_weight: f32, pub godray_decay: f32,
    pub caustic: f32, pub caustic_col: [f32; 3],   // underwater dapple: 0 off (most scenes), >0 aquatic
    // COMPOSER PEARL (screen-space, in the composite pass): a pale refractive pearl that scatters the scene
    // through itself with fresnel + thin-film + dispersion + cracks. 0 = off.
    pub pearl: f32, pub pearl_variant: f32, pub pearl_pos: [f32; 2], pub pearl_r: f32, pub aspect: f32,
    pub nacre: f32,                                // thin-film iridescence scale on water (1 = normal; 0 = off)
    pub underwater: f32,                           // 0 in the air dome; ->1 when the eye is outside in open water
    pub defocus: f32,                              // >0 frosts (blurs) the scene — the glass-UI backdrop sets it
    pub frost_a: f32, pub frost_b: f32,            // frost band thickness at each screen edge (UV); 0 = no band
    pub cam_warp: [f32; 2],                        // camera (yaw,pitch) for the 3D-aware underwater ripple
    pub opaque: f32, pub bg: [f32; 3],             // backdrop: force opaque + fill empty regions with bg color
    pub dof: f32, pub focus: f32,                  // depth-of-field strength + focal distance (eye units)
    pub saturation: f32,                           // final chroma gain (1 neutral, >1 vibrant) — counters fog grey
    pub shoulder: f32,                             // luminance highlight rolloff (0 off/linear, 1 full)
    pub light_viewproj: [f32; 16],                 // sun's ortho view-proj for the shadow map
    pub shadow_on: f32,                            // 0 = no shadow pass; 1 = shadow-mapped direct light
    pub fill_dir: [f32; 3], pub fill_col: [f32; 3], // colored secondary fill light (off-sun side)
}
impl Default for Uniforms {
    fn default() -> Self {
        Uniforms {
            viewproj: [0.0; 16], eye: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_col: [1.0; 3],
            sky: [0.46, 0.58, 0.77], ground: [0.25, 0.20, 0.16], ambient: 0.55,
            fog_col: [0.1, 0.12, 0.14], fog: 0.012, sun_uv: [0.5, 0.85], time: 0.0, bump: 0.7, exposure: 1.35, texscale: 0.35,
            threshold: 0.62, bloom_amt: 0.85, godray_amt: 0.45, godray_weight: 0.42, godray_decay: 0.92,
            caustic: 0.0, caustic_col: [0.0, 1.0, 0.82],
            pearl: 0.0, pearl_variant: 0.0, pearl_pos: [0.5, 0.5], pearl_r: 0.24, aspect: 1.6,
            nacre: 1.0, underwater: 0.0, defocus: 0.0, frost_a: 0.0, frost_b: 0.0, cam_warp: [0.0, 0.0], opaque: 0.0, bg: [0.04, 0.07, 0.10], dof: 0.0, focus: 18.0,
            saturation: 1.18,
            shoulder: 1.0,
            light_viewproj: [0.0; 16], shadow_on: 0.0,
            fill_dir: [0.0, 1.0, 0.0], fill_col: [0.0, 0.0, 0.0],
        }
    }
}

unsafe fn compile(gl: &glow::Context, ty: u32, src: &str) -> glow::Shader {
    let s = gl.create_shader(ty).expect("create_shader");
    gl.shader_source(s, src);
    gl.compile_shader(s);
    assert!(gl.get_shader_compile_status(s), "shader compile failed: {}", gl.get_shader_info_log(s));
    s
}
unsafe fn link(gl: &glow::Context, version: &str, vs_body: &str, fs_body: &str, fs_prec: bool) -> glow::Program {
    let p = gl.create_program().expect("create_program");
    let vs = compile(gl, glow::VERTEX_SHADER, &format!("{version}\n{vs_body}"));
    // GL ES needs an explicit precision for sampler2DArray (sampler2D gets a default; the array type doesn't)
    let prec = if !fs_prec { "" }
        else if version.contains("es") { "precision highp float;\nprecision highp sampler2DArray;\n" }
        else { "precision highp float;\n" };
    let fs = compile(gl, glow::FRAGMENT_SHADER, &format!("{version}\n{prec}{fs_body}"));
    gl.attach_shader(p, vs); gl.attach_shader(p, fs);
    gl.link_program(p);
    assert!(gl.get_program_link_status(p), "program link failed: {}", gl.get_program_info_log(p));
    gl.detach_shader(p, vs); gl.detach_shader(p, fs); gl.delete_shader(vs); gl.delete_shader(fs);
    p
}

// Offscreen render targets, lazily (re)sized to the scene rect in physical pixels.
struct Targets {
    w: i32, h: i32, sw: i32, sh: i32, hw: i32, hh: i32, rs: f32,
    scene_fbo: glow::Framebuffer, scene_color: glow::Texture, depth: glow::Texture,
    ping_fbo: glow::Framebuffer, ping_tex: glow::Texture,
    pong_fbo: glow::Framebuffer, pong_tex: glow::Texture,
    shadow_fbo: glow::Framebuffer, shadow_tex: glow::Texture, shadow_sz: i32,
}
/// RENDER-RESOLUTION LOD: render the (expensive) scene + bloom passes at this fraction of native, then let
/// the composite upscale (it samples by UV). 1.0 = native. Driven by LOGSCOPE_RENDER_SCALE, else LOGSCOPE_
/// QUALITY (≤1). A GPU-fragment lever shared by every bin that renders through glow_scene. Cuts scene shading
/// cost by rs². (Frame-update LOD lives in the caller; mesh-density LOD in scene3d::render_quality.)
fn render_scale() -> f32 {
    if let Ok(v) = std::env::var("LOGSCOPE_RENDER_SCALE") { if let Ok(f) = v.trim().parse::<f32>() { return f.clamp(0.4, 1.0); } }
    std::env::var("LOGSCOPE_QUALITY").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0).clamp(0.4, 1.0)
}
unsafe fn color_tex(gl: &glow::Context, w: i32, h: i32) -> glow::Texture {
    let t = gl.create_texture().unwrap();
    gl.bind_texture(glow::TEXTURE_2D, Some(t));
    // ZERO-INITIALISE rather than leaving the store undefined: a skipped pass (e.g. god-rays at low LOD) or the
    // very first frame can otherwise sample uninitialised memory, which some drivers surface as garbage/magenta.
    let zeros = vec![0u8; (w.max(1) * h.max(1) * 4) as usize];
    gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&zeros)));
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    // pin an identity R,G,B,A swizzle so a driver with non-identity defaults can't silently swap channels
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_R, glow::RED as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_G, glow::GREEN as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_B, glow::BLUE as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_A, glow::ALPHA as i32);
    t
}
/// A sampleable DEPTH texture (so the composite can read scene depth for depth-of-field). NEAREST, clamped.
unsafe fn depth_tex(gl: &glow::Context, w: i32, h: i32) -> glow::Texture {
    let t = gl.create_texture().unwrap();
    gl.bind_texture(glow::TEXTURE_2D, Some(t));
    gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::DEPTH_COMPONENT24 as i32, w.max(1), h.max(1), 0, glow::DEPTH_COMPONENT, glow::UNSIGNED_INT, glow::PixelUnpackData::Slice(None));
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    t
}
unsafe fn fbo_with(gl: &glow::Context, tex: glow::Texture) -> glow::Framebuffer {
    let f = gl.create_framebuffer().unwrap();
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
    gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
    f
}
/// Upload one RGBA8 `LAYERS`-deep `TEXTURE_2D_ARRAY` (mipmapped, repeating) from baked layer-major data —
/// used for both the albedo array and the normal+roughness array, sampled triplanar by the scene shader.
unsafe fn upload_array(gl: &glow::Context, data: &[u8], n: usize) -> glow::Texture {
    let n = n as i32;
    let t = gl.create_texture().unwrap();
    gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(t));
    gl.tex_image_3d(glow::TEXTURE_2D_ARRAY, 0, glow::RGBA8 as i32, n, n, crate::texgen::LAYERS as i32, 0,
                    glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(data)));
    gl.generate_mipmap(glow::TEXTURE_2D_ARRAY);
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
    t
}
impl Targets {
    unsafe fn new(gl: &glow::Context, w: i32, h: i32, rs: f32) -> Self {
        let (w, h) = (w.max(1), h.max(1));
        let (sw, sh) = (((w as f32 * rs) as i32).max(1), ((h as f32 * rs) as i32).max(1)); // scene render size
        let (hw, hh) = ((sw / 2).max(1), (sh / 2).max(1));
        let scene_color = color_tex(gl, sw, sh);
        let scene_fbo = fbo_with(gl, scene_color);
        let depth = depth_tex(gl, sw, sh); // sampleable depth (for depth-of-field), at scene resolution
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(scene_fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::TEXTURE_2D, Some(depth), 0);
        let ping_tex = color_tex(gl, hw, hh); let ping_fbo = fbo_with(gl, ping_tex);
        let pong_tex = color_tex(gl, hw, hh); let pong_fbo = fbo_with(gl, pong_tex);
        // SHADOW MAP: a fixed-size depth-only target rendered from the sun's view. Independent of scene LOD.
        let shadow_sz = std::env::var("LOGSCOPE_SHADOW_SIZE").ok().and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(2048).clamp(512, 4096);
        let shadow_tex = depth_tex(gl, shadow_sz, shadow_sz);
        let shadow_fbo = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(shadow_fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::TEXTURE_2D, Some(shadow_tex), 0);
        gl.draw_buffers(&[glow::NONE]); // depth-only FBO: no colour attachment
        gl.read_buffer(glow::NONE);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        Targets { w, h, sw, sh, hw, hh, rs, scene_fbo, scene_color, depth, ping_fbo, ping_tex, pong_fbo, pong_tex, shadow_fbo, shadow_tex, shadow_sz }
    }
    unsafe fn destroy(&self, gl: &glow::Context) {
        gl.delete_framebuffer(self.scene_fbo); gl.delete_texture(self.scene_color); gl.delete_texture(self.depth);
        gl.delete_framebuffer(self.ping_fbo); gl.delete_texture(self.ping_tex);
        gl.delete_framebuffer(self.pong_fbo); gl.delete_texture(self.pong_tex);
        gl.delete_framebuffer(self.shadow_fbo); gl.delete_texture(self.shadow_tex);
    }
}

struct Renderer {
    scene: glow::Program, bright: glow::Program, blur: glow::Program, godray: glow::Program, composite: glow::Program,
    shadow: glow::Program,
    vbo: glow::Buffer, scene_vao: glow::VertexArray, empty_vao: glow::VertexArray,
    mat_tex: glow::Texture, nrm_tex: glow::Texture, baked_tex: usize, targets: Option<Targets>,
}
impl Renderer {
    unsafe fn new(gl: &glow::Context) -> Self {
        // Native default is desktop GL 3.3; LOGSCOPE_GL_ES forces the GLES 3.0 path (same shaders — they target
        // both), which some drivers (e.g. Asahi/Mesa, GLES-first) handle more predictably. wasm is always GLES.
        let force_es = std::env::var("LOGSCOPE_GL_ES").map(|v| v != "0").unwrap_or(false);
        let version = if cfg!(target_arch = "wasm32") || force_es { "#version 300 es" } else { "#version 330" };
        // one-time provenance line so a tint/format bug can be tied to the actual driver + GLSL profile in use
        eprintln!("[logscope] GL_VENDOR={} GL_RENDERER={} GL_VERSION={} | GLSL {} | GL_SHADING_LANGUAGE_VERSION={}",
            gl.get_parameter_string(glow::VENDOR), gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_string(glow::VERSION), version, gl.get_parameter_string(glow::SHADING_LANGUAGE_VERSION));
        let scene = link(gl, version, gpuscene::SHADER_VS, gpuscene::SHADER_FS, true);
        let bright = link(gl, version, gpuscene::SHADER_POST_VS, gpuscene::SHADER_BRIGHT_FS, true);
        let blur = link(gl, version, gpuscene::SHADER_POST_VS, gpuscene::SHADER_BLUR_FS, true);
        let godray = link(gl, version, gpuscene::SHADER_POST_VS, gpuscene::SHADER_GODRAY_FS, true);
        let composite = link(gl, version, gpuscene::SHADER_POST_VS, gpuscene::SHADER_COMPOSITE_FS, true);
        let shadow = link(gl, version, gpuscene::SHADOW_VS, gpuscene::SHADOW_FS, false);
        let tex_n = crate::theme::detail(crate::theme::variant()).0; // current detail tier's texture resolution
        Renderer {
            scene, bright, blur, godray, composite, shadow,
            vbo: gl.create_buffer().unwrap(),
            scene_vao: gl.create_vertex_array().unwrap(),
            empty_vao: gl.create_vertex_array().unwrap(),
            mat_tex: upload_array(gl, &crate::texgen::bake_layers(tex_n), tex_n),
            nrm_tex: upload_array(gl, &crate::texgen::bake_normals(tex_n), tex_n),
            baked_tex: tex_n,
            targets: None,
        }
    }

    unsafe fn ensure(&mut self, gl: &glow::Context, w: i32, h: i32, rs: f32) {
        let need = self.targets.as_ref().map_or(true, |t| t.w != w || t.h != h || (t.rs - rs).abs() > 1e-3);
        if need {
            if let Some(t) = self.targets.take() { t.destroy(gl); }
            self.targets = Some(Targets::new(gl, w, h, rs));
        }
    }

    // bind a texture to a unit and point a sampler uniform at it
    unsafe fn sampler(&self, gl: &glow::Context, prog: glow::Program, name: &str, unit: u32, tex: glow::Texture) {
        gl.active_texture(glow::TEXTURE0 + unit);
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.uniform_1_i32(gl.get_uniform_location(prog, name).as_ref(), unit as i32);
    }
    unsafe fn fullscreen(&self, gl: &glow::Context) { gl.bind_vertex_array(Some(self.empty_vao)); gl.draw_arrays(glow::TRIANGLES, 0, 3); }

    unsafe fn paint(&mut self, gl: &glow::Context, intermediate_fbo: Option<glow::Framebuffer>, u: &Uniforms, verts: &[f32]) {
        if verts.is_empty() { return; }
        // live texture-detail: re-bake the material/normal arrays if the variant bumped the resolution
        let tex_n = crate::theme::detail(crate::theme::variant()).0;
        if tex_n != self.baked_tex {
            gl.delete_texture(self.mat_tex);
            gl.delete_texture(self.nrm_tex);
            self.mat_tex = upload_array(gl, &crate::texgen::bake_layers(tex_n), tex_n);
            self.nrm_tex = upload_array(gl, &crate::texgen::bake_normals(tex_n), tex_n);
            self.baked_tex = tex_n;
        }
        let mut vp = [0i32; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut vp); // egui set this to the scene rect (physical px)
        let (w, h) = (vp[2].max(1), vp[3].max(1));
        self.ensure(gl, w, h, render_scale());
        let t = self.targets.as_ref().unwrap();

        // egui calls us with GL_SCISSOR_TEST ENABLED and its scissor box clipped to the widget rect (in window
        // coords). If we leave it on, our glClear of the offscreen FBOs is clipped to that box — so most of each
        // target is never cleared and keeps UNDEFINED tile memory, which a tiler GPU (Apple/Asahi) surfaces as
        // magenta (llvmpipe happens to read ~black, hiding it). Disable it for our full-FBO passes; restore after.
        let scissor_was = gl.is_enabled(glow::SCISSOR_TEST);
        gl.disable(glow::SCISSOR_TEST);

        // ---- shared VBO upload + attribute layout (both the shadow pre-pass and the scene pass draw it) ----
        gl.bind_vertex_array(Some(self.scene_vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
        let bytes = std::slice::from_raw_parts(verts.as_ptr() as *const u8, verts.len() * 4);
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::DYNAMIC_DRAW);
        let st = 12 * 4;
        gl.enable_vertex_attrib_array(0); gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, st, 0);
        gl.enable_vertex_attrib_array(1); gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, st, 12);
        gl.enable_vertex_attrib_array(2); gl.vertex_attrib_pointer_f32(2, 4, glow::FLOAT, false, st, 24);
        gl.enable_vertex_attrib_array(3); gl.vertex_attrib_pointer_f32(3, 1, glow::FLOAT, false, st, 40);
        gl.enable_vertex_attrib_array(4); gl.vertex_attrib_pointer_f32(4, 1, glow::FLOAT, false, st, 44);
        let n_verts = (verts.len() / 12) as i32;

        // ---- 0) SHADOW MAP: depth-only pre-pass from the sun's ortho view (bright emitters skipped in the VS) ----
        if u.shadow_on > 0.5 {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.shadow_fbo));
            gl.viewport(0, 0, t.shadow_sz, t.shadow_sz);
            gl.enable(glow::DEPTH_TEST); gl.depth_func(glow::LESS);
            gl.disable(glow::BLEND);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.enable(glow::POLYGON_OFFSET_FILL); gl.polygon_offset(2.0, 4.0); // depth bias → less shadow acne
            gl.use_program(Some(self.shadow));
            gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(self.shadow, "u_light_viewproj").as_ref(), false, &u.light_viewproj);
            gl.draw_arrays(glow::TRIANGLES, 0, n_verts);
            gl.disable(glow::POLYGON_OFFSET_FILL);
        }

        // ---- 1) SCENE → offscreen color+depth (depth test on) at the (LOD) scene resolution ----
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.scene_fbo));
        gl.viewport(0, 0, t.sw, t.sh);
        gl.enable(glow::DEPTH_TEST); gl.depth_func(glow::LESS);
        // Alpha blend the scene pass: geometry is painter-sorted back-to-front (pack_sorted), so alpha<1 glass
        // shows the ocean through it while opaque meshes blend to a no-op. Premultiply-friendly src-over.
        gl.enable(glow::BLEND);
        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        gl.use_program(Some(self.scene));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.mat_tex));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.nrm_tex));
        let loc = |n: &str| gl.get_uniform_location(self.scene, n);
        gl.uniform_1_i32(loc("u_mat").as_ref(), 0);
        gl.uniform_1_i32(loc("u_nrm").as_ref(), 1);
        gl.uniform_1_f32(loc("u_texscale").as_ref(), u.texscale);
        gl.uniform_1_f32(loc("u_time").as_ref(), u.time);
        gl.uniform_1_f32(loc("u_bump").as_ref(), u.bump);
        gl.uniform_1_f32(loc("u_exposure").as_ref(), u.exposure);
        gl.uniform_matrix_4_f32_slice(loc("u_viewproj").as_ref(), false, &u.viewproj);
        gl.uniform_3_f32(loc("u_sun_dir").as_ref(), u.sun_dir[0], u.sun_dir[1], u.sun_dir[2]);
        gl.uniform_3_f32(loc("u_sun_col").as_ref(), u.sun_col[0], u.sun_col[1], u.sun_col[2]);
        gl.uniform_3_f32(loc("u_sky").as_ref(), u.sky[0], u.sky[1], u.sky[2]);
        gl.uniform_3_f32(loc("u_ground").as_ref(), u.ground[0], u.ground[1], u.ground[2]);
        gl.uniform_1_f32(loc("u_ambient").as_ref(), u.ambient);
        gl.uniform_3_f32(loc("u_eye").as_ref(), u.eye[0], u.eye[1], u.eye[2]);
        gl.uniform_3_f32(loc("u_fog_col").as_ref(), u.fog_col[0], u.fog_col[1], u.fog_col[2]);
        gl.uniform_1_f32(loc("u_fog").as_ref(), u.fog);
        gl.uniform_1_f32(loc("u_caustic").as_ref(), u.caustic);
        gl.uniform_3_f32(loc("u_caustic_col").as_ref(), u.caustic_col[0], u.caustic_col[1], u.caustic_col[2]);
        gl.uniform_1_f32(loc("u_nacre").as_ref(), u.nacre);
        // SHADOW MAP → texture unit 2 (units 0,1 hold the material/normal arrays)
        self.sampler(gl, self.scene, "u_shadow", 2, t.shadow_tex);
        gl.uniform_matrix_4_f32_slice(loc("u_light_viewproj").as_ref(), false, &u.light_viewproj);
        gl.uniform_1_f32(loc("u_shadow_on").as_ref(), u.shadow_on);
        gl.uniform_3_f32(loc("u_fill_dir").as_ref(), u.fill_dir[0], u.fill_dir[1], u.fill_dir[2]);
        gl.uniform_3_f32(loc("u_fill_col").as_ref(), u.fill_col[0], u.fill_col[1], u.fill_col[2]);
        gl.draw_arrays(glow::TRIANGLES, 0, n_verts);

        // post passes: depth off, no blend, half-res
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);
        gl.viewport(0, 0, t.hw, t.hh);

        // ---- 2) bright-pass: scene_color → ping ----
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.ping_fbo));
        gl.use_program(Some(self.bright));
        self.sampler(gl, self.bright, "u_tex", 0, t.scene_color);
        gl.uniform_1_f32(gl.get_uniform_location(self.bright, "u_threshold").as_ref(), u.threshold);
        self.fullscreen(gl);

        // ---- 3) separable blur: ping →H pong →V ping (ping = soft bloom) ----
        let (rx, ry) = (1.5 / t.hw as f32, 1.5 / t.hh as f32);
        gl.use_program(Some(self.blur));
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.pong_fbo));
        self.sampler(gl, self.blur, "u_tex", 0, t.ping_tex);
        gl.uniform_2_f32(gl.get_uniform_location(self.blur, "u_dir").as_ref(), rx, 0.0);
        self.fullscreen(gl);
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.ping_fbo));
        self.sampler(gl, self.blur, "u_tex", 0, t.pong_tex);
        gl.uniform_2_f32(gl.get_uniform_location(self.blur, "u_dir").as_ref(), 0.0, ry);
        self.fullscreen(gl);

        // ---- 4) god-rays: ping (bloom) → pong, streaking toward the sun. OPTICAL-EFFECT LOD: at low render
        // scale this whole pass is skipped and its contribution zeroed in the composite (saves a fullscreen pass).
        let low_q = t.rs < 0.7;
        if !low_q {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.pong_fbo));
            gl.use_program(Some(self.godray));
            self.sampler(gl, self.godray, "u_tex", 0, t.ping_tex);
            gl.uniform_2_f32(gl.get_uniform_location(self.godray, "u_sun").as_ref(), u.sun_uv[0], u.sun_uv[1]);
            gl.uniform_1_f32(gl.get_uniform_location(self.godray, "u_weight").as_ref(), u.godray_weight);
            gl.uniform_1_f32(gl.get_uniform_location(self.godray, "u_decay").as_ref(), u.godray_decay);
            self.fullscreen(gl);
        }

        // ---- 5) composite back into egui's framebuffer at the rect, blended over egui content ----
        // restore egui's scissor first, so the composite honours the widget clip (matters in the GUI where the
        // scene can sit in a clipped/scrolled sub-rect; harmless in the full-window scene-view).
        if scissor_was { gl.enable(glow::SCISSOR_TEST); }
        gl.bind_framebuffer(glow::FRAMEBUFFER, intermediate_fbo);
        gl.viewport(vp[0], vp[1], vp[2], vp[3]);
        gl.enable(glow::BLEND);
        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
        gl.use_program(Some(self.composite));
        self.sampler(gl, self.composite, "u_scene", 0, t.scene_color);
        self.sampler(gl, self.composite, "u_bloom", 1, t.ping_tex);
        self.sampler(gl, self.composite, "u_godray", 2, t.pong_tex);
        self.sampler(gl, self.composite, "u_depth", 3, t.depth);
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_bloom_amt").as_ref(), u.bloom_amt);        // diagnostic: LOGSCOPE_NO_POST drops bloom/god-rays/pearl so a tint can be bisected to the SCENE pass
        // vs. the post chain (if the tint survives with post off, it's the scene shader — likely the nacre).
        let no_post = std::env::var("LOGSCOPE_NO_POST").map(|v| v != "0").unwrap_or(false);
        if no_post { gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_bloom_amt").as_ref(), 0.0); }
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_godray_amt").as_ref(), if low_q || no_post { 0.0 } else { u.godray_amt });
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_saturation").as_ref(), if no_post { 1.0 } else { u.saturation });
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_shoulder").as_ref(), if no_post { 0.0 } else { u.shoulder });
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_pearl").as_ref(), if no_post { 0.0 } else { u.pearl });
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_pearl_variant").as_ref(), u.pearl_variant);
        gl.uniform_2_f32(gl.get_uniform_location(self.composite, "u_pearl_pos").as_ref(), u.pearl_pos[0], u.pearl_pos[1]);
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_pearl_r").as_ref(), u.pearl_r);
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_aspect").as_ref(), u.aspect);
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_time").as_ref(), u.time);
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_underwater").as_ref(), u.underwater);
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_defocus").as_ref(), u.defocus);
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_dof").as_ref(), if no_post { 0.0 } else { u.dof });
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_focus").as_ref(), u.focus.max(0.5));
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_frost_a").as_ref(), u.frost_a);
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_frost_b").as_ref(), u.frost_b);
        gl.uniform_2_f32(gl.get_uniform_location(self.composite, "u_cam_warp").as_ref(), u.cam_warp[0], u.cam_warp[1]);
        gl.uniform_1_f32(gl.get_uniform_location(self.composite, "u_opaque").as_ref(), u.opaque);
        gl.uniform_3_f32(gl.get_uniform_location(self.composite, "u_bg").as_ref(), u.bg[0], u.bg[1], u.bg[2]);
        self.fullscreen(gl);

        gl.bind_vertex_array(None);
        gl.active_texture(glow::TEXTURE0);
    }
}

thread_local! { static RENDERER: std::cell::RefCell<Option<Renderer>> = const { std::cell::RefCell::new(None) }; }

/// Build the egui paint callback that runs the full offscreen → bloom/god-ray → composite chain over
/// `verts` (from `gpuscene::pack_sorted`) with `u`. Add at the scene rect:
/// `ui.painter().add(glow_scene::callback(rect, u, verts));`
pub fn callback(rect: egui::Rect, u: Uniforms, verts: Vec<f32>) -> egui::PaintCallback {
    egui::PaintCallback {
        rect,
        callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
            let gl = painter.gl();
            let inter = painter.intermediate_fbo(); // egui's bound FBO — composite must end up here
            RENDERER.with(|cell| {
                let mut b = cell.borrow_mut();
                if b.is_none() { unsafe { *b = Some(Renderer::new(gl)); } }
                unsafe { b.as_mut().unwrap().paint(gl, inter, &u, &verts); }
            });
        })),
    }
}
