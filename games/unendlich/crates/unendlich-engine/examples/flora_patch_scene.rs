//! examples/flora_patch_scene.rs — a populated patch of the real world rendered as
//! actual voxel blocks: terrain surface + the flora that flora_map decides and
//! flora_blocks instantiates (trees = trunk+canopy, ground cover = single blocks).
//! Proves flora becomes real in-world geometry. CPU z-buffer cube raster.
//! Run: cargo run --release --example flora_patch_scene --features gpu

use unendlich_engine::world::blocks::BlockId;
use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::world::flora_blocks::region_flora_blocks;

const W: usize = 860;
const H: usize = 460;
const SEED: u32 = 7;
const X0: i32 = -104;
const Z0: i32 = -104;
const NP: i32 = 44;

type V3 = [f32; 3];
fn sub(a: V3, b: V3) -> V3 { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn dot(a: V3, b: V3) -> f32 { a[0]*b[0]+a[1]*b[1]+a[2]*b[2] }
fn cross(a: V3, b: V3) -> V3 { [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]] }
fn nrm(a: V3) -> V3 { let l=dot(a,a).sqrt().max(1e-6); [a[0]/l,a[1]/l,a[2]/l] }

struct Cam { eye:V3, right:V3, up:V3, fwd:V3, f:f32 }
impl Cam {
    fn new(eye:V3, target:V3)->Cam{ let fwd=nrm(sub(target,eye)); let right=nrm(cross(fwd,[0.0,1.0,0.0])); let up=cross(right,fwd);
        Cam{eye,right,up,fwd,f:(W as f32*0.5)/(30.0f32.to_radians()).tan()} }
    fn proj(&self,p:V3)->Option<(f32,f32,f32)>{ let r=sub(p,self.eye); let c=[dot(r,self.right),dot(r,self.up),dot(r,self.fwd)];
        if c[2]<0.05 {return None;} Some((W as f32*0.5+self.f*c[0]/c[2], H as f32*0.5-self.f*c[1]/c[2], c[2])) }
}
const FACES:[[usize;4];6]=[[0,1,3,2],[4,6,7,5],[0,4,5,1],[2,3,7,6],[1,5,7,3],[0,2,6,4]];
const SHADE:[f32;6]=[0.74,0.96,0.6,1.0,0.86,0.68];

fn cube(buf:&mut[u8], zb:&mut[f32], center:V3, base:[u8;3], cam:&Cam){
    let h=0.5;
    let c=[[center[0]-h,center[1]-h,center[2]-h],[center[0]+h,center[1]-h,center[2]-h],
           [center[0]-h,center[1]+h,center[2]-h],[center[0]+h,center[1]+h,center[2]-h],
           [center[0]-h,center[1]-h,center[2]+h],[center[0]+h,center[1]-h,center[2]+h],
           [center[0]-h,center[1]+h,center[2]+h],[center[0]+h,center[1]+h,center[2]+h]];
    let mut sp=[(0.0,0.0,0.0);8];
    for i in 0..8 { match cam.proj(c[i]){Some(p)=>sp[i]=p,None=>return} }
    for (fi,f) in FACES.iter().enumerate(){
        let (a,b,cc)=(sp[f[0]],sp[f[1]],sp[f[2]]);
        if (b.0-a.0)*(cc.1-a.1)-(b.1-a.1)*(cc.0-a.0) <= 0.0 { continue; }
        let s=SHADE[fi];
        let col=[(base[0]as f32*s)as u8,(base[1]as f32*s)as u8,(base[2]as f32*s)as u8];
        tri(buf,zb,sp[f[0]],sp[f[1]],sp[f[2]],col); tri(buf,zb,sp[f[0]],sp[f[2]],sp[f[3]],col);
    }
}
fn tri(buf:&mut[u8],zb:&mut[f32],a:(f32,f32,f32),b:(f32,f32,f32),c:(f32,f32,f32),col:[u8;3]){
    let minx=a.0.min(b.0).min(c.0).floor().max(0.0)as i32; let maxx=a.0.max(b.0).max(c.0).ceil().min(W as f32-1.0)as i32;
    let miny=a.1.min(b.1).min(c.1).floor().max(0.0)as i32; let maxy=a.1.max(b.1).max(c.1).ceil().min(H as f32-1.0)as i32;
    let area=(b.0-a.0)*(c.1-a.1)-(b.1-a.1)*(c.0-a.0); if area.abs()<1e-4 {return;}
    for y in miny..=maxy{ for x in minx..=maxx{ let(px,py)=(x as f32+0.5,y as f32+0.5);
        let w0=((b.0-px)*(c.1-py)-(b.1-py)*(c.0-px))/area; let w1=((c.0-px)*(a.1-py)-(c.1-py)*(a.0-px))/area; let w2=1.0-w0-w1;
        if w0 < -0.01 || w1 < -0.01 || w2 < -0.01 {continue;}
        let z=w0*a.2+w1*b.2+w2*c.2; let zi=(y as usize)*W+x as usize;
        if z < zb[zi]{ zb[zi]=z; let i=zi*4; buf[i]=col[0];buf[i+1]=col[1];buf[i+2]=col[2];buf[i+3]=255; } } }
}

fn main(){
    let g=WorldGenerator::new(SEED);
    // terrain: top block per column (relative heights, centred)
    let mut heights=vec![0i32; (NP*NP) as usize];
    let mut miny=i32::MAX; let mut maxy=i32::MIN;
    for cz in 0..NP { for cx in 0..NP {
        let sy=g.surface_y_at(X0+cx, Z0+cz, -16, 248).unwrap_or(64);
        heights[(cz*NP+cx) as usize]=sy; miny=miny.min(sy); maxy=maxy.max(sy);
    }}
    let flora=region_flora_blocks(&g, X0, Z0, NP, NP);
    println!("flora_patch_scene seed {SEED}: {} flora blocks over {NP}x{NP}", flora.len());

    // world->scene: centre the patch, drop to miny
    let cx0=X0 as f32 + NP as f32*0.5;
    let cz0=Z0 as f32 + NP as f32*0.5;
    let to_scene=|x:i32,y:i32,z:i32| [x as f32 - cx0, (y-miny) as f32, z as f32 - cz0];

    let mut buf=vec![0u8;W*H*4]; let mut zb=vec![f32::INFINITY;W*H];
    for (i,px) in buf.chunks_mut(4).enumerate(){ let t=(i/W)as f32/H as f32;
        px[0]=(120.0+60.0*t)as u8;px[1]=(150.0+55.0*t)as u8;px[2]=(195.0+40.0*t)as u8;px[3]=255; }

    let relmax=(maxy-miny) as f32;
    let cam=Cam::new([NP as f32*0.58, relmax + NP as f32*0.72, NP as f32*0.58],[0.0, relmax*0.4, 0.0]);

    // ground (grass-toned, shaded a touch by height)
    for cz in 0..NP { for cx in 0..NP {
        let sy=heights[(cz*NP+cx) as usize];
        let k=((sy-miny) as f32/14.0).clamp(0.0,1.0);
        let gcol=[(96.0+30.0*k)as u8,(150.0-26.0*k)as u8,(80.0-18.0*k)as u8];
        cube(&mut buf,&mut zb, to_scene(X0+cx,sy,Z0+cz), gcol, &cam);
    }}
    // flora blocks
    for (p,b) in &flora {
        cube(&mut buf,&mut zb, to_scene(p[0],p[1],p[2]), BlockId::base_color(*b), &cam);
    }

    write_png("/mnt/user-data/outputs/flora_patch_scene.png",W,H,&buf).expect("png");
    println!("wrote flora_patch_scene.png ({W}x{H}) — terrain + flora as real blocks");
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
