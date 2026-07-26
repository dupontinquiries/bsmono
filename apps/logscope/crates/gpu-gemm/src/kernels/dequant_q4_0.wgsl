
struct D { nw:u32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> q: array<u32>;
@group(0)@binding(1) var<storage,read> s: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let i=g.x; if(i>=d.nw){return;}
  let blk=i/32u; let within=i%32u;
  // Plain branches (no `select`): the false-branch of a `select` is still
  // evaluated, and `within-16u` underflows for within<16 — harmless on llvmpipe
  // but mis-lowered on some younger Vulkan drivers (e.g. Mesa's Asahi/AGX
  // Apple-GPU backend). Guarding the subtraction behind the condition is portable.
  var j:u32; var hi:bool;
  if(within<16u){ j=within; hi=false; } else { j=within-16u; hi=true; }
  let gb=blk*16u+j;
  let word=q[gb/4u];
  // extractBits (canonical WGSL bitfield op → NIR bitfield_extract) instead of
  // hand-rolled >>/& — the manual dynamic shift is the only such construct in
  // these kernels and a prime suspect for the Asahi/Mesa (AGX) mismatch.
  let byte=extractBits(word, (gb%4u)*8u, 8u);
  var nib:u32;
  if(hi){ nib=extractBits(byte, 4u, 4u); } else { nib=extractBits(byte, 0u, 4u); }
  o[i]=(f32(nib)-8.0)*s[blk];
}