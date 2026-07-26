
// `nswa`/`swaty` add sliding-window attention. Unlike decode — where a single query position gives one
// contiguous range — every query ROW here has its own window, so the bound is computed per row from that
// row's global position. It is still a range rather than a mask, because both windowing rules are
// contiguous: the tile loop simply starts at the window instead of at 0. swaty: 0 none, 1 standard
// (Gemma, last n positions), 2 chunked (Llama-4, current aligned chunk).
struct D { s:u32,hd:u32,nh:u32,nkv:u32,base:u32,nswa:u32,swaty:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kc: array<f32>;
@group(0)@binding(2) var<storage,read> vc: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
var<workgroup> sc: array<f32,2048>;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let row=wg.x;                                       // which (query row, head) this workgroup handles
  let i=row/dd.nh; let hh=row%dd.nh; let tid=lid.x;
  let dm=dd.nh*dd.hd; let kvdm=dd.nkv*dd.hd;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;   // GQA: query head -> shared KV head
  let scale=1.0/sqrt(f32(dd.hd));
  let qpos=dd.base+i;                                 // causal bound: attend to [0, qpos]
  let l=qpos+1u;
  let TILE=2048u;                                     // KV rows per shared-memory tile (matches sc size)
  // Online-softmax over KV TILES so any causal length works at 8 KB shared memory (high occupancy):
  // per tile, load scores into sc, then fold the tile into a running (max m, denom d, acc[hd]) with the
  // standard rescale. acc for this row's head_dim is kept in registers, strided across the 64 threads.
  var m=-3.4e38; var den=0.0;
  // per-thread partial accumulator over head_dim columns this thread owns (c = tid, tid+64, …)
  var acc0=0.0; var acc1=0.0; var acc2=0.0;           // hd<=192 for tid-strided (hd/64) cols; 128→2 cols
  // sliding window: this row's earliest visible KV position (0 when unwindowed)
  var lo=0u;
  if(dd.nswa>0u){
    if(dd.swaty==1u){ if(l>dd.nswa){ lo=l-dd.nswa; } }        // standard: keep the last nswa positions
    else if(dd.swaty==2u){ lo=(qpos/dd.nswa)*dd.nswa; }        // chunked: start of qpos's aligned chunk
  }
  var t0=lo;
  loop {
    if(t0>=l){break;}
    let t1=min(l, t0+TILE);
    // scores for this tile → sc[0 .. t1-t0), and this tile's local max
    var lmax=-3.4e38; var j=t0+tid;
    loop { if(j>=t1){break;} var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[i*dm+b+c]*kc[j*kvdm+kb+c];} let sv=dot*scale; sc[j-t0]=sv; lmax=max(lmax,sv); j=j+64u; }
    red[tid]=lmax; workgroupBarrier();
    for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=max(red[tid],red[tid+st]); } workgroupBarrier(); }
    let tilemax=red[0]; workgroupBarrier();
    // new running max and the rescale factor for the accumulator/denominator so far
    let newm=max(m, tilemax);
    let resc=exp(m-newm);
    // exponentiate this tile's scores against newm; tile denom
    var tden=0.0; j=t0+tid;
    loop { if(j>=t1){break;} let p=exp(sc[j-t0]-newm); sc[j-t0]=p; tden=tden+p; j=j+64u; }
    red[tid]=tden; workgroupBarrier();
    for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=red[tid]+red[tid+st]; } workgroupBarrier(); }
    let tilesum=red[0]; workgroupBarrier();
    den=den*resc+tilesum;
    // fold tile's V-weighted contribution into the register accumulators (rescaled), cols strided by 64
    acc0=acc0*resc; acc1=acc1*resc; acc2=acc2*resc;
    var c=tid; var slot=0u;
    loop { if(c>=dd.hd){break;}
      var a=0.0; for(var jj=t0;jj<t1;jj=jj+1u){ a=a+sc[jj-t0]*vc[jj*kvdm+kb+c]; }
      if(slot==0u){acc0=acc0+a;} else if(slot==1u){acc1=acc1+a;} else {acc2=acc2+a;}
      c=c+64u; slot=slot+1u;
    }
    m=newm; t0=t0+TILE;
    workgroupBarrier();
  }
  // write normalized output: each thread writes the cols it owns
  var c=tid; var slot=0u;
  loop { if(c>=dd.hd){break;}
    var a=acc0; if(slot==1u){a=acc1;} else if(slot==2u){a=acc2;}
    o[i*dm+b+c]=a/den; c=c+64u; slot=slot+1u;
  }
}