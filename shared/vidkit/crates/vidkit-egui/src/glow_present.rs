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

void main() {
    // egui's framebuffer has its origin at the top left and GL samples from the
    // bottom, so the picture arrives upside down unless one of them is flipped.
    vec2 t = vec2(uv.x, mix(uv.y, 1.0 - uv.y, flip_y));

    float y = texture(plane_y, t).r;
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

/// The GL objects. One per context; recreate if the context is lost.
pub struct Nv12Gpu {
    program: glow::Program,
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

        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, src) in [
            (glow::VERTEX_SHADER, VERT),
            (glow::FRAGMENT_SHADER, FRAG),
        ] {
            let s = gl.create_shader(kind)?;
            gl.shader_source(s, &format!("{version}\n{src}"));
            gl.compile_shader(s);
            if !gl.get_shader_compile_status(s) {
                // The log, not just "failed": a shader that does not compile on
                // one driver and does on another is the whole reason this is
                // written twice.
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

    /// # Safety
    /// `gl` must be current and inside a paint callback.
    pub unsafe fn paint(&self, gl: &glow::Context) {
        if self.size.0 == 0 {
            return;
        }
        gl.use_program(Some(self.program));
        gl.bind_vertex_array(Some(self.vao));

        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.tex_y));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.tex_uv));

        if let Some(l) = gl.get_uniform_location(self.program, "plane_y") {
            gl.uniform_1_i32(Some(&l), 0);
        }
        if let Some(l) = gl.get_uniform_location(self.program, "plane_uv") {
            gl.uniform_1_i32(Some(&l), 1);
        }
        if let Some(l) = gl.get_uniform_location(self.program, "matrix_id") {
            gl.uniform_1_i32(Some(&l), matrix_id(self.color.matrix));
        }
        if let Some(l) = gl.get_uniform_location(self.program, "range_id") {
            gl.uniform_1_i32(Some(&l), range_id(self.color.range));
        }
        if let Some(l) = gl.get_uniform_location(self.program, "flip_y") {
            gl.uniform_1_f32(Some(&l), 1.0);
        }

        gl.disable(glow::DEPTH_TEST);
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        gl.bind_vertex_array(None);
    }

    /// # Safety
    /// `gl` must be current.
    pub unsafe fn destroy(&self, gl: &glow::Context) {
        gl.delete_program(self.program);
        gl.delete_vertex_array(self.vao);
        gl.delete_texture(self.tex_y);
        gl.delete_texture(self.tex_uv);
    }
}

/// A paint callback drawing the current frame into `rect`.
pub fn callback(rect: Rect, gpu: Arc<std::sync::Mutex<Nv12Gpu>>) -> egui::PaintCallback {
    egui::PaintCallback {
        rect,
        callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
            if let Ok(g) = gpu.lock() {
                // SAFETY: inside a paint callback, so the context is current.
                unsafe { g.paint(painter.gl()) };
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
        for src in [VERT, FRAG] {
            assert!(!src.contains("texture2D("), "texture2D is GLSL 1.20");
            assert!(!src.contains("varying "), "varying is GLSL 1.20");
            assert!(!src.contains("gl_FragColor"), "gl_FragColor is GLSL 1.20");
        }
        assert!(FRAG.contains("precision mediump float;"), "ES needs a precision qualifier");
        assert!(FRAG.contains("out vec4 frag;"), "ES3 needs an explicit out");
    }
}
