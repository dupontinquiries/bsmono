
struct D { n:u32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){ let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} let v=x[i]; let inner=0.7978845608028654*(v+0.044715*v*v*v); o[i]=0.5*v*(1.0+tanh(inner)); }