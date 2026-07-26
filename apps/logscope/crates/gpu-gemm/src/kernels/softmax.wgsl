
struct D { rows:u32,cols:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let r=g.x; if(r>=d.rows){return;}
  let base=r*d.cols; var mx=-3.4e38;
  for(var c=0u;c<d.cols;c=c+1u){mx=max(mx,x[base+c]);}
  var sum=0.0;
  for(var c=0u;c<d.cols;c=c+1u){let e=exp(x[base+c]-mx); o[base+c]=e; sum=sum+e;}
  for(var c=0u;c<d.cols;c=c+1u){o[base+c]=o[base+c]/sum;}
}