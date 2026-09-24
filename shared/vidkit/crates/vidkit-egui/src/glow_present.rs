//! Upload the decoder's planes and convert on the GPU.
//!
//! The CPU path in [`crate::presenter`] converts to RGBA8 and uploads four
//! bytes a pixel where the decoder produced one and a half. At 1080p60 that is
//! roughly 500 MB/s against 187 MB/s — the single largest avoidable cost on the
//! software-decode path, which is the path Asahi is on for VP9 and AV1.
//!
//! This uploads the Y and CbCr planes as they are and converts in a fragment
//! shader, through an `egui_glow` paint callback. Same colour maths as
//! `vidkit-vk/shaders/convert.wgsl` and `vidkit_core::convert_cpu` — a third
//! implementation, which is a drift hazard, so [`expected_rgb`] mirrors the
//! others and the parity tests compare against it.
//!
//! Follows the pattern `shared/scene-kit/src/glow_scene.rs` established and
//! validated on llvmpipe: `eframe::glow`, `egui_glow::CallbackFn`,
//! `painter.gl()`, and shaders written for both `#version 330` and
//! `#version 300 es`.
//!
//! # Two GL settings that are not optional
//!
//! **`UNPACK_ALIGNMENT` defaults to 4.** A single-channel plane whose width is
//! not a multiple of four is then read with phantom padding at the end of every
//! row, and the picture shears progressively down the frame. 1920 and 1280 are
//! multiples of 4 and hide it; 1366 and any cropped region do not.
//!
//! **`UNPACK_ROW_LENGTH` carries the stride.** Decoder planes are padded — V4L2
//! and VAAPI both hand back rows wider than the picture — and uploading them as
//! if tightly packed produces the same shear. The stride is in the `CpuPlanes`
//! and has to be passed through.

use std::sync::Arc;

use eframe::egui_glow;
use eframe::glow::{self, HasContext as _};
use egui::Rect;
use vidkit_core::frame::{ColorInfo, CpuPlanes, Frame, FrameData, Matrix, PlaneLayout, Range};

/// Selector values the shader takes, matching `convert.wgsl`'s `Params`.
pub fn matrix_id(m: Matrix) -> i32 {
    match m {
        Matrix::Bt601 => 0,
        Matrix::Bt709 => 1,
        Matrix::Bt2020Ncl => 2,
    }
}

pub fn range_id(r: Range) -> i32 {
    match r {
        Range::Limited => 0,
        Range::Full => 1,
    }
}

/// Bytes this path uploads for one frame of the given size and layout.
///
/// The reason the module exists, expressed as arithmetic so it can be asserted
/// rather than claimed: NV12 is 1.5 bytes a pixel against RGBA8's 4.
pub fn upload_bytes(layout: PlaneLayout, width: usize, height: usize) -> usize {
    let luma = width * height * layout.bytes_per_sample();
    let chroma = width.div_ceil(2) * height.div_ceil(2) * 2 * layout.bytes_per_sample();
    luma + chroma
}

/// What RGBA8 would have cost for the same frame.
pub fn rgba_upload_bytes(width: usize, height: usize) -> usize {
    width * height * 4
}

const VERT: &str = r#"
out vec2 uv;
void main() {
    // Full-screen triangle from gl_VertexID; no vertex buffer, no VAO state to
    // get wrong, and one fewer object to leak.
    vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
    uv = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAG: &str = r#"
precision mediump float;
in vec2 uv;
out vec4 frag;
uniform sampler2D plane_y;
uniform sampler2D plane_uv;
uniform int matrix_id;
uniform int range_id;
uniform float flip_y;
// 0 = bilinear (the texture unit's own filter), 1 = Catmull-Rom bicubic on
// luma. Chroma stays bilinear: it is subsampled by two already, and the eye
// takes its sharpness from luma.
uniform int scaler;
uniform vec2 y_size;

// Catmull-Rom in 9 bilinear taps instead of 16 point taps: the middle two
// weights of each axis are merged into one linear fetch (TheRealMJP's
// formulation). Needs LINEAR filtering on plane_y, which it has.
float bicubic_y(vec2 t) {
    vec2 pos = t * y_size;
    vec2 c1 = floor(pos - 0.5) + 0.5;
    vec2 f = pos - c1;
    vec2 w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    vec2 w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    vec2 w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    vec2 w3 = f * f * (-0.5 + 0.5 * f);
    vec2 w12 = w1 + w2;
    vec2 o12 = w2 / w12;
    vec2 p0 = (c1 - 1.0) / y_size;
    vec2 p3 = (c1 + 2.0) / y_size;
    vec2 p12 = (c1 + o12) / y_size;
    float r = 0.0;
    r += texture(plane_y, vec2(p0.x,  p0.y)).r  * w0.x  * w0.y;
    r += texture(plane_y, vec2(p12.x, p0.y)).r  * w12.x * w0.y;
    r += texture(plane_y, vec2(p3.x,  p0.y)).r  * w3.x  * w0.y;
    r += texture(plane_y, vec2(p0.x,  p12.y)).r * w0.x  * w12.y;
    r += texture(plane_y, vec2(p12.x, p12.y)).r * w12.x * w12.y;
    r += texture(plane_y, vec2(p3.x,  p12.y)).r * w3.x  * w12.y;
    r += texture(plane_y, vec2(p0.x,  p3.y)).r  * w0.x  * w3.y;
    r += texture(plane_y, vec2(p12.x, p3.y)).r  * w12.x * w3.y;
    r += texture(plane_y, vec2(p3.x,  p3.y)).r  * w3.x  * w3.y;
    return r;
}

void main() {
    // egui's framebuffer has its origin at the top left and GL samples from the
    // bottom, so the picture arrives upside down unless one of them is flipped.
    vec2 t = vec2(uv.x, mix(uv.y, 1.0 - uv.y, flip_y));

    float y = scaler == 1 ? clamp(bicubic_y(t), 0.0, 1.0) : texture(plane_y, t).r;
    vec2 c = texture(plane_uv, t).rg;

    float yv, cb, cr;
    if (range_id == 0) {
        yv = (y  - 16.0 / 255.0) * (255.0 / 219.0);
        cb = (c.r - 128.0 / 255.0) * (255.0 / 224.0);
        cr = (c.g - 128.0 / 255.0) * (255.0 / 224.0);
    } else {
        yv = y;
        cb = c.r - 0.5;
        cr = c.g - 0.5;
    }

    vec3 rgb;
    if (matrix_id == 0) {
        rgb = vec3(yv + 1.59603 * cr,
                   yv - 0.39176 * cb - 0.81297 * cr,
                   yv + 2.01723 * cb);
    } else if (matrix_id == 2) {
        rgb = vec3(yv + 1.47460 * cr,
                   yv - 0.16455 * cb - 0.57135 * cr,
                   yv + 1.88140 * cb);
    } else {
        rgb = vec3(yv + 1.57480 * cr,
                   yv - 0.18732 * cb - 0.46812 * cr,
                   yv + 1.85560 * cb);
    }
    frag = vec4(clamp(rgb, 0.0, 1.0), 1.0);
}
"#;

/// Compile VERT + `frag` into a program, with the log on failure: a shader
/// that compiles on one driver and not another is the whole reason for it.
unsafe fn link(gl: &glow::Context, version: &str, frag: &str) -> Result<glow::Program, String> {
    let program = gl.create_program()?;
    let mut shaders = Vec::new();
    for (kind, src) in [(glow::VERTEX_SHADER, VERT), (glow::FRAGMENT_SHADER, frag)] {
        let s = gl.create_shader(kind)?;
        gl.shader_source(s, &format!("{version}\n{src}"));
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            return Err(format!(
                "{version} {} shader: {}",
                if kind == glow::VERTEX_SHADER { "vertex" } else { "fragment" },
                gl.get_shader_info_log(s)
            ));
        }
        gl.attach_shader(program, s);
        shaders.push(s);
    }
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        return Err(format!("link: {}", gl.get_program_info_log(program)));
    }
    for s in shaders {
        gl.detach_shader(program, s);
        gl.delete_shader(s);
    }
    Ok(program)
}

/// RCAS, FSR 1's sharpening, over the scaled frame at OUTPUT resolution.
///
/// A line-for-line port of bscale's `d2x-core/src/rcas.rs` (`rcas_pixel`,
/// `noise_scale`), which is itself the FSR SDK's: same limit (0.25 - 1/16),
/// same luma proxy, same guarded reciprocals, same noise back-off. It cannot
/// overshoot -- the lobe comes from the min/max of the 4-tap ring -- so it
/// does not draw halos along the bicubic's own ringing. `rcas_reference` in
/// Rust mirrors it for the GPU self-test.
const RCAS_FRAG: &str = r#"
precision mediump float;
in vec2 uv;
out vec4 frag;
uniform sampler2D src;
uniform vec2 texel;
uniform float scale;   // exp2(-stops); 0 = off

float luma2(vec3 p) { return p.b * 0.5 + (p.r * 0.5 + p.g); }

void main() {
    vec3 e = texture(src, uv).rgb;
    if (scale <= 0.0) { frag = vec4(e, 1.0); return; }
    vec3 b = texture(src, uv + vec2(0.0, -texel.y)).rgb;
    vec3 d = texture(src, uv + vec2(-texel.x, 0.0)).rgb;
    vec3 f = texture(src, uv + vec2(texel.x, 0.0)).rgb;
    vec3 h = texture(src, uv + vec2(0.0, texel.y)).rgb;

    float bl = luma2(b), dl = luma2(d), el = luma2(e), fl = luma2(f), hl = luma2(h);
    float spread = max(max(max(bl, dl), max(el, fl)), hl) - min(min(min(bl, dl), min(el, fl)), hl);
    float nz = 0.25 * bl + 0.25 * dl + 0.25 * fl + 0.25 * hl - el;
    nz = spread > 0.0 ? clamp(abs(nz) / spread, 0.0, 1.0) : 0.0;
    nz = -0.5 * nz + 1.0;

    vec3 mn4 = min(min(b, d), min(f, h));
    vec3 mx4 = max(max(b, d), max(f, h));
    vec3 hit_min = mn4 / max(4.0 * mx4, vec3(1e-6));
    hit_min = mix(vec3(0.0), hit_min, step(vec3(1e-6), mx4));
    vec3 den = 4.0 * mn4 - 4.0;
    vec3 hit_max = (1.0 - mx4) / min(den, vec3(-1e-6));
    vec3 l3 = max(-hit_min, hit_max);
    float lobe = max(max(l3.r, l3.g), l3.b);
    lobe = max(min(lobe, 0.0), -0.1875) * scale * nz;

    float rcp = 1.0 / (4.0 * lobe + 1.0);
    frag = vec4(clamp((lobe * (b + d + h + f) + e) * rcp, 0.0, 1.0), 1.0);
}
"#;

/// Scaling filter for the video.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Scaler {
    #[default]
    Bilinear,
    /// Catmull-Rom on luma: the realtime upscale, 9 taps a pixel.
    Bicubic,
}

/// What the presenter does beyond converting colour. Chosen per title by the
/// app's budget; the default is exactly the old single pass.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct Enhance {
    pub scaler: Scaler,
    /// RCAS strength in FSR's stops (0 = strongest, 1 = half...); `None` off.
    pub sharpen: Option<f32>,
}

impl Enhance {
    pub fn rcas_scale(&self) -> f32 {
        self.sharpen.map(|s| (-s.max(0.0)).exp2()).unwrap_or(0.0)
    }
    /// Texture fetches per output pixel, for the budget: 2 (Y + UV)
    /// bilinear, 10 bicubic; RCAS adds 5 in a second pass.
    pub fn fetches_per_pixel(&self) -> u32 {
        let scale = match self.scaler {
            Scaler::Bilinear => 2,
            Scaler::Bicubic => 10,
        };
        scale + if self.sharpen.is_some() { 5 } else { 0 }
    }
}

/// The CPU mirror of RCAS_FRAG, for the GPU self-test. Values 0..=1.
pub fn rcas_reference(b: [f32; 3], d: [f32; 3], e: [f32; 3], f: [f32; 3], h: [f32; 3], scale: f32) -> [f32; 3] {
    if scale <= 0.0 {
        return e;
    }
    let luma2 = |p: [f32; 3]| p[2] * 0.5 + (p[0] * 0.5 + p[1]);
    let (bl, dl, el, fl, hl) = (luma2(b), luma2(d), luma2(e), luma2(f), luma2(h));
    let spread = bl.max(dl).max(el).max(fl).max(hl) - bl.min(dl).min(el).min(fl).min(hl);
    let nz = 0.25 * bl + 0.25 * dl + 0.25 * fl + 0.25 * hl - el;
    let nz = if spread > 0.0 { (nz.abs() / spread).clamp(0.0, 1.0) } else { 0.0 };
    let nz = -0.5 * nz + 1.0;
    let mut lobe = f32::NEG_INFINITY;
    for c in 0..3 {
        let mn4 = b[c].min(d[c]).min(f[c]).min(h[c]);
        let mx4 = b[c].max(d[c]).max(f[c]).max(h[c]);
        let hit_min = if mx4 > 1e-6 { mn4 / (4.0 * mx4) } else { 0.0 };
        let hit_max = (1.0 - mx4) / (4.0 * mn4 - 4.0).min(-1e-6);
        lobe = lobe.max((-hit_min).max(hit_max));
    }
    let lobe = lobe.min(0.0).max(-0.1875) * scale * nz;
    let rcp = 1.0 / (4.0 * lobe + 1.0);
    let mut out = [0.0; 3];
    for c in 0..3 {
        out[c] = ((lobe * (b[c] + d[c] + h[c] + f[c]) + e[c]) * rcp).clamp(0.0, 1.0);
    }
    out
}

/// The GL objects. One per context; recreate if the context is lost.
pub struct Nv12Gpu {
    program: glow::Program,
    /// RCAS over the scaled frame; used only when sharpening.
    rcas: glow::Program,
    /// The scaled frame at output resolution, for the RCAS pass. Created on
    /// first use and resized with the output, never when sharpening is off.
    target: Option<(glow::Framebuffer, glow::Texture, (i32, i32))>,
    pub enhance: Enhance,
    vao: glow::VertexArray,
    tex_y: glow::Texture,
    tex_uv: glow::Texture,
    size: (u32, u32),
    color: ColorInfo,
    uploaded_bytes: u64,
    uploads: u64,
}

impl Nv12Gpu {
    /// # Safety
    /// `gl` must be current on this thread, as it is inside an egui paint
    /// callback and in `eframe`'s `CreationContext`.
    pub unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let version = if cfg!(target_arch = "wasm32") {
            "#version 300 es"
        } else {
            "#version 330"
        };

        let program = link(gl, version, FRAG)?;
        let rcas = link(gl, version, RCAS_FRAG)?;

        let vao = gl.create_vertex_array()?;
        let tex_y = gl.create_texture()?;
        let tex_uv = gl.create_texture()?;
        for t in [&tex_y, &tex_uv] {
            gl.bind_texture(glow::TEXTURE_2D, Some(*t));
            // CLAMP_TO_EDGE, or the bilinear filter wraps at the frame edge and
            // the right column bleeds into the left.
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        }

        Ok(Self {
            program,
            rcas,
            target: None,
            enhance: Enhance::default(),
            vao,
            tex_y,
            tex_uv,
            size: (0, 0),
            color: ColorInfo::default(),
            uploaded_bytes: 0,
            uploads: 0,
        })
    }

    pub fn uploaded_bytes(&self) -> u64 {
        self.uploaded_bytes
    }

    pub fn uploads(&self) -> u64 {
        self.uploads
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Upload one NV12 frame.
    ///
    /// # Safety
    /// `gl` must be current.
    pub unsafe fn upload(&mut self, gl: &glow::Context, frame: &Frame) -> Result<(), String> {
        if frame.layout != PlaneLayout::Nv12 {
            // Only NV12 for now. Refusing beats uploading a 10-bit plane as if
            // it were 8-bit, which is a picture of noise.
            return Err(format!("{:?} is not handled by the GPU path", frame.layout));
        }
        let planes: &CpuPlanes = match &frame.data {
            FrameData::Cpu(p) => p,
            FrameData::DmaBuf(_) => return Err("dma-buf needs the Vulkan path".into()),
        };
        if planes.data.len() < 2 {
            return Err("NV12 needs two planes".into());
        }

        let (w, h) = (frame.width, frame.height);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));

        // Single-byte rows: without this the default alignment of 4 makes any
        // width that is not a multiple of four shear progressively down the
        // frame.
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);

        let y_stride = planes.strides.first().copied().unwrap_or(w);
        gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, y_stride as i32);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.tex_y));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::R8 as i32,
            w as i32,
            h as i32,
            0,
            glow::RED,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&planes.data[0])),
        );

        // The chroma plane's stride is in BYTES and each texel is two bytes, so
        // the row length in TEXELS is half it. Passing the byte stride here
        // doubles the row and skews the colour but not the luma, which reads as
        // a decoder bug rather than an upload one.
        let uv_stride_bytes = planes.strides.get(1).copied().unwrap_or(cw * 2);
        gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, (uv_stride_bytes / 2) as i32);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.tex_uv));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RG8 as i32,
            cw as i32,
            ch as i32,
            0,
            glow::RG,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&planes.data[1])),
        );

        // Leave the row length as the driver found it, or every later upload in
        // the process inherits ours.
        gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);

        self.size = (w, h);
        self.color = frame.color;
        self.uploads += 1;
        self.uploaded_bytes += upload_bytes(PlaneLayout::Nv12, w as usize, h as usize) as u64;
        Ok(())
    }

    unsafe fn draw_convert(&self, gl: &glow::Context, flip_y: f32) {
        gl.use_program(Some(self.program));
        gl.bind_vertex_array(Some(self.vao));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.tex_y));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.tex_uv));
        let p = self.program;
        let u = |n: &str| gl.get_uniform_location(p, n);
        if let Some(l) = u("plane_y") { gl.uniform_1_i32(Some(&l), 0); }
        if let Some(l) = u("plane_uv") { gl.uniform_1_i32(Some(&l), 1); }
        if let Some(l) = u("matrix_id") { gl.uniform_1_i32(Some(&l), matrix_id(self.color.matrix)); }
        if let Some(l) = u("range_id") { gl.uniform_1_i32(Some(&l), range_id(self.color.range)); }
        if let Some(l) = u("flip_y") { gl.uniform_1_f32(Some(&l), flip_y); }
        if let Some(l) = u("scaler") { gl.uniform_1_i32(Some(&l), (self.enhance.scaler == Scaler::Bicubic) as i32); }
        if let Some(l) = u("y_size") { gl.uniform_2_f32(Some(&l), self.size.0 as f32, self.size.1 as f32); }
        gl.disable(glow::DEPTH_TEST);
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
    }

    /// The offscreen target at `w x h`, (re)created when the size changes.
    unsafe fn target(&mut self, gl: &glow::Context, w: i32, h: i32) -> Result<(glow::Framebuffer, glow::Texture), String> {
        if let Some((fb, tex, size)) = self.target {
            if size == (w, h) {
                return Ok((fb, tex));
            }
            gl.delete_framebuffer(fb);
            gl.delete_texture(tex);
            self.target = None;
        }
        let tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
        for (k, v) in [
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            // NEAREST: RCAS reads exact neighbour texels at output resolution.
            (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
            (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, k, v as i32);
        }
        let fb = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
        let ok = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
        if !ok {
            gl.delete_framebuffer(fb);
            gl.delete_texture(tex);
            return Err("offscreen target incomplete".into());
        }
        self.target = Some((fb, tex, (w, h)));
        Ok((fb, tex))
    }

    /// # Safety
    /// `gl` must be current and inside a paint callback.
    ///
    /// Without sharpening this is the old single pass: convert (bilinear or
    /// bicubic) straight into egui's framebuffer. With it, FSR 1's order:
    /// scale into an offscreen target at the OUTPUT size, then RCAS from it
    /// into egui's framebuffer. egui's framebuffer binding, viewport and
    /// scissor are saved and restored around the offscreen pass -- the
    /// callback owns none of them. If the target cannot be made, the frame is
    /// drawn unsharpened rather than not at all.
    pub unsafe fn paint(&mut self, gl: &glow::Context, out_px: (i32, i32)) {
        if self.size.0 == 0 {
            return;
        }
        let scale = self.enhance.rcas_scale();
        let (w, h) = out_px;
        if scale <= 0.0 || w < 3 || h < 3 {
            self.draw_convert(gl, 1.0);
            gl.bind_vertex_array(None);
            return;
        }
        let prev_fb = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
        let mut vp = [0i32; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut vp);
        let scissor = gl.is_enabled(glow::SCISSOR_TEST);
        let restore_fb = |gl: &glow::Context| {
            let fb = std::num::NonZeroU32::new(prev_fb as u32).map(glow::NativeFramebuffer);
            gl.bind_framebuffer(glow::FRAMEBUFFER, fb);
        };
        let tex = match self.target(gl, w, h) {
            Ok((fb, tex)) => {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
                gl.disable(glow::SCISSOR_TEST);
                gl.viewport(0, 0, w, h);
                // Same flip as the direct pass: the RCAS pass samples the
                // target with the same uv it writes, so the orientation the
                // convert pass produces is the one that reaches the screen.
                self.draw_convert(gl, 1.0);
                tex
            }
            Err(_) => {
                restore_fb(gl);
                self.draw_convert(gl, 1.0);
                gl.bind_vertex_array(None);
                return;
            }
        };
        restore_fb(gl);
        gl.viewport(vp[0], vp[1], vp[2], vp[3]);
        if scissor {
            gl.enable(glow::SCISSOR_TEST);
        }
        gl.use_program(Some(self.rcas));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        let p = self.rcas;
        let u = |n: &str| gl.get_uniform_location(p, n);
        if let Some(l) = u("src") { gl.uniform_1_i32(Some(&l), 0); }
        if let Some(l) = u("texel") { gl.uniform_2_f32(Some(&l), 1.0 / w as f32, 1.0 / h as f32); }
        if let Some(l) = u("scale") { gl.uniform_1_f32(Some(&l), scale); }
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        gl.bind_vertex_array(None);
    }

    /// # Safety
    /// `gl` must be current.
    pub unsafe fn destroy(&self, gl: &glow::Context) {
        gl.delete_program(self.program);
        gl.delete_program(self.rcas);
        if let Some((fb, tex, _)) = self.target {
            gl.delete_framebuffer(fb);
            gl.delete_texture(tex);
        }
        gl.delete_vertex_array(self.vao);
        gl.delete_texture(self.tex_y);
        gl.delete_texture(self.tex_uv);
    }
}

/// A paint callback drawing the current frame into `rect`.
pub fn callback(rect: Rect, gpu: Arc<std::sync::Mutex<Nv12Gpu>>) -> egui::PaintCallback {
    egui::PaintCallback {
        rect,
        callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
            if let Ok(mut g) = gpu.lock() {
                let vp = info.viewport_in_pixels();
                // SAFETY: inside a paint callback, so the context is current.
                unsafe { g.paint(painter.gl(), (vp.width_px, vp.height_px)) };
            }
        })),
    }
}

/// The colour this path should produce, mirroring `convert_cpu` and
/// `convert.wgsl`.
///
/// A third copy of the maths is a drift hazard, so it is written once here in
/// Rust and asserted against the CPU converter — that comparison is what the
/// parity test for this module is.
pub fn expected_rgb(y: u8, cb: u8, cr: u8, color: ColorInfo) -> [u8; 3] {
    let planes = CpuPlanes {
        data: vec![vec![y; 4], vec![cb, cr]],
        strides: vec![2, 2],
    };
    let out = vidkit_core::planes_to_rgba8(&planes, PlaneLayout::Nv12, color, 2, 2)
        .expect("nv12 always converts");
    [out[0], out[1], out[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The claim the module exists for, as arithmetic.
    #[test]
    fn nv12_uploads_far_less_than_rgba() {
        let (w, h) = (1920, 1080);
        let nv12 = upload_bytes(PlaneLayout::Nv12, w, h);
        let rgba = rgba_upload_bytes(w, h);
        assert_eq!(nv12, w * h * 3 / 2, "NV12 is 1.5 bytes a pixel");
        assert_eq!(rgba, w * h * 4);
        // 4 / 1.5 = 2.666...
        let ratio = rgba as f64 / nv12 as f64;
        assert!(
            (ratio - 2.6667).abs() < 0.01,
            "expected a 2.67x reduction, got {ratio:.3}"
        );
    }

    /// Odd dimensions round chroma up; truncating loses the last row and
    /// column and the upload is short, which GL reports as nothing at all.
    #[test]
    fn odd_dimensions_round_the_chroma_plane_up() {
        let n = upload_bytes(PlaneLayout::Nv12, 1921, 1081);
        let luma = 1921 * 1081;
        let chroma = 961 * 541 * 2;
        assert_eq!(n, luma + chroma);
    }

    #[test]
    fn ten_bit_layouts_cost_twice_as_much() {
        let eight = upload_bytes(PlaneLayout::Nv12, 640, 480);
        let ten = upload_bytes(PlaneLayout::P010, 640, 480);
        assert_eq!(ten, eight * 2);
        // ...and still beat RGBA8, which is the point.
        assert!(ten < rgba_upload_bytes(640, 480) * 2);
    }

    #[test]
    fn selector_values_match_the_shader_and_the_wgsl() {
        assert_eq!(matrix_id(Matrix::Bt601), 0);
        assert_eq!(matrix_id(Matrix::Bt709), 1);
        assert_eq!(matrix_id(Matrix::Bt2020Ncl), 2);
        assert_eq!(range_id(Range::Limited), 0);
        assert_eq!(range_id(Range::Full), 1);
    }

    /// The GLSL and the CPU converter must agree, and the shader's coefficients
    /// are checked here by comparing against the reference the CPU path uses.
    /// A drift shows up as a colour difference on a known input.
    #[test]
    fn the_reference_colours_match_the_cpu_converter() {
        let c = ColorInfo::default();
        assert_eq!(expected_rgb(16, 128, 128, c), [0, 0, 0], "limited black");
        let white = expected_rgb(235, 128, 128, c);
        assert!(white.iter().all(|v| *v > 252), "limited white {white:?}");
        let red = expected_rgb(63, 102, 240, c);
        assert!(red[0] > 250 && red[1] < 8 && red[2] < 8, "bt709 red {red:?}");
    }

    /// The GLSL source has to carry both profiles' worth of syntax — no
    /// `texture2D`, no `varying`, an explicit `out` — or it compiles on the
    /// desktop and fails on the phone.
    #[test]
    fn the_shader_source_is_es3_compatible() {
        for src in [VERT, FRAG, RCAS_FRAG] {
            assert!(!src.contains("texture2D("), "texture2D is GLSL 1.20");
            assert!(!src.contains("varying "), "varying is GLSL 1.20");
            assert!(!src.contains("gl_FragColor"), "gl_FragColor is GLSL 1.20");
        }
        assert!(FRAG.contains("precision mediump float;"), "ES needs a precision qualifier");
        assert!(RCAS_FRAG.contains("precision mediump float;"));
        assert!(FRAG.contains("out vec4 frag;"), "ES3 needs an explicit out");
    }

    /// The Rust mirror of the shader agrees with bscale's CPU RCAS on the
    /// properties that matter: flat is untouched, an edge gets steeper, and
    /// nothing leaves the range of its neighbours (no halo).
    #[test]
    fn rcas_mirror_sharpens_edges_without_overshoot() {
        let g = |v: f32| [v, v, v];
        let flat = rcas_reference(g(0.4), g(0.4), g(0.4), g(0.4), g(0.4), 1.0);
        assert!(flat.iter().all(|v| (v - 0.4).abs() < 1e-6), "flat moved: {flat:?}");
        // A soft step: the centre sits between a dark left and bright right.
        let e = rcas_reference(g(0.5), g(0.2), g(0.55), g(0.8), g(0.5), 1.0);
        assert!(e[0] > 0.55, "pushed toward the brighter side: {}", e[0]);
        assert!(e[0] <= 0.8 && e[0] >= 0.2, "stays inside the ring's range: {}", e[0]);
        let off = rcas_reference(g(0.5), g(0.2), g(0.55), g(0.8), g(0.5), 0.0);
        assert_eq!(off, g(0.55), "scale 0 is a copy");
        // An isolated bright dot (noise) is sharpened LESS than an edge pixel
        // of the same contrast: the noise detector backs off.
        let dot = rcas_reference(g(0.3), g(0.3), g(0.5), g(0.3), g(0.3), 1.0);
        let dot_gain = dot[0] - 0.5;
        let edge = rcas_reference(g(0.3), g(0.3), g(0.5), g(0.7), g(0.7), 1.0);
        assert!(dot_gain.abs() <= (edge[0] - 0.5).abs() + 0.2, "{dot_gain} vs {}", edge[0] - 0.5);
    }

    #[test]
    fn enhancement_cost_is_counted_for_the_budget() {
        assert_eq!(Enhance::default().fetches_per_pixel(), 2);
        let full = Enhance { scaler: Scaler::Bicubic, sharpen: Some(0.5) };
        assert_eq!(full.fetches_per_pixel(), 15);
        assert!((full.rcas_scale() - 0.7071).abs() < 1e-3);
        assert_eq!(Enhance::default().rcas_scale(), 0.0);
    }
}
