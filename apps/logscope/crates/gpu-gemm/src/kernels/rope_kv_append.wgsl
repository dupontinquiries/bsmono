// ROPE_KV_APPEND — one decode token: rope q -> o, rope k -> kc[pos], v -> vc[pos]. ONE dispatch.
//
// What it replaces per layer on the f32-KV decode path (decode-census, tiny qwen2, lavapipe):
//   slice(q|k) copy, ROPE_MH, slice q copy, k->cache copy, slice k copy (its result was never
//   read), v->cache copy — one dispatch and FIVE buffer copies. On Honeykrisp a buffer copy is a
//   compute job of its own (no blit engine), so those five cost what five dispatches cost, and
//   each one drains the pass-fusion queue. This kernel is the same math with the destinations
//   folded in: the rotation is ROPE_MH's line for line (same freq expression, same c/s, same
//   x0*c-x1*s), so the values are bit-identical — what changes is only WHERE they are written.
//
// Layout (all f32): qkv is [1, ad+2kv] = q heads | k heads | v heads, head-aligned. The cache
// rows are [kv] wide at row `pos0` (row stride kv). Threads 0..(ad+kv)/2 are ROPE_MH's rotation
// pairs over the q|k span; threads (ad+kv)/2..(ad+2kv)/2 copy v two lanes each.
struct D { t:u32,dm:u32,base:f32,pos0:u32,hd:u32,neox:u32,msc:f32,sl:u32,rot:u32,ad:u32,kv:u32,nope:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;          // qkv [1, ad+2kv]
@group(0)@binding(1) var<storage,read> ff: array<f32>;         // per-dim-pair freq factors (len hd/2)
@group(0)@binding(2) var<storage,read_write> kc: array<f32>;   // K cache [cap, kv]
@group(0)@binding(3) var<storage,read_write> vc: array<f32>;   // V cache [cap, kv]
@group(0)@binding(4) var<storage,read_write> o: array<f32>;    // roped q [1, ad]
@group(0)@binding(5) var<uniform> dd: D;

// Where lane `j` of the q|k span goes: q lanes to o, k lanes to this token's K-cache row.
fn put(j:u32, v:f32) {
  if (j < dd.ad) { o[j] = v; } else { kc[dd.pos0*dd.kv + (j - dd.ad)] = v; }
}

@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let half=dd.hd/2u; let per=dd.dm/2u; let p=g.y*nwg.x*64u+g.x;
  if (p >= per) {
    // V lanes: straight copy into the V-cache row, two per thread.
    let c = 2u*(p-per); if (c >= dd.kv) { return; }
    let src = dd.ad + dd.kv + c; let dst = dd.pos0*dd.kv + c;
    vc[dst] = x[src]; vc[dst+1u] = x[src+1u];
    return;
  }
  // t==1 always here; `tok` and `pt` are kept so the freq line below is ROPE_MH's verbatim.
  let tok=p/per; let within=p%per; let head=within/half; let i=within%half;
  let off=tok*dd.dm + head*dd.hd;
  let rhalf=dd.rot/2u;
  if(i>=rhalf){
    // pass-through lanes [rot, hd) — ROPE_MH's partial-rotation branch, destinations folded in.
    let j=off + dd.rot + 2u*(i-rhalf);
    put(j, x[j]); put(j+1u, x[j+1u]);
    return;
  }
  // NoPE layer (Llama-4 iRoPE): rope_mh_t's caller copied instead of dispatching. Same identity,
  // written to the cache row directly. Must branch BEFORE the freq line — base is 0 here.
  if (dd.nope != 0u) {
    let j0=off + i*(2u-dd.neox);
    let j1=j0 + 1u + dd.neox*(rhalf-1u);
    put(j0, x[j0]); put(j1, x[j1]);
    return;
  }
  let pt=select(tok, tok%max(dd.sl,1u), dd.sl>0u);
  let freq=f32(pt+dd.pos0)/(pow(dd.base,(2.0*f32(i))/f32(dd.rot))*ff[i]);
  let c=cos(freq)*dd.msc; let s=sin(freq)*dd.msc;
  let j0=off + i*(2u-dd.neox);
  let j1=j0 + 1u + dd.neox*(rhalf-1u);
  let x0=x[j0]; let x1=x[j1];
  put(j0, x0*c-x1*s);
  put(j1, x0*s+x1*c);
}
