
struct D { n:u32,k:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> src: array<f32>;
@group(0)@binding(1) var<storage,read> selw: array<u32>;
@group(0)@binding(2) var<storage,read> resid: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;}
  var acc=resid[i];
  for(var s=0u;s<d.k;s=s+1u){ acc=acc+bitcast<f32>(selw[d.k+s])*src[s*d.n+i]; }
  o[i]=acc;
}