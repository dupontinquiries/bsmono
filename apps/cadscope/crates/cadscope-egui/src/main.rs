//! cadscope — the M0 shell. Thin by decree: the kernel owns geometry, the forge owns history, the
//! view owns GPU, motion owns keys; this file is the wiring. What M0 proves end-to-end:
//!
//!   forge Doc → kernel Model → viewport draws (skydribble pass inside egui-wgpu)
//!                          ↘ SDF pick under the pointer = the same shapes the mesh shows
//!
//! Touch story at M0: one-finger drag orbits, two-finger/right drag pans, pinch or scroll dollies,
//! tap picks. (uikit-egui gesture_layer momentum + the FP5-tuned two-tier recognizer wire in at
//! M2 — PARITY.md tracks it.) Keyboard: cad-motion Normal mode — hjkl orbit, +/- dolly, f frame,
//! n/N cycle, g Grow (stub), Esc clear.

use std::collections::HashMap;
use std::sync::Arc;

use cad_forge::{BoolMode, Doc, Op, OpWarning, ShapeSpec, SpineNode};
use cad_kernel::{Grower, MeshOpts, Model, Sdf, Section};
use cad_motion::{Action, GrowAction, Mode};
use cad_view::camera::{ndc_of, OrbitCamera};
use cad_view::meshgen::{from_trimesh, MeshData};
use cad_view::renderer::{FrameDraw, Globals, MeshKind, ObjectRaw, ViewRenderer};
use eframe::egui;
use glam::{Mat4, Quat, Vec2, Vec3};

fn main() -> eframe::Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--selftest") {
        selftest();
    }
    if let Some(i) = args.iter().position(|a| a == "--ref-index") {
        ref_index(args.get(i + 1).map(String::as_str).unwrap_or("."));
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        // 32 → Depth32Float on egui's pass (egui_wgpu::depth_format_from_bits) — the format the
        // ViewRenderer pipeline declares. Skydribble parity; see cad-view/src/renderer.rs.
        depth_buffer: 32,
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native("CADscope", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

/// The M1 demo document. Primitives exercise the cached parametric meshes; the last three parts
/// are CSG and therefore go through the dual contourer — a drilled bracket (subtract), a fused
/// lamp (blend, which is the operator the UI sells as a fillet), and a patterned row of studs.
fn demo_doc() -> Doc {
    let add = |name: &str, shape: ShapeSpec, at: [f32; 3], rot: Option<([f32; 3], f32)>, color: [f32; 4]| Op::Add {
        name: name.into(),
        shape,
        at,
        rot_axis_deg: rot,
        color,
    };
    let steel = [0.62, 0.66, 0.72, 1.0];
    Doc {
        name: "m1-bench".into(),
        ops: vec![
            add("base", ShapeSpec::Cuboid { half: [7.0, 1.0, 5.0] }, [0.0, 1.0, 0.0], None, steel),
            add("pillar", ShapeSpec::Cylinder { r: 1.4, hh: 3.2 }, [-3.2, 5.2, 0.0], None, [0.82, 0.55, 0.28, 1.0]),
            add("knob", ShapeSpec::Sphere { r: 1.8 }, [-3.2, 10.1, 0.0], None, [0.85, 0.30, 0.30, 1.0]),
            add("ring", ShapeSpec::Torus { major: 2.6, minor: 0.7 }, [3.4, 4.6, 0.0], Some(([1.0, 0.0, 0.0], 90.0)), [0.32, 0.62, 0.85, 1.0]),
            add("handle", ShapeSpec::Capsule { r: 0.8, hh: 2.2 }, [3.4, 4.6, 0.0], Some(([0.0, 0.0, 1.0], 90.0)), [0.45, 0.78, 0.50, 1.0]),

            // bracket: a plate with a bore through it — the subtract the M0 viewport could not draw
            add("bracket", ShapeSpec::Cuboid { half: [2.4, 0.5, 1.6] }, [0.0, 3.4, 3.2], None, [0.70, 0.72, 0.76, 1.0]),
            add("bore", ShapeSpec::Cylinder { r: 0.9, hh: 1.2 }, [0.0, 3.4, 3.2], None, steel),
            Op::Combine { into: "bracket".into(), other: "bore".into(), mode: BoolMode::Subtract, k: 0.0 },

            // lamp: sphere fused to a post with a blend radius, so the join reads as a fillet
            add("lamp", ShapeSpec::Sphere { r: 1.5 }, [0.0, 5.4, -3.4], None, [0.90, 0.78, 0.42, 1.0]),
            add("post", ShapeSpec::Cylinder { r: 0.5, hh: 1.8 }, [0.0, 3.2, -3.4], None, [0.90, 0.78, 0.42, 1.0]),
            Op::Combine { into: "lamp".into(), other: "post".into(), mode: BoolMode::Union, k: 0.9 },

            // studs: one cylinder, patterned — a union tree, meshed as a single solid
            add("studs", ShapeSpec::Cylinder { r: 0.45, hh: 0.35 }, [-4.5, 2.35, 3.4], None, [0.55, 0.58, 0.64, 1.0]),
            Op::Pattern { name: "studs".into(), count: 4, step: [3.0, 0.0, 0.0] },
        ],
    }
}

/// Scan a tree into the reference corpus (plan §17) and report what it found. This is the verb
/// behind "same volume as `puma`" — before any of that can be trusted, the index has to be able to
/// walk a real, messy asset tree and come back with facts rather than guesses.
fn ref_index(root: &str) -> ! {
    use cad_ref::{Facts, Index, Kind, ScanOpts};
    let t0 = std::time::Instant::now();
    let mut fail_ref = 0usize;
    let idx = Index::scan(std::path::Path::new(root), &ScanOpts::default());
    println!(
        "indexed {} entries from {root} in {:.2}s",
        idx.len(),
        t0.elapsed().as_secs_f32()
    );
    for (kind, label) in [
        (Kind::Model, "models"),
        (Kind::Texture, "textures"),
        (Kind::Niftyplot, "niftyplot"),
        (Kind::Doc, "docs"),
    ] {
        println!("  {:10} {}", label, idx.count(kind));
    }

    // What the corpus actually knows about the models it parsed.
    let mut parsed = 0usize;
    let mut with_bounds = 0usize;
    let mut named_parts = 0usize;
    let mut biggest: Vec<(&str, u32)> = Vec::new();
    for e in &idx.entries {
        if let Facts::Model(m) = &e.facts {
            if m.triangles > 0 {
                parsed += 1;
                biggest.push((e.name.as_str(), m.triangles));
            }
            if m.bounds.is_some() {
                with_bounds += 1;
            }
            named_parts += m.mesh_names.len();
        }
    }
    biggest.sort_by(|a, b| b.1.cmp(&a.1));
    biggest.truncate(5);
    println!("  parsed {parsed} model headers, {with_bounds} with bounds, {named_parts} named sub-meshes");
    for (n, t) in &biggest {
        println!("    {:34} {:>8} tris", n, t);
    }

    // Materials are the bridge to the texture corpus: a model names them, and the index groups
    // <name>.png / norm_ / spec_ under one entry name, so `texture from <model>` is a join.
    let mut joined = 0usize;
    let mut example = String::new();
    for e in &idx.entries {
        if let Facts::Model(m) = &e.facts {
            for mat in &m.materials {
                let stem = std::path::Path::new(mat)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(mat);
                if !idx.material_set(stem).is_empty() {
                    joined += 1;
                    if example.is_empty() {
                        example = format!("{} -> {}", e.name, stem);
                    }
                }
            }
        }
    }
    println!("  {joined} model->texture material joins resolved{}", if example.is_empty() { String::new() } else { format!("  (e.g. {example})") });

    // Measured geometry, not just headers: OBJ files carry vertices, so they carry a volume.
    let mut measured: Vec<(&str, f32, f32)> = Vec::new();
    for e in &idx.entries {
        if let Facts::Model(m) = &e.facts {
            if let (Some(v), Some(a)) = (m.volume, m.area) {
                if v > 0.0 {
                    measured.push((e.name.as_str(), v, a));
                }
            }
        }
    }
    measured.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    // Topology census: how much of the corpus is actually closed enough to have a volume.
    let (mut with_geom, mut closed, mut open_shells) = (0usize, 0usize, 0usize);
    for e in &idx.entries {
        if let Facts::Model(m) = &e.facts {
            if m.area.is_some() {
                with_geom += 1;
                if m.closed { closed += 1 } else { open_shells += 1 }
            }
        }
    }
    println!("  {with_geom} models with geometry: {closed} closed, {open_shells} open (no volume reported)");
    println!("  {} models measured from vertex data (volume + area)", measured.len());
    for (n, v, a) in measured.iter().take(3) {
        println!("    {:34} vol {:>12.1}  area {:>12.1}", n, v, a);
    }

    // The whole §17 chain, end to end on a real file: resolve a name, take its MEASURED volume,
    // and build a part that is scaled to match — with the match verified rather than asserted.
    if let Some((name, vol, _)) = measured.iter().find(|(_, v, _)| *v > 1.0 && *v < 1.0e6) {
        let doc = Doc {
            name: "volume-of".into(),
            ops: vec![
                Op::Add {
                    name: "blank".into(),
                    shape: ShapeSpec::Cuboid { half: [1.0, 1.0, 1.0] },
                    at: [0.0, 0.0, 0.0],
                    rot_axis_deg: None,
                    color: [0.7, 0.7, 0.7, 1.0],
                },
                Op::MatchVolume { name: "blank".into(), volume: *vol, tol: 0.02, resolution: 40 },
            ],
        };
        let (m, w) = doc.apply();
        let got = m
            .parts
            .first()
            .map(|p| {
                cad_kernel::dual_contour(&p.sdf, MeshOpts { resolution: 48 })
                    .signed_volume()
                    .abs()
            })
            .unwrap_or(0.0);
        let ok = w.is_empty() && (got - vol).abs() / vol < 0.02;
        if !ok {
            fail_ref += 1;
        }
        println!(
            "  volume-of {:20} target {:.1} -> built {:.1} ({:+.2}%)  {}",
            name,
            vol,
            got,
            (got - vol) / vol * 100.0,
            if ok { "ok" } else { "FAIL" }
        );
        for warning in &w {
            println!("      warning: {}", warning.reason);
        }
    }

    // The `texture from <model>` verb, on whatever model in the corpus resolves the most maps.
    let mut best: Option<(&str, Vec<cad_ref::MaterialMaps>)> = None;
    for e in idx.entries.iter().filter(|e| e.kind == Kind::Model) {
        let maps = idx.textures_of_model(&e.name);
        let found: usize = maps.iter().map(|m| m.found()).sum();
        if found > best.as_ref().map_or(0, |(_, m)| m.iter().map(|x| x.found()).sum()) {
            best = Some((e.name.as_str(), maps));
        }
        if found >= 6 {
            break;
        }
    }
    match best {
        Some((name, maps)) => {
            let missing = maps.iter().filter(|m| m.unresolved()).count();
            println!(
                "  texture-from {:20} {} materials, {} maps resolved, {} unresolved",
                name,
                maps.len(),
                maps.iter().map(|m| m.found()).sum::<usize>(),
                missing
            );
            for m in maps.iter().filter(|m| !m.unresolved()).take(3) {
                println!(
                    "      {:24} base {} norm {} spec {}",
                    m.name,
                    m.base.is_some() as u8,
                    m.normal.is_some() as u8,
                    m.specular.is_some() as u8
                );
            }
        }
        None => println!("  texture-from: no model resolved any material"),
    }

    // Name resolution, which is what a person actually uses.
    for q in ["puma", "shark", "obsidian", "nosuchmodelanywhere"] {
        let hits = idx.resolve(q, None, 3);
        if hits.is_empty() {
            println!("  resolve {:22} -> nothing indexed under that name", q);
        } else {
            let names: Vec<String> = hits.iter().map(|(e, s)| format!("{}({})", e.name, s)).collect();
            println!("  resolve {:22} -> {}", q, names.join(", "));
        }
    }
    if fail_ref > 0 {
        println!("ref-index: FAIL");
    }
    std::process::exit(if fail_ref == 0 { 0 } else { 1 });
}

/// Headless verification of the whole model pipeline — evaluate the document, mesh every CSG
/// part, probe it with rays — with no window and no GPU. This is the house `--selftest` convention
/// (logscope), and it is precisely what CI, or a build box without a display, can actually run.
fn selftest() -> ! {
    let t0 = std::time::Instant::now();
    let doc = demo_doc();
    let (model, warns) = doc.apply();
    let mut fail = 0usize;
    println!("doc '{}': {} ops -> {} parts", doc.name, doc.ops.len(), model.parts.len());
    for w in &warns {
        println!("  FAIL  op #{}: {}", w.index, w.reason);
        fail += 1;
    }

    for part in &model.parts {
        match display_of(&part.sdf) {
            Some((kind, _)) => println!("  {:9} primitive  {:?}", part.name, kind),
            None => {
                let (lo, hi) = part.sdf.aabb();
                let cell = (hi - lo).max_element() / 56.0;
                let t = cad_kernel::dual_contour(&part.sdf, MeshOpts { resolution: 56 });
                let vol = t.signed_volume();
                let err = t
                    .positions
                    .iter()
                    .map(|p| part.sdf.eval(Vec3::from(*p)).abs())
                    .fold(0.0f32, f32::max);
                // A vertex further off the surface than half a cell means the QEF solve or the
                // clamp misplaced it; negative volume means the winding is inverted.
                let ok = !t.is_empty() && vol > 0.0 && err < cell * 0.5;
                if !ok {
                    fail += 1;
                }
                println!(
                    "  {:9} meshed  {:6} tris   vol {:8.2}   max|d| {:.4} (cell {:.4})  {}",
                    part.name,
                    t.tri_count(),
                    vol,
                    err,
                    cell,
                    if ok { "ok" } else { "FAIL" }
                );
            }
        }
    }

    // Ray probes: what the tap-picker will do, on geometry whose answer we know by hand.
    let probe = |x: f32, z: f32| {
        model.pick(&cad_kernel::Ray::new(Vec3::new(x, 60.0, z), Vec3::NEG_Y), 500.0)
    };
    let mut check = |label: &str, got: Option<cad_kernel::Pick>, want: &str| {
        let name = got
            .and_then(|p| model.part(p.part_id).map(|q| q.name.clone()))
            .unwrap_or_else(|| "<miss>".into());
        let ok = name == want;
        if !ok {
            fail += 1;
        }
        println!(
            "  probe {:22} -> {:9} (want {:9}) {}",
            label,
            name,
            want,
            if ok { "ok" } else { "FAIL" }
        );
    };
    check("down at origin", probe(0.0, 0.0), "base");
    // Straight down the bracket's bore: the hole must be genuinely open, so the ray reaches the
    // slab underneath rather than stopping on the plate.
    check("down through the bore", probe(0.0, 3.2), "base");
    // Same plate, off-axis: solid material, so it stops here.
    check("down beside the bore", probe(1.8, 3.2), "bracket");

    // A scripted Grow session, driven through exactly the calls the keyboard produces, then
    // committed the way Esc commits it. This is the only way to exercise the grammar without a
    // keyboard, and it checks the part the eye would otherwise have to: that steering, branching,
    // knuckles, section changes and the mirror all survive into a closed solid.
    let mut g = Grower::new(0.12);
    for i in 0..90 {
        if i % 4 == 0 {
            g.grow_turtle_yaw();
        }
        if i == 30 {
            g.node_bulge();
        }
        if i == 45 {
            g.branch();
        }
        if i == 60 {
            g.turtle().set_section(Section::Hex);
        }
        if i % 12 == 0 {
            g.turtle().thicken(1.0);
        }
        g.grow(1.0);
    }
    g.toggle_mirror();
    let stations: usize = g.spines.iter().map(|s| s.nodes.len()).sum();
    let grown = g.to_sdf().expect("a grown session must produce a solid");
    let gm = cad_kernel::dual_contour(&grown, MeshOpts { resolution: 44 });
    let gvol = gm.signed_volume();
    let gok = !gm.is_empty() && gvol > 0.0 && g.spines.len() == 2;
    if !gok {
        fail += 1;
    }
    println!(
        "  grow      {} spines, {} stations, mirrored -> {:6} tris   vol {:8.2}  {}",
        g.spines.len(),
        stations,
        gm.tri_count(),
        gvol,
        if gok { "ok" } else { "FAIL" }
    );

    // ...and the same session as document history: every spine becomes a named op that rebuilds.
    let mut doc2 = Doc { name: "grown".into(), ops: Vec::new() };
    for (i, spine) in g.spines.iter().enumerate() {
        doc2.ops.push(Op::Spine {
            name: format!("grown{i}"),
            nodes: spine
                .nodes
                .iter()
                .map(|n| SpineNode { at: n.pos.to_array(), up: n.up.to_array(), r: n.radius, section: 1 })
                .collect(),
            blend: spine.blend,
            mirror: g.mirror,
            color: [0.72, 0.84, 0.62, 1.0],
        });
    }
    let round_trips = Doc::from_json(&doc2.to_json()).map(|d| d == doc2).unwrap_or(false);
    let (m2, w2) = doc2.apply();
    let doc_ok = round_trips && w2.is_empty() && m2.parts.len() == g.spines.len();
    if !doc_ok {
        fail += 1;
    }
    println!(
        "  commit    {} ops -> {} parts, json round-trip {}  {}",
        doc2.ops.len(),
        m2.parts.len(),
        round_trips,
        if doc_ok { "ok" } else { "FAIL" }
    );

    println!(
        "selftest: {} ({:.1}s)",
        if fail == 0 { "PASS" } else { "FAIL" },
        t0.elapsed().as_secs_f32()
    );
    std::process::exit(if fail == 0 { 0 } else { 1 });
}

struct App {
    doc: Doc,
    model: Model,
    warns: Vec<OpWarning>,
    cam: OrbitCamera,
    selected: Option<u32>,
    mode: Mode,
    start: std::time::Instant,
    /// Dual-contoured geometry for parts the parametric mesh path cannot express (any CSG tree),
    /// keyed by part id. Rebuilt only when the document is re-evaluated, never per frame.
    meshed: HashMap<u32, Arc<MeshData>>,
    /// Bumped on every re-evaluation so the renderer can drop stale generated buffers.
    epoch: u64,
    /// Live Grow session, if any.
    grower: Option<Grower>,
    /// Seconds banked toward the next station, so growth rate is wall-clock and not frame-rate.
    grow_accum: f32,
    /// Coarse preview of the growing form, re-meshed on a throttle. Its key changes every rebuild
    /// so the renderer evicts the previous one.
    preview: Option<Arc<MeshData>>,
    preview_key: u64,
    preview_due: f32,
    grown: u32,
}

/// One station every 33 ms of growth: fine enough that the swept chain reads as a smooth tube,
/// coarse enough that a long session stays cheap to mesh.
const STATION_DT: f32 = 1.0 / 30.0;
/// Preview re-mesh interval. Coarse resolution while growing; the committed part gets full quality.
const PREVIEW_EVERY: f32 = 0.12;
const PREVIEW_RES: u32 = 28;

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .expect("CADscope needs the wgpu backend (NativeOptions.renderer = Wgpu)");
        rs.renderer.write().callback_resources.insert(ViewRenderer::new(
            &rs.device,
            rs.target_format,
            Some(wgpu::TextureFormat::Depth32Float), // pairs with depth_buffer: 32 above
            1,                                       // eframe default msaa; 4x is a PARITY debt
        ));

        let doc = demo_doc();
        let (model, warns) = doc.apply();
        let mut app = App {
            doc,
            model,
            warns,
            cam: OrbitCamera::default(),
            selected: None,
            mode: Mode::Normal,
            start: std::time::Instant::now(),
            meshed: HashMap::new(),
            epoch: 0,
            grower: None,
            grow_accum: 0.0,
            preview: None,
            preview_key: 1 << 40,
            preview_due: 0.0,
            grown: 0,
        };
        app.remesh();
        app.frame_all();
        app
    }

    /// Mesh every part the parametric path cannot draw. Cost is O(resolution³) per part, which is
    /// why it happens here and not in the frame loop; adaptive octree refinement (PARITY) is what
    /// eventually makes this cheap enough to run on every edit keystroke.
    fn remesh(&mut self) {
        self.meshed.clear();
        self.epoch = self.epoch.wrapping_add(1);
        for part in &self.model.parts {
            if display_of(&part.sdf).is_some() {
                continue;
            }
            let t = cad_kernel::dual_contour(&part.sdf, MeshOpts { resolution: 56 });
            if t.is_empty() {
                log::warn!("part '{}' produced no surface", part.name);
                continue;
            }
            log::info!("meshed '{}': {} tris", part.name, t.tri_count());
            self.meshed.insert(part.id, Arc::new(from_trimesh(&t)));
        }
    }

    fn frame_all(&mut self) {
        let (lo, hi) = self.model.aabb();
        self.cam.frame((lo + hi) * 0.5, ((hi - lo).length() * 0.5).max(1.0));
    }

    fn act(&mut self, a: Action) {
        match a {
            Action::OrbitLeft => self.cam.orbit(-0.14, 0.0),
            Action::OrbitRight => self.cam.orbit(0.14, 0.0),
            Action::OrbitUp => self.cam.orbit(0.0, 0.10),
            Action::OrbitDown => self.cam.orbit(0.0, -0.10),
            Action::DollyIn => self.cam.dolly(0.85),
            Action::DollyOut => self.cam.dolly(1.18),
            Action::FrameSelection => match self.selected.and_then(|id| self.model.part(id)) {
                Some(p) => {
                    let (lo, hi) = p.sdf.aabb();
                    self.cam.frame((lo + hi) * 0.5, ((hi - lo).length() * 0.5).max(0.5));
                }
                None => self.frame_all(),
            },
            Action::SelectNext | Action::SelectPrev => {
                let ids: Vec<u32> = self.model.parts.iter().map(|p| p.id).collect();
                if ids.is_empty() {
                    return;
                }
                let cur = self.selected.and_then(|s| ids.iter().position(|&i| i == s));
                let next = match (cur, a) {
                    (Some(i), Action::SelectNext) => (i + 1) % ids.len(),
                    (Some(i), Action::SelectPrev) => (i + ids.len() - 1) % ids.len(),
                    _ => 0,
                };
                self.selected = Some(ids[next]);
            }
            Action::ClearSelection => self.selected = None,
            Action::EnterMode(Mode::Grow) => self.enter_grow(),
            Action::EnterMode(m) => self.mode = m,
        }
    }

    /// Start a Grow session at the origin, framed so the form has room to appear.
    fn enter_grow(&mut self) {
        self.mode = Mode::Grow;
        self.grower = Some(Grower::new(0.12));
        self.grow_accum = 0.0;
        self.preview_due = 0.0;
        self.preview = None;
        self.selected = None;
        self.cam.frame(Vec3::new(0.0, 1.6, 1.5), 6.0);
    }

    /// Advance the session by wall-clock time and refresh the preview on its throttle.
    fn tick_grow(&mut self, dt: f32) {
        let Some(g) = self.grower.as_mut() else { return };
        self.grow_accum += dt;
        let mut grew = false;
        // Cap the catch-up so a stalled frame cannot dump a hundred stations at once.
        let mut budget = 8;
        while self.grow_accum >= STATION_DT && budget > 0 {
            self.grow_accum -= STATION_DT;
            budget -= 1;
            g.grow(1.0);
            grew = true;
        }
        self.preview_due -= dt;
        if grew && self.preview_due <= 0.0 {
            self.preview_due = PREVIEW_EVERY;
            self.rebuild_preview();
        }
    }

    fn rebuild_preview(&mut self) {
        let Some(g) = self.grower.as_ref() else { return };
        let Some(sdf) = g.to_sdf() else { return };
        let t = cad_kernel::dual_contour(&sdf, MeshOpts { resolution: PREVIEW_RES });
        if t.is_empty() {
            return;
        }
        self.preview_key = self.preview_key.wrapping_add(1);
        self.preview = Some(Arc::new(from_trimesh(&t)));
    }

    /// Apply one Grow keystroke. Steering is handled separately, per frame, while held.
    fn grow_action(&mut self, a: GrowAction, dt: f32) {
        let Some(g) = self.grower.as_mut() else { return };
        match a {
            GrowAction::Yaw(s) => g.turtle().yaw(s * dt),
            GrowAction::Pitch(s) => g.turtle().pitch(s * dt),
            GrowAction::Roll(s) => g.turtle().roll(s * dt),
            GrowAction::Thicken(s) => g.turtle().thicken(s),
            GrowAction::Flare(s) => g.turtle().flare(s),
            GrowAction::Twist(s) => g.turtle().twist_by(s),
            GrowAction::Straighten => g.turtle().straighten(),
            GrowAction::ToggleRidge => g.turtle().toggle_ridge(),
            GrowAction::ToggleWobble => g.turtle().toggle_wobble(),
            GrowAction::Section(d) => {
                if let Some(sec) = Section::from_digit(d) {
                    g.turtle().set_section(sec);
                }
            }
            GrowAction::Node => g.node_bulge(),
            GrowAction::Branch => g.branch(),
            GrowAction::Parent => g.parent(),
            GrowAction::Mirror => g.toggle_mirror(),
        }
    }

    /// Leave Grow mode, writing what was grown into the document as a real op — the session
    /// becomes history, not a scratch mesh. A form too short to sweep is simply discarded.
    fn commit_grow(&mut self) {
        self.mode = Mode::Normal;
        self.preview = None;
        let Some(g) = self.grower.take() else { return };
        let stations: usize = g.spines.iter().map(|s| s.nodes.len()).sum();
        if g.to_sdf().is_none() || stations < 2 {
            return;
        }
        self.grown += 1;
        let name = format!("grown{}", self.grown);
        // Every spine of the session becomes its own op, so the history stays legible and any one
        // branch can be edited or removed later by name.
        let mirror = g.mirror;
        for (i, spine) in g.spines.iter().enumerate() {
            if spine.nodes.len() < 2 {
                continue;
            }
            let nodes: Vec<SpineNode> = spine
                .nodes
                .iter()
                .map(|n| SpineNode {
                    at: n.pos.to_array(),
                    up: n.up.to_array(),
                    r: n.radius,
                    section: match n.section {
                        Section::Circle => 1,
                        Section::Square => 2,
                        Section::Triangle => 3,
                        Section::Star => 4,
                        Section::Hex => 5,
                    },
                })
                .collect();
            self.doc.ops.push(Op::Spine {
                name: if i == 0 { name.clone() } else { format!("{name}_{i}") },
                nodes,
                blend: spine.blend,
                mirror,
                color: [0.72, 0.84, 0.62, 1.0],
            });
        }
        let (model, warns) = self.doc.apply();
        self.model = model;
        self.warns = warns;
        self.remesh();
        self.selected = self.model.parts.iter().find(|p| p.name == name).map(|p| p.id);
    }

    fn viewport(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let aspect = rect.width() / rect.height().max(1.0);

        // ── input: orbit / pan / dolly / pick ────────────────────────────────────────────────
        let drag = response.drag_delta();
        if response.dragged_by(egui::PointerButton::Primary) {
            self.cam.orbit(drag.x * 0.008, drag.y * 0.008);
        }
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.cam.pan(Vec2::new(drag.x / rect.width(), drag.y / rect.height()), aspect);
        }
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        if response.hovered() && scroll.abs() > 0.0 {
            self.cam.dolly((1.0f32 - scroll * 0.0016).clamp(0.5, 1.6));
        }
        if (zoom - 1.0).abs() > 1e-4 {
            self.cam.dolly((1.0 / zoom).clamp(0.5, 1.8)); // pinch
        }

        let (vp, inv_vp) = self.cam.view_proj(aspect);
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let ray = self.cam.screen_ray(ndc_of(pos, rect), &inv_vp);
                self.selected = self.model.pick(&ray, 4000.0).map(|p| p.part_id);
            }
        }

        // ── frame payload: globals + ground + one draw per part ─────────────────────────────
        let eye = self.cam.eye();
        let t = self.start.elapsed().as_secs_f32();
        let sun = Vec3::new(0.45, 0.80, 0.35).normalize();
        let globals = Globals {
            view_proj: vp.to_cols_array(),
            inv_view_proj: inv_vp.to_cols_array(),
            cam: [eye.x, eye.y, eye.z, t],
            sun: [sun.x, sun.y, sun.z, 1.10],
            fog: [0.74, 0.77, 0.82, 0.0028],
            misc: [self.selected.map_or(0.0, |id| id as f32 + 1.0), 0.0, 0.0, 0.0],
        };

        let mut draws = Vec::with_capacity(self.model.parts.len() + 1);
        draws.push(FrameDraw {
            kind: MeshKind::GroundSlab,
            object: object_raw(
                Mat4::from_scale_rotation_translation(Vec3::new(90.0, 0.12, 90.0), Quat::IDENTITY, Vec3::new(0.0, -0.12, 0.0)),
                [0.52, 0.55, 0.58, 1.0],
                [1.0, 0.0, 0.05, 0.0], // mode=ground, no pick id
            ),
            mesh: None,
        });
        for part in &self.model.parts {
            let params = [0.0, 0.0, 0.30, part.id as f32 + 1.0];
            match display_of(&part.sdf) {
                // A primitive: cached parametric mesh, placed by its model matrix.
                Some((kind, model)) => draws.push(FrameDraw {
                    kind,
                    object: object_raw(model, part.color, params),
                    mesh: None,
                }),
                // CSG: dual-contoured in world space already, so the model matrix is identity.
                None => {
                    if let Some(m) = self.meshed.get(&part.id) {
                        draws.push(FrameDraw {
                            kind: MeshKind::Generated(part.id as u64),
                            object: object_raw(Mat4::IDENTITY, part.color, params),
                            mesh: Some(m.clone()),
                        });
                    }
                }
            }
        }

        if let Some(m) = self.preview.clone() {
            draws.push(FrameDraw {
                kind: MeshKind::Generated(self.preview_key),
                object: object_raw(Mat4::IDENTITY, [0.72, 0.84, 0.62, 1.0], [0.0, 0.15, 0.30, 0.0]),
                mesh: Some(m),
            });
        }

        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            cad_view::renderer::ViewportCallback { globals, draws, epoch: self.epoch },
        ));
        // Repaint continuously while growing (the form is moving) or while something is selected
        // (the tint breathes); otherwise stay idle, which is what keeps a phone's battery alive.
        if self.selected.is_some() || self.mode == Mode::Grow {
            ui.ctx().request_repaint();
        }
    }
}

/// mat4 + normal matrix + color + params → one 256-byte object slot.
fn object_raw(model: Mat4, color: [f32; 4], params: [f32; 4]) -> ObjectRaw {
    ObjectRaw {
        model: model.to_cols_array(),
        nrm: model.inverse().transpose().to_cols_array(),
        color,
        params,
        _pad: [0.0; 24],
    }
}

/// Walk Translate/Rotate wrappers to the primitive leaf: (offset, rotation, leaf).
fn unwrap_placement(sdf: &Sdf) -> (Vec3, Quat, &Sdf) {
    match sdf {
        Sdf::Translate { off, node } => {
            let (o, r, leaf) = unwrap_placement(node);
            (*off + o, r, leaf)
        }
        Sdf::Rotate { inv, node } => {
            let (o, r, leaf) = unwrap_placement(node);
            (inv.inverse() * o, inv.inverse() * r, leaf)
        }
        other => (Vec3::ZERO, Quat::IDENTITY, other),
    }
}

/// The M0 display map: primitive leaf → cached mesh kind + model matrix. CSG-shaped parts return
/// None (drawn once the M1 mesher lands); the kernel still picks and measures them exactly.
fn display_of(sdf: &Sdf) -> Option<(MeshKind, Mat4)> {
    let (off, rot, leaf) = unwrap_placement(sdf);
    let (kind, scale) = match *leaf {
        Sdf::Cuboid { half } => (MeshKind::Cuboid, half),
        Sdf::Sphere { r } => (MeshKind::Sphere, Vec3::splat(r)),
        Sdf::Cylinder { r, hh } => (MeshKind::Cylinder, Vec3::new(r, hh, r)),
        Sdf::Torus { major, minor } => (
            MeshKind::Torus { ratio_milli: ((minor / major) * 1000.0).round() as u32 },
            Vec3::splat(major),
        ),
        Sdf::Capsule { r, hh } => (
            MeshKind::Capsule { hh_milli: ((hh / r) * 1000.0).round() as u32 },
            Vec3::splat(r),
        ),
        _ => return None,
    };
    Some((kind, Mat4::from_scale_rotation_translation(scale, rot, off)))
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        // keyboard → cad-motion actions (skip when a text widget wants the keys)
        let dt = ctx.input(|i| i.stable_dt).clamp(0.001, 0.1);
        if ctx.memory(|m| m.focused().is_none()) {
            let mut chars: Vec<char> = Vec::new();
            let mut esc = false;
            let mut held: Vec<GrowAction> = Vec::new();
            ctx.input(|i| {
                for ev in &i.events {
                    match ev {
                        egui::Event::Text(t) => chars.extend(t.chars()),
                        egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => esc = true,
                        _ => {}
                    }
                }
                // Steering is sampled from key state rather than text events, because it has to
                // apply every frame the key is down — that is what makes the form turn smoothly
                // instead of in keyboard-repeat jerks. Shift turns yaw into roll, matching H/L.
                if self.mode == Mode::Grow {
                    let shift = i.modifiers.shift;
                    if i.key_down(egui::Key::H) {
                        held.push(if shift { GrowAction::Roll(1.0) } else { GrowAction::Yaw(1.0) });
                    }
                    if i.key_down(egui::Key::L) {
                        held.push(if shift { GrowAction::Roll(-1.0) } else { GrowAction::Yaw(-1.0) });
                    }
                    if i.key_down(egui::Key::J) {
                        held.push(GrowAction::Pitch(-1.0));
                    }
                    if i.key_down(egui::Key::K) {
                        held.push(GrowAction::Pitch(1.0));
                    }
                }
            });

            match self.mode {
                Mode::Normal => {
                    for ch in chars {
                        if let Some(a) = cad_motion::normal_key(ch) {
                            self.act(a);
                        }
                    }
                    if esc {
                        let (mode, action) = cad_motion::escape(self.mode);
                        self.mode = mode;
                        if let Some(a) = action {
                            self.act(a);
                        }
                    }
                }
                Mode::Grow => {
                    for a in held {
                        self.grow_action(a, dt);
                    }
                    for ch in chars {
                        if let Some(a) = cad_motion::grow_key(ch) {
                            if !a.is_continuous() {
                                self.grow_action(a, dt);
                            }
                        }
                    }
                    if esc {
                        self.commit_grow();
                    } else {
                        self.tick_grow(dt);
                    }
                }
                Mode::Sketch => {
                    if esc {
                        self.mode = Mode::Normal;
                    }
                }
            }
        }

        egui::Panel::left("parts")
            .resizable(true)
            .default_size(180.0)
            .show_inside(root, |ui| {
                ui.heading(&self.doc.name);
                ui.separator();
                let mut clicked = None;
                for part in &self.model.parts {
                    let sel = self.selected == Some(part.id);
                    if ui.selectable_label(sel, &part.name).clicked() {
                        clicked = Some(if sel { None } else { Some(part.id) });
                    }
                }
                if let Some(c) = clicked {
                    self.selected = c;
                }
                if !self.warns.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new("op warnings").strong());
                    for w in &self.warns {
                        ui.label(format!("#{}: {}", w.index, w.reason));
                    }
                }
                ui.separator();
                let tris: usize = self.meshed.values().map(|m| m.idx.len() / 3).sum();
                ui.label(format!("{} ops → {} parts", self.doc.ops.len(), self.model.parts.len()));
                ui.label(format!("{} meshed, {tris} tris", self.meshed.len()));
            });

        egui::Panel::bottom("status").show_inside(root, |ui| {
            ui.horizontal(|ui| {
                let mode = match self.mode {
                    Mode::Normal => "NORMAL",
                    Mode::Grow => "GROW",
                    Mode::Sketch => "SKETCH (M2 stub — Esc to leave)",
                };
                ui.label(egui::RichText::new(mode).strong().monospace());
                ui.separator();
                match (self.mode, self.grower.as_ref()) {
                    (Mode::Grow, Some(g)) => {
                        let t = g.turtles[g.active];
                        let stations: usize = g.spines.iter().map(|s| s.nodes.len()).sum();
                        ui.label(
                            egui::RichText::new(format!(
                                "r {:.2}  taper {:.2}  twist {:+.1}  {:?}{}{}  ·  {} spine{}  {} stations",
                                t.radius,
                                t.taper,
                                t.twist,
                                t.section,
                                if t.ridge > 0.0 { " ridge" } else { "" },
                                if t.wobble { " wobble" } else { "" },
                                g.spines.len(),
                                if g.spines.len() == 1 { "" } else { "s" },
                                stations,
                            ))
                            .monospace(),
                        );
                        if g.mirror {
                            ui.label(egui::RichText::new("MIRROR").strong());
                        }
                    }
                    _ => {
                        match self.selected.and_then(|id| self.model.part(id)) {
                            Some(p) => ui.label(format!("▸ {}", p.name)),
                            None => ui.label("nothing selected"),
                        };
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let help = match self.mode {
                        Mode::Grow => "hjkl steer · HL roll · wb thick · fp flare · [] twist · 0 straight · e ridge · ` wobble · 1-5 section · space node · t branch · y parent · m mirror · Esc commit",
                        _ => "drag orbit · right-drag pan · pinch/scroll dolly · tap pick · hjkl + - f n · g grow · Esc",
                    };
                    ui.label(egui::RichText::new(help).small());
                });
            });
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(26, 28, 32)))
            .show_inside(root, |ui| self.viewport(ui));
    }
}
