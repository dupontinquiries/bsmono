
struct D { l:u32,hd:u32,nh:u32,nkv:u32,chunk:u32,nch:u32,coff:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kq: array<u32>;
@group(0)@binding(2) var<storage,read> ks: array<f32>;
@group(0)@binding(3) var<storage,read> vq: array<u32>;
@group(0)@binding(4) var<storage,read> vs: array<f32>;
@group(0)@binding(5) var<storage,read_write> part: array<f32>;
@group(0)@binding(6) var<uniform> dd: D;
var<workgroup> sc: array<f32,2048>;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let hh=wg.x; let ch=wg.y; let tid=lid.x;
  let kvdm=dd.nkv*dd.hd; let kvw=kvdm/4u;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;
  let scale=1.0/sqrt(f32(dd.hd));
  let j0=ch*dd.chunk; let j1=min(dd.l, j0+dd.chunk);
  // this span's chunks are placed AFTER the hot span's chunks (coff), so pass-2 sees one chunk list
  let pbase=(hh*dd.nch+(dd.coff+ch))*(dd.hd+2u);
  var lmax=-3.4e38; var j=j0+tid;
  loop { if(j>=j1){break;}
    let ksc=ks[j]; var dot=0.0;
    for(var c=0u;c<dd.hd;c=c+1u){
      let idx=kb+c; let word=kq[j*kvw + idx/4u];
      let ub=extractBits(word,(idx%4u)*8u,8u); let qi=select(i32(ub), i32(ub)-256, ub>=128u);
      dot=dot+q[b+c]*(f32(qi)*ksc);
    }
    let s=dot*scale; sc[j-j0]=s; lmax=max(lmax,s); j=j+64u; }
  red[tid]=lmax; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=max(red[tid],red[tid+st]); } workgroupBarrier(); }
  let mx=red[0]; workgroupBarrier();
  var lsum=0.0; j=j0+tid;
  loop { if(j>=j1){break;} let p=exp(sc[j-j0]-mx); sc[j-j0]=p; lsum=lsum+p; j=j+64u; }
  red[tid]=lsum; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=red[tid]+red[tid+st]; } workgroupBarrier(); }
  let sum=red[0]; workgroupBarrier();
  var c=tid;
  loop { if(c>=dd.hd){break;}
    var acc=0.0;
    for(var jj=j0;jj<j1;jj=jj+1u){
      let idx=kb+c; let word=vq[jj*kvw + idx/4u];
      let ub=extractBits(word,(idx%4u)*8u,8u); let qi=select(i32(ub), i32(ub)-256, ub>=128u);
      acc=acc+sc[jj-j0]*(f32(qi)*vs[jj]);
    }
    part[pbase+c]=acc; c=c+64u; }
  if(tid==0u){ part[pbase+dd.hd]=mx; part[pbase+dd.hd+1u]=sum; }
}