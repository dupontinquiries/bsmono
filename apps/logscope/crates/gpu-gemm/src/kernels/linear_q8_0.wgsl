
struct D { m:u32,n:u32,k:u32,kb:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;   // [m,k] activations
@group(0)@binding(1) var<storage,read> qw: array<u32>;  // [n,k] int8, 4 packed per u32
@group(0)@binding(2) var<storage,read> qs: array<f32>;  // [n,kb] per-block scales
@group(0)@binding(3) var<storage,read_write> c: array<f32>; // [m,n]
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let col=g.x; let row=g.y;
  if(row>=d.m || col>=d.n){return;}
  let wbase=col*(d.k/4u); let sbase=col*d.kb; let arow=row*d.k;
  var acc=0.0;
  for(var b=0u;b<d.kb;b=b+1u){
    let scale=qs[sbase+b]; let k0=b*32u;
    for(var j=0u;j<32u;j=j+1u){
      let kk=k0+j;
      let word=qw[wbase+(kk/4u)];
      let ub=extractBits(word,(kk%4u)*8u,8u);          // unsigned byte (AGX-safe dynamic offset)
      let q=select(i32(ub), i32(ub)-256, ub>=128u);    // sign-extend i8, no underflow branch
      acc=acc+a[arow+kk]*scale*f32(q);
    }
  }
  c[row*d.n+col]=acc;
}