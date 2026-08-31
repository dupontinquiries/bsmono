// GATHER
struct D { n:u32,d:u32,id_off:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> table: array<f32>;
@group(0)@binding(1) var<storage,read> ids: array<u32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  // 2D-SAFE INDEX. A whole-canvas diffusion forward gathers n*d elements at once: at d=2048 a
  // 2048-token canvas is 4194304 elements = exactly 65536 workgroups, one past the 65535 per-
  // dimension limit, so every dispatch was rejected as a NON-FATAL validation error and the
  // embeddings silently never happened. wg1() already spills into y for kernels that read it;
  // this one only read g.x, so it had to be dispatched flat and could not be spilled.
  let p=g.y*nwg.x*64u+g.x; if(p>=dd.n*dd.d){return;}
  let i=p/dd.d; let c=p%dd.d;
  o[p]=table[ids[dd.id_off+i]*dd.d+c];
}
