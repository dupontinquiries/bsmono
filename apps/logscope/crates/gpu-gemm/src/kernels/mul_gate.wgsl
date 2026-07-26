// Elementwise gate: mode 0 -> y = a * silu(b)   (DeltaNet gated norm, z gate)
//                   mode 1 -> y = a * sigmoid(b) (Qwen3.5 gated attention)
struct D { n:u32,mode:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> b: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let i=g.x; if(i>=dd.n){return;}
  let sg=1.0/(1.0+exp(-b[i]));
  o[i]=a[i]*select(sg, b[i]*sg, dd.mode==0u);
}
