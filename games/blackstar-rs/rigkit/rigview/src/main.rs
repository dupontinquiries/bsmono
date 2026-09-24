//! rigview -- orion's shell dissolve through a real GL pipeline.
//!
//!     rigview <rig-viewer.html> [t1,t2,...]
//!
//! Draws six timeline values in a 3x2 grid, reading order, so one X11 capture
//! shows the dissolve as the GPU renders it. The vertex shader's dissolve is
//! the page's own GLSL, extracted by evaluating its voxHook -- the renderer is
//! new, the effect under test is not.

mod bundle;
mod geom;
mod timeline;
mod scene;

use miniquad::*;

/// Read at RUN time, so iterating on the page's shader needs the page rebuilt
/// and the branch re-extracted, not this binary recompiled.
fn vertex_shader() -> String {
    let path = std::env::var("RIGVIEW_GLSL")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/assets/stream_branch.glsl").into());
    let src = std::fs::read_to_string(&path).expect("read the extracted stream branch");
    let (head, branch) = src.split_once("//@@BRANCH").expect("marker");
    format!(r#"#version 100
attribute vec3 in_pos;
attribute vec3 in_nrm;
attribute vec3 aCell;
uniform mat4 mvp;
uniform vec3 offset;
uniform float uVox;
uniform float uTime;
uniform float uStream;
varying vec3 v_n;
{head}
void main() {{
  vec3 transformed = in_pos;
  {branch}
  v_n = in_nrm;
  gl_Position = mvp * vec4(transformed + offset, 1.0);
}}
"#)
}

static mut TESS: (f32, f32) = (8.0, 25.0);

fn scene_vs() -> String {
    let path = std::env::var("RIGVIEW_GLSL")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/assets/stream_branch.glsl").into());
    let src = std::fs::read_to_string(&path).expect("read the extracted stream branch");
    let (head, branch) = src.split_once("//@@BRANCH").expect("marker");
    format!(r#"#version 100
attribute vec3 in_pos;
attribute vec3 in_nrm;
attribute vec3 aCell;
attribute float aBone;
attribute vec3 aAnchor;
uniform vec4 uBQ[22];
uniform vec4 uBT[22];
uniform float uMorph;
uniform float uMorphT;
uniform vec3 uAxL;
uniform vec3 uAxM;
uniform vec3 uAxT;
uniform vec3 uHalf;
uniform mat4 mvp;
uniform mat4 model;
uniform float uVox;
uniform float uTime;
uniform float uStream;
uniform vec3 color;
varying vec3 v_n;
varying vec3 v_c;
{head}
void main() {{
  vec3 transformed = in_pos;
  {branch}
  v_n = (model * vec4(in_nrm, 0.0)).xyz;
  v_c = color;
  gl_Position = mvp * model * vec4(transformed, 1.0);
}}
"#)
}

const SCENE_FS: &str = r#"#version 100
precision mediump float;
varying vec3 v_n;
varying vec3 v_c;
void main() {
  vec3 n = normalize(v_n);
  float d = abs(dot(n, normalize(vec3(-0.4, 0.8, 0.45))));
  gl_FragColor = vec4(v_c * (0.22 + 0.78 * d), 1.0);
}
"#;

#[repr(C)]
struct SUni { mvp: [f32; 16], model: [f32; 16], vox: f32, time: f32, stream: f32, color: [f32; 3],
              bq: [[f32; 4]; 22], bt: [[f32; 4]; 22],
              morph: f32, morph_t: f32, ax_l: [f32; 3], ax_m: [f32; 3], ax_t: [f32; 3], half: [f32; 3] }

struct SceneStage {
    ctx: Box<dyn RenderingBackend>, pipe: Pipeline,
    geos: Vec<(Bindings, i32)>, frames: Vec<scene::Frame>, target: [f32; 3], dist: f32,
    filter: u8,
    /// per-cell framing (RIGVIEW_PERCELL): each frame fitted to its own bounds
    cells: Vec<([f32; 3], f32)>, per_cell: bool, cols: usize, rows: usize,
}

impl SceneStage {
    fn new(geos: Vec<scene::Geo>, frames: Vec<scene::Frame>) -> SceneStage {
        let mut ctx = window::new_rendering_backend();
        let vs = scene_vs();
        let shader = ctx.new_shader(ShaderSource::Glsl { vertex: &vs, fragment: SCENE_FS }, ShaderMeta {
            images: vec![], uniforms: UniformBlockLayout { uniforms: vec![
                UniformDesc::new("mvp", UniformType::Mat4), UniformDesc::new("model", UniformType::Mat4),
                UniformDesc::new("uVox", UniformType::Float1), UniformDesc::new("uTime", UniformType::Float1),
                UniformDesc::new("uStream", UniformType::Float1), UniformDesc::new("color", UniformType::Float3),
                UniformDesc::new("uBQ", UniformType::Float4).array(22), UniformDesc::new("uBT", UniformType::Float4).array(22),
                UniformDesc::new("uMorph", UniformType::Float1), UniformDesc::new("uMorphT", UniformType::Float1),
                UniformDesc::new("uAxL", UniformType::Float3), UniformDesc::new("uAxM", UniformType::Float3),
                UniformDesc::new("uAxT", UniformType::Float3), UniformDesc::new("uHalf", UniformType::Float3)]},
        }).unwrap_or_else(|e| { eprintln!("SHADER FAILED: {e:?}"); std::process::exit(3) });
        let pipe = ctx.new_pipeline(&[BufferLayout::default()],
            &[VertexAttribute::new("in_pos", VertexFormat::Float3), VertexAttribute::new("in_nrm", VertexFormat::Float3),
              VertexAttribute::new("aCell", VertexFormat::Float3), VertexAttribute::new("aBone", VertexFormat::Float1),
              VertexAttribute::new("aAnchor", VertexFormat::Float3)], shader,
            PipelineParams { depth_test: Comparison::LessOrEqual, depth_write: true, cull_face: CullFace::Nothing, ..Default::default() });
        let gb: Vec<(Bindings, i32)> = geos.iter().map(|g| {
            let vb = ctx.new_buffer(BufferType::VertexBuffer, BufferUsage::Immutable, BufferSource::slice(&g.verts));
            let ib = ctx.new_buffer(BufferType::IndexBuffer, BufferUsage::Immutable, BufferSource::slice(&g.idx));
            (Bindings { vertex_buffers: vec![vb], index_buffer: ib, images: vec![] }, g.idx.len() as i32)
        }).collect();
        // frame the whole run: every draw's translation, across all frames
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for f in &frames { for d in &f.draws { for k in 0..3 { lo[k] = lo[k].min(d.m[12+k]); hi[k] = hi[k].max(d.m[12+k]); } } }
        let target = [(lo[0]+hi[0])*0.5, (lo[1]+hi[1])*0.5, (lo[2]+hi[2])*0.5];
        let dist = ((hi[0]-lo[0]).powi(2)+(hi[1]-lo[1]).powi(2)+(hi[2]-lo[2]).powi(2)).sqrt() * 1.35;
        eprintln!("rigview scene: {} geometries, {} frames, target {:?}", gb.len(), frames.len(), target.map(|v| v.round()));
        let filter = match std::env::var("RIGVIEW_FILTER").as_deref() { Ok("shell") => 1, Ok("rest") => 2, _ => 0 };
        // each frame's own bounds, from its geometry's corners through its matrices
        let cells = frames.iter().map(|f| {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for d in &f.draws { let g = &geos[d.geo];
                // frame the ROBOT: unit-sized helper geometry (shots, barrels,
                // flames -- unit shapes scaled up) flies far and zoomed every
                // cell out to specks; it is still drawn, just not framed
                let diag = ((g.hi[0]-g.lo[0]).powi(2) + (g.hi[1]-g.lo[1]).powi(2) + (g.hi[2]-g.lo[2]).powi(2)).sqrt();
                if diag < 3.5 { continue; }
                for c in 0..8 { let p = [if c & 1 == 0 { g.lo[0] } else { g.hi[0] }, if c & 2 == 0 { g.lo[1] } else { g.hi[1] },
                                         if c & 4 == 0 { g.lo[2] } else { g.hi[2] }];
                    for k in 0..3 { let w = d.m[k]*p[0] + d.m[4+k]*p[1] + d.m[8+k]*p[2] + d.m[12+k];
                                    lo[k] = lo[k].min(w); hi[k] = hi[k].max(w); } } }
            let t = [(lo[0]+hi[0])*0.5, (lo[1]+hi[1])*0.5, (lo[2]+hi[2])*0.5];
            (t, ((hi[0]-lo[0]).powi(2)+(hi[1]-lo[1]).powi(2)+(hi[2]-lo[2]).powi(2)).sqrt() * 1.05)
        }).collect();
        let (cols, rows) = std::env::var("RIGVIEW_GRID").ok().and_then(|g| { let mut it = g.split('x');
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?)) }).unwrap_or((3, 2));
        SceneStage { ctx, pipe, geos: gb, frames, target, dist, filter,
                     cells, per_cell: std::env::var("RIGVIEW_PERCELL").is_ok(), cols, rows }
    }
}

impl EventHandler for SceneStage {
    fn update(&mut self) {}
    fn draw(&mut self) {
        let (w, h) = window::screen_size();
        let (cw, ch) = (w / self.cols as f32, h / self.rows as f32);
        self.ctx.begin_default_pass(PassAction::clear_color(0.07, 0.08, 0.09, 1.0));
        self.ctx.apply_pipeline(&self.pipe);
        let proj = perspective(34f32.to_radians(), cw / ch, 1.0, 6000.0);
        for (ci, f) in self.frames.iter().enumerate().take(self.cols * self.rows) {
            let (t, d) = if self.per_cell { self.cells[ci] } else { (self.target, self.dist) };
            let view = look_at([t[0] - d * 0.55, t[1] + d * 0.36, t[2] + d * 0.78], t, [0.0, 1.0, 0.0]);
            let mvp = mul(&proj, &view);
            let (cx, cy) = ((ci % self.cols) as f32 * cw, (self.rows - 1 - ci / self.cols) as f32 * ch);
            self.ctx.apply_viewport(cx as i32, cy as i32, cw as i32, ch as i32);
            self.ctx.apply_scissor_rect(cx as i32, cy as i32, cw as i32, ch as i32);
            for dr in &f.draws {
                // isolate: only the dissolving shell, or only everything else
                if self.filter == 1 && dr.stream < 0.5 { continue; }
                if self.filter == 2 && dr.stream > 0.5 { continue; }
                let (b, n) = &self.geos[dr.geo];
                self.ctx.apply_bindings(b);
                self.ctx.apply_uniforms(UniformsSource::table(&SUni { mvp, model: dr.m, vox: dr.vox, time: 1.0,
                    stream: dr.stream, color: dr.c, bq: dr.bq, bt: dr.bt,
                    morph: dr.mo[0], morph_t: dr.mo[1],
                    ax_l: onehot(dr.mo[2]), ax_m: onehot(dr.mo[3]), ax_t: onehot(dr.mo[4]),
                    half: [dr.mo[5], dr.mo[6], dr.mo[7]] }));
                self.ctx.draw(0, *n, 1);
            }
        }
        self.ctx.end_render_pass();
        self.ctx.commit_frame();
    }
}

const FS: &str = r#"#version 100
precision mediump float;
varying vec3 v_n;
void main() {
  vec3 n = normalize(v_n);
  vec3 L = normalize(vec3(-0.4, 0.8, 0.45));
  float d = abs(dot(n, L));
  float c = 0.16 + 0.74 * d;
  gl_FragColor = vec4(c * 0.80, c * 0.84, c * 0.90, 1.0);
}
"#;

#[repr(C)]
struct Uni {
    mvp: [f32; 16],
    offset: [f32; 3],
    vox: f32,
    time: f32,
    stream: f32,
}

struct Part {
    bind: Bindings,
    n: i32,
    centre: [f32; 3],
    dis: [f32; 2],
    stream: bool,
}

struct Stage {
    ctx: Box<dyn RenderingBackend>,
    pipe: Pipeline,
    parts: Vec<Part>,
    ts: Vec<f32>,
    frames: u32,
    only_stream: bool,
}

fn onehot(i: f32) -> [f32; 3] {
    let k = i.round() as i32;
    [(k == 0) as i32 as f32, (k == 1) as i32 as f32, (k == 2) as i32 as f32]
}

fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut o = [0f32; 16];
    for c in 0..4 { for r in 0..4 {
        o[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
    }}
    o
}

fn perspective(fovy: f32, aspect: f32, n: f32, f: f32) -> [f32; 16] {
    let t = 1.0 / (fovy * 0.5).tan();
    let mut m = [0f32; 16];
    m[0] = t / aspect; m[5] = t; m[10] = (f + n) / (n - f); m[11] = -1.0; m[14] = 2.0 * f * n / (n - f);
    m
}

fn look_at(e: [f32; 3], c: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let nrm = |v: [f32; 3]| { let l = (v[0]*v[0]+v[1]*v[1]+v[2]*v[2]).sqrt(); [v[0]/l, v[1]/l, v[2]/l] };
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0]*b[0]+a[1]*b[1]+a[2]*b[2];
    let f = nrm(sub(c, e));
    let s = nrm(cross(f, up));
    let u = cross(s, f);
    [s[0], u[0], -f[0], 0.0,
     s[1], u[1], -f[1], 0.0,
     s[2], u[2], -f[2], 0.0,
     -dot(s, e), -dot(u, e), dot(f, e), 1.0]
}

impl Stage {
    fn new(groups: Vec<bundle::Group>, ts: Vec<f32>) -> Stage {
        let mut ctx = window::new_rendering_backend();
        let vs = vertex_shader();
        let shader = ctx
            .new_shader(ShaderSource::Glsl { vertex: &vs, fragment: FS }, ShaderMeta {
                images: vec![],
                uniforms: UniformBlockLayout { uniforms: vec![
                    UniformDesc::new("mvp", UniformType::Mat4),
                    UniformDesc::new("offset", UniformType::Float3),
                    UniformDesc::new("uVox", UniformType::Float1),
                    UniformDesc::new("uTime", UniformType::Float1),
                    UniformDesc::new("uStream", UniformType::Float1),
                ]},
            })
            .unwrap_or_else(|e| { eprintln!("SHADER FAILED: {e:?}"); std::process::exit(3) });
        let pipe = ctx.new_pipeline(
            &[BufferLayout::default()],
            &[VertexAttribute::new("in_pos", VertexFormat::Float3),
              VertexAttribute::new("in_nrm", VertexFormat::Float3),
              VertexAttribute::new("aCell", VertexFormat::Float3)],
            shader,
            PipelineParams { depth_test: Comparison::LessOrEqual, depth_write: true,
                             cull_face: CullFace::Nothing, ..Default::default() },
        );
        let mut parts = Vec::new();
        let (mut tris, mut sv) = (0usize, 0usize);
        for g in &groups {
            let (e, a) = unsafe { TESS };
            let m = if g.stream { geom::streamed(&g.pos, &g.idx, e, a) } else { geom::plain(&g.pos, &g.idx) };
            tris += m.idx.len() / 3;
            if g.stream { sv += m.verts.len() / 9; }
            let vb = ctx.new_buffer(BufferType::VertexBuffer, BufferUsage::Immutable, BufferSource::slice(&m.verts));
            let ib = ctx.new_buffer(BufferType::IndexBuffer, BufferUsage::Immutable, BufferSource::slice(&m.idx));
            parts.push(Part { bind: Bindings { vertex_buffers: vec![vb], index_buffer: ib, images: vec![] },
                              n: m.idx.len() as i32, centre: g.centre, dis: g.dis, stream: g.stream });
        }
        eprintln!("rigview: {} parts, {} triangles, {} unshared streamed corners", parts.len(), tris, sv);
        Stage { ctx, pipe, parts, ts, frames: 0,
                only_stream: std::env::var("RIGVIEW_ONLY_STREAM").is_ok() }
    }
}

impl EventHandler for Stage {
    fn update(&mut self) {}
    fn draw(&mut self) {
        let (w, h) = window::screen_size();
        let (cw, ch) = (w / 3.0, h / 2.0);
        self.ctx.begin_default_pass(PassAction::clear_color(0.07, 0.08, 0.09, 1.0));
        self.ctx.apply_pipeline(&self.pipe);
        let proj = perspective(38f32.to_radians(), cw / ch, 5.0, 3000.0);
        let view = look_at([-380.0, 240.0, 430.0], [0.0, 40.0, 0.0], [0.0, 1.0, 0.0]);
        let mvp = mul(&proj, &view);
        for (ci, &t) in self.ts.iter().enumerate().take(6) {
            let (cx, cy) = ((ci % 3) as f32 * cw, (1 - ci / 3) as f32 * ch);   // reading order
            self.ctx.apply_viewport(cx as i32, cy as i32, cw as i32, ch as i32);
            self.ctx.apply_scissor_rect(cx as i32, cy as i32, cw as i32, ch as i32);
            for p in &self.parts {
                let u = timeline::progress(t, p.dis, p.stream);
                if !timeline::visible(u, p.stream) { continue; }
                // measuring mode: only the dissolving shell, so pixel coverage
                // reads the dissolve itself and not the armour that stays
                if self.only_stream && !p.stream { continue; }
                self.ctx.apply_bindings(&p.bind);
                self.ctx.apply_uniforms(UniformsSource::table(&Uni {
                    mvp, offset: p.centre,
                    vox: if p.stream { u } else { 0.0 }, time: 1.0,
                    stream: if p.stream { 1.0 } else { 0.0 },
                }));
                self.ctx.draw(0, p.n, 1);
            }
        }
        self.ctx.end_render_pass();
        self.ctx.commit_frame();
        self.frames += 1;
        if self.frames == 3 { eprintln!("rigview: drawn"); }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|a| a == "--scene").unwrap_or(false) {
        let b = std::fs::read(&args[2]).expect("read scene dump");
        let (geos, frames) = scene::load(&b).unwrap_or_else(|e| { eprintln!("scene: {e}"); std::process::exit(2) });
        let conf = conf::Conf { window_title: "rigview".into(), window_width: 1500, window_height: 800, ..Default::default() };
        miniquad::start(conf, move || Box::new(SceneStage::new(geos, frames)));
        return;
    }
    let path = args.get(1).cloned().unwrap_or_else(|| "rig-viewer.html".into());
    let ts: Vec<f32> = args.get(2).map(|s| s.split(',').filter_map(|v| v.parse().ok()).collect())
        .unwrap_or_else(|| vec![0.40, 0.50, 0.58, 0.64, 0.70, 0.78]);
    let html = std::fs::read_to_string(&path).expect("read page");
    let (groups, cubes) = bundle::load(&html).unwrap_or_else(|e| { eprintln!("load: {e}"); std::process::exit(2) });
    // the CALL site, not the definition `tessellate(P, I, maxEdge, maxArea)`
    let tess = html.find("T=tessellate(P, I, ").map(|i| {
        let rest = &html[i + 19..];
        let args: Vec<f32> = rest[..rest.find(')').unwrap()].split(',').filter_map(|v| v.trim().parse().ok()).collect();
        assert!(args.len() == 2, "could not read the page's tessellate limits");
        (args[0], args[1])
    }).expect("the page has no tessellate call");
    eprintln!("rigview: page splits triangles past edge {} / area {}", tess.0, tess.1);
    unsafe { TESS = tess; }
    if cubes { eprintln!("note: this page uses cube particles; only the shader path is drawn"); }
    let conf = conf::Conf { window_title: "rigview".into(), window_width: 1500, window_height: 800,
                            ..Default::default() };
    miniquad::start(conf, move || Box::new(Stage::new(groups, ts)));
}
