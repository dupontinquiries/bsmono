
struct D { m:u32,n:u32,k:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> wt: array<f32>;
@group(0)@binding(2) var<storage,read_write> c: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
const TILE:u32=16u;
var<workgroup> as_: array<f32,256>; var<workgroup> ws: array<f32,256>;
@compute @workgroup_size(16,16,1)
fn main(@builtin(local_invocation_id) l:vec3<u32>, @builtin(workgroup_id) w:vec3<u32>){
  let row=w.y*TILE+l.y; let col=w.x*TILE+l.x; var acc=0.0;
  let nt=(d.k+TILE-1u)/TILE;
  for(var t=0u;t<nt;t=t+1u){
    let ak=t*TILE+l.x; let wk=t*TILE+l.y; let wcol=w.x*TILE+l.x;
    if(row<d.m && ak<d.k){as_[l.y*TILE+l.x]=a[row*d.k+ak];}else{as_[l.y*TILE+l.x]=0.0;}
    if(wcol<d.n && wk<d.k){ws[l.y*TILE+l.x]=wt[wcol*d.k+wk];}else{ws[l.y*TILE+l.x]=0.0;}
    workgroupBarrier();
    for(var kk=0u;kk<TILE;kk=kk+1u){acc=acc+as_[l.y*TILE+kk]*ws[kk*TILE+l.x];}
    workgroupBarrier();
  }
  if(row<d.m && col<d.n){c[row*d.n+col]=acc;}
}