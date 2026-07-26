
struct D { n:u32,k:u32,kb:u32,gx:u32,_slot:u32,rstride:u32,xper:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> selw: array<u32>;
@group(0)@binding(4) var<storage,read_write> o: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
var<workgroup> rg: array<f32,64>;
var<workgroup> ru: array<f32,64>;
fn dotq8(row:u32, tid:u32, k:u32, kb:u32, abase:u32) -> f32 {
  let wbase=row*(k/4u); let sbase=row*kb;
  var acc=0.0; var kk=tid;
  loop { if(kk>=k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/4u)];
    let ub=extractBits(word,(kk%4u)*8u,8u);
    let q=select(i32(ub), i32(ub)-256, ub>=128u);
    acc=acc+a[abase+kk]*scale*f32(q);
    kk=kk+64u;
  }
  return acc;
}
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x; let slot=wg.z;
  if(col>=d.n){ return; }
  let abase=select(0u, slot*d.k, d.xper==1u);
  let expert=selw[slot];
  let grow=expert*d.rstride+col;         // gate row: column j
  let urow=expert*d.rstride+d.n+col;     // up row:   column ff+j
  rg[tid]=dotq8(grow,tid,d.k,d.kb,abase);
  ru[tid]=dotq8(urow,tid,d.k,d.kb,abase);
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ rg[tid]=rg[tid]+rg[tid+s]; ru[tid]=ru[tid]+ru[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ let g=rg[0]; o[slot*d.n+col]=(g/(1.0+exp(-g)))*ru[0]; }
}