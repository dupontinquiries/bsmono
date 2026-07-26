
struct D { l:u32,hd:u32,nh:u32,nkv:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kc: array<f32>;
@group(0)@binding(2) var<storage,read> vc: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
var<workgroup> sc: array<f32,4096>;
var<workgroup> red: array<f32,64>;
// One workgroup per query head (so the GPU's many cores all work, vs the serial kernel's
// single workgroup). 64 threads split the KV rows for QK + softmax (shared-memory reduction),
// then split head_dim for the V-weighted sum (coalesced). Caller uses this only when l<=4096.
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let hh=wg.x; let tid=lid.x;
  let kvdm=dd.nkv*dd.hd;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;  // GQA: query head -> shared KV head
  let scale=1.0/sqrt(f32(dd.hd));
  var lmax=-3.4e38; var j=tid;
  loop { if(j>=dd.l){break;} var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[b+c]*kc[j*kvdm+kb+c];} let s=dot*scale; sc[j]=s; lmax=max(lmax,s); j=j+64u; }
  red[tid]=lmax; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=max(red[tid],red[tid+st]); } workgroupBarrier(); }
  let mx=red[0]; workgroupBarrier();
  var lsum=0.0; j=tid;
  loop { if(j>=dd.l){break;} let p=exp(sc[j]-mx); sc[j]=p; lsum=lsum+p; j=j+64u; }
  red[tid]=lsum; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=red[tid]+red[tid+st]; } workgroupBarrier(); }
  let sum=red[0]; workgroupBarrier();
  var c=tid;
  loop { if(c>=dd.hd){break;} var acc=0.0; for(var jj=0u;jj<dd.l;jj=jj+1u){ acc=acc+sc[jj]*vc[jj*kvdm+kb+c]; } o[b+c]=acc/sum; c=c+64u; }
}