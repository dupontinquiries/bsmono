
struct D { d:u32,k:u32,kb:u32,gx:u32,slots:u32,_r:u32,_x:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> hmid: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> selw: array<u32>;
@group(0)@binding(4) var<storage,read> resid: array<f32>;
@group(0)@binding(5) var<storage,read_write> o: array<f32>;
@group(0)@binding(6) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  if(col>=d.d){ return; }
  var total=resid[col];   // match MOE_WSUM order: acc=resid then += weighted terms
  for(var s=0u;s<d.slots;s=s+1u){
    let expert=selw[s];
    let w=bitcast<f32>(selw[d.slots+s]);
    let row=expert*d.d+col;
    let wbase=row*(d.k/4u); let sbase=row*d.kb; let abase=s*d.k;
    var acc=0.0; var kk=tid;
    loop { if(kk>=d.k){break;}
      let scale=qs[sbase+(kk/32u)];
      let word=qw[wbase+(kk/4u)];
      let ub=extractBits(word,(kk%4u)*8u,8u);
      let q=select(i32(ub), i32(ub)-256, ub>=128u);
      acc=acc+hmid[abase+kk]*scale*f32(q);
      kk=kk+64u;
    }
    red[tid]=acc; workgroupBarrier();
    for(var st=32u;st>0u;st=st>>1u){ if(tid<st){ red[tid]=red[tid]+red[tid+st]; } workgroupBarrier(); }
    if(tid==0u){ total=total+w*red[0]; }
    workgroupBarrier();
  }
  if(tid==0u){ o[col]=total; }
}