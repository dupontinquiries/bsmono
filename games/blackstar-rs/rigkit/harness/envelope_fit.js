// SHAPE-AWARE fold fit. A bounding box is useless for a jet: it includes the
// wings, so a robot three times wider than the fuselage "fits". The envelope
// is the vehicle's real extent per (length slice, height band): a robot point
// is outside if it reaches beyond the material at its slice and height.
module.exports=async function(page, dxs){
  delete require.cache[require.resolve('./boot.js')];
  const {rt}=await require('./boot.js')(page);
  rt.setDyn(false); rt.tIn.value='0'; rt.refresh(); rt.scene.updateMatrixWorld(true);
  const SX=12, SY=10, env=new Map(), v=new THREE.Vector3();
  const carSet=new Set(Object.values(rt.carMeshes));
  for(const m of carSet){ const p=m.geometry.attributes.position;
    for(let i=0;i<p.count;i++){ v.fromBufferAttribute(p,i).applyMatrix4(m.matrixWorld);
      const k=Math.floor(v.x/SX)+','+Math.floor(v.y/SY), cur=env.get(k)||[1e9,-1e9];
      env.set(k,[Math.min(cur[0],v.z),Math.max(cur[1],v.z)]); } }
  // robot points: every visible non-car mesh
  const pts=[];
  // the robot's BODY only: scanning every non-car mesh picked up the floor
  // shadow, which read as parts 300 units outside the jet
  for(const o of Object.values(rt.bodyMesh)){ if(!o.visible) continue;
    const p=o.geometry.attributes.position; const step=Math.max(1,Math.floor(p.count/200));
    for(let i=0;i<p.count;i+=step){ v.fromBufferAttribute(p,i).applyMatrix4(o.matrixWorld); if(isFinite(v.x)) pts.push(v.clone()); } }
  const cen=pts.reduce((a,p)=>a.add(p),new THREE.Vector3()).multiplyScalar(1/pts.length);
  const res=[];
  for(const [dx,sc] of dxs){ let out=0, far=0; const where={front:0,rear:0,side:0,vert:0};
    for(const p0 of pts){ const p=p0.clone().sub(cen).multiplyScalar(sc).add(cen); p.x+=dx;
      const k=Math.floor(p.x/SX)+','+Math.floor(p.y/SY), e=env.get(k);
      let d; if(!e){ d=30; if(p.x-dx>cen.x+100) where.front++; else if(p.x-dx<cen.x-100) where.rear++; else where.vert++; }
      else { d=Math.max(0,e[0]-p.z,p.z-e[1]); if(d>2) where.side++; }
      if(d>2){ out++; far=Math.max(far,d); } }
    res.push({dx, sc, out:out/pts.length, far, where}); }
  return {res, n:pts.length};
};
