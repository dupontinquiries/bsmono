// WEDGE SOLVER. When a moving armour piece lands in another piece's MATERIAL,
// find the small nudge (in its own frame) that seats it in that piece's GAPS
// instead -- between spokes, inside a tyre, behind a panel -- and bake the
// nudges over time. Runs on the page's own frames.
//   node wedge.js <page.html> <out.json>
const fs=require('fs');
(async()=>{
  const [page,out]=process.argv.slice(2);
  const {rt}=await require('./boot.js')(page);
  rt.setDyn(false);
  const P=rt.RIG.car.place, H=3.0;                         // occupancy cell
  const arm=Object.keys(P).filter(g=>P[g].f==='armour');
  // material: every cell a part's surface passes through, in its own frame
  const occ={}, pts={};
  const v=new THREE.Vector3(), a=new THREE.Vector3(), b=new THREE.Vector3(), c=new THREE.Vector3();
  for(const g of arm){
    const m=rt.carMeshes[g], geo=m.geometry, pos=geo.attributes.position, idx=geo.index?geo.index.array:null;
    const set=new Set(), key=(x,y,z)=>Math.floor(x/H)+','+Math.floor(y/H)+','+Math.floor(z/H);
    const nt=idx?idx.length/3:pos.count/3, samp=[];
    for(let t=0;t<nt;t++){
      a.fromBufferAttribute(pos, idx?idx[t*3]:t*3); b.fromBufferAttribute(pos, idx?idx[t*3+1]:t*3+1); c.fromBufferAttribute(pos, idx?idx[t*3+2]:t*3+2);
      // enough samples per triangle that no cell of its surface is skipped
      const L=Math.max(a.distanceTo(b),b.distanceTo(c),c.distanceTo(a)), n=Math.max(1,Math.ceil(L/(H*0.7)));
      for(let i=0;i<=n;i++) for(let j=0;j<=n-i;j++){ const u=i/n, w=j/n;
        v.set(a.x+(b.x-a.x)*u+(c.x-a.x)*w, a.y+(b.y-a.y)*u+(c.y-a.y)*w, a.z+(b.z-a.z)*u+(c.z-a.z)*w);
        set.add(key(v.x,v.y,v.z)); if(Math.random()<0.02) samp.push(v.clone()); }
    }
    occ[g]=set; pts[g]=samp.length>500?samp.filter((_,i)=>i%Math.ceil(samp.length/500)===0):samp;
  }
  const hitsIn=(A,B,off,MA,IB)=>{ let h=0; const o=new THREE.Vector3(...off);
    for(const p of pts[A]){ v.copy(p).add(o).applyMatrix4(MA).applyMatrix4(IB);
      if(occ[B].has(Math.floor(v.x/H)+','+Math.floor(v.y/H)+','+Math.floor(v.z/H))) h++; }
    return h/pts[A].length; };

  let tot=0, cnt=0, worst=[0,'',0];
  for(let t=0.20;t<=0.90001;t+=0.02){ rt.tIn.value=String(t); rt.refresh(); rt.scene.updateMatrixWorld(true);
    const vis=arm.filter(g=>rt.carMeshes[g].visible), M={}, I={}, box={};
    for(const g of vis){ const mm=rt.carMeshes[g]; M[g]=mm.matrixWorld.clone(); I[g]=M[g].clone().invert(); mm.geometry.computeBoundingBox(); box[g]=mm.geometry.boundingBox.clone().applyMatrix4(mm.matrixWorld); }
    for(const A of vis) for(const B of vis){ if(A>=B || !box[A].intersectsBox(box[B])) continue;
      const h=Math.max(hitsIn(A,B,[0,0,0],M[A],I[B]), hitsIn(B,A,[0,0,0],M[B],I[A]));
      if(h>0.04){ tot+=h; cnt++; if(h>worst[0]) worst=[h,A+' x '+B,t]; } } }
  console.log('ALL pairs, material: '+cnt+' colliding pair-frames, mean '+(tot/Math.max(1,cnt)*100).toFixed(1)+'%, total '+(tot).toFixed(2)+'   worst '+(worst[0]*100).toFixed(0)+'% '+worst[1]+' @'+worst[2].toFixed(2));
  process.exit(0);
})().catch(e=>{ console.log('MEASURE FAILED', e.stack); process.exit(1); });
