
struct D { nk:u32, dk:u32, dv:u32, _p:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;    // [dk]
@group(0)@binding(1) var<storage,read> k: array<f32>;    // [nk*dk]
@group(0)@binding(2) var<storage,read> v: array<f32>;    // [nk*dv]
@group(0)@binding(3) var<storage,read> mask: array<f32>; // [nk]
@group(0)@binding(4) var<storage,read_write> o: array<f32>; // [dv]
@group(0)@binding(5) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let c=g.x; if(c>=d.dv){return;}
  let scale=1.0/sqrt(f32(d.dk));
  // pass 1: max over selected keys (same score each thread recomputes — cheap, dk small; keeps it
  // one-output-per-thread with no cross-thread reduction).
  var mx=-3.4e38;
  for(var t=0u;t<d.nk;t=t+1u){
    if(mask[t]==0.0){continue;}
    var dot=0.0; let kb=t*d.dk; for(var i=0u;i<d.dk;i=i+1u){dot=dot+q[i]*k[kb+i];}
    let s=dot*scale; if(s>mx){mx=s;}
  }
  // pass 2: weighted sum of this column
  var denom=0.0; var acc=0.0;
  for(var t=0u;t<d.nk;t=t+1u){
    if(mask[t]==0.0){continue;}
    var dot=0.0; let kb=t*d.dk; for(var i=0u;i<d.dk;i=i+1u){dot=dot+q[i]*k[kb+i];}
    let p=exp(dot*scale-mx); denom=denom+p;
    acc=acc+p*v[t*d.dv+c];
  }
  o[c]=acc/denom;
}