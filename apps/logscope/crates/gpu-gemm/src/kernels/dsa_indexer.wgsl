
struct D { nk:u32, nh:u32, hd:u32, _p:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;   // [nh*hd]
@group(0)@binding(1) var<storage,read> k: array<f32>;   // [nk*hd]
@group(0)@binding(2) var<storage,read> w: array<f32>;   // [nh]
@group(0)@binding(3) var<storage,read_write> o: array<f32>; // [nk]
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let t=g.x; if(t>=d.nk){return;}
  let wsc=1.0/sqrt(f32(d.nh));
  let rs=1.0/sqrt(f32(d.hd));
  let kbase=t*d.hd;
  var a=0.0;
  for(var h=0u;h<d.nh;h=h+1u){
    let qbase=h*d.hd;
    var d0=0.0;
    for(var i=0u;i<d.hd;i=i+1u){ d0=d0+q[qbase+i]*k[kbase+i]; }
    d0=d0*rs;
    if(d0>0.0){ a=a+w[h]*d0; }   // ReLU on the per-head score, then weight
  }
  o[t]=a*wsc;
}