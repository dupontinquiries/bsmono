
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32,tile:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw4: array<vec4<u32>>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,512>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col0=(wg.y*d.gx+wg.x)*d.tile; let tid=lid.x;
  if(col0>=d.n){ return; }
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  let words4=d.k/16u;               // vec4<u32> per column-row (16 q8 values each)
  var w=tid;
  loop {
    if(w>=words4){break;}
    let kk0=w*16u;
    let blk=kk0/32u;                  // all 16 q8 in a vec4 load share ONE 32-block → ONE scale index
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      let col=col0+ci;
      if(col<d.n){
        let v=qw4[col*words4+w];     // ONE 128-bit load = 16 q8 values
        let base=col*d.kb;
        let sc=qs[base+blk];        // ONE scale load, reused across all 16 lanes (was 16 identical loads)
        for(var jj=0u;jj<4u;jj=jj+1u){
          let word=v[jj];
          let kkw=kk0+jj*4u;
          let u0=extractBits(word,0u,8u);  let u1=extractBits(word,8u,8u);
          let u2=extractBits(word,16u,8u); let u3=extractBits(word,24u,8u);
          let q0=select(i32(u0),i32(u0)-256,u0>=128u);
          let q1=select(i32(u1),i32(u1)-256,u1>=128u);
          let q2=select(i32(u2),i32(u2)-256,u2>=128u);
          let q3=select(i32(u3),i32(u3)-256,u3>=128u);
          acc[ci]=acc[ci]
            + a[kkw]*sc*f32(q0)
            + a[kkw+1u]*sc*f32(q1)
            + a[kkw+2u]*sc*f32(q2)
            + a[kkw+3u]*sc*f32(q3);
        }
      }
    }
    w=w+64u;
  }
  for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=acc[ci]; }
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){
    if(tid<s){ for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=red[ci*64u+tid]+red[ci*64u+tid+s]; } }
    workgroupBarrier();
  }
  if(tid==0u){ for(var ci=0u;ci<d.tile;ci=ci+1u){ let col=col0+ci; if(col<d.n){ c[col]=red[ci*64u]; } } }
}