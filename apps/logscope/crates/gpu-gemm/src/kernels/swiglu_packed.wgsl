
struct D { n:u32,h:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> gu: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){ let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} let r=i/d.h; let c=i%d.h; let base=r*2u*d.h; let v=gu[base+c]; o[i]=(v/(1.0+exp(-v)))*gu[base+d.h+c]; }