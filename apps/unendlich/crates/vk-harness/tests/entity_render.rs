//! Mob rendering, proven in-sandbox: feed a real entity (built through the
//! existing EntityStore + snapshot_to_vertices bake) to the REAL entity.vert /
//! entity.frag through the offscreen harness, with a CameraUbo, and assert the
//! mob shows up in its colour. The shaders + bake are unchanged engine code; this
//! verifies the render path the bin/renderer will wire is actually correct before
//! that (un-runnable) wiring is written.

use std::path::PathBuf;
use std::process::Command;
use vk_harness::graphics::{GraphicsHarness, VertexAttr};
use ash::vk;
use unendlich_engine::world::entity::{
    default_color_for_kind, snapshot_to_vertices, EntityKind, EntityRenderSnapshot, EntityStore,
};

fn shaders_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../unendlich-engine/src/shaders")
}
fn tool(name: &str) -> bool {
    Command::new(name).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Compile a real shader file (entity.vert / entity.frag) to SPIR-V.
fn compile_file(name: &str) -> Option<Vec<u32>> {
    let src = shaders_dir().join(name);
    let out = std::env::temp_dir().join(format!("vk_entity_{}.spv", name.replace('.', "_")));
    let st = Command::new("glslangValidator")
        .args(["-V", src.to_str()?, "-o", out.to_str()?]).output().ok()?;
    if !st.status.success() {
        eprintln!("glslang {name}: {}", String::from_utf8_lossy(&st.stdout));
        return None;
    }
    let bytes = std::fs::read(&out).ok()?;
    Some(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn pixel(px: &[u8], dim: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * dim + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

// CameraUbo is 256 bytes: mat4 view_proj, 8x vec4, mat4 light_view_proj.
// Only view_proj matters here; the rest is zero.
fn camera_ubo(view_proj: [f32; 16]) -> Vec<u8> {
    let mut b = vec![0u8; 256];
    for (i, f) in view_proj.iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
    }
    b
}

// Ortho mapping world [0,4] in x,y to NDC [-1,1] (z flattened); column-major.
const VP_0_4: [f32; 16] = [
    0.5, 0.0, 0.0, 0.0,
    0.0, 0.5, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0,
    -1.0, -1.0, 0.0, 1.0,
];

// Front view (look down -X): screen x = world z (wingspan), y = world y (height),
// ortho over world [-6,6]. Column-major.
const VP_FRONT: [f32; 16] = [
    0.0, 0.0, 0.0, 0.0,
    0.0, 1.0 / 6.0, 0.0, 0.0,
    1.0 / 6.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 1.0,
];

fn count_lit(px: &[u8]) -> u32 {
    px.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 20).count() as u32
}

fn render_creature(gh: &GraphicsHarness, vs: &[u32], fs: &[u32], kind: unendlich_engine::item::CreatureKind, dim: u32) -> u32 {
    let creature = unendlich_engine::spawn::spawn(kind, [0.0, 0.0, 0.0]);
    let (verts, indices) = unendlich_engine::world::entity::creature_to_vertices(&creature);
    let mut vbytes = Vec::with_capacity(verts.len() * 24);
    for v in &verts {
        for f in v.position.iter().chain(v.color.iter()) {
            vbytes.extend_from_slice(&f.to_le_bytes());
        }
    }
    let ibytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let attrs = [
        VertexAttr { location: 0, format: vk::Format::R32G32B32_SFLOAT, offset: 0 },
        VertexAttr { location: 1, format: vk::Format::R32G32B32_SFLOAT, offset: 12 },
    ];
    let px = gh
        .render_offscreen(
            dim, [0.0, 0.0, 0.0, 1.0], vs, fs,
            Some((&vbytes, 24, &attrs)), verts.len() as u32, &[],
            Some((&ibytes, indices.len() as u32)),
            Some(&camera_ubo(VP_FRONT)),
        )
        .expect("render creature");
    count_lit(&px)
}

#[test]
fn a_dragon_renders_bigger_than_a_lauscher_through_entity_shaders() {
    let gh = match GraphicsHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    if !tool("glslangValidator") { eprintln!("skip: no glslangValidator"); return; }
    let vs = match compile_file("entity.vert") { Some(s) => s, None => { eprintln!("skip"); return; } };
    let fs = match compile_file("entity.frag") { Some(s) => s, None => { eprintln!("skip"); return; } };

    let dim = 64;
    let dragon = render_creature(&gh, &vs, &fs, unendlich_engine::item::CreatureKind::Dragon, dim);
    let lauscher = render_creature(&gh, &vs, &fs, unendlich_engine::item::CreatureKind::Lauscher, dim);
    assert!(dragon > 0 && lauscher > 0, "both creatures should draw (dragon={dragon}, lauscher={lauscher})");
    assert!(dragon > lauscher * 3, "the winged dragon should cover far more than the tiny lauscher ({dragon} vs {lauscher})");
}

#[test]
fn a_seeker_mob_renders_reddish_through_entity_shaders() {
    let gh = match GraphicsHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    if !tool("glslangValidator") { eprintln!("skip"); return; }
    let vs = match compile_file("entity.vert") { Some(s) => s, None => { eprintln!("skip"); return; } };
    let fs = match compile_file("entity.frag") { Some(s) => s, None => { eprintln!("skip"); return; } };

    use unendlich_engine::sim::fixed::{Fixed, Vec3};
    use unendlich_engine::sim::mob::{Mob, MobKind};
    let mut seeker = Mob::new_walker(Vec3::new(Fixed::ZERO, Fixed::ZERO, Fixed::ZERO), 20);
    seeker.kind = MobKind::Seeker;
    let (verts, indices) = unendlich_engine::world::entity::mobs_to_vertices(&[seeker]);

    let mut vbytes = Vec::with_capacity(verts.len() * 24);
    for v in &verts {
        for f in v.position.iter().chain(v.color.iter()) {
            vbytes.extend_from_slice(&f.to_le_bytes());
        }
    }
    let ibytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let attrs = [
        VertexAttr { location: 0, format: vk::Format::R32G32B32_SFLOAT, offset: 0 },
        VertexAttr { location: 1, format: vk::Format::R32G32B32_SFLOAT, offset: 12 },
    ];
    let dim = 32;
    let px = gh
        .render_offscreen(
            dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs,
            Some((&vbytes, 24, &attrs)), verts.len() as u32, &[],
            Some((&ibytes, indices.len() as u32)),
            Some(&camera_ubo(VP_FRONT)),
        )
        .expect("render mob");
    let c = pixel(&px, dim, dim / 2, dim / 2);
    assert!(c[0] > 60 && c[0] > c[1] && c[0] > c[2], "predator should render reddish at centre, got {c:?}");
}

#[test]
fn entity_shaders_render_a_mob_in_its_colour() {
    let gh = match GraphicsHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    if !tool("glslangValidator") { eprintln!("skip: no glslangValidator"); return; }
    let vs = match compile_file("entity.vert") { Some(s) => s, None => { eprintln!("skip: entity.vert"); return; } };
    let fs = match compile_file("entity.frag") { Some(s) => s, None => { eprintln!("skip: entity.frag"); return; } };

    // Build one mob through the real registry + bake. A Critter spans a 1-block
    // cube around its position; place it centred in the [0,4] view so it covers
    // the middle of the frame.
    let mut store = EntityStore::new();
    store.spawn(EntityKind::Critter, [2.0, 2.0, 0.0]);
    let snap = EntityRenderSnapshot::capture(&store);
    let (verts, indices) = snapshot_to_vertices(&snap, default_color_for_kind);
    assert_eq!(verts.len(), 8, "one cube = 8 corners");
    assert_eq!(indices.len(), 36, "one cube = 36 indices");
    let expected = default_color_for_kind(EntityKind::Critter);

    // Pack EntityVertex { position[3], color[3] } as 24-byte interleaved.
    let mut vbytes = Vec::with_capacity(verts.len() * 24);
    for v in &verts {
        for f in v.position.iter().chain(v.color.iter()) {
            vbytes.extend_from_slice(&f.to_le_bytes());
        }
    }
    let ibytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();

    let attrs = [
        VertexAttr { location: 0, format: vk::Format::R32G32B32_SFLOAT, offset: 0 },
        VertexAttr { location: 1, format: vk::Format::R32G32B32_SFLOAT, offset: 12 },
    ];
    let dim = 32;
    let px = gh
        .render_offscreen(
            dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs,
            Some((&vbytes, 24, &attrs)), verts.len() as u32, &[],
            Some((&ibytes, indices.len() as u32)),
            Some(&camera_ubo(VP_0_4)),
        )
        .expect("render entity");

    // The mob's cube covers the centre; the pixel there should be its colour,
    // not the black clear. Compare each channel to the expected (×255) loosely.
    let c = pixel(&px, dim, dim / 2, dim / 2);
    let exp = [(expected[0] * 255.0) as i32, (expected[1] * 255.0) as i32, (expected[2] * 255.0) as i32];
    assert!((c[0] as u32 + c[1] as u32 + c[2] as u32) > 30, "mob should cover the centre, got {c:?}");
    for ch in 0..3 {
        assert!((c[ch] as i32 - exp[ch]).abs() < 40, "channel {ch}: rendered {} vs expected {} ({c:?} vs {expected:?})", c[ch], exp[ch]);
    }

    // A corner well outside the cube stays the clear colour.
    let corner = pixel(&px, dim, 1, 1);
    assert!((corner[0] as u32 + corner[1] as u32 + corner[2] as u32) < 10, "outside the mob is clear, got {corner:?}");
}

// ── Creature design candidates: render each blueprint to a file for a sheet ──

/// 3/4 orthographic view-projection that frames a creature of the given bounding
/// sphere (center, radius). Column-major; y-flipped for Vulkan NDC; z packed into
/// 0..1 for the depth test.
fn fit_vp_three_quarter(center: [f32; 3], radius: f32) -> [f32; 16] {
    let (sy, cy) = 35.0_f32.to_radians().sin_cos();
    let (sp, cp) = 22.0_f32.to_radians().sin_cos();
    let r0 = [cy, 0.0, sy];
    let r1 = [sp * sy, cp, -sp * cy];
    let r2 = [-cp * sy, sp, cp * cy];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let s = 0.82 / radius;
    let k = 0.40 / radius;
    [
        s * r0[0], -s * r1[0], k * r2[0], 0.0,
        s * r0[1], -s * r1[1], k * r2[1], 0.0,
        s * r0[2], -s * r1[2], k * r2[2], 0.0,
        -s * dot(r0, center), s * dot(r1, center), 0.5 - k * dot(r2, center), 1.0,
    ]
}

#[test]
fn render_creature_candidate_sheet() {
    let gh = match GraphicsHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    if !tool("glslangValidator") { eprintln!("skip"); return; }
    let vs = match compile_file("entity.vert") { Some(s) => s, None => { eprintln!("skip"); return; } };
    let fs = match compile_file("entity.frag") { Some(s) => s, None => { eprintln!("skip"); return; } };

    let seed: u64 = std::env::var("CAND_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(42);
    let n: usize = std::env::var("CAND_N").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    let dim: u32 = 192;
    let outdir = std::path::Path::new("/tmp/cand");
    std::fs::create_dir_all(outdir).unwrap();

    let attrs = [
        VertexAttr { location: 0, format: vk::Format::R32G32B32_SFLOAT, offset: 0 },
        VertexAttr { location: 1, format: vk::Format::R32G32B32_SFLOAT, offset: 12 },
    ];

    for (i, bp) in unendlich_engine::creature_design::generate_candidates(seed, n).into_iter().enumerate() {
        let (verts, indices) = unendlich_engine::world::entity::blueprint_to_vertices(&bp);
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in &verts {
            for k in 0..3 {
                lo[k] = lo[k].min(v.position[k]);
                hi[k] = hi[k].max(v.position[k]);
            }
        }
        let center = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
        let mut rr = 0.0f32;
        for k in 0..3 { let d = (hi[k] - lo[k]) * 0.5; rr += d * d; }
        let radius = rr.sqrt().max(0.5);
        let vp = fit_vp_three_quarter(center, radius);

        let mut vbytes = Vec::with_capacity(verts.len() * 24);
        for v in &verts {
            for f in v.position.iter().chain(v.color.iter()) {
                vbytes.extend_from_slice(&f.to_le_bytes());
            }
        }
        let ibytes: Vec<u8> = indices.iter().flat_map(|x| x.to_le_bytes()).collect();
        let px = gh
            .render_offscreen(
                dim, [0.13, 0.15, 0.18, 1.0], &vs, &fs,
                Some((&vbytes, 24, &attrs)), verts.len() as u32, &[],
                Some((&ibytes, indices.len() as u32)),
                Some(&camera_ubo(vp)),
            )
            .expect("render candidate");
        let lit = px.chunks_exact(4).filter(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) > 110).count();
        assert!(lit > 50, "candidate {i} barely rendered ({lit} px)");
        std::fs::write(outdir.join(format!("cand_{i:02}.raw")), &px).unwrap();
    }
    std::fs::write(outdir.join("meta.txt"), format!("{dim} {n} {seed}")).unwrap();
}
