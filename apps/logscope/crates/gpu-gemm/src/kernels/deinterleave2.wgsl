// Split per-head interleaved [heads][2*hd] into one buffer [ p0: heads*hd | p1: heads*hd ]:
// p0 = first hd of each head block (Qwen3.5: the query), p1 = second hd (the gate).
struct D { heads:u32,hd:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let i=g.x; let n=dd.heads*dd.hd; if(i>=n){return;}
  let h=i/dd.hd; let c=i%dd.hd;
  o[i]=x[h*2u*dd.hd+c];
  o[n+i]=x[h*2u*dd.hd+dd.hd+c];
}
