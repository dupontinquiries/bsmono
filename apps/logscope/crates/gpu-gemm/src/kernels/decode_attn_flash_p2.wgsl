
struct D { hd:u32,nch:u32,nh:u32,_a:u32 };
@group(0)@binding(0) var<storage,read> part: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> dd: D;
var<workgroup> wm: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let hh=wg.x; let tid=lid.x;
  var m=-3.4e38; var i=tid;
  loop { if(i>=dd.nch){break;} m=max(m, part[(hh*dd.nch+i)*(dd.hd+2u)+dd.hd]); i=i+64u; }
  wm[tid]=m; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ wm[tid]=max(wm[tid],wm[tid+st]); } workgroupBarrier(); }
  let M=wm[0]; workgroupBarrier();
  var s=0.0; i=tid;
  loop { if(i>=dd.nch){break;} let pb=(hh*dd.nch+i)*(dd.hd+2u); s=s+part[pb+dd.hd+1u]*exp(part[pb+dd.hd]-M); i=i+64u; }
  wm[tid]=s; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ wm[tid]=wm[tid]+wm[tid+st]; } workgroupBarrier(); }
  let S=wm[0];
  var c=tid;
  loop { if(c>=dd.hd){break;}
    var acc=0.0;
    for(var ii=0u;ii<dd.nch;ii=ii+1u){ let pb=(hh*dd.nch+ii)*(dd.hd+2u); acc=acc+part[pb+c]*exp(part[pb+dd.hd]-M); }
    o[hh*dd.hd+c]=acc/S; c=c+64u; }
}