// SwiGLU fused into the q4 down-projection matvec + residual — q4 twin of the
// q8_0 kernel above; same one-dispatch replacement of activation + matvec + add.
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32 };
@group(0)@binding(0) var<storage,read> gu: array<f32>;   // packed [gate: k | up: k]
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> resid: array<f32>;
@group(0)@binding(4) var<storage,read_write> c: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
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
    let nib=extractBits(word,(kk%8u)*4u,4u);
    let q=i32(nib)-8;
    let g=gu[kk];
    let act=(g/(1.0+exp(-g)))*gu[d.k+kk];   // silu(gate)*up, computed inline in f32 —
                                            // no intermediate h buffer, no extra pass
    acc=acc+act*scale*f32(q);
    kk=kk+64u;
  }
  red[tid]=acc; workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ red[tid]=red[tid]+red[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ c[col]=red[0]+resid[col]; }
}
