
struct D { l:u32,hd:u32,nh:u32,nkv:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kc: array<f32>;
@group(0)@binding(2) var<storage,read> vc: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let hh=g.x; if(hh>=dd.nh){return;}
  let dm=dd.nh*dd.hd; let kvdm=dd.nkv*dd.hd;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;  // GQA: query head -> shared KV head
  let scale=1.0/sqrt(f32(dd.hd));
  var mx=-3.4e38;
  for(var j=0u;j<dd.l;j=j+1u){ var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[b+c]*kc[j*kvdm+kb+c];} mx=max(mx,dot*scale); }
  for(var c=0u;c<dd.hd;c=c+1u){o[b+c]=0.0;}
  var sum=0.0;
  for(var j=0u;j<dd.l;j=j+1u){
    var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[b+c]*kc[j*kvdm+kb+c];}
    let p=exp(dot*scale-mx); sum=sum+p;
    for(var c=0u;c<dd.hd;c=c+1u){o[b+c]=o[b+c]+p*vc[j*kvdm+kb+c];}
  }
  for(var c=0u;c<dd.hd;c=c+1u){o[b+c]=o[b+c]/sum;}
}