
struct D { m:u32,n:u32,k:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> b: array<f32>;
@group(0)@binding(2) var<storage,read_write> c: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
const TILE:u32=16u;
var<workgroup> as_: array<f32,256>; var<workgroup> bs: array<f32,256>;
@compute @workgroup_size(16,16,1)
fn main(@builtin(local_invocation_id) l:vec3<u32>, @builtin(workgroup_id) w:vec3<u32>){
  let row=w.y*TILE+l.y; let col=w.x*TILE+l.x; var acc=0.0;
  let nt=(d.k+TILE-1u)/TILE;
  for(var t=0u;t<nt;t=t+1u){
    let ak=t*TILE+l.x; let bk=t*TILE+l.y;
    if(row<d.m && ak<d.k){as_[l.y*TILE+l.x]=a[row*d.k+ak];}else{as_[l.y*TILE+l.x]=0.0;}
    if(bk<d.k && col<d.n){bs[l.y*TILE+l.x]=b[bk*d.n+col];}else{bs[l.y*TILE+l.x]=0.0;}
    workgroupBarrier();
    for(var kk=0u;kk<TILE;kk=kk+1u){acc=acc+as_[l.y*TILE+kk]*bs[kk*TILE+l.x];}
    workgroupBarrier();
  }
  if(row<d.m && col<d.n){c[row*d.n+col]=acc;}
}