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
  /* BASELINE: the car's own construction. Pieces modelled nested -- a vent in
   * its arch, glass in its frame -- overlap in the car pose by design, and the
   * first solve shoved intact car parts about from t=0.20 (glass_r jumped 10.5
   * in one step). Only overlap BEYOND the car-pose amount is a collision. */
  rt.tIn.value='0'; rt.refresh(); rt.scene.updateMatrixWorld(true);
  const base={};
  { const M0={}, I0={}; for(const g of arm){ M0[g]=rt.carMeshes[g].matrixWorld.clone(); I0[g]=M0[g].clone().invert(); }
    for(const A of arm){ base[A]={}; for(const B of arm) if(A!==B) base[A][B]=hitsIn(A,B,[0,0,0],M0[A],I0[B]); } }
  const ts=[]; for(let t=0.28;t<=0.90001;t+=0.02) ts.push(+t.toFixed(2));
  const raw={}; const report=[];
  const R=14, S=3.5;                                        // search radius and step, own-frame units
  const cands=[]; for(let x=-R;x<=R;x+=S) for(let y=-R;y<=R;y+=S) for(let z=-R;z<=R;z+=S) cands.push([x,y,z]);
  cands.sort((p,q)=>Math.hypot(...p)-Math.hypot(...q));
  for(const t of ts){
    rt.tIn.value=String(t); rt.refresh(); rt.scene.updateMatrixWorld(true);
    const vis=arm.filter(g=>rt.carMeshes[g].visible);
    const M={}, I={}, box={};
    for(const g of vis){ const m=rt.carMeshes[g]; M[g]=m.matrixWorld.clone(); I[g]=m.matrixWorld.clone().invert();
      m.geometry.computeBoundingBox(); box[g]=m.geometry.boundingBox.clone().applyMatrix4(m.matrixWorld); }
    for(const A of vis){
      // which pieces is A actually inside the MATERIAL of?
      // every piece a nudge could reach, not just the ones touched now: a
      // nudge scored only against current contacts pushed pieces into NEW
      // neighbours, and the baked page gained back what wedging removed
      const reach=box[A].clone().expandByScalar(R*2.2);
      const others=vis.filter(B=>B!==A && reach.intersectsBox(box[B]));
      if(!others.length) continue;
      const cost=off=>others.reduce((s,B)=>s+Math.max(0,hitsIn(A,B,off,M[A],I[B])-base[A][B]-0.02),0);
      const c0=cost([0,0,0]); if(c0<0.04) continue;
      /* COHERENT, not greedy: score each nudge by the material it lands in
       * PLUS how far it is from last frame's nudge. Greedy picks jumped
       * direction frame to frame -- (7,14,-3.5), then (-7,0,7), then
       * (14,0,-14) -- a twitching path whose average cleared nothing. */
      const prev=(raw[A]&&raw[A][+(t-0.02).toFixed(2)])||[0,0,0];
      const score=(o,cc)=>cc+0.006*Math.hypot(o[0]-prev[0],o[1]-prev[1],o[2]-prev[2])+0.002*Math.hypot(...o);
      let best=[0,0,0], bc=c0, bs=score(best,c0);
      for(const o of cands){ const cc=cost(o), sc=score(o,cc); if(sc<bs){ bs=sc; bc=cc; best=o; } }
      (raw[A]=raw[A]||{})[t]=best;
      report.push([t,A,others.join('+'),c0,bc,best]);
    }
  }
  // smooth over time, and ramp in and out so a piece eases into the gap
  const wedge={};
  for(const [g,byT] of Object.entries(raw)){
    const curve=ts.map(t=>byT[t]||[0,0,0]);
    const sm=curve.map((_,i)=>[0,1,2].map(k=>{ let s=0,w=0; for(let d=-3;d<=3;d++){ const j=i+d; if(j<0||j>=curve.length) continue; const ww=4-Math.abs(d); s+=curve[j][k]*ww; w+=ww; } return +(s/w).toFixed(2); }));
    wedge[g]=ts.map((t,i)=>[t,...sm[i]]).filter((r,i)=>Math.hypot(r[1],r[2],r[3])>0.05 || (sm[i-1]&&Math.hypot(...sm[i-1])>0.05) || (sm[i+1]&&Math.hypot(...sm[i+1])>0.05));
  }
  // ITERATE: when solving on a page that already has paths baked in, these
  // are residual nudges -- add them onto the existing path
  const prior=process.argv[4]?JSON.parse(fs.readFileSync(process.argv[4],'utf8')):{};
  for(const g of new Set([...Object.keys(prior),...Object.keys(wedge)])){
    const byT={}; for(const r of (prior[g]||[])) byT[r[0]]=r.slice(1);
    for(const r of (wedge[g]||[])){ const b=byT[r[0]]||[0,0,0]; byT[r[0]]=[b[0]+r[1],b[1]+r[2],b[2]+r[3]]; }
    wedge[g]=Object.keys(byT).map(Number).sort((a,b)=>a-b).map(t=>[t,...byT[t].map(v=>+v.toFixed(2))]);
  }
  fs.writeFileSync(out, JSON.stringify(wedge));
  // the path actually baked: re-measure material overlap WITH the smoothed nudges
  const at=(g,t)=>{ const c=wedge[g]; if(!c||!c.length) return [0,0,0];
    for(let i=0;i<c.length-1;i++) if(t>=c[i][0]&&t<=c[i+1][0]){ const f=(t-c[i][0])/(c[i+1][0]-c[i][0]||1); return [1,2,3].map(k=>c[i][k]+(c[i+1][k]-c[i][k])*f); }
    return (t<c[0][0]||t>c[c.length-1][0])?[0,0,0]:c[c.length-1].slice(1); };
  let before=0, after=0, n=0;
  for(const t of ts){ rt.tIn.value=String(t); rt.refresh(); rt.scene.updateMatrixWorld(true);
    const vis=arm.filter(g=>rt.carMeshes[g].visible), M={}, I={};
    for(const g of vis){ M[g]=rt.carMeshes[g].matrixWorld.clone(); I[g]=M[g].clone().invert(); }
    for(const A of Object.keys(wedge)){ if(!vis.includes(A)) continue;
      for(const B of vis){ if(B===A) continue;
        // B moves too if it has a path: fold its nudge into its matrix
        const oB=at(B,t), MB=M[B].clone().multiply(new THREE.Matrix4().makeTranslation(...oB)), IB=MB.clone().invert();
        const h0=hitsIn(A,B,[0,0,0],M[A],I[B]), h1=hitsIn(A,B,at(A,t),M[A],IB);
        if(h0>0.04){ before+=h0; after+=h1; n++; } } } }
  console.log('(post-check on previously colliding pairs only: '+(before/Math.max(1,n)*100).toFixed(0)+'% -> '+(after/Math.max(1,n)*100).toFixed(0)+'%)');
  // the table: what each collision was, and what wedging it did
  const worst={};
  for(const [t,A,Bs,c0,bc,o] of report){ const k=A+' into '+Bs; if(!worst[k]||worst[k][1]<c0) worst[k]=[t,c0,bc,o]; }
  console.log('piece into piece                 worst at   in material   after wedging   nudge');
  for(const [k,[t,c0,bc,o]] of Object.entries(worst).sort((x,y)=>y[1][1]-x[1][1]).slice(0,10))
    console.log('  '+k.padEnd(30)+t.toFixed(2).padStart(8)+(c0*100).toFixed(0).padStart(11)+'%'+(bc*100).toFixed(0).padStart(13)+'%     ('+o.map(x=>x.toFixed(1)).join(', ')+')');
  console.log(Object.keys(wedge).length+' parts get wedge paths -> '+out);
  process.exit(0);
})().catch(e=>{ console.log('WEDGE FAILED', e.stack); process.exit(1); });
