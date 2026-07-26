
struct D { n:u32,c:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> scales: array<f32>;
@group(0)@binding(2) var<storage,read_write> q: array<u32>;
@group(0)@binding(3) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let words=dd.c/4u; let p=g.x; if(p>=dd.n*words){return;}
  let row=p/words; let w=p%words;
  let sc=scales[row]; let inv=select(1.0/sc, 0.0, sc==0.0);
  var packed=0u;
  for(var b=0u;b<4u;b=b+1u){
    let qi=clamp(i32(round(x[row*dd.c + w*4u + b]*inv)), -127, 127);
    packed=packed | ((u32(qi) & 0xffu) << (b*8u));
  }
  q[p]=packed;
}