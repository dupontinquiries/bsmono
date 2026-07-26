
struct D { n:u32,c:u32,_b:u32,_d:u32 };
@group(0)@binding(0) var<storage,read> src: array<f32>;
@group(0)@binding(1) var<storage,read> idx: array<u32>;
@group(0)@binding(2) var<storage,read> wgt: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.n*d.c){return;}
  let r=i/d.c; let col=i%d.c; o[idx[r]*d.c+col]=o[idx[r]*d.c+col]+wgt[r]*src[i]; }