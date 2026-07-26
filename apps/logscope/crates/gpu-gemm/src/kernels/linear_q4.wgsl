
struct D { m:u32,n:u32,k:u32,kb:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;   // [m,k] activations
@group(0)@binding(1) var<storage,read> qw: array<u32>;  // [n,k] nibbles, 8 packed per u32
@group(0)@binding(2) var<storage,read> qs: array<f32>;  // [n,kb] per-block scales
@group(0)@binding(3) var<storage,read_write> c: array<f32>; // [m,n]
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let col=g.x; let row=g.y;
  if(row>=d.m || col>=d.n){return;}
  let wbase=col*(d.k/8u); let sbase=col*d.kb; let arow=row*d.k;
  var acc=0.0;
  for(var b=0u;b<d.kb;b=b+1u){
    let scale=qs[sbase+b]; let k0=b*32u;
    for(var j=0u;j<32u;j=j+1u){
      let kk=k0+j;
      let word=qw[wbase+(kk/8u)];
      let nib=extractBits(word,(kk%8u)*4u,4u);
      let q=i32(nib)-8;
      acc=acc+a[arow+kk]*scale*f32(q);
    }
  }
  c[row*d.n+col]=acc;
}