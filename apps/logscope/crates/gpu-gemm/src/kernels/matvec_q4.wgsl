
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;  // [n,k] nibbles, 8 packed per u32
@group(0)@binding(2) var<storage,read> qs: array<f32>;  // [n,kb] per-block scales
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  if(col>=d.n){ return; }
  let wbase=col*(d.k/8u); let sbase=col*d.kb;
  var acc=0.0; var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/8u)];
    let nib=extractBits(word,(kk%8u)*4u,4u);   // 0..15 (AGX-safe dynamic offset)
    let q=i32(nib)-8;                          // symmetric: value−8 ∈ [−7,7]
    acc=acc+a[kk]*scale*f32(q);
    kk=kk+64u;
  }
  red[tid]=acc; workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ red[tid]=red[tid]+red[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ c[col]=red[0]; }
}