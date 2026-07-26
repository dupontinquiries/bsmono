
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;   // [m,k]
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>; // [m,n]
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,512>; // up to 8 rows x 64 threads
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  if(col>=d.n){ return; }
  let m=d.m;
  let wbase=col*(d.k/4u); let sbase=col*d.kb;
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/4u)];
    let ub=extractBits(word,(kk%4u)*8u,8u);
    let q=f32(select(i32(ub), i32(ub)-256, ub>=128u))*scale; // weight read once
    for(var r=0u;r<m;r=r+1u){ acc[r]=acc[r]+a[r*d.k+kk]*q; }
    kk=kk+64u;
  }
  for(var r=0u;r<m;r=r+1u){ red[r*64u+tid]=acc[r]; }
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){
    if(tid<s){ for(var r=0u;r<m;r=r+1u){ red[r*64u+tid]=red[r*64u+tid]+red[r*64u+tid+s]; } }
    workgroupBarrier();
  }
  if(tid==0u){ for(var r=0u;r<m;r=r+1u){ c[r*d.n+col]=red[r*64u]; } }
}