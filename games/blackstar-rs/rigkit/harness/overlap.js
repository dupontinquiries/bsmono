// Real interpenetration at a frame: the share of A's vertices that lie inside
// B's OWN (oriented) box, both ways. World-axis boxes balloon for tilted,
// curved panels and reported overlaps that were barely touching.
module.exports=async function(page, t){
  delete require.cache[require.resolve('./boot.js')];
  const {rt}=await require('./boot.js')(page);
  rt.setDyn(false); rt.tIn.value=String(t); rt.refresh(); rt.scene.updateMatrixWorld(true);
  const P=rt.RIG.car.place;
  const arm=Object.keys(P).filter(g=>P[g].f==='armour' && rt.carMeshes[g].visible);
  const info={};
  for(const g of arm){ const m=rt.carMeshes[g]; m.geometry.computeBoundingBox();
    const inv=m.matrixWorld.clone().invert(), p=m.geometry.attributes.position, pts=[], v=new THREE.Vector3();
    const step=Math.max(1,Math.floor(p.count/400));
    for(let i=0;i<p.count;i+=step){ v.fromBufferAttribute(p,i).applyMatrix4(m.matrixWorld); pts.push(v.clone()); }
    info[g]={bb:m.geometry.boundingBox, inv, pts}; }
  const inside=(pts,o)=>{ let n=0; const v=new THREE.Vector3(); const b=o.bb, sh=0.92;   // a little inset: touching is fine
    const c=b.getCenter(new THREE.Vector3()), h=b.getSize(new THREE.Vector3()).multiplyScalar(0.5*sh);
    for(const p of pts){ v.copy(p).applyMatrix4(o.inv).sub(c); if(Math.abs(v.x)<h.x&&Math.abs(v.y)<h.y&&Math.abs(v.z)<h.z) n++; }
    return n/pts.length; };
  const pairs=[];
  for(let i=0;i<arm.length;i++) for(let j=i+1;j<arm.length;j++){
    const a=arm[i], b=arm[j], f=Math.max(inside(info[a].pts,info[b]), inside(info[b].pts,info[a]));
    if(f>0.02) pairs.push([a,b,f]); }
  return pairs.sort((x,y)=>y[2]-x[2]);
};
