//! examples/title_blocks_scene.rs — the UNENDLICH title as REAL voxel blocks in a
//! 3D scene (perspective cubes, sky + ground), proving the in-game-blocks look the
//! menu/world will show. CPU raster (painter's), dependency-free PNG.
//! Run: cargo run --release --example title_blocks_scene --features gpu

use unendlich_engine::world::blocks::BlockId;
use unendlich_engine::world::title_structure::{unendlich_blocks, TITLE_BLOCK};

const W: usize = 920;
const H: usize = 360;

type V3 = [f32; 3];
fn sub(a: V3, b: V3) -> V3 { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn dot(a: V3, b: V3) -> f32 { a[0]*b[0]+a[1]*b[1]+a[2]*b[2] }
fn cross(a: V3, b: V3) -> V3 { [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]] }
fn norm(a: V3) -> V3 { let l=dot(a,a).sqrt().max(1e-6); [a[0]/l,a[1]/l,a[2]/l] }

struct Cam { eye:V3, right:V3, up:V3, fwd:V3, f:f32 }
impl Cam {
    fn new(eye:V3, target:V3)->Cam{ let fwd=norm(sub(target,eye)); let right=norm(cross(fwd,[0.0,1.0,0.0])); let up=cross(right,fwd);
        Cam{eye,right,up,fwd,f:(W as f32*0.5)/(30.0f32.to_radians()).tan()} }
    fn proj(&self,p:V3)->Option<(f32,f32,f32)>{ let r=sub(p,self.eye); let c=[dot(r,self.right),dot(r,self.up),dot(r,self.fwd)];
        if c[2]<0.05 {return None;} Some((W as f32*0.5+self.f*c[0]/c[2], H as f32*0.5-self.f*c[1]/c[2], c[2])) }
}
struct Face{ pts:[(f32,f32);4], depth:f32, col:[u8;3] }
const FACES:[[usize;4];6]=[[0,1,3,2],[4,6,7,5],[0,4,5,1],[2,3,7,6],[1,5,7,3],[0,2,6,4]];
const SHADE:[f32;6]=[0.78,0.95,0.62,1.0,0.85,0.7]; // per-face directional shade

fn cube(center:V3, h:f32, base:[u8;3], emissive:f32, cam:&Cam, out:&mut Vec<Face>){
    let c=[[center[0]-h,center[1]-h,center[2]-h],[center[0]+h,center[1]-h,center[2]-h],
           [center[0]-h,center[1]+h,center[2]-h],[center[0]+h,center[1]+h,center[2]-h],
           [center[0]-h,center[1]-h,center[2]+h],[center[0]+h,center[1]-h,center[2]+h],
           [center[0]-h,center[1]+h,center[2]+h],[center[0]+h,center[1]+h,center[2]+h]];
    let mut sp=[(0.0,0.0,0.0);8];
    for i in 0..8 { match cam.proj(c[i]){Some(p)=>sp[i]=p,None=>return} }
    for (fi,f) in FACES.iter().enumerate(){
        let (a,b,cc)=(sp[f[0]],sp[f[1]],sp[f[2]]);
        if (b.0-a.0)*(cc.1-a.1)-(b.1-a.1)*(cc.0-a.0) <= 0.0 { continue; }
        let s=(SHADE[fi]*(1.0-emissive)+emissive).min(1.0);
        let col=[(base[0]as f32*s)as u8,(base[1]as f32*s)as u8,(base[2]as f32*s)as u8];
        let d=(sp[f[0]].2+sp[f[1]].2+sp[f[2]].2+sp[f[3]].2)*0.25;
        out.push(Face{pts:[(sp[f[0]].0,sp[f[0]].1),(sp[f[1]].0,sp[f[1]].1),(sp[f[2]].0,sp[f[2]].1),(sp[f[3]].0,sp[f[3]].1)],depth:d,col});
    }
}
fn tri(buf:&mut[u8],a:(f32,f32),b:(f32,f32),c:(f32,f32),col:[u8;3]){
    let minx=a.0.min(b.0).min(c.0).floor().max(0.0)as i32; let maxx=a.0.max(b.0).max(c.0).ceil().min(W as f32-1.0)as i32;
    let miny=a.1.min(b.1).min(c.1).floor().max(0.0)as i32; let maxy=a.1.max(b.1).max(c.1).ceil().min(H as f32-1.0)as i32;
    let area=(b.0-a.0)*(c.1-a.1)-(b.1-a.1)*(c.0-a.0); if area.abs()<1e-3 {return;}
    for y in miny..=maxy { for x in minx..=maxx { let(px,py)=(x as f32+0.5,y as f32+0.5);
        let w0=((b.0-px)*(c.1-py)-(b.1-py)*(c.0-px))/area; let w1=((c.0-px)*(a.1-py)-(c.1-py)*(a.0-px))/area; let w2=1.0-w0-w1;
        if w0>=-0.01&&w1>=-0.01&&w2>=-0.01 { let i=(y as usize*W+x as usize)*4; buf[i]=col[0];buf[i+1]=col[1];buf[i+2]=col[2];buf[i+3]=255; } } }
}

fn main(){
    let base = BlockId::base_color(TITLE_BLOCK);
    // title with lower-left at world origin; center it in front of the camera.
    let blocks = unendlich_blocks([0,0,0]);
    let cx = 53.0*0.5; let cy = 3.0;
    let mut buf=vec![0u8;W*H*4];
    for (i,px) in buf.chunks_mut(4).enumerate(){ let t=(i/W)as f32/H as f32;
        let (r,g,b)= if t<0.62 {(20.0+40.0*t, 26.0+60.0*t, 54.0+70.0*t)} else {(46.0,60.0,38.0)};
        px[0]=r as u8;px[1]=g as u8;px[2]=b as u8;px[3]=255; }
    // camera: front-on, slightly above + right, looking at the title centre
    let cam=Cam::new([3.0, cy+2.5, 54.0],[0.0, cy, 0.0]);
    let mut faces=Vec::new();
    for b in &blocks {
        let center=[b[0]as f32 - cx, b[1]as f32 + 0.5, b[2]as f32];
        // crystal letters glow; gradient tint across the title for life
        let t=(b[0] as f32)/53.0;
        let tint=[ (base[0]as f32*(0.7+0.3*(1.0-t)))as u8, base[1], (base[2]as f32*(0.8+0.2*t)).min(255.0)as u8 ];
        cube(center, 0.5, tint, 0.45, &cam, &mut faces);
    }
    faces.sort_by(|p,q| q.depth.partial_cmp(&p.depth).unwrap_or(std::cmp::Ordering::Equal));
    for f in &faces { tri(&mut buf,f.pts[0],f.pts[1],f.pts[2],f.col); tri(&mut buf,f.pts[0],f.pts[2],f.pts[3],f.col); }
    write_png("/mnt/user-data/outputs/title_blocks_scene.png",W,H,&buf).expect("png");
    println!("wrote title_blocks_scene.png ({W}x{H}) — UNENDLICH as {} voxel blocks ({:?})", blocks.len(), TITLE_BLOCK);
}

fn write_png(path:&str,w:usize,h:usize,rgba:&[u8])->std::io::Result<()>{use std::io::Write;
    let mut out=vec![0x89,b'P',b'N',b'G',0x0d,0x0a,0x1a,0x0a];
    let mut ih=Vec::new();ih.extend_from_slice(&(w as u32).to_be_bytes());ih.extend_from_slice(&(h as u32).to_be_bytes());ih.extend_from_slice(&[8,6,0,0,0]);
    ch(&mut out,b"IHDR",&ih);
    let mut raw=Vec::with_capacity(h*(1+w*4));for y in 0..h{raw.push(0);raw.extend_from_slice(&rgba[y*w*4..(y+1)*w*4]);}
    ch(&mut out,b"IDAT",&zl(&raw));ch(&mut out,b"IEND",&[]);std::fs::File::create(path)?.write_all(&out)}
fn ch(out:&mut Vec<u8>,k:&[u8;4],d:&[u8]){out.extend_from_slice(&(d.len()as u32).to_be_bytes());out.extend_from_slice(k);out.extend_from_slice(d);
    let mut c=0xffff_ffffu32;for b in k.iter().chain(d.iter()){c^=*b as u32;for _ in 0..8{c=if c&1!=0{(c>>1)^0xedb8_8320}else{c>>1};}}out.extend_from_slice(&(c^0xffff_ffff).to_be_bytes());}
fn zl(d:&[u8])->Vec<u8>{let mut z=vec![0x78,0x01];let mut i=0;while i<d.len(){let n=(d.len()-i).min(0xffff);let f=i+n>=d.len();z.push(if f{1}else{0});z.extend_from_slice(&(n as u16).to_le_bytes());z.extend_from_slice(&(!(n as u16)).to_le_bytes());z.extend_from_slice(&d[i..i+n]);i+=n;}
    let(mut a,mut b)=(1u32,0u32);for &x in d{a=(a+x as u32)%65521;b=(b+a)%65521;}z.extend_from_slice(&((b<<16)|a).to_be_bytes());z}
