use early_fps::camera::{scale_for_fov, FpsCam};
use early_fps::mesh::{build_material_atlas, Light};
use early_fps::math::{Vec2, Vec3};
use early_fps::procgen::{generate_themed, ActivationNet, ProcParams};
use early_fps::render::Fog;
use early_fps::rosetta::{extract, split_specs};
const W: usize=1000; const H: usize=620; const COLS: usize=5;
fn main() {
    let doc = std::fs::read_to_string("docs/design/map-specs.txt").unwrap();
    let blocks = split_specs(&doc);
    let (atlas,aw,ah)=build_material_atlas(512,COLS);
    let params=ProcParams::default();
    let net=ActivationNet::default();
    for want in ["Elevated Spine","Archivist","Gondola"] {
        let block = blocks.iter().find(|b| extract(b).name.contains(want)).unwrap();
        let target = extract(block);
        let (level,_) = generate_themed(7, &net, &params, &target);
        // frame densest cluster
        let (mut bx,mut bz,mut bn)=(0.0f32,0.0f32,0usize);
        for o in &level.obstacles {
            let n=level.obstacles.iter().filter(|q|(q.center.x-o.center.x).abs()<18.0&&(q.center.z-o.center.z).abs()<18.0).count();
            if n>bn {bn=n;bx=o.center.x;bz=o.center.z;}
        }
        if bn==0 { bx=level.w as f32*level.ts()*0.5; bz=level.h as f32*level.ts()*0.5; }
        let dist=16.0; let ang=2.4f32;
        let pos=Vec3::new(bx+ang.cos()*dist, level.floor_y+5.0, bz+ang.sin()*dist);
        let tgt=Vec3::new(bx,level.floor_y+1.2,bz);
        let f=(tgt-pos).normalized();
        let cam=FpsCam{pos,yaw:f.x.atan2(-f.z),pitch:f.y.clamp(-1.4,1.4).asin()};
        let mut light=Light::sun(Vec3::new(-0.4,0.92,0.42), early_fps::mesh::rgb(255,248,230));
        light.sky=target.palette.sky; light.ambient=1.05;
        let fog=Fog::new(target.palette.fog,40.0,170.0);
        let scale=scale_for_fov(H as f32,68f32.to_radians());
        let meshes: Vec<_> = if let Some(fl)=&level.lod_field { fl.render_meshes().into_iter().cloned().collect() } else { level.meshes.clone() };
        let mesh=early_fps::render::render(&meshes,&cam,W as f32*0.5,H as f32*0.5,scale,&light,&fog,W as f32,H as f32,0.0,Vec2::new(0.0,0.0),egui::TextureId::Managed(1),COLS,&[]);
        let mut buf=vec![0u8;W*H*3];
        let sky=[target.palette.sky.r(),target.palette.sky.g(),target.palette.sky.b()];
        let gc=target.palette.fog; let grd=[(gc.r() as f32*0.6)as u8,(gc.g()as f32*0.62)as u8,(gc.b()as f32*0.58)as u8];
        for y in 0..H { let c=if y<H/2{sky}else{grd}; for x in 0..W { let i=(y*W+x)*3; buf[i..i+3].copy_from_slice(&c);}}
        for tri in mesh.indices.chunks_exact(3){
            let p=[mesh.vertices[tri[0]as usize],mesh.vertices[tri[1]as usize],mesh.vertices[tri[2]as usize]];
            raster(&mut buf,[[p[0].pos.x,p[0].pos.y],[p[1].pos.x,p[1].pos.y],[p[2].pos.x,p[2].pos.y]],[p[0].color,p[1].color,p[2].color],[[p[0].uv.x,p[0].uv.y],[p[1].uv.x,p[1].uv.y],[p[2].uv.x,p[2].uv.y]],&atlas,aw,ah);
        }
        let fname=want.to_lowercase().replace(' ',"_").replace('\'',"");
        image::save_buffer(&format!("rosetta_{fname}.png"),&buf,W as u32,H as u32,image::ColorType::Rgb8).unwrap();
        println!("wrote rosetta_{fname}.png  ({} obst, {} water, palette sky=({},{},{}))", level.obstacles.len(), level.water.len(), sky[0],sky[1],sky[2]);
    }
}
#[allow(clippy::too_many_arguments)]
fn raster(buf:&mut[u8],p:[[f32;2];3],col:[egui::Color32;3],uv:[[f32;2];3],atlas:&[u8],aw:usize,ah:usize){
    let minx=p.iter().map(|v|v[0]).fold(f32::MAX,f32::min).floor().max(0.0)as usize;
    let maxx=p.iter().map(|v|v[0]).fold(f32::MIN,f32::max).ceil().min(W as f32-1.0)as usize;
    let miny=p.iter().map(|v|v[1]).fold(f32::MAX,f32::min).floor().max(0.0)as usize;
    let maxy=p.iter().map(|v|v[1]).fold(f32::MIN,f32::max).ceil().min(H as f32-1.0)as usize;
    if minx>maxx||miny>maxy{return;}
    let area=edge(p[0],p[1],p[2]); if area.abs()<1e-6{return;} let inv=1.0/area;
    for y in miny..=maxy{for x in minx..=maxx{
        let pt=[x as f32+0.5,y as f32+0.5];
        let w0=edge(p[1],p[2],pt)*inv; let w1=edge(p[2],p[0],pt)*inv; let w2=edge(p[0],p[1],pt)*inv;
        if (w0<0.0||w1<0.0||w2<0.0)&&(w0>0.0||w1>0.0||w2>0.0){continue;}
        let u=w0*uv[0][0]+w1*uv[1][0]+w2*uv[2][0]; let v=w0*uv[0][1]+w1*uv[1][1]+w2*uv[2][1];
        let tx=(u.clamp(0.0,0.999)*aw as f32)as usize; let ty=(v.clamp(0.0,0.999)*ah as f32)as usize;
        let ti=(ty*aw+tx)*4;
        let cr=w0*col[0].r()as f32+w1*col[1].r()as f32+w2*col[2].r()as f32;
        let cg=w0*col[0].g()as f32+w1*col[1].g()as f32+w2*col[2].g()as f32;
        let cb=w0*col[0].b()as f32+w1*col[1].b()as f32+w2*col[2].b()as f32;
        let i=(y*W+x)*3;
        buf[i]=(atlas[ti]as f32*cr/255.0).clamp(0.0,255.0)as u8;
        buf[i+1]=(atlas[ti+1]as f32*cg/255.0).clamp(0.0,255.0)as u8;
        buf[i+2]=(atlas[ti+2]as f32*cb/255.0).clamp(0.0,255.0)as u8;
    }}
}
#[inline] fn edge(a:[f32;2],b:[f32;2],c:[f32;2])->f32{(c[0]-a[0])*(b[1]-a[1])-(c[1]-a[1])*(b[0]-a[0])}
