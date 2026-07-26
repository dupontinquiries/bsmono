
struct D { n:u32,c:u32,_b:u32,_d:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> bias: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){ let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} o[i]=a[i]+bias[i%d.c]; }