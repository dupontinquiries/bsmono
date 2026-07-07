// Headless verification: render a small scene through the real renderer, then CPU-rasterize the textured
// egui mesh (sampling the material atlas) to a PNG so we can SEE the textures map correctly.
use early_fps::camera::FpsCam;
use early_fps::math::{Vec2, Vec3};
use early_fps::mesh::{boxx, build_material_atlas, rgb, Light, Material, Mesh3, FACE_ALL};
use early_fps::render::render;
use std::io::Write;

fn main() {
    let (atlas, aw, ah) = build_material_atlas(512, 5);
    let sample = |u: f32, v: f32| -> [f32; 3] {
        let x = ((u.fract().abs()) * aw as f32) as usize % aw;
        let y = ((v.fract().abs()) * ah as f32) as usize % ah;
        let i = (y * aw + x) * 4;
        [atlas[i] as f32, atlas[i + 1] as f32, atlas[i + 2] as f32]
    };

    // scene: a brick floor, a plaster wall, a planks box, a sand patch
    let mut meshes = Vec::new();
    let mut floor = Mesh3::new(rgb(255, 255, 255));
    floor.push_quad(Vec3::new(-6.0, -1.6, 0.0), Vec3::new(6.0, -1.6, 0.0), Vec3::new(6.0, -1.6, 16.0), Vec3::new(-6.0, -1.6, 16.0), Vec3::UP, rgb(255, 255, 255));
    // tessellate floor into tiles so each shows one material tile
    let mut floor2 = Mesh3::new(rgb(255, 255, 255));
    for zt in 0..8 {
        for xt in 0..6 {
            let x0 = -6.0 + xt as f32 * 2.0;
            let z0 = 1.0 + zt as f32 * 2.0;
            floor2.push_quad(Vec3::new(x0, -1.6, z0), Vec3::new(x0 + 2.0, -1.6, z0), Vec3::new(x0 + 2.0, -1.6, z0 + 2.0), Vec3::new(x0, -1.6, z0 + 2.0), Vec3::UP, rgb(255, 255, 255));
        }
    }
    floor2.material = Some(Material::Brick);
    meshes.push(floor2);
    let mut wall = boxx(Vec3::new(0.0, 0.4, 9.0), Vec3::new(5.0, 2.0, 0.3), rgb(255, 255, 255), FACE_ALL);
    wall.material = Some(Material::Plaster);
    meshes.push(wall);
    let mut crate_box = boxx(Vec3::new(-2.5, -0.9, 5.0), Vec3::splat(0.7), rgb(255, 255, 255), FACE_ALL);
    crate_box.material = Some(Material::Planks);
    meshes.push(crate_box);
    let mut stone = boxx(Vec3::new(2.5, -0.7, 5.5), Vec3::new(0.9, 0.9, 0.9), rgb(255, 255, 255), FACE_ALL);
    stone.material = Some(Material::StoneBlock);
    meshes.push(stone);

    let cam = FpsCam { pos: Vec3::new(0.0, 0.6, -2.0), yaw: std::f32::consts::PI, pitch: -0.12 };
    let light = Light::sun(Vec3::new(0.4, 1.0, 0.3), rgb(255, 246, 220));
    let fog = early_fps::render::Fog::new(rgb(150, 162, 178), 30.0, 80.0);
    let (vw, vh) = (640.0f32, 400.0f32);
    let mesh = render(&meshes, &cam, vw / 2.0, vh / 2.0, 420.0, &light, &fog, vw, vh, 0.0, Vec2::new(0.0, 0.0), egui::TextureId::Managed(1), 5, &[]);

    // CPU rasterize the textured mesh
    let (w, h) = (vw as usize, vh as usize);
    let mut img = vec![28u8; w * h * 3];
    let v = &mesh.vertices;
    for tri in mesh.indices.chunks(3) {
        let p = [&v[tri[0] as usize], &v[tri[1] as usize], &v[tri[2] as usize]];
        let (minx, maxx) = (p.iter().map(|q| q.pos.x).fold(f32::MAX, f32::min).max(0.0), p.iter().map(|q| q.pos.x).fold(f32::MIN, f32::max).min(w as f32 - 1.0));
        let (miny, maxy) = (p.iter().map(|q| q.pos.y).fold(f32::MAX, f32::min).max(0.0), p.iter().map(|q| q.pos.y).fold(f32::MIN, f32::max).min(h as f32 - 1.0));
        let area = (p[1].pos.x - p[0].pos.x) * (p[2].pos.y - p[0].pos.y) - (p[2].pos.x - p[0].pos.x) * (p[1].pos.y - p[0].pos.y);
        if area.abs() < 1e-3 { continue; }
        for py in (miny as usize)..=(maxy as usize) {
            for px in (minx as usize)..=(maxx as usize) {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let w0 = ((p[1].pos.x - fx) * (p[2].pos.y - fy) - (p[2].pos.x - fx) * (p[1].pos.y - fy)) / area;
                let w1 = ((p[2].pos.x - fx) * (p[0].pos.y - fy) - (p[0].pos.x - fx) * (p[2].pos.y - fy)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 { continue; }
                let u = w0 * p[0].uv.x + w1 * p[1].uv.x + w2 * p[2].uv.x;
                let vv = w0 * p[0].uv.y + w1 * p[1].uv.y + w2 * p[2].uv.y;
                let tex = sample(u, vv);
                let cr = w0 * p[0].color.r() as f32 + w1 * p[1].color.r() as f32 + w2 * p[2].color.r() as f32;
                let cg = w0 * p[0].color.g() as f32 + w1 * p[1].color.g() as f32 + w2 * p[2].color.g() as f32;
                let cb = w0 * p[0].color.b() as f32 + w1 * p[1].color.b() as f32 + w2 * p[2].color.b() as f32;
                let i = (py * w + px) * 3;
                img[i] = (tex[0] * cr / 255.0).min(255.0) as u8;
                img[i + 1] = (tex[1] * cg / 255.0).min(255.0) as u8;
                img[i + 2] = (tex[2] * cb / 255.0).min(255.0) as u8;
            }
        }
    }
    let mut f = std::fs::File::create("/tmp/texscene.ppm").unwrap();
    write!(f, "P6\n{} {}\n255\n", w, h).unwrap();
    f.write_all(&img).unwrap();
    println!("rendered {} tris, wrote /tmp/texscene.ppm", mesh.indices.len() / 3);
}
