
struct D { n:u32,k:u32,kb:u32,gx:u32,_slot:u32,rstride:u32,xper:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> selw: array<u32>;
@group(0)@binding(4) var<storage,read_write> c: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x; let slot=wg.z;
  if(col>=d.n){ return; }
  let row=selw[slot]*d.rstride+col;
  let wbase=row*(d.k/8u); let sbase=row*d.kb;
  let abase=select(0u, slot*d.k, d.xper==1u);
  var acc=0.0; var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/8u)];
    let nib=extractBits(word,(kk%8u)*4u,4u);
    let q=i32(nib)-8;
    acc=acc+a[abase+kk]*scale*f32(q);
    kk=kk+64u;
  }
  red[tid]=acc; workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ red[tid]=red[tid]+red[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ c[slot*d.n+col]=red[0]; }
}