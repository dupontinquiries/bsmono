
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32,tile:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,512>; // tile(≤8) x 64 threads
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col0=(wg.y*d.gx+wg.x)*d.tile; let tid=lid.x;
  if(col0>=d.n){ return; }
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  let words=d.k/4u;             // 32-bit words per column-row (4 q8 values each)
  var w=tid;                    // each thread owns whole words → one global read per word, no redundancy
  loop {
    if(w>=words){break;}
    let kk0=w*4u;
    let a0=a[kk0]; let a1=a[kk0+1u]; let a2=a[kk0+2u]; let a3=a[kk0+3u];
    let b0=kk0/32u; let b1=(kk0+1u)/32u; let b2=(kk0+2u)/32u; let b3=(kk0+3u)/32u; // block index per value
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      let col=col0+ci;
      if(col<d.n){
        let word=qw[col*words+w];         // ONE read, all four bytes consumed
        let u0=extractBits(word,0u,8u);  let u1=extractBits(word,8u,8u);
        let u2=extractBits(word,16u,8u); let u3=extractBits(word,24u,8u);
        let q0=select(i32(u0),i32(u0)-256,u0>=128u);
        let q1=select(i32(u1),i32(u1)-256,u1>=128u);
        let q2=select(i32(u2),i32(u2)-256,u2>=128u);
        let q3=select(i32(u3),i32(u3)-256,u3>=128u);
        let base=col*d.kb;
        acc[ci]=acc[ci]
          + a0*qs[base+b0]*f32(q0)
          + a1*qs[base+b1]*f32(q1)
          + a2*qs[base+b2]*f32(q2)
          + a3*qs[base+b3]*f32(q3);
      }
    }
    w=w+64u;
  }
  for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=acc[ci]; }
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){
    if(tid<s){ for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=red[ci*64u+tid]+red[ci*64u+tid+s]; } }
    workgroupBarrier();
  }
  if(tid==0u){ for(var ci=0u;ci<d.tile;ci=ci+1u){ let col=col0+ci; if(col<d.n){ c[col]=red[ci*64u]; } } }
}