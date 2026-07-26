// Depthwise causal conv step (Gated DeltaNet), one token. SINGLE read_write buffer
// `o` packs [ y: ch floats | cache: ch*(d_conv-1) floats ] so the persistent conv
// history lives in the same allocation the engine's one-output dispatch writes.
// window per channel = [cache oldest..newest, x]; y = silu(conv); cache shifts.
struct D { ch:u32,dc:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> w: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let c=g.x; if(c>=dd.ch){return;}
  let hist=dd.dc-1u; let cb=dd.ch + c*hist;
  var acc=0.0;
  for(var t=0u;t<hist;t=t+1u){ acc=acc+w[c*dd.dc+t]*o[cb+t]; }
  acc=acc+w[c*dd.dc+hist]*x[c];
  o[c]=acc/(1.0+exp(-acc));            // silu AFTER conv (llama.cpp order)
  for(var t=0u;t+1u<hist;t=t+1u){ o[cb+t]=o[cb+t+1u]; }
  o[cb+hist-1u]=x[c];
}
