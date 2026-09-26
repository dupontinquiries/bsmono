//! The AI half-rate frame synthesiser on the GPU: vidkit-core's `aiframes`
//! (the specification, itself checked against PyTorch) as fragment passes.
//!
//! Everything below full size runs at 1/16, 1/8 and 1/4 of the frame:
//! pooling, two Lucas-Kanade passes per iteration, one pass per CNN layer.
//! What comes out is a flow + mask field at 1/4 and a residual whose mipmap
//! top is the frame's mean -- the conversion shader samples those to warp
//! and blend the two neighbours at full size (four fetches a pixel).
//!
//! Conventions match the reference exactly: integer texel fetches for the
//! convolutions (zero padding by bounds check), texture() with LINEAR and
//! CLAMP_TO_EDGE for warps and upsampling (= grid_sample border /
//! interpolate align_corners=False), texel-centre coordinates throughout.
//! Weights are vec4 uniforms (144 for the largest layer): a float[576]
//! array can occupy 576 VECTORS on GLES, over the 224 guaranteed.

use eframe::glow::{self, HasContext as _};
use vidkit_core::aiframes_weights as wts;

const HEADER: &str = "#version 330\nprecision highp float;\nprecision highp int;\n";

const VERT: &str = r#"
void main() {
    vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// avg_pool of `src` by `k` (4 from full size; 2 between levels).
const POOL: &str = r#"
uniform sampler2D src;
uniform int k;
out vec4 o;
void main() {
    ivec2 p = ivec2(gl_FragCoord.xy) * k;
    float s = 0.0;
    for (int y = 0; y < k; y++)
        for (int x = 0; x < k; x++)
            s += texelFetch(src, p + ivec2(x, y), 0).r;
    o = vec4(s / float(k * k));
}
"#;

/// Coarse flow to this level: bilinear, doubled.
const UPFLOW: &str = r#"
uniform sampler2D coarse;
uniform vec2 dims;
out vec4 o;
void main() { o = texture(coarse, gl_FragCoord.xy / dims) * 2.0; }
"#;

/// Clear the flow at the coarsest level.
const ZERO: &str = r#"
out vec4 o;
void main() { o = vec4(0.0); }
"#;

/// Lucas-Kanade pass A: warp both planes (at p and its 4 neighbours,
/// replicate-clamped), gradients of their sum, products.
const LK_A: &str = r#"
uniform sampler2D a;
uniform sampler2D c;
uniform sampler2D flow;
uniform vec2 dims;
layout(location = 0) out vec4 o0;
layout(location = 1) out vec4 o1;
ivec2 cl(ivec2 q) { return clamp(q, ivec2(0), ivec2(dims) - 1); }
vec2 wac(ivec2 q) {
    vec4 f = texelFetch(flow, q, 0);
    vec2 base = vec2(q) + 0.5;
    return vec2(texture(a, (base + f.xy) / dims).r, texture(c, (base + f.zw) / dims).r);
}
float s(ivec2 q) { vec2 v = wac(cl(q)); return v.x + v.y; }
void main() {
    ivec2 p = ivec2(gl_FragCoord.xy);
    float gx = 0.5 * (s(p + ivec2(1, 0)) - s(p - ivec2(1, 0)));
    float gy = 0.5 * (s(p + ivec2(0, 1)) - s(p - ivec2(0, 1)));
    vec2 w = wac(p);
    float it = w.y - w.x;
    o0 = vec4(gx * gx, gx * gy, gy * gy, gx * it);
    o1 = vec4(gy * it, 0.0, 0.0, 0.0);
}
"#;

/// Lucas-Kanade pass B: box-average the products, solve, update the flow.
const LK_B: &str = r#"
uniform sampler2D p0;
uniform sampler2D p1;
uniform sampler2D flow;
uniform vec2 dims;
uniform int r;
out vec4 o;
void main() {
    ivec2 p = ivec2(gl_FragCoord.xy);
    vec4 s0 = vec4(0.0);
    float s1 = 0.0;
    for (int y = -r; y <= r; y++)
        for (int x = -r; x <= r; x++) {
            ivec2 q = clamp(p + ivec2(x, y), ivec2(0), ivec2(dims) - 1);
            s0 += texelFetch(p0, q, 0);
            s1 += texelFetch(p1, q, 0).r;
        }
    float n = float((2 * r + 1) * (2 * r + 1));
    float a11 = s0.x / n, a12 = s0.y / n, a22 = s0.z / n, b1 = s0.w / n, b2 = s1 / n;
    float tr = a11 + a22;
    float det = a11 * a22 - a12 * a12 + 1e-3 * tr * tr + 1e-12;
    vec2 d = clamp(vec2(a22 * b1 - a12 * b2, a11 * b2 - a12 * b1) / det, -2.0, 2.0);
    o = texelFetch(flow, p, 0) + vec4(d, -d);
}
"#;

/// CNN input at 1/4: warped prev, warped next, difference, flow * 0.25.
const CNN_IN: &str = r#"
uniform sampler2D a;
uniform sampler2D c;
uniform sampler2D flow;
uniform vec2 dims;
layout(location = 0) out vec4 o0;
layout(location = 1) out vec4 o1;
void main() {
    ivec2 p = ivec2(gl_FragCoord.xy);
    vec4 f = texelFetch(flow, p, 0);
    vec2 base = vec2(p) + 0.5;
    float wa = texture(a, (base + f.xy) / dims).r;
    float wc = texture(c, (base + f.zw) / dims).r;
    o0 = vec4(wa, wc, wa - wc, f.x * 0.25);
    o1 = vec4(f.y * 0.25, f.z * 0.25, f.w * 0.25, 0.0);
}
"#;

/// One dilated 3x3 convolution, up to 8 in / 8 out, zero padding.
const CONV: &str = r#"
uniform sampler2D in0;
uniform sampler2D in1;
uniform int cin;
uniform int cout;
uniform int dil;
uniform int act;
uniform vec4 w4[144];
uniform vec4 b4[2];
uniform ivec2 size;
layout(location = 0) out vec4 o0;
layout(location = 1) out vec4 o1;
float W(int i) { return w4[i / 4][i % 4]; }
void main() {
    ivec2 p = ivec2(gl_FragCoord.xy);
    float acc[8];
    for (int o = 0; o < 8; o++) acc[o] = b4[o / 4][o % 4];
    for (int ky = 0; ky < 3; ky++)
        for (int kx = 0; kx < 3; kx++) {
            ivec2 q = p + ivec2(kx - 1, ky - 1) * dil;
            if (q.x < 0 || q.y < 0 || q.x >= size.x || q.y >= size.y) continue;
            vec4 v0 = texelFetch(in0, q, 0);
            vec4 v1 = texelFetch(in1, q, 0);
            float x[8] = float[8](v0.x, v0.y, v0.z, v0.w, v1.x, v1.y, v1.z, v1.w);
            for (int o = 0; o < 8; o++) {
                if (o >= cout) break;
                for (int i = 0; i < 8; i++) {
                    if (i >= cin) break;
                    acc[o] += W(((o * cin + i) * 3 + ky) * 3 + kx) * x[i];
                }
            }
        }
    if (act == 1)
        for (int o = 0; o < 8; o++) acc[o] = acc[o] > 0.0 ? acc[o] : 0.1 * acc[o];
    o0 = vec4(acc[0], acc[1], acc[2], acc[3]);
    o1 = vec4(acc[4], acc[5], acc[6], acc[7]);
}
"#;

/// Final flow = Lucas-Kanade + CNN correction; mask logit; residual.
const COMBINE: &str = r#"
uniform sampler2D lk;
uniform sampler2D h0;
uniform sampler2D h1;
uniform sampler2D a;
uniform sampler2D c;
uniform vec2 dims;
layout(location = 0) out vec4 flow;
layout(location = 1) out vec4 mask;
layout(location = 2) out vec4 resid;
void main() {
    ivec2 p = ivec2(gl_FragCoord.xy);
    vec4 f = texelFetch(lk, p, 0) + texelFetch(h0, p, 0);
    flow = f;
    mask = vec4(texelFetch(h1, p, 0).r);
    vec2 base = vec2(p) + 0.5;
    resid = vec4(abs(texture(a, (base + f.xy) / dims).r - texture(c, (base + f.zw) / dims).r));
}
"#;

/// Luma-only synthesis at full size, for the parity self-test. The
/// presenter's conversion shader does the same inline with colour.
const SYNTH_Y: &str = r#"
uniform sampler2D y0;
uniform sampler2D y2;
uniform sampler2D flow;
uniform sampler2D mask;
uniform vec2 dims;
uniform vec2 fdims;
out vec4 o;
void main() {
    vec2 t = gl_FragCoord.xy / dims;
    vec4 f = texture(flow, t);
    float m = 1.0 / (1.0 + exp(-texture(mask, t).r));
    o = vec4(m * texture(y0, t + f.xy / fdims).r + (1.0 - m) * texture(y2, t + f.zw / fdims).r);
}
"#;

struct Target {
    fbo: glow::Framebuffer,
    tex: Vec<glow::Texture>,
    w: i32,
    h: i32,
}

struct Level {
    a: Target,
    c: Target,
    flow: [Target; 2],
    prod: Target,
}

pub struct AiFramesGl {
    vao: glow::VertexArray,
    pool: glow::Program,
    upflow: glow::Program,
    zero: glow::Program,
    lk_a: glow::Program,
    lk_b: glow::Program,
    cnn_in: glow::Program,
    conv: glow::Program,
    combine: glow::Program,
    synth_y: glow::Program,
    size: (u32, u32),
    levels: Vec<Level>,
    feat: Vec<Target>,
    head: Option<Target>,
    out: Option<Target>,
    /// Passes run last time, for the diagnostics line.
    pub passes: u32,
    /// Which of each level's two flow targets holds the final flow.
    lk_cur: usize,
}

unsafe fn program(gl: &glow::Context, frag: &str) -> Result<glow::Program, String> {
    let p = gl.create_program()?;
    let mut shaders = Vec::new();
    for (ty, src) in [(glow::VERTEX_SHADER, VERT), (glow::FRAGMENT_SHADER, frag)] {
        let s = gl.create_shader(ty)?;
        gl.shader_source(s, &format!("{HEADER}{src}"));
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            return Err(format!("aiframes shader: {}", gl.get_shader_info_log(s)));
        }
        gl.attach_shader(p, s);
        shaders.push(s);
    }
    gl.link_program(p);
    if !gl.get_program_link_status(p) {
        return Err(format!("aiframes link: {}", gl.get_program_info_log(p)));
    }
    for s in shaders {
        gl.detach_shader(p, s);
        gl.delete_shader(s);
    }
    Ok(p)
}

/// A render target with `formats` attachments (internal format, format).
unsafe fn target(gl: &glow::Context, w: i32, h: i32, formats: &[(u32, u32)], mip: bool) -> Result<Target, String> {
    let fbo = gl.create_framebuffer()?;
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    let mut tex = Vec::new();
    for (i, &(internal, format)) in formats.iter().enumerate() {
        let t = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(t));
        gl.tex_image_2d(glow::TEXTURE_2D, 0, internal as i32, w, h, 0, format, glow::FLOAT, glow::PixelUnpackData::Slice(None));
        let min = if mip { glow::LINEAR_MIPMAP_LINEAR } else { glow::LINEAR };
        for (k, v) in [(glow::TEXTURE_MIN_FILTER, min), (glow::TEXTURE_MAG_FILTER, glow::LINEAR), (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE), (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE)] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, k, v as i32);
        }
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0 + i as u32, glow::TEXTURE_2D, Some(t), 0);
        tex.push(t);
    }
    if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
        return Err("aiframes target incomplete (float render targets unsupported?)".into());
    }
    Ok(Target { fbo, tex, w, h })
}

unsafe fn free(gl: &glow::Context, t: &Target) {
    gl.delete_framebuffer(t.fbo);
    for x in &t.tex {
        gl.delete_texture(*x);
    }
}

const R16F: (u32, u32) = (glow::R16F, glow::RED);
const RGBA16F: (u32, u32) = (glow::RGBA16F, glow::RGBA);
/// Lucas-Kanade products are ~1e-6 on flat content: fp16's subnormal
/// range. Only texelFetch'd, so no float-linear filtering is needed.
const RGBA32F: (u32, u32) = (glow::RGBA32F, glow::RGBA);

impl AiFramesGl {
    /// # Safety
    /// `gl` must be current.
    pub unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        Ok(Self {
            vao: gl.create_vertex_array()?,
            pool: program(gl, POOL)?,
            upflow: program(gl, UPFLOW)?,
            zero: program(gl, ZERO)?,
            lk_a: program(gl, LK_A)?,
            lk_b: program(gl, LK_B)?,
            cnn_in: program(gl, CNN_IN)?,
            conv: program(gl, CONV)?,
            combine: program(gl, COMBINE)?,
            synth_y: program(gl, SYNTH_Y)?,
            size: (0, 0),
            levels: Vec::new(),
            feat: Vec::new(),
            head: None,
            out: None,
            passes: 0,
            lk_cur: 0,
        })
    }

    unsafe fn ensure(&mut self, gl: &glow::Context, w: u32, h: u32) -> Result<(), String> {
        if self.size == (w, h) {
            return Ok(());
        }
        self.release(gl);
        for &lv in wts::LEVELS.iter() {
            let (lw, lh) = ((w as usize / lv) as i32, (h as usize / lv) as i32);
            self.levels.push(Level {
                a: target(gl, lw, lh, &[R16F], false)?,
                c: target(gl, lw, lh, &[R16F], false)?,
                flow: [target(gl, lw, lh, &[RGBA16F], false)?, target(gl, lw, lh, &[RGBA16F], false)?],
                prod: target(gl, lw, lh, &[RGBA32F, RGBA32F], false)?,
            });
        }
        let (qw, qh) = ((w / 4) as i32, (h / 4) as i32);
        self.feat = vec![target(gl, qw, qh, &[RGBA16F, RGBA16F], false)?, target(gl, qw, qh, &[RGBA16F, RGBA16F], false)?];
        self.head = Some(target(gl, qw, qh, &[RGBA16F, RGBA16F], false)?);
        let out = target(gl, qw, qh, &[RGBA16F, R16F, R16F], true)?;
        // Only the residual gets mipmaps. A mipmapped min filter on the
        // flow and mask would leave them mip-INCOMPLETE, and an incomplete
        // texture samples as (0,0,0,1) -- zero flow, fixed mask: a plain
        // blend, silently (the GPU self-test caught exactly this).
        for &t in &out.tex[..2] {
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        }
        self.out = Some(out);
        self.size = (w, h);
        Ok(())
    }

    /// # Safety
    /// `gl` must be current.
    pub unsafe fn release(&mut self, gl: &glow::Context) {
        for l in self.levels.drain(..) {
            for t in [&l.a, &l.c, &l.flow[0], &l.flow[1], &l.prod] {
                free(gl, t);
            }
        }
        for t in self.feat.drain(..).chain(self.head.take()).chain(self.out.take()) {
            free(gl, &t);
        }
        self.size = (0, 0);
    }

    /// Final (flow, mask logit, residual) textures at 1/4, after [`run`].
    pub fn outputs(&self) -> Option<(glow::Texture, glow::Texture, glow::Texture, (i32, i32))> {
        self.out.as_ref().map(|o| (o.tex[0], o.tex[1], o.tex[2], (o.w, o.h)))
    }

    unsafe fn draw(&mut self, gl: &glow::Context, t: &Target, prog: glow::Program, bind: &[(&str, glow::Texture)], setup: &dyn Fn(&glow::Context, glow::Program)) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.fbo));
        let bufs: Vec<u32> = (0..t.tex.len() as u32).map(|i| glow::COLOR_ATTACHMENT0 + i).collect();
        gl.draw_buffers(&bufs);
        gl.viewport(0, 0, t.w, t.h);
        gl.use_program(Some(prog));
        for (unit, (name, tex)) in bind.iter().enumerate() {
            gl.active_texture(glow::TEXTURE0 + unit as u32);
            gl.bind_texture(glow::TEXTURE_2D, Some(*tex));
            if let Some(l) = gl.get_uniform_location(prog, name) {
                gl.uniform_1_i32(Some(&l), unit as i32);
            }
        }
        if let Some(l) = gl.get_uniform_location(prog, "dims") {
            gl.uniform_2_f32(Some(&l), t.w as f32, t.h as f32);
        }
        setup(gl, prog);
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        self.passes += 1;
    }

    /// Estimate the motion between two full-size luma textures (R8 or
    /// R16F, LINEAR, CLAMP_TO_EDGE). Leaves GL state (framebuffer,
    /// viewport, program, blend, scissor, depth) as it found them.
    ///
    /// # Safety
    /// `gl` must be current.
    pub unsafe fn run(&mut self, gl: &glow::Context, prev_y: glow::Texture, next_y: glow::Texture, w: u32, h: u32) -> Result<(), String> {
        let prev_fb = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
        let mut vp = [0i32; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut vp);
        let (scissor, blend, depth) = (gl.is_enabled(glow::SCISSOR_TEST), gl.is_enabled(glow::BLEND), gl.is_enabled(glow::DEPTH_TEST));
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::BLEND);
        gl.disable(glow::DEPTH_TEST);
        gl.bind_vertex_array(Some(self.vao));
        let r = self.run_inner(gl, prev_y, next_y, w, h);
        gl.bind_vertex_array(None);
        gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
        let fb = std::num::NonZeroU32::new(prev_fb as u32).map(glow::NativeFramebuffer);
        gl.bind_framebuffer(glow::FRAMEBUFFER, fb);
        if fb.is_none() {
            gl.draw_buffer(glow::BACK);
        }
        gl.viewport(vp[0], vp[1], vp[2], vp[3]);
        if scissor {
            gl.enable(glow::SCISSOR_TEST);
        }
        if blend {
            gl.enable(glow::BLEND);
        }
        if depth {
            gl.enable(glow::DEPTH_TEST);
        }
        r
    }

    unsafe fn run_inner(&mut self, gl: &glow::Context, prev_y: glow::Texture, next_y: glow::Texture, w: u32, h: u32) -> Result<(), String> {
        self.ensure(gl, w, h)?;
        self.passes = 0;
        let seti = |gl: &glow::Context, p: glow::Program, n: &str, v: i32| {
            if let Some(l) = gl.get_uniform_location(p, n) {
                gl.uniform_1_i32(Some(&l), v);
            }
        };
        // Pyramid. LEVELS are coarse-to-fine; build fine-to-coarse.
        let order: Vec<usize> = (0..self.levels.len()).rev().collect();
        let mut src = (prev_y, next_y);
        let mut prev_div = 1usize;
        for &li in &order {
            let k = (wts::LEVELS[li] / prev_div) as i32;
            let (ta, tc) = (self.levels[li].a.clone_ref(), self.levels[li].c.clone_ref());
            let pool = self.pool;
            self.draw(gl, &ta, pool, &[("src", src.0)], &|gl, p| seti(gl, p, "k", k));
            self.draw(gl, &tc, pool, &[("src", src.1)], &|gl, p| seti(gl, p, "k", k));
            src = (ta.tex[0], tc.tex[0]);
            prev_div = wts::LEVELS[li];
        }
        // Lucas-Kanade, coarse to fine.
        let mut cur = 0usize;
        for li in 0..self.levels.len() {
            let dst = self.levels[li].flow[0].clone_ref();
            if li == 0 {
                let z = self.zero;
                self.draw(gl, &dst, z, &[], &|_, _| {});
            } else {
                let coarse = self.levels[li - 1].flow[cur].tex[0];
                let up = self.upflow;
                self.draw(gl, &dst, up, &[("coarse", coarse)], &|_, _| {});
            }
            cur = 0;
            for _ in 0..wts::LK_ITERS {
                let l = &self.levels[li];
                let (a, c, f, prod, next) = (l.a.tex[0], l.c.tex[0], l.flow[cur].tex[0], l.prod.clone_ref(), l.flow[1 - cur].clone_ref());
                let (pa, pb) = (self.lk_a, self.lk_b);
                self.draw(gl, &prod, pa, &[("a", a), ("c", c), ("flow", f)], &|_, _| {});
                let r = (wts::LK_WIN / 2) as i32;
                self.draw(gl, &next, pb, &[("p0", prod.tex[0]), ("p1", prod.tex[1]), ("flow", f)], &|gl, p| seti(gl, p, "r", r));
                cur = 1 - cur;
            }
        }
        self.lk_cur = cur;
        let fine = self.levels.last().ok_or("no levels")?;
        let (a4, c4, lk) = (fine.a.tex[0], fine.c.tex[0], fine.flow[cur].tex[0]);
        // The CNN at 1/4.
        let f0 = self.feat[0].clone_ref();
        let ci = self.cnn_in;
        self.draw(gl, &f0, ci, &[("a", a4), ("c", c4), ("flow", lk)], &|_, _| {});
        let mut src_i = 0usize;
        let last = wts::LAYERS.len() - 1;
        for (li, &(cin, cout, dil, wt, b)) in wts::LAYERS.iter().enumerate() {
            let input = self.feat[src_i].clone_ref();
            let dst = if li == last { self.head.as_ref().ok_or("head")?.clone_ref() } else { self.feat[1 - src_i].clone_ref() };
            let mut w4 = vec![0.0f32; 576];
            w4[..wt.len()].copy_from_slice(wt);
            let mut b4 = [0.0f32; 8];
            b4[..b.len()].copy_from_slice(b);
            let conv = self.conv;
            let (qw, qh) = (dst.w, dst.h);
            self.draw(gl, &dst, conv, &[("in0", input.tex[0]), ("in1", input.tex[1])], &|gl, p| {
                seti(gl, p, "cin", cin as i32);
                seti(gl, p, "cout", cout as i32);
                seti(gl, p, "dil", dil as i32);
                seti(gl, p, "act", (li != last) as i32);
                if let Some(l) = gl.get_uniform_location(p, "size") {
                    gl.uniform_2_i32(Some(&l), qw, qh);
                }
                if let Some(l) = gl.get_uniform_location(p, "w4") {
                    gl.uniform_4_f32_slice(Some(&l), &w4);
                }
                if let Some(l) = gl.get_uniform_location(p, "b4") {
                    gl.uniform_4_f32_slice(Some(&l), &b4);
                }
            });
            src_i = 1 - src_i;
        }
        let head = self.head.as_ref().ok_or("head")?.clone_ref();
        let out = self.out.as_ref().ok_or("out")?.clone_ref();
        let comb = self.combine;
        self.draw(gl, &out, comb, &[("lk", lk), ("h0", head.tex[0]), ("h1", head.tex[1]), ("a", a4), ("c", c4)], &|_, _| {});
        gl.bind_texture(glow::TEXTURE_2D, Some(out.tex[2]));
        gl.generate_mipmap(glow::TEXTURE_2D);
        Ok(())
    }

    /// Synthesised luma at full size into an R16F target, read back: the
    /// parity self-test's measurement.
    ///
    /// # Safety
    /// `gl` must be current; call after [`run`] with the same inputs.
    pub unsafe fn synth_luma_readback(&mut self, gl: &glow::Context, prev_y: glow::Texture, next_y: glow::Texture, w: u32, h: u32) -> Result<Vec<f32>, String> {
        let (flow, mask, _, (fw, fh)) = self.outputs().ok_or("run first")?;
        let t = target(gl, w as i32, h as i32, &[(glow::R32F, glow::RED)], false)?;
        let prev_fb = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
        gl.bind_vertex_array(Some(self.vao));
        let sy = self.synth_y;
        self.draw(gl, &t, sy, &[("y0", prev_y), ("y2", next_y), ("flow", flow), ("mask", mask)], &|gl, p| {
            if let Some(l) = gl.get_uniform_location(p, "fdims") {
                gl.uniform_2_f32(Some(&l), fw as f32, fh as f32);
            }
        });
        let mut px = vec![0.0f32; (w * h) as usize];
        gl.read_pixels(0, 0, w as i32, h as i32, glow::RED, glow::FLOAT, glow::PixelPackData::Slice(Some(bytemuck_f32(&mut px))));
        gl.bind_vertex_array(None);
        gl.bind_framebuffer(glow::FRAMEBUFFER, std::num::NonZeroU32::new(prev_fb as u32).map(glow::NativeFramebuffer));
        free(gl, &t);
        Ok(px)
    }

    /// Read back an intermediate as RGBA f32 rows (bottom row first, which
    /// is texture row 0): `"a"` / `"c"` (pooled luma) and `"lk"` (flow) per
    /// level index, `"feat"` (CNN input), `"flow"` / `"mask"` (outputs).
    /// For localising a parity failure; not used on the frame path.
    ///
    /// # Safety
    /// `gl` must be current; call after [`run`].
    pub unsafe fn debug_read(&self, gl: &glow::Context, what: &str, level: usize) -> Option<(i32, i32, Vec<[f32; 4]>)> {
        let (tex, w, h) = match what {
            "a" => { let l = self.levels.get(level)?; (l.a.tex[0], l.a.w, l.a.h) }
            "c" => { let l = self.levels.get(level)?; (l.c.tex[0], l.c.w, l.c.h) }
            "lk" => { let l = self.levels.get(level)?; (l.flow[self.lk_cur].tex[0], l.w(), l.h()) }
            "feat" => (self.feat[0].tex[level], self.feat[0].w, self.feat[0].h),
            "flow" | "mask" => { let o = self.out.as_ref()?; (o.tex[if what == "flow" { 0 } else { 1 }], o.w, o.h) }
            _ => return None,
        };
        let fbo = gl.create_framebuffer().ok()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
        let mut px = vec![0.0f32; (w * h * 4) as usize];
        gl.read_pixels(0, 0, w, h, glow::RGBA, glow::FLOAT, glow::PixelPackData::Slice(Some(bytemuck_f32(&mut px))));
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.delete_framebuffer(fbo);
        Some((w, h, px.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect()))
    }

    /// Read a single-channel float target (tests).
    ///
    /// # Safety
    /// `gl` must be current.
    pub unsafe fn mean_residual(&self, gl: &glow::Context) -> Option<f32> {
        let (_, _, resid, (w, h)) = self.outputs()?;
        let t = Target { fbo: gl.create_framebuffer().ok()?, tex: vec![resid], w, h };
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(resid), 0);
        let mut px = vec![0.0f32; (w * h) as usize];
        gl.read_pixels(0, 0, w, h, glow::RED, glow::FLOAT, glow::PixelPackData::Slice(Some(bytemuck_f32(&mut px))));
        gl.delete_framebuffer(t.fbo);
        Some(px.iter().sum::<f32>() / px.len() as f32)
    }
}

impl Level {
    fn w(&self) -> i32 {
        self.a.w
    }
    fn h(&self) -> i32 {
        self.a.h
    }
}

impl Target {
    /// A non-owning copy of the handles (the struct owns them; draws only
    /// borrow).
    fn clone_ref(&self) -> Target {
        Target { fbo: self.fbo, tex: self.tex.clone(), w: self.w, h: self.h }
    }
}

fn bytemuck_f32(v: &mut [f32]) -> &mut [u8] {
    // SAFETY: f32 has no invalid bit patterns; length in bytes is exact.
    unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr().cast::<u8>(), std::mem::size_of_val(v)) }
}
