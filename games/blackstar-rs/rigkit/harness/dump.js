// Dump what the page's own refresh() puts on screen, for rigview to draw.
//   node dump.js <page.html> <out.bin> t1,t2,...
const fs=require('fs'); const boot=require('./boot.js');
(async()=>{
  const [page,out,tl]=process.argv.slice(2);
  const ts=tl.split(',').map(Number);
  const {rt}=await boot(page);
  const name=new Map(Object.entries(rt.carMeshes).map(([g,m])=>[m,g]));
  const place=rt.RIG.car.place, cubes=rt.RIG.car.cubes!==false;
  const geos=[], gid=new Map();          // key: uuid+version
  const frames=[];
  const shown=o=>{ for(let p=o;p;p=p.parent) if(!p.visible) return false; return true; };
  if(process.env.MOTION){ globalThis.__FAKE=0; Object.defineProperty(globalThis,'performance',{value:{now:()=>globalThis.__FAKE},configurable:true,writable:true}); }
    rt.setDyn(false);
  for(const t of ts){
    // MOTION=<clip>: t is the motion clock (seconds) at the end of the
    // transform, pinned exactly -- node's performance.now() is real time
    if(process.env.THRUST && rt.MOTION) rt.MOTION.P.thrust=+process.env.THRUST;
    if(process.env.MOTION){ globalThis.__FAKE=t*1000; rt.tIn.value='1'; rt.setMotion(process.env.MOTION,0); }
    else rt.tIn.value=String(t);
    rt.refresh(); rt.scene.updateMatrixWorld(true);
    const draws=[];
    rt.scene.traverse(o=>{
      if(!o.isMesh || !shown(o)) return;
      const g=o.geometry, pa=g.attributes.position; if(!pa || pa.count===0) return;
      const key=g.uuid+':'+pa.version;
      if(!gid.has(key)){
        if(!g.attributes.normal) g.computeVertexNormals();
        gid.set(key, geos.length);
        geos.push({pos:Float32Array.from(pa.array), nrm:Float32Array.from(g.attributes.normal.array),
          cell:g.attributes.aCell?Float32Array.from(g.attributes.aCell.array):null,
          bone:g.attributes.aBone?Float32Array.from(g.attributes.aBone.array):null,
          anch:g.attributes.aAnchor?Float32Array.from(g.attributes.aAnchor.array):null,
          idx:g.index?Uint32Array.from(g.index.array):null, n:pa.count});
      }
      const gi=gid.get(key);
      const c=o.material&&o.material.color?[o.material.color.r,o.material.color.g,o.material.color.b]:[0.6,0.6,0.65];
      const gname=name.get(o), pl=gname?place[gname]:null;
      const vox=gname?(o.userData.vox||0):0;
      const stream=(pl&&pl.st&&!cubes)?1:0;
      if(o.isInstancedMesh){
        const im=new THREE.Matrix4(), w=new THREE.Matrix4();
        for(let i=0;i<o.count;i++){ o.getMatrixAt(i,im); w.multiplyMatrices(o.matrixWorld,im);
          draws.push({gi, m:Array.from(w.elements), vox:0, stream:0, c}); }
      } else draws.push({gi, m:Array.from(o.matrixWorld.elements), vox, stream, c,
                         // COPY now: the page reuses these arrays every frame, and the
                         // file is written after all frames -- a reference drew every
                         // frame with the last frame's bone motion
                         bq:o.userData.bq?o.userData.bq.map(x=>x.slice()):null,
                         bt:o.userData.bt?o.userData.bt.map(x=>x.slice()):null,
                         mo:o.userData.mo?o.userData.mo.slice():null});
    });
    frames.push({t, draws});
    console.log(`t=${t}: ${draws.length} draws`);
  }
  // binary: RVSC | ngeo nframe | geos | frames
  const parts=[]; const u32=v=>{const b=Buffer.alloc(4); b.writeUInt32LE(v); parts.push(b);};
  const f32=v=>{const b=Buffer.alloc(4); b.writeFloatLE(v); parts.push(b);};
  const arr=a=>parts.push(Buffer.from(a.buffer, a.byteOffset, a.byteLength));
  parts.push(Buffer.from('RVSC')); u32(geos.length); u32(frames.length);
  for(const g of geos){ u32(g.n); u32(g.idx?g.idx.length:0); u32((g.cell?1:0)|(g.bone?2:0));
    arr(g.pos); arr(g.nrm); if(g.cell) arr(g.cell); if(g.bone){ arr(g.bone); arr(g.anch); } if(g.idx) arr(g.idx); }
  for(const f of frames){ f32(f.t); u32(f.draws.length);
    for(const d of f.draws){ u32(d.gi); for(const v of d.m) f32(v); f32(d.vox); f32(d.stream); for(const v of d.c) f32(v);
      for(let i=0;i<22;i++){ const q=d.bq?d.bq[i]:[0,0,0,1]; for(const v of q) f32(v); }
      for(let i=0;i<22;i++){ const t=d.bt?d.bt[i]:[0,0,0,0]; for(const v of t) f32(v); }
      const mo=d.mo||[0,0,0,1,2,1,1,1]; for(const v of mo) f32(v); } }
  fs.writeFileSync(out, Buffer.concat(parts));
  console.log(`${geos.length} geometries, ${(fs.statSync(out).size/1048576).toFixed(1)} MB -> ${out}`);
  process.exit(0);
})().catch(e=>{ console.log('DUMP FAILED', e.stack); process.exit(1); });
