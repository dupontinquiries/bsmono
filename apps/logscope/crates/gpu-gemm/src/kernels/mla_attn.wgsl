
struct D { t:u32, nh:u32, qk:u32, vh:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> k: array<f32>;
@group(0)@binding(2) var<storage,read> v: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let hh=g.x; if(hh>=d.nh){return;}
  let qkdm=d.nh*d.qk; let vdm=d.nh*d.vh;
  let qb=hh*d.qk; let vb=hh*d.vh;
  let scale=1.0/sqrt(f32(d.qk));
  var mx=-3.4e38;
  for(var j=0u;j<d.t;j=j+1u){ var dot=0.0; for(var c=0u;c<d.qk;c=c+1u){dot=dot+q[qb+c]*k[j*qkdm+qb+c];} mx=max(mx,dot*scale); }
  for(var c=0u;c<d.vh;c=c+1u){o[vb+c]=0.0;}
  var sum=0.0;
  for(var j=0u;j<d.t;j=j+1u){
    var dot=0.0; for(var c=0u;c<d.qk;c=c+1u){dot=dot+q[qb+c]*k[j*qkdm+qb+c];}
    let pw=exp(dot*scale-mx); sum=sum+pw;
    for(var c=0u;c<d.vh;c=c+1u){o[vb+c]=o[vb+c]+pw*v[j*vdm+vb+c];}
  }
  for(var c=0u;c<d.vh;c=c+1u){o[vb+c]=o[vb+c]/sum;}
}