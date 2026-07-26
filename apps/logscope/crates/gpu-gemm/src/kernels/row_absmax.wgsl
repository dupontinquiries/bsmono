
struct D { n:u32,c:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> scales: array<f32>;
@group(0)@binding(2) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let row=g.x; if(row>=dd.n){return;}
  var mx=0.0;
  for(var c=0u;c<dd.c;c=c+1u){ mx=max(mx, abs(x[row*dd.c+c])); }
  scales[row]=mx/127.0;
}