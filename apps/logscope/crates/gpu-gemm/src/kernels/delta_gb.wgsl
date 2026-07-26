// Per-v-head decay/write gates from the projected scalars:
//   eg = exp( a_h * softplus(alpha_h + dtb_h) )   (a stores -exp(A_log))
//   be = sigmoid(beta_h)
struct D { hv:u32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> alpha: array<f32>;
@group(0)@binding(1) var<storage,read> beta: array<f32>;
@group(0)@binding(2) var<storage,read> dtb: array<f32>;
@group(0)@binding(3) var<storage,read> a: array<f32>;
@group(0)@binding(4) var<storage,read_write> o: array<f32>;
@group(0)@binding(5) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let h=g.x; if(h>=dd.hv){return;}
  let x=alpha[h]+dtb[h];
  let sp=select(log(1.0+exp(x)), x, x>20.0);      // softplus, overflow-safe
  o[h*2u]=exp(a[h]*sp);
  o[h*2u+1u]=1.0/(1.0+exp(-beta[h]));
}
