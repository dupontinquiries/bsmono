// MOE_TOPK
// `mode` selects the WEIGHTING, not the selection: 0 = softmax over the chosen k, 1 = sigmoid of each
// logit. Selection is identical either way because sigmoid is monotonic, so top-k by raw logit picks the
// same experts — which is why this is a small change rather than a second kernel.
struct D { n:u32,k:u32,mode:u32,normalize:u32,scale:f32,ng:u32,tg:u32,_c2:u32 };
@group(0)@binding(0) var<storage,read> logits: array<f32>;
@group(0)@binding(1) var<storage,read_write> selw: array<u32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(1,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>){
  let tok=wg.x;
  var idx: array<u32,16>; var lg: array<f32,16>;
  for(var s=0u;s<d.k;s=s+1u){
    var bi=0u; var bv=-3.4e38;
    for(var i=0u;i<d.n;i=i+1u){
      var taken=false;
      for(var t=0u;t<s;t=t+1u){ if(idx[t]==i){ taken=true; } }
      if(!taken && logits[tok*d.n+i]>bv){ bv=logits[tok*d.n+i]; bi=i; }
    }
    idx[s]=bi; lg[s]=bv;
  }
  // softmax over the selected (selection is descending, so lg[0] is the max — hoist it BEFORE the loop
  // mutates lg in place; using lg[0] inside the loop reads exp(0)=1.0 for s>=1, mis-weighting slot 0 by
  // exp(max-1) after normalization — the exact wgtΔmax the differential checker caught)
  let mx=lg[0];
  var sum=0.0;
  if (d.mode == 1u) {
    // Sigmoid gating (Llama-4, GLM). Each weight is sigmoid(logit) INDEPENDENTLY — there is no shared
    // denominator unless the model asks for one, so a softmax here would silently renormalize weights
    // the reference leaves alone.
    for(var s=0u;s<d.k;s=s+1u){ lg[s]=1.0/(1.0+exp(-lg[s])); sum=sum+lg[s]; }
  } else {
    for(var s=0u;s<d.k;s=s+1u){ lg[s]=exp(lg[s]-mx); sum=sum+lg[s]; }
  }
  // Softmax is normalized by construction; sigmoid only when the model says so.
  let dnm = select(1.0, sum, d.mode == 0u || d.normalize == 1u);
  // ROUTED SCALE: expert_weights_scale multiplies the applied weight (llada2/GLM: 2.5). The CPU
  // route always applied it; this kernel silently did not — argmax survives, p/entropy do not.
  for(var s=0u;s<d.k;s=s+1u){ selw[tok*2u*d.k+s]=idx[s]; selw[tok*2u*d.k+d.k+s]=bitcast<u32>(lg[s]/dnm*d.scale); }
}
