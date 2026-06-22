//! examples/forge_grime_collage.rs — proves creature_grime is WIRED into the forge
//! dragon's material. Renders the SAME forge dragon (forge_to_vertices) twice: LEFT
//! freshly clean, RIGHT after wallowing in mud — the belly weathers, the back stays
//! mostly clean (belly_weight), eyes/mouth glow untouched. CPU z-buffer raster.
//! Run: cargo run --release --example forge_grime_collage --features gpu

use unendlich_engine::sim::creature_grime::GrimeSample;
use unendlich_engine::sim::dragon_archetype::Genome;
use unendlich_engine::world::entity::{forge_to_vertices, EntityVertex};

const CW: usize = 460;
const H: usize = 380;
const GAP: usize = 8;
const W: usize = CW * 2 + GAP;

type V3 = [f32; 3];
fn sub(a: V3, b: V3) -> V3 { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn dot(a: V3, b: V3) -> f32 { a[0]*b[0]+a[1]*b[1]+a[2]*b[2] }
fn cross(a: V3, b: V3) -> V3 { [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]] }
fn norm(a: V3) -> V3 { let l=dot(a,a).sqrt().max(1e-6); [a[0]/l,a[1]/l,a[2]/l] }

struct Cam { eye:V3, right:V3, up:V3, fwd:V3, f:f32 }
impl Cam {
    fn new(eye:V3, target:V3)->Cam{ let fwd=norm(sub(target,eye)); let right=norm(cross(fwd,[0.0,1.0,0.0])); let up=cross(right,fwd);
        Cam{eye,right,up,fwd,f:(CW as f32*0.5)/(26.0f32.to_radians()).tan()} }
    fn proj(&self,p:V3)->Option<(f32,f32,f32)>{ let r=sub(p,self.eye); let c=[dot(r,self.right),dot(r,self.up),dot(r,self.fwd)];
        if c[2]<0.05 {return None;} Some((CW as f32*0.5+self.f*c[0]/c[2], H as f32*0.5-self.f*c[1]/c[2], c[2])) }
}

fn raster(buf:&mut[u8], zb:&mut[f32], xoff:usize, verts:&[EntityVertex], idx:&[u32], cam:&Cam){
    let light=norm([0.4,0.8,0.5]);
    for t in idx.chunks(3){
        let a=verts[t[0]as usize].position; let b=verts[t[1]as usize].position; let c=verts[t[2]as usize].position;
        let n=norm(cross(sub(b,a),sub(c,a)));
        let sh=(0.45+0.55*dot(n,light).max(0.0)).min(1.0);
        let base=verts[t[0]as usize].color;
        let col=[(base[0]*sh*255.0).min(255.0)as u8,(base[1]*sh*255.0).min(255.0)as u8,(base[2]*sh*255.0).min(255.0)as u8];
        let (pa,pb,pc)=match(cam.proj(a),cam.proj(b),cam.proj(c)){(Some(x),Some(y),Some(z))=>(x,y,z),_=>continue};
        fill(buf,zb,xoff,pa,pb,pc,col);
    }
}
fn fill(buf:&mut[u8],zb:&mut[f32],xoff:usize,a:(f32,f32,f32),b:(f32,f32,f32),c:(f32,f32,f32),col:[u8;3]){
    let minx=a.0.min(b.0).min(c.0).floor().max(0.0)as i32; let maxx=a.0.max(b.0).max(c.0).ceil().min(CW as f32-1.0)as i32;
    let miny=a.1.min(b.1).min(c.1).floor().max(0.0)as i32; let maxy=a.1.max(b.1).max(c.1).ceil().min(H as f32-1.0)as i32;
    let area=(b.0-a.0)*(c.1-a.1)-(b.1-a.1)*(c.0-a.0); if area.abs()<1e-4 {return;}
    for y in miny..=maxy{ for x in minx..=maxx{ let(px,py)=(x as f32+0.5,y as f32+0.5);
        let w0=((b.0-px)*(c.1-py)-(b.1-py)*(c.0-px))/area; let w1=((c.0-px)*(a.1-py)-(c.1-py)*(a.0-px))/area; let w2=1.0-w0-w1;
        if w0 < -0.01 || w1 < -0.01 || w2 < -0.01 {continue;}
        let z=w0*a.2+w1*b.2+w2*c.2; let zi=(y as usize)*W+(xoff+x as usize);
        if z < zb[zi]{ zb[zi]=z; let i=zi*4; buf[i]=col[0];buf[i+1]=col[1];buf[i+2]=col[2];buf[i+3]=255; } } }
}

fn main(){
    let g=Genome::from_seed(7);
    // clean dragon
    let mut clean=unendlich_engine::spawn::spawn_dragon_with_genome([0.0,0.0,0.0],&g);
    clean.yaw=0.6; // turn so the flank/belly faces the camera
    // muddy dragon — same genome/pose, wallowed in mud for a few seconds
    let mut muddy=clean.clone();
    muddy.grime.accumulate(&GrimeSample{soil_grime01:1.0,wetness01:0.0,activity01:1.0,soil_tint:[0.34,0.24,0.15],dt:6.0});
    let model=clean.forge.clone().expect("forge dragon");

    let (vc,ic)=forge_to_vertices(&clean,&model);
    let (vm,im)=forge_to_vertices(&muddy,&model);

    // frame the dragon: centre on its bounds, view from front-side + a touch low
    let (mut lo,mut hi)=([1e9f32;3],[-1e9f32;3]);
    for v in &vc { for k in 0..3 { lo[k]=lo[k].min(v.position[k]); hi[k]=hi[k].max(v.position[k]); } }
    let ctr=[(lo[0]+hi[0])*0.5,(lo[1]+hi[1])*0.5,(lo[2]+hi[2])*0.5];
    let span=(hi[0]-lo[0]).max(hi[1]-lo[1]).max(hi[2]-lo[2]);
    let cam=Cam::new([ctr[0]+span*0.5, ctr[1]-span*0.18, ctr[2]+span*1.5],[ctr[0],ctr[1],ctr[2]]);

    let mut buf=vec![0u8;W*H*4]; let mut zb=vec![f32::INFINITY;W*H];
    for (i,px) in buf.chunks_mut(4).enumerate(){ let t=(i/W)as f32/H as f32;
        px[0]=(28.0+22.0*t)as u8;px[1]=(32.0+26.0*t)as u8;px[2]=(40.0+30.0*t)as u8;px[3]=255; }
    raster(&mut buf,&mut zb,0,&vc,&ic,&cam);
    raster(&mut buf,&mut zb,CW+GAP,&vm,&im,&cam);

    write_png("/mnt/user-data/outputs/forge_grime_collage.png",W,H,&buf).expect("png");
    println!("forge_grime_collage: clean grime01={:.2} muddy grime01={:.2}", clean.grime.grime_factor01(), muddy.grime.grime_factor01());
    println!("wrote forge_grime_collage.png ({W}x{H}) — LEFT clean · RIGHT mud-weathered belly");
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
