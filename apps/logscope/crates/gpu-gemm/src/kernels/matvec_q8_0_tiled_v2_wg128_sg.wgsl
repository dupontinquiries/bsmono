
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32,tile:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> part: array<f32,64>; // nsg(≤8) x tile(≤8) partials — tiny vs red[tile*128]
@compute @workgroup_size(128,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>,
        @builtin(subgroup_invocation_id) sglid:u32, @builtin(subgroup_size) sgsz:u32){
  let col0=(wg.y*d.gx+wg.x)*d.tile; let tid=lid.x;
  if(col0>=d.n){ return; }
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  let words=d.k/4u;
  var w=tid;
  loop {
    if(w>=words){break;}
    let kk0=w*4u;
    let a0=a[kk0]; let a1=a[kk0+1u]; let a2=a[kk0+2u]; let a3=a[kk0+3u];
    let blk=kk0/32u; // 4 consecutive k share ONE 32-block → ONE scale index (was b0..b3, all equal)
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      let col=col0+ci;
      if(col<d.n){
        let word=qw[col*words+w];
        let u0=extractBits(word,0u,8u);  let u1=extractBits(word,8u,8u);
        let u2=extractBits(word,16u,8u); let u3=extractBits(word,24u,8u);
        let q0=select(i32(u0),i32(u0)-256,u0>=128u); let q1=select(i32(u1),i32(u1)-256,u1>=128u);
        let q2=select(i32(u2),i32(u2)-256,u2>=128u); let q3=select(i32(u3),i32(u3)-256,u3>=128u);
        let sc=qs[col*d.kb+blk]; // ONE scale load, reused across all 4 lanes
        acc[ci]=acc[ci]+a0*sc*f32(q0)+a1*sc*f32(q1)+a2*sc*f32(q2)+a3*sc*f32(q3);
      }
    }
    w=w+128u;
  }
  let sgid=tid/sgsz;            // which subgroup
  let nsg=128u/sgsz;           // subgroups per workgroup (4 at size 32)
  for(var ci=0u;ci<d.tile;ci=ci+1u){
    let s=subgroupAdd(acc[ci]); // reduce this subgroup's 32 lanes in one op, no barrier
    if(sglid==0u){ part[sgid*d.tile+ci]=s; }
  }
  workgroupBarrier();
  if(tid==0u){
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      var sum=0.0;
      for(var g=0u;g<nsg;g=g+1u){ sum=sum+part[g*d.tile+ci]; }
      let col=col0+ci; if(col<d.n){ c[col]=sum; }
    }
  }
}