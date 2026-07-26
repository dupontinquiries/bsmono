
struct D { nk:u32, keep:u32, _a:u32, _b:u32 };
@group(0)@binding(0) var<storage,read> s: array<f32>;        // [nk] scores
@group(0)@binding(1) var<storage,read_write> mask: array<f32>; // [nk]
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let t=g.x; if(t>=d.nk){return;}
  let st=s[t];
  var rank=0u;
  for(var j=0u;j<d.nk;j=j+1u){
    let sj=s[j];
    if(sj>st){ rank=rank+1u; }
    else if(sj==st && j<t){ rank=rank+1u; }   // stable tie-break: earlier position ranks higher
  }
  if(rank<d.keep){ mask[t]=1.0; } else { mask[t]=0.0; }
}