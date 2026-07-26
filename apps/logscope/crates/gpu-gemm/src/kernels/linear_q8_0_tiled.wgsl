
override TILE: u32 = 16u;
struct D { m:u32,n:u32,k:u32,kb:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;   // [m,k]
@group(0)@binding(1) var<storage,read> qw: array<u32>;  // [n,k] int8, 4 packed per u32
@group(0)@binding(2) var<storage,read> qs: array<f32>;  // [n,kb] per-block scales
@group(0)@binding(3) var<storage,read_write> c: array<f32>; // [m,n]
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> As: array<f32,1024>; // max tile 32×32; smaller tiles use the leading corner
var<workgroup> Ws: array<f32,1024>;
@compute @workgroup_size(TILE,TILE,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let ty=lid.y; let tx=lid.x;
  let row=wg.y*TILE+ty; let col=wg.x*TILE+tx;
  var acc=0.0;
  let ntiles=(d.k+TILE-1u)/TILE;
  for(var t=0u;t<ntiles;t=t+1u){
    let kt=t*TILE;
    let ak=kt+tx;                                   // A[row][kt+tx]
    As[ty*TILE+tx]=select(0.0, a[row*d.k+ak], row<d.m && ak<d.k);
    let wk=kt+ty;                                   // W[col][kt+ty], dequantized
    var wv=0.0;
    if(col<d.n && wk<d.k){
      let scale=qs[col*d.kb+(wk/32u)];
      let word=qw[col*(d.k/4u)+(wk/4u)];
      let ub=extractBits(word,(wk%4u)*8u,8u);
      let q=select(i32(ub), i32(ub)-256, ub>=128u);
      wv=scale*f32(q);
    }
    Ws[ty*TILE+tx]=wv;
    workgroupBarrier();
    for(var j=0u;j<TILE;j=j+1u){ acc=acc+As[ty*TILE+j]*Ws[j*TILE+tx]; } // k in order
    workgroupBarrier();
  }
  if(row<d.m && col<d.n){ c[row*d.n+col]=acc; }
}