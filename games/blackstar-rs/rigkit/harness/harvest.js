// Harvest training data from a viewer page's own frames.
//   node harvest.js <page.html> <vehicle> <out.json>
const fs=require('fs');
(async()=>{
  const [page, veh, out]=process.argv.slice(2);
  let FAKE=0; Object.defineProperty(globalThis,'performance',{value:{now:()=>FAKE},configurable:true,writable:true});
  const {rt}=await require('./boot.js')(page); rt.setDyn(false);
  const P=rt.RIG.car.place, BONES=rt.RIG.bones.map(b=>b.n);
  const src=fs.readFileSync(page,'utf8'); const META=JSON.parse(src.match(/const CAR_META = (\[.*?\]);\n/s)[1]);
  const meta=Object.fromEntries(META.map(m=>[m.g,m]));
  // vehicle bounds, from its parts (wheels included)
  const lo=[1e9,1e9,1e9], hi=[-1e9,-1e9,-1e9];
  for(const m of META) for(let k=0;k<3;k++){ lo[k]=Math.min(lo[k],m.c[k]-m.r[k]); hi[k]=Math.max(hi[k],m.c[k]+m.r[k]); }
  const parts=Object.keys(P).filter(g=>meta[g]);
  const stat=parts.map(g=>{ const p=P[g], m=meta[g];
    return {g, c:m.c, r:m.r, nt:m.nt,
      fate:p.f==='armour'?1:0, morph:p.mo?p.mo[0]:0, hinge:p.hinge?1:0,
      bone:BONES.indexOf(p.b), seat:p.o, scale:p.s, dis:p.dis}; });
  const frames=[];
  const grab=(anim, tt, params)=>{
    rt.refresh(); rt.scene.updateMatrixWorld(true);
    const pe=rt.bodyMesh.pelvis.matrixWorld.elements, pv=[pe[12],pe[13],pe[14]];
    const rows=parts.map(g=>{ const m=rt.carMeshes[g]; let vis=m.visible; for(let q=m.parent;q;q=q.parent) if(!q.visible) vis=false;
      const e=m.matrixWorld.elements;
      return [ +vis, m.userData.vox||0, e[12]-pv[0], e[13]-pv[1], e[14]-pv[2] ]; });
    // every bone's frame, relative to the pelvis, read off its body mesh (which
    // carries exactly [m * bulk | head + lift]; bulk is 1 once transformed)
    const bones={};
    for(const [n,o] of Object.entries(rt.bodyMesh)){ const E=o.matrixWorld.elements;
      const col=k=>{ const v=[E[k],E[k+1],E[k+2]], L=Math.hypot(...v)||1; return v.map(x=>x/L); };
      const cx=col(0), cy=col(4), cz=col(8);
      bones[n]=[E[12]-pv[0],E[13]-pv[1],E[14]-pv[2], cx[0],cy[0],cz[0], cx[1],cy[1],cz[1], cx[2],cy[2],cz[2]]; }
    frames.push({anim, t:tt, params, rows, bones});
  };
  const phys=(g,mass,thr)=>{ rt.MOTION.P.g=981*g; rt.MOTION.P.mass=mass; rt.MOTION.P.thrust=thr; };
  // the transform, both ways where the vehicle has both
  const modes=rt.RIG.poses_air?['ground','air']:['ground'];
  for(const mode of modes){
    // the page's own mode switch is not exposed; swap the pose list the same way it does
    if(mode==='air'){ rt.RIG.__g=rt.RIG.__g||{p:rt.RIG.poses,w:rt.RIG.weights}; rt.RIG.poses=rt.RIG.poses_air; rt.RIG.weights=rt.RIG.weights_air; }
    else if(rt.RIG.__g){ rt.RIG.poses=rt.RIG.__g.p; rt.RIG.weights=rt.RIG.__g.w; }
    rt.resetLift && rt.resetLift();
    rt.setMotion('',0);
    for(let i=0;i<=20;i++){ const t=i/20; rt.tIn.value=String(t); grab('transform_'+mode, t, [1,1,0]); }
  }
  if(rt.RIG.__g){ rt.RIG.poses=rt.RIG.__g.p; rt.RIG.weights=rt.RIG.__g.w; rt.resetLift&&rt.resetLift(); }
  // robot-form motions
  rt.tIn.value='1';
  for(const clip of ['idle','walk','run','jump','dash','punch']){
    const variants= clip==='jump' ? [[1,1,0],[1,1,0.4],[0.5,1,0],[1,1.8,0]] : [[1,1,0]];
    for(const v of variants){ phys(...v);
      for(let i=0;i<8;i++){ const tt=i*0.23; FAKE=tt*1000; rt.setMotion(clip,0); grab(clip, tt, v); } }
  }
  phys(1,1,0);
  fs.writeFileSync(out, JSON.stringify({veh, bones:BONES, lo, hi, stat, frames}));
  console.log(veh+': '+parts.length+' parts ('+stat.filter(s=>s.fate).length+' armour), '+frames.length+' frames -> '+(parts.length*frames.length)+' part-samples');
  process.exit(0);
})().catch(e=>{ console.log('HARVEST FAILED', e.stack); process.exit(1); });
