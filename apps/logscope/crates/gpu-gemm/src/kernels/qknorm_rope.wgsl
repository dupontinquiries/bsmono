// QKNORM_ROPE
//
// ENVELOPE: head_dim <= 512 (the `nx` scratch), any even rot <= hd. It was written for Qwen3 —
// head_dim 128, full rotation — with `nx: array<f32,128>` and ONE rotation pair per thread, so 64
// threads covered exactly 64 pairs. Qwen3.5 / Ornith / Qwen3.8 attention layers are head_dim 256
// with 64 rotary dims and reach this kernel on the DEFAULT decode path: pairs >= 64 were never
// written (exact zeros in the K cache), lanes >= 128 of the normed head were lost to the scratch
// bound, and the frequency exponent divided by hd where the reference divides by the rotary span.
// gate-qknorm-envelope measured the result as logits off by more than the logit scale. Threads now
// LOOP (tid, tid+64, ...) over pairs and over pass-through lanes, and `rot` is a parameter.
// For rot == hd <= 128 every expression below is the old one with rot substituted for hd, the
// loops run once, and the pass-through loop is empty — bit-identical on the same driver.
struct D { nq:u32,nk:u32,hd:u32,base:f32,pos:u32,neox:u32,eps:f32,msc:f32,rot:u32,_p1:u32,_p2:u32,_p3:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> qn: array<f32>;
@group(0)@binding(2) var<storage,read> kn: array<f32>;
@group(0)@binding(3) var<storage,read> ff: array<f32>;
@group(0)@binding(4) var<storage,read_write> o: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
var<workgroup> nx: array<f32,512>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let head=wg.x; let tid=lid.x; let hd=d.hd; let rhalf=d.rot/2u;
  let off=head*hd; // heads are contiguous: q heads [0,nq), then k heads
  // per-head sum of squares (each thread covers hd/64 elems)
  var ss=0.0; var i=tid;
  loop { if(i>=hd){break;} let v=x[off+i]; ss=ss+v*v; i=i+64u; }
  red[tid]=ss; workgroupBarrier();
  for(var s2=32u;s2>0u;s2=s2>>1u){ if(tid<s2){ red[tid]=red[tid]+red[tid+s2]; } workgroupBarrier(); }
  let inv=inverseSqrt(red[0]/f32(hd)+d.eps);
  // normalize with the head-family weight into workgroup memory
  i=tid;
  loop { if(i>=hd){break;}
    let w=select(kn[i], qn[i], head<d.nq);
    nx[i]=x[off+i]*inv*w; i=i+64u; }
  workgroupBarrier();
  // Rotation pairs tid, tid+64, ... < rot/2 (branch-free NORM/NEOX pairing, ROPE_MH's arithmetic:
  // the exponent runs over the ROTARY span and the NEOX half-split is rot's, not the head's).
  var p=tid;
  loop { if(p>=rhalf){break;}
    let freq=f32(d.pos)/(pow(d.base,(2.0*f32(p))/f32(d.rot))*ff[p]);
    let c=cos(freq)*d.msc; let s=sin(freq)*d.msc;
    let j0=p*(2u-d.neox);
    let j1=j0 + 1u + d.neox*(rhalf-1u);
    let x0=nx[j0]; let x1=nx[j1];
    o[off+j0]=x0*c-x1*s;
    o[off+j1]=x0*s+x1*c;
    p=p+64u; }
  // Pass-through lanes [rot, hd): NORMED, unrotated, and unscaled by msc — ROPE_MH's rule. Without
  // this loop those lanes of `o` are whatever the pooled buffer last held.
  var l=d.rot+tid;
  loop { if(l>=hd){break;} o[off+l]=nx[l]; l=l+64u; }
}
