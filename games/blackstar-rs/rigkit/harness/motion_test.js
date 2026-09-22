// Headless check of the motion core, through the page's own refresh.
module.exports=async function(page, clips, opts={}){
  delete require.cache[require.resolve('./boot.js')];
  const {rt}=await require('./boot.js')(page); rt.setDyn(false);
  // a CONTROLLED clock: node's performance.now() is real time, and the test
  // runs far slower than 120 fps -- every frame's motion time was off by
  // however long the frames before it took
  let FAKE=0; Object.defineProperty(globalThis,'performance',{value:{now:()=>FAKE},configurable:true,writable:true});
  if(opts.g) rt.MOTION.P.g=opts.g; if(opts.mass) rt.MOTION.P.mass=opts.mass; if(opts.thrust!==undefined) rt.MOTION.P.thrust=opts.thrust;
  rt.tIn.value='1';
  const out={}; const v=new THREE.Vector3();
  const footLow=n=>{ const o=rt.bodyMesh[n]; const p=o.geometry.attributes.position; let lo=null;
    for(let i=0;i<p.count;i+=3){ v.fromBufferAttribute(p,i).applyMatrix4(o.matrixWorld); if(!lo||v.y<lo.y) lo=v.clone(); } return lo; };
  for(const clip of clips){
    const dt=1/120, T=opts.T||4.0; let wrapN=0, xs=null, bad=0, lowest=1e9, peak=-1e9, restY=null, slip=0, slipN=0, prev=null, x0=null, x1=null, air=0, airT=0, onAir=false;
    for(let k=0;k*dt<=T;k++){ const t=k*dt;
      FAKE=t*1000; rt.setMotion(clip, 0); rt.refresh(); rt.scene.updateMatrixWorld(true);
      for(const o of Object.values(rt.bodyMesh)) if(o.matrixWorld.elements.some(x=>!isFinite(x))) bad++;
      const L=footLow('foot_l'), R=footLow('foot_r'); if(!L||!R) continue;
      const low=Math.min(L.y,R.y); lowest=Math.min(lowest,low);
      const pel=rt.bodyMesh.pelvis.matrixWorld.elements; const py=pel[13], px=pel[12];
      if(restY===null) restY=py; peak=Math.max(peak,py);
      // unwrap the 900-unit loop back
      if(x1!==null && px-x1>400) wrapN++;
      if(x0===null) x0=px; x1=px; xs=x1-wrapN*900;
      // slip: the PLANTED foot (the lower one, on the floor) should not move horizontally
      const planted=L.y<R.y?['l',L]:['r',R];
      if(prev && prev[0]===planted[0] && planted[1].y<rt.RIG.floor+3 && Math.abs(planted[1].x-prev[1].x)<200){ slip+=Math.hypot(planted[1].x-prev[1].x, planted[1].z-prev[1].z); slipN++; }
      prev=planted;
      if(low>rt.RIG.floor+4){ if(!onAir){onAir=true;} airT+=dt; } else onAir=false;
    }
    const info=(document=>{ return null; })();
    out[clip]={bad, lowest, rise:peak-restY, slipPerFrame:slipN?slip/slipN:0, travel:x0-xs, airT};
  }
  return {out, P:rt.MOTION.P, G:{walk:rt.MOTION.gaitInfo(0.25,0.6), run:rt.MOTION.gaitInfo(1.6,0.34)}};
};
