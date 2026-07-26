
struct D { s:u32,hd:u32,nh:u32,nkv:u32,base:u32,nswa:u32,swaty:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kc: array<f32>;
@group(0)@binding(2) var<storage,read> vc: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let id=g.x; if(id>=dd.s*dd.nh){return;}
  let i=id/dd.nh; let hh=id%dd.nh;
  let dm=dd.nh*dd.hd; let kvdm=dd.nkv*dd.hd;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;  // GQA: query head -> shared KV head
  let scale=1.0/sqrt(f32(dd.hd));
  let qpos=dd.base+i;                                 // global position of this query row
  // Sliding window — SAME semantics as the parallel kernel: standard keeps the last nswa
  // positions ending at qpos; chunked starts at qpos's aligned chunk. lo==0 when off.
  var lo=0u;
  if(dd.nswa>0u){
    let l=qpos+1u;
    if(dd.swaty==1u){ if(l>dd.nswa){ lo=l-dd.nswa; } }
    else if(dd.swaty==2u){ lo=(qpos/dd.nswa)*dd.nswa; }
  }
  var mx=-3.4e38;
  for(var j=lo;j<=qpos;j=j+1u){ var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[i*dm+b+c]*kc[j*kvdm+kb+c];} mx=max(mx,dot*scale); }
  for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=0.0;}
  var sum=0.0;
  for(var j=lo;j<=qpos;j=j+1u){
    var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[i*dm+b+c]*kc[j*kvdm+kb+c];}
    let p=exp(dot*scale-mx); sum=sum+p;
    for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=o[i*dm+b+c]+p*vc[j*kvdm+kb+c];}
  }
  for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=o[i*dm+b+c]/sum;}
}