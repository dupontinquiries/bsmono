//! examples/gameplay_pov_collage.rs — "a player using them" proof.
//!
//! A headless first-person POV that composites the unwired feature cores into one
//! scene the way the bin would: the forge dragon ahead (sim::dragon_forge), dirtied
//! by sim::creature_grime, shaded by render::dynamic_lights, with a held item in the
//! corner via sim::viewmodel, over ground scattered by world::flora_placement.
//! Four panels = four gameplay situations. CPU raster (perspective + painter's),
//! dependency-free PNG. Run: cargo run --release --example gameplay_pov_collage --features gpu

use unendlich_engine::render::dynamic_lights::{apply_lights, PointLight};
use unendlich_engine::sim::creature_grime::{apply_grime_color, GrimeSample, GrimeState};
use unendlich_engine::sim::dragon_forge::{build, skin, ForgeAnim};
use unendlich_engine::sim::dragon_archetype::Genome;
use unendlich_engine::sim::elemental::ItemForm;
use unendlich_engine::sim::viewmodel::{box_corners_in_view, item_model, viewmodel_pose};
use unendlich_engine::world::biomes::Biome;
use unendlich_engine::world::flora_placement::{place_at, FloraCtx, FloraSpecies};

type V3 = [f32; 3];
const PW: usize = 380; // panel
const PH: usize = 320;

fn sub(a: V3, b: V3) -> V3 { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn dot(a: V3, b: V3) -> f32 { a[0]*b[0]+a[1]*b[1]+a[2]*b[2] }
fn cross(a: V3, b: V3) -> V3 { [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]] }
fn norm(a: V3) -> V3 { let l=dot(a,a).sqrt().max(1e-6); [a[0]/l,a[1]/l,a[2]/l] }

struct Cam { eye: V3, right: V3, up: V3, fwd: V3, f: f32 }
impl Cam {
    fn new(eye: V3, target: V3) -> Cam {
        let fwd = norm(sub(target, eye));
        let right = norm(cross(fwd, [0.0,1.0,0.0]));
        let up = cross(right, fwd);
        Cam { eye, right, up, fwd, f: (PW as f32 * 0.5) / (35.0f32.to_radians()).tan() }
    }
    // world point -> (sx, sy, depth) ; depth>0 means in front
    fn proj_world(&self, p: V3) -> Option<(f32,f32,f32)> {
        let r = sub(p, self.eye);
        self.proj_cam([dot(r,self.right), dot(r,self.up), dot(r,self.fwd)])
    }
    // camera-space point (+Z forward) -> screen
    fn proj_cam(&self, c: V3) -> Option<(f32,f32,f32)> {
        if c[2] < 0.05 { return None; }
        Some((PW as f32*0.5 + self.f*c[0]/c[2], PH as f32*0.5 - self.f*c[1]/c[2], c[2]))
    }
}

struct Face { pts: [(f32,f32);4], depth: f32, col: [u8;3] }

// cube corners (lo/hi) and the 6 faces with their world normal-y sign
const FACES: [[usize;4];6] = [[0,1,3,2],[4,6,7,5],[0,4,5,1],[2,3,7,6],[1,5,7,3],[0,2,6,4]];
const FACE_NY: [f32;6] = [0.0,0.0,-1.0,1.0,0.0,0.0]; // -Y bottom(idx2), +Y top(idx3)

fn box_faces(center: V3, half: V3, base: [f32;3], ny_grime: &GrimeState, lights: &[PointLight],
             ambient: V3, emissive: bool, cam: &Cam, out: &mut Vec<Face>) {
    let c = [ [center[0]-half[0],center[1]-half[1],center[2]-half[2]],
              [center[0]+half[0],center[1]-half[1],center[2]-half[2]],
              [center[0]-half[0],center[1]+half[1],center[2]-half[2]],
              [center[0]+half[0],center[1]+half[1],center[2]-half[2]],
              [center[0]-half[0],center[1]-half[1],center[2]+half[2]],
              [center[0]+half[0],center[1]-half[1],center[2]+half[2]],
              [center[0]-half[0],center[1]+half[1],center[2]+half[2]],
              [center[0]+half[0],center[1]+half[1],center[2]+half[2]] ];
    let mut sp = [(0.0,0.0,0.0);8];
    for i in 0..8 { match cam.proj_world(c[i]) { Some(p)=>sp[i]=p, None=>return } }
    let light = if emissive { [1.0,1.0,1.0] } else { apply_lights(center, lights) };
    for (fi, f) in FACES.iter().enumerate() {
        // backface cull (screen winding)
        let (a,b,cc) = (sp[f[0]], sp[f[1]], sp[f[2]]);
        let area = (b.0-a.0)*(cc.1-a.1) - (b.1-a.1)*(cc.0-a.0);
        if area <= 0.0 { continue; }
        // grime by this face's up-ness, then light
        let dirty = if emissive { base } else { apply_grime_color(base, FACE_NY[fi], ny_grime) };
        let lit = if emissive { base } else {
            [ (dirty[0]*(ambient[0]+light[0])).min(1.0),
              (dirty[1]*(ambient[1]+light[1])).min(1.0),
              (dirty[2]*(ambient[2]+light[2])).min(1.0) ]
        };
        let depth = (sp[f[0]].2+sp[f[1]].2+sp[f[2]].2+sp[f[3]].2)*0.25;
        out.push(Face{ pts:[(sp[f[0]].0,sp[f[0]].1),(sp[f[1]].0,sp[f[1]].1),(sp[f[2]].0,sp[f[2]].1),(sp[f[3]].0,sp[f[3]].1)],
                       depth, col:[(lit[0]*255.0)as u8,(lit[1]*255.0)as u8,(lit[2]*255.0)as u8] });
    }
}

fn fill_tri(buf: &mut [u8], ox: usize, oy: usize, a:(f32,f32),b:(f32,f32),c:(f32,f32), col:[u8;3]) {
    let minx = a.0.min(b.0).min(c.0).floor().max(0.0) as i32;
    let maxx = a.0.max(b.0).max(c.0).ceil().min(PW as f32-1.0) as i32;
    let miny = a.1.min(b.1).min(c.1).floor().max(0.0) as i32;
    let maxy = a.1.max(b.1).max(c.1).ceil().min(PH as f32-1.0) as i32;
    let area = (b.0-a.0)*(c.1-a.1)-(b.1-a.1)*(c.0-a.0);
    if area.abs() < 1e-3 { return; }
    for y in miny..=maxy { for x in minx..=maxx {
        let (px,py)=(x as f32+0.5,y as f32+0.5);
        let w0=((b.0-px)*(c.1-py)-(b.1-py)*(c.0-px))/area;
        let w1=((c.0-px)*(a.1-py)-(c.1-py)*(a.0-px))/area;
        let w2=1.0-w0-w1;
        if w0>=-0.01 && w1>=-0.01 && w2>=-0.01 {
            let i=((oy+y as usize)*(PW*2)+(ox+x as usize))*4;
            if i+3 < buf.len() { buf[i]=col[0]; buf[i+1]=col[1]; buf[i+2]=col[2]; buf[i+3]=255; }
        }
    }}
}

fn draw_faces(buf: &mut [u8], ox: usize, oy: usize, faces: &mut Vec<Face>) {
    faces.sort_by(|p,q| q.depth.partial_cmp(&p.depth).unwrap_or(std::cmp::Ordering::Equal));
    for f in faces.iter() {
        fill_tri(buf, ox, oy, f.pts[0],f.pts[1],f.pts[2], f.col);
        fill_tri(buf, ox, oy, f.pts[0],f.pts[2],f.pts[3], f.col);
    }
}

struct Scene { grime_mud: f32, lights: Vec<PointLight>, ambient: V3, sky_top: [u8;3], sky_bot: [u8;3], form: ItemForm, label: &'static str }

fn render_panel(buf: &mut [u8], ox: usize, oy: usize, sc: &Scene) {
    // sky gradient + ground
    for y in 0..PH { for x in 0..PW {
        let t = y as f32/PH as f32;
        let horizon = 0.52;
        let col = if t < horizon {
            let k=t/horizon; [lerp(sc.sky_top[0],sc.sky_bot[0],k),lerp(sc.sky_top[1],sc.sky_bot[1],k),lerp(sc.sky_top[2],sc.sky_bot[2],k)]
        } else { [40,52,32] };
        let i=((oy+y)*(PW*2)+(ox+x))*4; buf[i]=col[0];buf[i+1]=col[1];buf[i+2]=col[2];buf[i+3]=255;
    }}
    let cam = Cam::new([2.8,2.2,4.8],[0.0,1.5,-0.3]);
    // ground flora dots (project a grid of cells in front of the player)
    let fctx = FloraCtx{ biome:Biome::Plains, moisture:0.6, temperature01:0.6, slope:0.05, surface_y:80, sea_level:62, is_rock:false };
    for gz in -10..2 { for gx in -8..8 {
        let wx=gx as f32*0.6; let wz=gz as f32*0.6;
        if let Some(sp)=place_at(gx*3, gz*3, &fctx, 20251) {
            if let Some((sx,sy,_))=cam.proj_world([wx,0.0,wz]) {
                let c=species_color(sp);
                for dy in 0..2 { for dx in 0..2 {
                    let (px,py)=(sx as i32+dx, sy as i32+dy);
                    if px>=0&&py>=0&&(px as usize)<PW&&(py as usize)<PH {
                        let i=((oy+py as usize)*(PW*2)+(ox+px as usize))*4; buf[i]=c[0];buf[i+1]=c[1];buf[i+2]=c[2];
                    }
                }}
            }
        }
    }}
    // the dragon (forge), grimed + lit
    let g = Genome::from_seed(7);
    let model = build(&g, 22);
    let mut grime = GrimeState::clean();
    for _ in 0..200 { grime.accumulate(&GrimeSample{ soil_grime01: sc.grime_mud, wetness01:0.0, activity01:0.7, soil_tint:[0.34,0.26,0.18], dt:1.0/60.0 }); }
    let anim = ForgeAnim{ gait_phase: 1.7, anim_time: 0.6, ..Default::default() };
    let boxes = skin(&model, &anim);
    // scale forge model to ~2.8m tall, sit on ground, ROTATED so its long
    // (Z/length) axis runs left-right across the view — side profile, not head-on.
    let (mn,mx)=(model.min,model.max);
    let mh=(mx[1]-mn[1]).max(0.1); let scale=3.7/mh;
    let mut faces=Vec::new();
    for b in &boxes {
        let ctr=[ (b.center[2]-(mn[2]+mx[2])*0.5)*scale,        // model Z (length) -> world X
                  (b.center[1]-mn[1])*scale + 0.05,             // up
                  -(b.center[0]-(mn[0]+mx[0])*0.5)*scale - 0.3 ]; // model X (width) -> world Z (depth)
        let half=[b.half[2]*scale, b.half[1]*scale, b.half[0]*scale];
        let base = if b.glow { model.eye_color } else if b.mouth { model.mouth_color } else { model.body_color };
        box_faces(ctr, half, base, &grime, &sc.lights, sc.ambient, b.glow, &cam, &mut faces);
    }
    draw_faces(buf, ox, oy, &mut faces);
    // viewmodel item in hand (view space, always on top)
    let pose = viewmodel_pose(sc.form, 1.7, 0.6, [0.04,0.0]);
    let mut vfaces=Vec::new();
    for ib in item_model(sc.form) {
        let (vc,col)=box_corners_in_view(&pose,&ib);
        // vc are view-space corners (−Z fwd); convert to cam-space (+Z fwd) and project
        let mut sp=[(0.0,0.0,0.0);8];
        let mut ok=true;
        for i in 0..8 { match cam.proj_cam([vc[i][0],vc[i][1],-vc[i][2]]) { Some(p)=>sp[i]=p, None=>{ok=false;break} } }
        if !ok { continue; }
        for f in FACES.iter() {
            let (a,b,cc)=(sp[f[0]],sp[f[1]],sp[f[2]]);
            if (b.0-a.0)*(cc.1-a.1)-(b.1-a.1)*(cc.0-a.0) <= 0.0 { continue; }
            let d=(sp[f[0]].2+sp[f[1]].2+sp[f[2]].2+sp[f[3]].2)*0.25;
            vfaces.push(Face{pts:[(sp[f[0]].0,sp[f[0]].1),(sp[f[1]].0,sp[f[1]].1),(sp[f[2]].0,sp[f[2]].1),(sp[f[3]].0,sp[f[3]].1)],depth:d,col});
        }
    }
    draw_faces(buf, ox, oy, &mut vfaces);
    draw_label(buf, ox+8, oy+8, sc.label);
}

fn lerp(a:u8,b:u8,t:f32)->u8 { (a as f32+(b as f32-a as f32)*t) as u8 }
fn species_color(s: FloraSpecies)->[u8;3]{ use FloraSpecies::*; match s{
    Grass=>[120,210,90],Shrub=>[70,150,70],Flower=>[240,140,200],Fern=>[40,170,120],Reed=>[150,200,120],
    Mushroom=>[210,150,110],Cactus=>[60,200,160],Conifer=>[30,110,90],Broadleaf=>[80,200,70],Palm=>[200,200,90]}}

fn main() {
    let panels = [
        Scene{ grime_mud:0.0, ambient:[0.9,0.9,0.95], lights:vec![PointLight{pos:[6.0,9.0,6.0],color:[1.0,0.97,0.85],intensity:0.6,radius:40.0}], sky_top:[120,165,225], sky_bot:[195,215,235], form:ItemForm::Staff, label:"DAYLIGHT - clean - staff" },
        Scene{ grime_mud:0.9, ambient:[0.75,0.78,0.8], lights:vec![PointLight{pos:[6.0,9.0,6.0],color:[1.0,0.97,0.85],intensity:0.5,radius:40.0}], sky_top:[150,150,140], sky_bot:[180,175,160], form:ItemForm::Axe, label:"MUDDY - grime on belly/feet - axe" },
        Scene{ grime_mud:0.3, ambient:[0.14,0.14,0.20], lights:vec![
            PointLight{pos:[-2.0,2.2,1.0],color:[1.0,0.55,0.2],intensity:3.0,radius:11.0},   // forge/torch
            PointLight{pos:[0.5,2.9,1.0],color:[0.5,0.8,1.0],intensity:1.8,radius:9.0},      // eye glow
            PointLight{pos:[2.6,0.6,1.6],color:[1.0,0.25,0.15],intensity:2.6,radius:10.0}],  // fire on ground
            sky_top:[14,16,30], sky_bot:[26,28,44], form:ItemForm::Blade, label:"CAVE/NIGHT - dynamic point lights - blade" },
        Scene{ grime_mud:0.5, ambient:[0.55,0.55,0.62], lights:vec![
            PointLight{pos:[5.0,8.0,5.0],color:[1.0,0.95,0.8],intensity:0.4,radius:40.0},
            PointLight{pos:[-1.0,1.8,0.5],color:[0.3,0.9,0.6],intensity:1.1,radius:7.0}],
            sky_top:[80,110,150], sky_bot:[150,170,180], form:ItemForm::Tool, label:"DUSK - mixed light + grime - tool" },
    ];
    let (cw,ch)=(PW*2, PH*2);
    let mut buf=vec![0u8; cw*ch*4];
    for (i,sc) in panels.iter().enumerate() {
        render_panel(&mut buf, (i%2)*PW, (i/2)*PH, sc);
    }
    write_png("/mnt/user-data/outputs/gameplay_pov_collage.png", cw, ch, &buf).expect("png");
    println!("wrote gameplay_pov_collage.png ({cw}x{ch}) — 4 POV panels (grime+lights+viewmodel+flora on the forge dragon)");
}

// tiny 5x7 label font (only the glyphs the labels use)
fn draw_label(buf:&mut [u8], ox:usize, oy:usize, s:&str){
    for (i,ch) in s.chars().enumerate(){ let g=glyph(ch); for (r,row) in g.iter().enumerate(){ for c in 0..5 {
        if (row>>(4-c))&1==1 { let (x,y)=(ox+i*6+c, oy+r); if x<PW*2&&y<PH*2 { let idx=(y*(PW*2)+x)*4; buf[idx]=255;buf[idx+1]=255;buf[idx+2]=255;buf[idx+3]=255; } } } } }
}
fn glyph(c:char)->[u8;7]{ match c.to_ascii_uppercase() {
    'A'=>[0b01110,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001],'B'=>[0b11110,0b10001,0b11110,0b10001,0b10001,0b10001,0b11110],
    'C'=>[0b01110,0b10001,0b10000,0b10000,0b10000,0b10001,0b01110],'D'=>[0b11110,0b10001,0b10001,0b10001,0b10001,0b10001,0b11110],
    'E'=>[0b11111,0b10000,0b11110,0b10000,0b10000,0b10000,0b11111],'F'=>[0b11111,0b10000,0b11110,0b10000,0b10000,0b10000,0b10000],
    'G'=>[0b01110,0b10001,0b10000,0b10111,0b10001,0b10001,0b01110],'H'=>[0b10001,0b10001,0b11111,0b10001,0b10001,0b10001,0b10001],
    'I'=>[0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b11111],'K'=>[0b10001,0b10010,0b11100,0b10010,0b10001,0b10001,0b10001],
    'L'=>[0b10000,0b10000,0b10000,0b10000,0b10000,0b10000,0b11111],'M'=>[0b10001,0b11011,0b10101,0b10001,0b10001,0b10001,0b10001],
    'N'=>[0b10001,0b11001,0b10101,0b10011,0b10001,0b10001,0b10001],'O'=>[0b01110,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],
    'P'=>[0b11110,0b10001,0b10001,0b11110,0b10000,0b10000,0b10000],'R'=>[0b11110,0b10001,0b10001,0b11110,0b10010,0b10001,0b10001],
    'S'=>[0b01111,0b10000,0b10000,0b01110,0b00001,0b00001,0b11110],'T'=>[0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100],
    'U'=>[0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],'V'=>[0b10001,0b10001,0b10001,0b10001,0b10001,0b01010,0b00100],
    'W'=>[0b10001,0b10001,0b10001,0b10101,0b10101,0b11011,0b10001],'Y'=>[0b10001,0b10001,0b01010,0b00100,0b00100,0b00100,0b00100],
    '-'=>[0,0,0,0b11111,0,0,0],' '=>[0,0,0,0,0,0,0],'/'=>[0b00001,0b00010,0b00100,0b00100,0b01000,0b10000,0b10000],
    _=>[0,0,0b11111,0,0b11111,0,0]}}

fn write_png(path:&str,w:usize,h:usize,rgba:&[u8])->std::io::Result<()>{use std::io::Write;
    let mut out=vec![0x89,b'P',b'N',b'G',0x0d,0x0a,0x1a,0x0a];
    let mut ih=Vec::new(); ih.extend_from_slice(&(w as u32).to_be_bytes()); ih.extend_from_slice(&(h as u32).to_be_bytes()); ih.extend_from_slice(&[8,6,0,0,0]);
    ch(&mut out,b"IHDR",&ih);
    let mut raw=Vec::with_capacity(h*(1+w*4)); for y in 0..h{raw.push(0);raw.extend_from_slice(&rgba[y*w*4..(y+1)*w*4]);}
    ch(&mut out,b"IDAT",&zl(&raw)); ch(&mut out,b"IEND",&[]); std::fs::File::create(path)?.write_all(&out)}
fn ch(out:&mut Vec<u8>,k:&[u8;4],d:&[u8]){out.extend_from_slice(&(d.len()as u32).to_be_bytes());out.extend_from_slice(k);out.extend_from_slice(d);
    let mut c=0xffff_ffffu32; for b in k.iter().chain(d.iter()){c^=*b as u32; for _ in 0..8{c=if c&1!=0{(c>>1)^0xedb8_8320}else{c>>1};}} out.extend_from_slice(&(c^0xffff_ffff).to_be_bytes());}
fn zl(d:&[u8])->Vec<u8>{let mut z=vec![0x78,0x01];let mut i=0;while i<d.len(){let n=(d.len()-i).min(0xffff);let f=i+n>=d.len();z.push(if f{1}else{0});z.extend_from_slice(&(n as u16).to_le_bytes());z.extend_from_slice(&(!(n as u16)).to_le_bytes());z.extend_from_slice(&d[i..i+n]);i+=n;}
    let(mut a,mut b)=(1u32,0u32);for &x in d{a=(a+x as u32)%65521;b=(b+a)%65521;}z.extend_from_slice(&((b<<16)|a).to_be_bytes());z}
