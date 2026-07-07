//! material_tour — stand in one map per material family and render it first-person, so the atlas materials
//! (now that chunking preserves them) are actually visible. Prints the dominant materials in each shot.
use early_fps::camera::{scale_for_fov, FpsCam};
use early_fps::game::Game;
use early_fps::level::PRESETS;
use early_fps::loadout::Loadout;
use early_fps::math::Vec3;
use early_fps::mesh::build_material_atlas;

const W: usize = 960;
const H: usize = 600;
const COLS: usize = 5;

fn main() {
    let (atlas, asz, _) = build_material_atlas(512, COLS);
    // (preset, what family it showcases)
    let tour = [
        ("Iron Works", "metal / rust / corrugated"),
        ("Pyrefall", "volcanic stone"),
        ("Frostbite", "snow + stone"),
        ("Mistfall", "dirt / cobble / stone"),
        ("Canopy Reach", "grass + foliage"),
        ("Sunfield", "meadow grass + foliage"),
    ];
    for (map, family) in tour {
        let Some(i) = PRESETS.iter().position(|p| p.name == map) else { println!("{map}: not found"); continue };
        let mut game = Game::new_preset(3, Some(i), Loadout::default());
        let inp = early_fps::player::Input::default();
        for _ in 0..3 { game.step(&inp, 1.0 / 60.0); }
        let lvl = &game.level;
        let (mcx, mcz) = (lvl.w as f32 * 0.5 * lvl.ts(), lvl.h as f32 * 0.5 * lvl.ts());
        let spawn = lvl.rooms[0].center;
        let mut d = Vec3::new(mcx - spawn.x, 0.0, mcz - spawn.z);
        let dl = (d.x * d.x + d.z * d.z).sqrt().max(0.001); d = d * (1.0 / dl);
        let px = (spawn.x - d.x * 3.0).clamp(lvl.ts() * 1.5, mcx * 2.0 - lvl.ts() * 1.5);
        let pz = (spawn.z - d.z * 3.0).clamp(lvl.ts() * 1.5, mcz * 2.0 - lvl.ts() * 1.5);
        let cam = FpsCam { pos: Vec3::new(px, lvl.floor_h(px, pz) + 1.75, pz), yaw: d.x.atan2(-d.z), pitch: -0.12 };
        let scale = scale_for_fov(H as f32, 72f32.to_radians());
        let meshes = game.frame_meshes(&cam, false);
        let mesh = early_fps::render::render(&meshes, &cam, W as f32 * 0.5, H as f32 * 0.5, scale, &game.light, &game.fog, W as f32, H as f32, game.time, game.wind(), egui::TextureId::Managed(1), COLS, &game.dynamic_lights());
        let mut buf = vec![0u8; W * H * 3];
        let sky = game.light.sky; let fog = game.fog.color;
        for y in 0..H { let c = if y < H/2 { [(sky.r() as f32*0.85) as u8,(sky.g() as f32*0.85) as u8,(sky.b() as f32*0.85) as u8] } else { [fog.r(),fog.g(),fog.b()] };
            for x in 0..W { buf[(y*W+x)*3..(y*W+x)*3+3].copy_from_slice(&c); } }
        for tri in mesh.indices.chunks_exact(3) {
            let p = [mesh.vertices[tri[0] as usize], mesh.vertices[tri[1] as usize], mesh.vertices[tri[2] as usize]];
            raster(&mut buf, [[p[0].pos.x,p[0].pos.y],[p[1].pos.x,p[1].pos.y],[p[2].pos.x,p[2].pos.y]], [p[0].color,p[1].color,p[2].color], [[p[0].uv.x,p[0].uv.y],[p[1].uv.x,p[1].uv.y],[p[2].uv.x,p[2].uv.y]], &atlas, asz);
        }
        let fname = format!("tour_{}.png", map.replace(' ', "_").to_lowercase());
        image::save_buffer(&fname, &buf, W as u32, H as u32, image::ColorType::Rgb8).unwrap();
        // dominant materials
        let mut hist = std::collections::BTreeMap::<String,u32>::new();
        for m in &meshes { if let Some(x)=m.material { *hist.entry(format!("{x:?}")).or_default() += m.tris.len() as u32; } }
        let mut v: Vec<_> = hist.into_iter().collect(); v.sort_by(|a,b| b.1.cmp(&a.1));
        let top: Vec<String> = v.into_iter().take(4).map(|(k,c)| format!("{k}({c})")).collect();
        println!("{map:<14} [{family}] -> {}", top.join(" "));
    }
}
#[allow(clippy::too_many_arguments)]
fn raster(buf: &mut [u8], p: [[f32;2];3], col: [egui::Color32;3], uv: [[f32;2];3], atlas: &[u8], asz: usize) {
    let minx=p.iter().map(|v|v[0]).fold(f32::MAX,f32::min).floor().max(0.0) as usize;
    let maxx=p.iter().map(|v|v[0]).fold(f32::MIN,f32::max).ceil().min(W as f32-1.0) as usize;
    let miny=p.iter().map(|v|v[1]).fold(f32::MAX,f32::min).floor().max(0.0) as usize;
    let maxy=p.iter().map(|v|v[1]).fold(f32::MIN,f32::max).ceil().min(H as f32-1.0) as usize;
    if minx>maxx||miny>maxy {return;}
    let area=edge(p[0],p[1],p[2]); if area.abs()<1e-6 {return;} let inv=1.0/area;
    for y in miny..=maxy { for x in minx..=maxx {
        let pt=[x as f32+0.5,y as f32+0.5];
        let (w0,w1,w2)=(edge(p[1],p[2],pt)*inv,edge(p[2],p[0],pt)*inv,edge(p[0],p[1],pt)*inv);
        if (w0<0.0||w1<0.0||w2<0.0)&&(w0>0.0||w1>0.0||w2>0.0) {continue;}
        let u=w0*uv[0][0]+w1*uv[1][0]+w2*uv[2][0]; let v=w0*uv[0][1]+w1*uv[1][1]+w2*uv[2][1];
        let tx=(u.clamp(0.0,0.999)*asz as f32) as usize; let ty=(v.clamp(0.0,0.999)*asz as f32) as usize;
        let ti=(ty*asz+tx)*4;
        let cr=w0*col[0].r() as f32+w1*col[1].r() as f32+w2*col[2].r() as f32;
        let cg=w0*col[0].g() as f32+w1*col[1].g() as f32+w2*col[2].g() as f32;
        let cb=w0*col[0].b() as f32+w1*col[1].b() as f32+w2*col[2].b() as f32;
        let i=(y*W+x)*3;
        buf[i]=(atlas[ti] as f32*cr/255.0).clamp(0.0,255.0) as u8;
        buf[i+1]=(atlas[ti+1] as f32*cg/255.0).clamp(0.0,255.0) as u8;
        buf[i+2]=(atlas[ti+2] as f32*cb/255.0).clamp(0.0,255.0) as u8;
    }}
}
#[inline]
fn edge(a:[f32;2],b:[f32;2],c:[f32;2])->f32 {(c[0]-a[0])*(b[1]-a[1])-(c[1]-a[1])*(b[0]-a[0])}
