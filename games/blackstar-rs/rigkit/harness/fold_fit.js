// How far the folded robot pokes out of the car shell, per side.
module.exports=async function(page, ts){
  const {rt}=await require('./boot.js')(page);
  rt.setDyn(false);
  const carSet=new Set(Object.values(rt.carMeshes));
  // the car shell's bounds, from its panels at the car pose (t=0), wheels excluded
  rt.tIn.value='0'; rt.refresh(); rt.scene.updateMatrixWorld(true);
  const lo=[1e9,1e9,1e9], hi=[-1e9,-1e9,-1e9], v=new THREE.Vector3();
  for(const [g,m] of Object.entries(rt.carMeshes)){ if(g.startsWith('wheel')) continue;
    const p=m.geometry.attributes.position; for(let i=0;i<p.count;i+=7){ v.fromBufferAttribute(p,i).applyMatrix4(m.matrixWorld);
      lo[0]=Math.min(lo[0],v.x);lo[1]=Math.min(lo[1],v.y);lo[2]=Math.min(lo[2],v.z);hi[0]=Math.max(hi[0],v.x);hi[1]=Math.max(hi[1],v.y);hi[2]=Math.max(hi[2],v.z);} }
  const out=[];
  for(const t of ts){
    rt.tIn.value=String(t); rt.refresh(); rt.scene.updateMatrixWorld(true);
    const P={front:0,rear:0,top:0,bottom:0,left:0,right:0}; const who={};
    const shown=o=>{ for(let q=o;q;q=q.parent) if(!q.visible) return false; return true; };
    rt.scene.traverse(o=>{ if(!o.isMesh||carSet.has(o)||!shown(o)) return;
      const p=o.geometry.attributes.position; if(!p) return;
      const mats=o.isInstancedMesh?[...Array(o.count).keys()].map(i=>{const a=new THREE.Matrix4();o.getMatrixAt(i,a);return new THREE.Matrix4().multiplyMatrices(o.matrixWorld,a);}):[o.matrixWorld];
      for(const M of mats){ const step=Math.max(1,Math.floor(p.count/300));
        for(let i=0;i<p.count;i+=step){ v.fromBufferAttribute(p,i).applyMatrix4(M); if(!isFinite(v.x)) continue;
          const f=[['front',v.x-hi[0]],['rear',lo[0]-v.x],['top',v.y-hi[1]],['bottom',lo[1]-v.y],['right',v.z-hi[2]],['left',lo[2]-v.z]];
          for(const [k,d] of f) if(d>P[k]){ P[k]=d; who[k]=o.name||o.type; } } } });
    out.push({t,P});
  }
  return {lo,hi,out};
};
