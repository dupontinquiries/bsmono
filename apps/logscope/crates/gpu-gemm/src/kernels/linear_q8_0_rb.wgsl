
const BM:u32=64u; const BN:u32=64u; const BK:u32=16u;
struct D { m:u32,n:u32,k:u32,kb:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> As: array<f32,1024>; // BM*BK
var<workgroup> Ws: array<f32,1024>; // BK*BN
@compute @workgroup_size(16,16,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let brow=wg.y*BM; let bcol=wg.x*BN;
  let tx=lid.x; let ty=lid.y; let tid=ty*16u+tx;
  // 4×4 register tile as four vec4 rows — NAMED (constant-indexed) so they stay in registers, not spilled
  // local memory (a dynamically-indexed array<f32,16> would spill and kill the whole point of blocking).
  var acc0=vec4<f32>(0.0,0.0,0.0,0.0); var acc1=vec4<f32>(0.0,0.0,0.0,0.0);
  var acc2=vec4<f32>(0.0,0.0,0.0,0.0); var acc3=vec4<f32>(0.0,0.0,0.0,0.0);
  let ntiles=(d.k+BK-1u)/BK;
  for(var t=0u;t<ntiles;t=t+1u){
    let kbase=t*BK;
    for(var i=tid;i<1024u;i=i+256u){
      let r=i/BK; let kk=i%BK; let gk=kbase+kk;
      As[i]=select(0.0, a[(brow+r)*d.k+gk], (brow+r)<d.m && gk<d.k);
    }
    for(var i=tid;i<1024u;i=i+256u){
      let kk=i/BN; let cc=i%BN; let gk=kbase+kk; let gc=bcol+cc;
      var wv=0.0;
      if(gc<d.n && gk<d.k){
        let scale=qs[gc*d.kb+(gk/32u)];
        let word=qw[gc*(d.k/4u)+(gk/4u)];
        let ub=extractBits(word,(gk%4u)*8u,8u);
        let q=select(i32(ub), i32(ub)-256, ub>=128u);
        wv=scale*f32(q);
      }
      Ws[i]=wv;
    }
    workgroupBarrier();
    for(var kk=0u;kk<BK;kk=kk+1u){
      let wb=kk*BN+tx*4u;
      let br=vec4<f32>(Ws[wb],Ws[wb+1u],Ws[wb+2u],Ws[wb+3u]);
      let ab=ty*4u;
      let a0=As[(ab+0u)*BK+kk]; let a1=As[(ab+1u)*BK+kk]; let a2=As[(ab+2u)*BK+kk]; let a3=As[(ab+3u)*BK+kk];
      acc0=acc0+a0*br; acc1=acc1+a1*br; acc2=acc2+a2*br; acc3=acc3+a3*br;
    }
    workgroupBarrier();
  }
  let r0=brow+ty*4u; let c0=bcol+tx*4u;
  if(r0<d.m){ if(c0<d.n){c[r0*d.n+c0]=acc0.x;} if(c0+1u<d.n){c[r0*d.n+c0+1u]=acc0.y;} if(c0+2u<d.n){c[r0*d.n+c0+2u]=acc0.z;} if(c0+3u<d.n){c[r0*d.n+c0+3u]=acc0.w;} }
  if(r0+1u<d.m){ let rr=r0+1u; if(c0<d.n){c[rr*d.n+c0]=acc1.x;} if(c0+1u<d.n){c[rr*d.n+c0+1u]=acc1.y;} if(c0+2u<d.n){c[rr*d.n+c0+2u]=acc1.z;} if(c0+3u<d.n){c[rr*d.n+c0+3u]=acc1.w;} }
  if(r0+2u<d.m){ let rr=r0+2u; if(c0<d.n){c[rr*d.n+c0]=acc2.x;} if(c0+1u<d.n){c[rr*d.n+c0+1u]=acc2.y;} if(c0+2u<d.n){c[rr*d.n+c0+2u]=acc2.z;} if(c0+3u<d.n){c[rr*d.n+c0+3u]=acc2.w;} }
  if(r0+3u<d.m){ let rr=r0+3u; if(c0<d.n){c[rr*d.n+c0]=acc3.x;} if(c0+1u<d.n){c[rr*d.n+c0+1u]=acc3.y;} if(c0+2u<d.n){c[rr*d.n+c0+2u]=acc3.z;} if(c0+3u<d.n){c[rr*d.n+c0+3u]=acc3.w;} }
}