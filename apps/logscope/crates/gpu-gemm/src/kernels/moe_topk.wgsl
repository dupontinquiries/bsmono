
struct D { n:u32,k:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> logits: array<f32>;
@group(0)@binding(1) var<storage,read_write> selw: array<u32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(1,1,1)
fn main(){
  var idx: array<u32,16>; var lg: array<f32,16>;
  for(var s=0u;s<d.k;s=s+1u){
    var bi=0u; var bv=-3.4e38;
    for(var i=0u;i<d.n;i=i+1u){
      var taken=false;
      for(var t=0u;t<s;t=t+1u){ if(idx[t]==i){ taken=true; } }
      if(!taken && logits[i]>bv){ bv=logits[i]; bi=i; }
    }
    idx[s]=bi; lg[s]=bv;
  }
  // softmax over the selected (selection is descending, so lg[0] is the max — hoist it BEFORE the loop
  // mutates lg in place; using lg[0] inside the loop reads exp(0)=1.0 for s>=1, mis-weighting slot 0 by
  // exp(max-1) after normalization — the exact wgtΔmax the differential checker caught)
  let mx=lg[0];
  var sum=0.0;
  for(var s=0u;s<d.k;s=s+1u){ lg[s]=exp(lg[s]-mx); sum=sum+lg[s]; }
  for(var s=0u;s<d.k;s=s+1u){ selw[s]=idx[s]; selw[d.k+s]=bitcast<u32>(lg[s]/sum); }
}