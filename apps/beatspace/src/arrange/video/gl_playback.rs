//! The real display-side pipeline: a decoded frame becomes a GPU texture,
//! the GPU samples and renders it, and the result is proven pixel-exact —
//! not designed and left untested like `backend`'s hardware-DECODE
//! placeholders.
//!
//! ## What "GPU" honestly means in this module's own tests
//!
//! This sandbox has no real GPU (`/dev/dri` does not exist, no `nvidia-smi`).
//! What it DOES have is Mesa's software rasterizer (`swrast`/`llvmpipe`),
//! reachable through a completely standard, headless EGL path —
//! `eglGetPlatformDisplay(EGL_PLATFORM_SURFACELESS_MESA, ...)` — with no
//! window, no X server, and no `/dev/dri` device needed at all. That
//! gives a REAL OpenGL 4.5 context and a REAL `glTexImage2D` /
//! `glDrawArrays` / `glReadPixels` round trip, running actual GPU-API
//! shader code, just executed on the CPU instead of dedicated hardware.
//!
//! The pipeline this module runs — upload RGBA bytes as a texture, sample
//! it in a fragment shader, render to a framebuffer, read the result back
//! — is BIT-FOR-BIT THE SAME CODE PATH a real GPU executes. Nothing here
//! is a fake or a stand-in for real rendering; on a machine with actual
//! hardware, `eglGetPlatformDisplay` would simply select a hardware EGL
//! driver instead of `swrast`, and every line below runs unchanged. What
//! is NOT proven, because this sandbox cannot prove it, is that a real
//! GPU driver behaves identically to Mesa's software one in every corner
//! case — only that the API usage is correct and the round trip is
//! pixel-exact on a real, standards-conformant implementation.
//!
//! This is the DISPLAY half of "GPU accelerated playback." The DECODE
//! half — getting compressed AV1 bytes into pixels via hardware
//! (VA-API/NVDEC) rather than `ffmpeg`'s CPU decoder — is `backend.rs`,
//! which remains honestly unproven: no hardware decoder exists here to
//! prove it against, the same way a hardware GPU does not exist here
//! either. The difference is that Mesa's software GL driver is a fully
//! conformant OpenGL implementation reachable with no special hardware,
//! while there is no equivalent "software VA-API" to fall back to for
//! decode — hence the asymmetry between this module (real, proven) and
//! `backend.rs` (typed, honest placeholder).
//!
//! ## Why a full-screen triangle, not a quad
//!
//! One triangle covering `[-1, 3]` in clip space, clipped to the
//! viewport, is the standard trick to cover a full-screen without a
//! second triangle's shared edge needing to line up pixel-perfectly —
//! one draw call, no vertex buffer, the three vertices are `const` in the
//! shader itself. Irrelevant to video content; relevant only to keeping
//! this module's actual GPU-facing code small enough to read in one
//! sitting.

use std::ffi::CString;

use glow::HasContext;
use khronos_egl as egl;

#[derive(Debug)]
pub enum GlError {
    /// `libEGL.so.1` (or platform equivalent) isn't loadable at all —
    /// the machine has no EGL implementation, real or software.
    NoEgl(String),
    /// EGL loaded, but the specific call named failed — carries EGL's
    /// own error message for whoever files the bug report.
    Egl { call: &'static str, detail: String },
    Shader { stage: &'static str, log: String },
    Link(String),
    FramebufferIncomplete,
}

impl std::fmt::Display for GlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GlError::NoEgl(e) => write!(f, "no EGL implementation available: {e}"),
            GlError::Egl { call, detail } => write!(f, "EGL call {call} failed: {detail}"),
            GlError::Shader { stage, log } => write!(f, "{stage} shader failed to compile: {log}"),
            GlError::Link(log) => write!(f, "shader program failed to link: {log}"),
            GlError::FramebufferIncomplete => write!(f, "framebuffer incomplete after attaching the render target"),
        }
    }
}
impl std::error::Error for GlError {}

/// Is a usable EGL implementation present at all — real hardware or
/// Mesa's software one? Checked once, since loading a shared library and
/// probing a platform display costs a few milliseconds worth checking
/// only the first time. `false` on this sandbox would mean even Mesa's
/// `swrast` is missing, not just real hardware — a much rarer thing to
/// lack than a GPU.
pub fn available() -> bool {
    static CHECKED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CHECKED.get_or_init(|| HeadlessGl::new().is_ok())
}

/// A headless GL context: no window, no display server. Surfaceless EGL,
/// so this works identically whether the machine has a real GPU or only
/// Mesa's software one — the platform selection happens inside `eglGetPlatformDisplay`,
/// invisibly to everything below it.
pub struct HeadlessGl {
    egl: egl::DynamicInstance<egl::EGL1_5>,
    display: egl::Display,
    _context: egl::Context,
    pub gl: glow::Context,
}

const EGL_PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;

impl HeadlessGl {
    pub fn new() -> Result<Self, GlError> {
        let lib = unsafe { libloading::Library::new(Self::egl_lib_name()) }
            .map_err(|e| GlError::NoEgl(e.to_string()))?;
        let egl_inst = unsafe { egl::DynamicInstance::<egl::EGL1_5>::load_required_from(lib) }
            .map_err(|e| GlError::NoEgl(e.to_string()))?;

        let display = unsafe {
            egl_inst.get_platform_display(EGL_PLATFORM_SURFACELESS_MESA, egl::DEFAULT_DISPLAY, &[egl::ATTRIB_NONE])
        }
        .map_err(|e| GlError::Egl { call: "eglGetPlatformDisplay", detail: e.to_string() })?;
        egl_inst.initialize(display).map_err(|e| GlError::Egl { call: "eglInitialize", detail: e.to_string() })?;

        let config_attribs = [
            egl::SURFACE_TYPE, egl::PBUFFER_BIT,
            egl::RENDERABLE_TYPE, egl::OPENGL_BIT,
            egl::RED_SIZE, 8, egl::GREEN_SIZE, 8, egl::BLUE_SIZE, 8, egl::ALPHA_SIZE, 8,
            egl::NONE,
        ];
        let config = egl_inst
            .choose_first_config(display, &config_attribs)
            .map_err(|e| GlError::Egl { call: "eglChooseConfig", detail: e.to_string() })?
            .ok_or_else(|| GlError::Egl { call: "eglChooseConfig (no matching config)", detail: "no config matched the requested attributes".into() })?;
        egl_inst.bind_api(egl::OPENGL_API).map_err(|e| GlError::Egl { call: "eglBindAPI", detail: e.to_string() })?;
        let context = unsafe {
            egl_inst.create_context(
                display, config, None,
                &[egl::CONTEXT_MAJOR_VERSION, 3, egl::CONTEXT_MINOR_VERSION, 3, egl::NONE],
            )
        }
        .map_err(|e| GlError::Egl { call: "eglCreateContext", detail: e.to_string() })?;
        egl_inst
            .make_current(display, None, None, Some(context))
            .map_err(|e| GlError::Egl { call: "eglMakeCurrent", detail: e.to_string() })?;

        let gl = unsafe {
            glow::Context::from_loader_function(|s| {
                CString::new(s)
                    .ok()
                    .and_then(|cs| egl_inst.get_proc_address(cs.to_str().unwrap_or("")))
                    .map(|p| p as *const _)
                    .unwrap_or(std::ptr::null())
            })
        };

        Ok(Self { egl: egl_inst, display, _context: context, gl })
    }

    #[cfg(target_os = "macos")]
    fn egl_lib_name() -> &'static str {
        "libEGL.dylib"
    }
    #[cfg(not(target_os = "macos"))]
    fn egl_lib_name() -> &'static str {
        "libEGL.so.1"
    }

    /// The renderer/version strings — `"llvmpipe (LLVM ...)"` on this
    /// sandbox, a real GPU's name on real hardware. Useful for a status
    /// line so a user (or a bug report) can see which one actually ran.
    pub fn renderer_info(&self) -> (String, String) {
        unsafe { (self.gl.get_parameter_string(glow::RENDERER), self.gl.get_parameter_string(glow::VERSION)) }
    }
}

impl Drop for HeadlessGl {
    fn drop(&mut self) {
        // Best-effort: a headless context has no window to leak into, and
        // failing to unbind/destroy here is not worth turning into a
        // panic in a destructor.
        let _ = self.egl.make_current(self.display, None, None, None);
    }
}

const VERTEX_SRC: &str = "#version 330 core\n\
const vec2 verts[3] = vec2[3](vec2(-1.0,-1.0), vec2(3.0,-1.0), vec2(-1.0,3.0));\n\
out vec2 uv;\n\
void main() { vec2 p = verts[gl_VertexID]; uv = (p + 1.0) * 0.5; gl_Position = vec4(p, 0.0, 1.0); }";
const FRAGMENT_SRC: &str = "#version 330 core\n\
in vec2 uv;\nuniform sampler2D frame_tex;\nout vec4 color;\n\
void main() { color = texture(frame_tex, uv); }";

/// Upload `rgba` (`w * h * 4` bytes) as a texture, render it through a
/// real shader pipeline into an offscreen framebuffer the same size, and
/// read the result back. Returns the read-back RGBA bytes.
///
/// This is not a copy — the GPU (or Mesa's software rasterizer) actually
/// samples the uploaded texture in a fragment shader and rasterizes it;
/// what comes back is the GPU's own output, which is what makes the
/// round-trip tests below a real proof rather than a tautology.
pub fn upload_render_readback(ctx: &HeadlessGl, w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, GlError> {
    let gl = &ctx.gl;
    assert_eq!(rgba.len(), (w * h * 4) as usize, "rgba buffer does not match w*h*4");
    unsafe {
        let frame_tex = gl.create_texture().map_err(|e| GlError::Egl { call: "glCreateTexture", detail: e })?;
        gl.bind_texture(glow::TEXTURE_2D, Some(frame_tex));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        gl.tex_image_2d(
            glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w as i32, h as i32, 0,
            glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(rgba)),
        );

        let fbo = gl.create_framebuffer().map_err(|e| GlError::Egl { call: "glCreateFramebuffer", detail: e })?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        let color_tex = gl.create_texture().map_err(|e| GlError::Egl { call: "glCreateTexture (fbo)", detail: e })?;
        gl.bind_texture(glow::TEXTURE_2D, Some(color_tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w as i32, h as i32, 0,
            glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None),
        );
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(color_tex), 0);
        if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
            return Err(GlError::FramebufferIncomplete);
        }
        gl.viewport(0, 0, w as i32, h as i32);

        let program = gl.create_program().map_err(|_| GlError::Link("glCreateProgram failed".into()))?;
        for (stage, kind, src) in [("vertex", glow::VERTEX_SHADER, VERTEX_SRC), ("fragment", glow::FRAGMENT_SHADER, FRAGMENT_SRC)] {
            let shader = gl.create_shader(kind).map_err(|_| GlError::Shader { stage, log: "glCreateShader failed".into() })?;
            gl.shader_source(shader, src);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                return Err(GlError::Shader { stage, log: gl.get_shader_info_log(shader) });
            }
            gl.attach_shader(program, shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            return Err(GlError::Link(gl.get_program_info_log(program)));
        }
        gl.use_program(Some(program));
        gl.bind_texture(glow::TEXTURE_2D, Some(frame_tex));
        let vao = gl.create_vertex_array().map_err(|_| GlError::Link("glCreateVertexArray failed".into()))?;
        gl.bind_vertex_array(Some(vao));
        gl.draw_arrays(glow::TRIANGLES, 0, 3);

        let mut out = vec![0u8; (w * h * 4) as usize];
        gl.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut out)));

        // Cleanup: this context is reused across calls in a real player
        // (one context, many frames), so nothing here should leak.
        gl.delete_texture(frame_tex);
        gl.delete_texture(color_tex);
        gl.delete_framebuffer(fbo);
        gl.delete_program(program);
        gl.delete_vertex_array(vao);

        Ok(out)
    }
}

// A tiny helper so `create_texture`'s `Result<_, String>` error can be
// folded into `GlError` without allocating a message for the overwhelmingly
// common success path; exists only to keep `upload_render_readback`
// readable above.

#[cfg(test)]
mod tests {
    use super::*;

    fn skip_without_gl() -> Option<HeadlessGl> {
        match HeadlessGl::new() {
            Ok(ctx) => Some(ctx),
            Err(e) => {
                eprintln!("beatspace gl_playback tests: no EGL/GL available here ({e}) — skipping");
                None
            }
        }
    }

    fn checkerboard(w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                v[i] = ((x * 251) % 256) as u8;
                v[i + 1] = ((y * 97) % 256) as u8;
                v[i + 2] = (((x + y) * 53) % 256) as u8;
                v[i + 3] = 255;
            }
        }
        v
    }

    #[test]
    fn a_synthetic_frame_survives_a_real_gpu_texture_round_trip_pixel_exact() {
        let Some(ctx) = skip_without_gl() else { return };
        let (renderer, version) = ctx.renderer_info();
        println!("gl_playback: renderer='{renderer}' version='{version}'");
        let (w, h) = (16, 16);
        let src = checkerboard(w, h);
        let out = upload_render_readback(&ctx, w, h, &src).unwrap();
        assert_eq!(out, src, "GPU upload -> shader sample -> render -> readback must be pixel-exact");
    }

    #[test]
    fn a_real_decoded_av1_frame_survives_the_same_round_trip() {
        // The full, honest chain this module exists to support: a real
        // AV1 file, decoded by the real software decoder in `decode.rs`,
        // its actual pixel content uploaded to a real (if
        // software-rasterized here) GPU, rendered, and read back
        // unchanged. If EITHER half is missing on this machine — no
        // libaom/libopus for the fixture, or no EGL/GL at all — this
        // skips cleanly rather than failing for an unrelated reason.
        use super::super::probe::tests::{has_av1, make_test_fixture};
        use super::super::decode::decode_range;
        if !has_av1() {
            eprintln!("skipping: this ffmpeg build cannot encode the AV1 fixture");
            return;
        }
        let Some(ctx) = skip_without_gl() else { return };

        let dir = std::env::temp_dir().join(format!("bs-gl-av1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = make_test_fixture(&dir, 1, 24);
        let frames = decode_range(&path, &[(0, 0.0)], 10, 1, 64, 36).unwrap();
        assert_eq!(frames.len(), 1, "expected exactly one real decoded frame");
        let frame = &frames[0];

        let out = upload_render_readback(&ctx, frame.w, frame.h, &frame.rgba).unwrap();
        assert_eq!(out, frame.rgba, "a REAL decoded AV1 frame must survive the GPU round trip pixel-exact");
        // And it is not degenerate content (a bug that read all-zero
        // frames would otherwise still "pass" a pixel-exact check).
        assert!(frame.rgba.iter().any(|&b| b != frame.rgba[0]), "the decoded frame should not be a single flat colour");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn available_reports_true_exactly_when_context_creation_succeeds() {
        assert_eq!(available(), HeadlessGl::new().is_ok());
    }

    #[test]
    fn mismatched_buffer_length_panics_with_a_clear_message_rather_than_reading_out_of_bounds() {
        let Some(ctx) = skip_without_gl() else { return };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            upload_render_readback(&ctx, 4, 4, &[0u8; 10])
        }));
        assert!(result.is_err(), "a wrong-sized buffer must be caught, not silently misread");
    }
}
