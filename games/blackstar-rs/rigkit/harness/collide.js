// Tyre paths and bot-pose overlaps, from the page's own frames.
module.exports=async function(page){
  delete require.cache[require.resolve('./boot.js')];
  const {rt}=await require('./boot.js')(page);
  rt.setDyn(false);
  const B=rt.RIG.bones, R=rt.RIG.radius, DR=rt.RIG.default_r, P=rt.RIG.car.place;
  const frame=t=>{ rt.tIn.value=String(t); rt.refresh(); rt.scene.updateMatrixWorld(true);
    const {p,bulk}=rt.poseAt(t); const p2=Object.assign({},p); const sp=rt.spinAt(t);
    if(Math.abs(sp)>0.01){ const a=(p2.spine||[0,0,0]).slice(); p2.spine=[a[0],a[1]+sp,a[2]]; }
    const s=rt.solve(p2); const lift=Math.max(rt.smoothLift(t), rt.groundLift(s,bulk));
    return B.filter(b=>b.l>0).map(b=>{const J=s[b.n]; return {n:b.n, r:(R[b.n]!==undefined?R[b.n]:DR), h:[J.head[0],J.head[1]+lift,J.head[2]], t:[J.tail[0],J.tail[1]+lift,J.tail[2]]};}); };
  const segDist=(c,g)=>{ const d=[g.t[0]-g.h[0],g.t[1]-g.h[1],g.t[2]-g.h[2]], L2=d[0]**2+d[1]**2+d[2]**2;
    const f=L2>1e-9?Math.max(0,Math.min(1,((c[0]-g.h[0])*d[0]+(c[1]-g.h[1])*d[1]+(c[2]-g.h[2])*d[2])/L2)):0;
    return Math.hypot(c[0]-g.h[0]-d[0]*f,c[1]-g.h[1]-d[1]*f,c[2]-g.h[2]-d[2]*f); };
  const wbox=m=>{ m.geometry.computeBoundingBox(); const bb=m.geometry.boundingBox, lo=[1e9,1e9,1e9], hi=[-1e9,-1e9,-1e9], v=new THREE.Vector3();
    for(let k=0;k<8;k++){ v.set(k&1?bb.max.x:bb.min.x,k&2?bb.max.y:bb.min.y,k&4?bb.max.z:bb.min.z).applyMatrix4(m.matrixWorld);
      lo[0]=Math.min(lo[0],v.x);lo[1]=Math.min(lo[1],v.y);lo[2]=Math.min(lo[2],v.z);hi[0]=Math.max(hi[0],v.x);hi[1]=Math.max(hi[1],v.y);hi[2]=Math.max(hi[2],v.z);}
    return {lo,hi}; };
  // 1. tyre paths
  const wheels=Object.keys(P).filter(g=>g.startsWith('wheel'));
  const tyres={};
  for(const g of wheels) tyres[g]={vmax:0,atV:0,worstIn:1e9,atIn:0,inBone:'',below:1e9,atBelow:0,prev:null};
  for(let i=0;i<=100;i++){ const t=i/100, segs=frame(t);
    for(const g of wheels){ const m=rt.carMeshes[g]; if(!m.visible) continue; const e=m.matrixWorld.elements, c=[e[12],e[13],e[14]], T=tyres[g];
      if(T.prev){ const v=Math.hypot(c[0]-T.prev[0],c[1]-T.prev[1],c[2]-T.prev[2]); if(v>T.vmax){T.vmax=v;T.atV=t;} } T.prev=c;
      const bb=wbox(m); if(bb.lo[1]<T.below){T.below=bb.lo[1];T.atBelow=t;}
      if(t>0.42) for(const sg of segs){ if(sg.n===P[g].b) continue; const d=segDist(c,sg)-sg.r; if(d<T.worstIn){T.worstIn=d;T.atIn=t;T.inBone=sg.n;} } } }
  // 2. overlaps at t=1
  frame(1.0);
  const arm=Object.keys(P).filter(g=>P[g].f==='armour' && rt.carMeshes[g].visible);
  const boxes=Object.fromEntries(arm.map(g=>[g,wbox(rt.carMeshes[g])]));
  const pairs=[];
  for(let i=0;i<arm.length;i++) for(let j=i+1;j<arm.length;j++){ const a=boxes[arm[i]], b=boxes[arm[j]];
    const ov=[0,1,2].map(k=>Math.max(0,Math.min(a.hi[k],b.hi[k])-Math.max(a.lo[k],b.lo[k])));
    const vol=ov[0]*ov[1]*ov[2]; if(vol<=0) continue;
    const va=(a.hi[0]-a.lo[0])*(a.hi[1]-a.lo[1])*(a.hi[2]-a.lo[2]), vb=(b.hi[0]-b.lo[0])*(b.hi[1]-b.lo[1])*(b.hi[2]-b.lo[2]);
    pairs.push([arm[i],arm[j],vol/Math.min(va,vb)]); }
  pairs.sort((x,y)=>y[2]-x[2]);
  return {tyres, pairs, floor:rt.RIG.floor};
};
