
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32,tile:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,512>; // 8 columns x 64 threads
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col0=(wg.y*d.gx+wg.x)*d.tile; let tid=lid.x;
  if(col0>=d.n){ return; }
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let av=a[kk];                    // ONE activation read, reused across the tile
    let blk=kk/32u; let widx=kk/4u; let bytepos=(kk%4u)*8u;
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      let col=col0+ci;
      if(col<d.n){
        let scale=qs[col*d.kb+blk];
        let word=qw[col*(d.k/4u)+widx];
        let ub=extractBits(word,bytepos,8u);
        let q=select(i32(ub), i32(ub)-256, ub>=128u);
        acc[ci]=acc[ci]+av*scale*f32(q);
      }
    }
    kk=kk+64u;
  }
  for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=acc[ci]; }
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){
    if(tid<s){ for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=red[ci*64u+tid]+red[ci*64u+tid+s]; } }
    workgroupBarrier();
  }
  if(tid==0u){ for(var ci=0u;ci<d.tile;ci=ci+1u){ let col=col0+ci; if(col<d.n){ c[col]=red[ci*64u]; } } }
}