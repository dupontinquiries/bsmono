// QKNORM_ROPE_KV_APPEND — QKNORM_ROPE for one decode token with the cache rows as destinations:
// q heads normed+roped -> o, k heads normed+roped -> kc[pos], v heads copied -> vc[pos].
//
// Replaces QKNORM_ROPE plus three buffer copies per layer (slice q, k->cache, v->cache) on the
// Qwen3-family decode path. The norm and the rotation are QKNORM_ROPE's line for line, so the
// values are bit-identical; only the destinations change. Workgroups: nq (q heads) + nk (k
// heads) + nk (v heads, plain copy — v is neither normed nor roped).
//
// ENVELOPE (same as QKNORM_ROPE, and kept in step with it): head_dim <= 512, any even rot <= hd.
// Threads loop over pairs and pass-through lanes; see QKNORM_ROPE's header for what the fixed
// one-pair-per-thread / 128-lane form did to head_dim 256 + partial rotary (Qwen3.5 / Ornith).
struct D { nq:u32,nk:u32,hd:u32,base:f32,pos:u32,neox:u32,eps:f32,msc:f32,kv:u32,rot:u32,_p2:u32,_p3:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;          // qkv [1, nq*hd + 2*nk*hd]
@group(0)@binding(1) var<storage,read> qn: array<f32>;         // q-norm weight [hd]
@group(0)@binding(2) var<storage,read> kn: array<f32>;         // k-norm weight [hd]
@group(0)@binding(3) var<storage,read> ff: array<f32>;         // freq factors [hd/2]
@group(0)@binding(4) var<storage,read_write> kc: array<f32>;   // K cache [cap, kv]
@group(0)@binding(5) var<storage,read_write> vc: array<f32>;   // V cache [cap, kv]
@group(0)@binding(6) var<storage,read_write> o: array<f32>;    // normed+roped q [1, nq*hd]
@group(0)@binding(7) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
var<workgroup> nx: array<f32,512>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let head=wg.x; let tid=lid.x; let hd=d.hd; let rhalf=d.rot/2u;
  let off=head*hd; // heads are contiguous: q heads [0,nq), then k heads, then v heads
  if (head >= d.nq + d.nk) {
    // V head: copy into this token's V-cache row.
    let vh = head - d.nq - d.nk;
    var i=tid;
    loop { if(i>=hd){break;} vc[d.pos*d.kv + vh*hd + i] = x[off+i]; i=i+64u; }
    return;
  }
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
  // Rotation pairs tid, tid+64, ... < rot/2 (QKNORM_ROPE's loop, destinations folded in).
  let kb=d.pos*d.kv + (head-d.nq)*hd;   // only read when head >= nq
  var p=tid;
  loop { if(p>=rhalf){break;}
    let freq=f32(d.pos)/(pow(d.base,(2.0*f32(p))/f32(d.rot))*ff[p]);
    let c=cos(freq)*d.msc; let s=sin(freq)*d.msc;
    let j0=p*(2u-d.neox);
    let j1=j0 + 1u + d.neox*(rhalf-1u);
    let x0=nx[j0]; let x1=nx[j1];
    let r0=x0*c-x1*s; let r1=x0*s+x1*c;
    if (head < d.nq) { o[off+j0]=r0; o[off+j1]=r1; }
    else { kc[kb+j0]=r0; kc[kb+j1]=r1; }
    p=p+64u; }
  // Pass-through lanes [rot, hd): normed, unrotated, unscaled.
  var l=d.rot+tid;
  loop { if(l>=hd){break;}
    if (head < d.nq) { o[off+l]=nx[l]; } else { kc[kb+l]=nx[l]; }
    l=l+64u; }
}
