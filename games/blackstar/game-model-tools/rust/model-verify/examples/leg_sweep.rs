
use model_verify::fold::*;
use std::fs;
fn load(p:&str)->Vec<[f64;3]>{let b=fs::read(p).unwrap_or_default();
 if b.len()<64||&b[..15]!=b"INTERQUAKEMODEL"{return vec![]}
 let g=|o:usize| u32::from_le_bytes(b[o..o+4].try_into().unwrap()) as usize;
 let (nva,nv,ofs)=(g(44),g(48),g(52));
 for i in 0..nva{let e=ofs+i*20;let(ty,fmt,size,off)=(g(e),g(e+8),g(e+12),g(e+16));
  if ty==0&&fmt==7{return (0..nv).map(|k|{let o=off+k*size*4;
   let f=|j:usize| f32::from_le_bytes(b[o+j*4..o+j*4+4].try_into().unwrap()) as f64;
   [f(0),f(1),f(2)]}).collect()}}
 vec![]}
fn main(){
 let pts=load(&std::env::args().nth(1).unwrap());
 let vol=Volume::from_points(&pts,12);
 let s=vol.span.iter().cloned().fold(0.0f64,f64::max)*0.42;
 let mid=[vol.lo[0]+vol.span[0]*0.5,vol.lo[1]+vol.span[1]*0.5,vol.lo[2]+vol.span[2]*0.5];
 let bn=|a:[f64;3],b:[f64;3],r:f64,p:i32| Bone{a:[mid[0]+a[0]*s,mid[1]+a[1]*s,mid[2]+a[2]*s],
   b:[mid[0]+b[0]*s,mid[1]+b[1]*s,mid[2]+b[2]*s],r:r*s,parent:p};
 let base=vec![bn([0.0,0.0,0.0],[0.0,1.0,0.0],0.22,-1),bn([0.0,1.0,0.0],[0.0,1.35,0.0],0.16,0),
   bn([0.0,0.85,0.0],[-0.75,0.4,0.0],0.12,0),bn([0.0,0.85,0.0],[0.75,0.4,0.0],0.12,0),
   bn([0.0,0.0,0.0],[-0.25,-1.15,0.0],0.15,0),bn([0.0,0.0,0.0],[0.25,-1.15,0.0],0.15,0)];
 println!("vehicle span: {:.2} {:.2} {:.2}", vol.span[0],vol.span[1],vol.span[2]);
 for deg in [0,30,60,90,120,150] {
   let a=(deg as f64).to_radians();
   let mut b=base.clone();
   bend(&mut b,4,[1.0,0.0,0.0],a); bend(&mut b,5,[1.0,0.0,0.0],a);
   let f=evaluate(&b,&vol,5);
   println!("legs {deg:3}deg: spill {:.3} vacancy {:.3} error {:.3} imbalance {:.3}",
     f.spill,f.vacancy,f.error(),imbalance(&b,&vol));
 }}
