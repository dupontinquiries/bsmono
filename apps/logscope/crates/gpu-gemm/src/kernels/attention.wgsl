
struct D { t:u32,d:u32,causal:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> k: array<f32>;
@group(0)@binding(2) var<storage,read> v: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let i=g.x; if(i>=dd.t){return;}
  let dim=dd.d; let scale=1.0/sqrt(f32(dim));
  let last=select(dd.t-1u, i, dd.causal==1u);
  var mx=-3.4e38;
  for(var jj=0u;jj<=last;jj=jj+1u){
    var dot=0.0;
    for(var c=0u;c<dim;c=c+1u){dot=dot+q[i*dim+c]*k[jj*dim+c];}
    mx=max(mx,dot*scale);
  }
  for(var c=0u;c<dim;c=c+1u){o[i*dim+c]=0.0;}
  var sum=0.0;
  for(var jj=0u;jj<=last;jj=jj+1u){
    var dot=0.0;
    for(var c=0u;c<dim;c=c+1u){dot=dot+q[i*dim+c]*k[jj*dim+c];}
    let p=exp(dot*scale-mx); sum=sum+p;
    for(var c=0u;c<dim;c=c+1u){o[i*dim+c]=o[i*dim+c]+p*v[jj*dim+c];}
  }
  for(var c=0u;c<dim;c=c+1u){o[i*dim+c]=o[i*dim+c]/sum;}
}