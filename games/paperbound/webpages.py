"""
HTML pages served by webhost.py. Kept out of webhost.py so the server logic
stays readable. Three pages:
  PAGE_INDEX   — landing / navigation
  PAGE_PLAY    — three.js live board viewer (spectate + tap-to-deploy)
  PAGE_CONTROL — tournament control panel (create bracket, report, advance)

The three.js client loads the library from a CDN; if that fails (pure offline
LAN), it falls back to a built-in Canvas-2D renderer so the board still shows.
"""

_STYLE = """
<style>
  :root { --ink:#2a2018; --page:#efe4cf; --wax:#b4462e; --gold:#c8a13e;
          --panel:#1c1712; --panel2:#26201a; --line:#3a3128; }
  * { box-sizing:border-box; }
  body { margin:0; font-family:Georgia,'Times New Roman',serif; background:#14100c;
         color:var(--page); -webkit-tap-highlight-color:transparent; }
  header { padding:14px 18px; background:linear-gradient(#241d16,#191410);
           border-bottom:2px solid var(--line); display:flex; align-items:center; gap:14px; }
  header h1 { margin:0; font-size:20px; letter-spacing:3px; color:var(--gold); }
  header .sub { color:#9a8c76; font-size:13px; }
  a.btn, button.btn { display:inline-block; padding:10px 16px; margin:4px;
     background:var(--panel2); color:var(--page); border:1px solid var(--line);
     border-radius:8px; text-decoration:none; font-size:15px; cursor:pointer; }
  a.btn:hover, button.btn:hover { background:#332a20; border-color:var(--gold); }
  button.btn.primary { background:var(--wax); border-color:#d06a4e; }
  .wrap { max-width:1100px; margin:0 auto; padding:18px; }
  .card { background:var(--panel); border:1px solid var(--line); border-radius:12px;
          padding:16px; margin:12px 0; }
  input, select, textarea { background:#0f0c08; color:var(--page); border:1px solid var(--line);
          border-radius:6px; padding:8px; font-family:inherit; font-size:15px; }
  label { display:block; margin:8px 0 4px; color:#c7b79b; font-size:13px; }
  .row { display:flex; gap:10px; flex-wrap:wrap; align-items:center; }
  .muted { color:#8b7d68; font-size:13px; }
  .pill { display:inline-block; padding:3px 9px; border-radius:20px; font-size:12px;
          background:#2a2118; border:1px solid var(--line); }
</style>
"""

PAGE_INDEX = """<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1">
<title>Paperbound — LAN</title>__STYLE__</head><body>
<header><h1>PAPERBOUND</h1><span class="sub">served on your local network</span></header>
<div class="wrap">
  <div class="card">
    <h2 style="margin-top:0">Welcome</h2>
    <p class="muted">This device is hosting Paperbound for everyone on the same Wi-Fi /
    LAN. Open a section below on any phone, tablet or laptop on the network.</p>
    <div class="row">
      <a class="btn primary" href="/play">▶ Spectate / Play the live board</a>
      <a class="btn" href="/control">⚑ Tournament control</a>
    </div>
    <p class="muted" id="status" style="margin-top:12px">checking for a live match…</p>
  </div>
  <script>
    async function poll(){
      try{ const r=await fetch('/api/status'); const s=await r.json();
        const el=document.getElementById('status');
        let bits=[];
        if(s.live && s.units>0){ bits.push('🟢 Live match: <b>'+(s.title||'in progress')+'</b> — '+s.units+' units on the board'); }
        else if(s.live){ bits.push('A match is set up, waiting for deploys.'); }
        if(s.result==='win') bits.push('· bottom team won'); else if(s.result==='lose') bits.push('· top team won');
        if(s.seats_taken) bits.push('· '+s.seats_taken+'/2 seats claimed');
        if(s.tournament) bits.push('· a tournament is running');
        el.innerHTML = bits.length? bits.join(' ') : 'No match running yet — open the board or start a tournament.';
      }catch(e){}
      setTimeout(poll, 2000);
    }
    poll();
  </script>
  <div class="card">
    <h3 style="margin-top:0">How this works</h3>
    <p class="muted">The match simulation runs on the host device. Your browser shows a
    live 3D view of it (three.js) and can drop cards for the bottom team by tapping the
    board. The control page runs brackets — build one, seat players, and report results
    as matches finish. The desktop/mobile app remains the full-fidelity way to play.</p>
  </div>
</div></body></html>""".replace("__STYLE__", _STYLE)


# ─────────────────────────────────────────────────────────────────────────────
#  PLAY / SPECTATE — three.js board viewer
# ─────────────────────────────────────────────────────────────────────────────
PAGE_PLAY = """<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1">
<title>Paperbound — Board</title>__STYLE__
<style>
  #stage { width:100%; height:70vh; min-height:360px; background:#0c0a07;
           border:1px solid var(--line); border-radius:12px; overflow:hidden; touch-action:none; }
  #hud { display:flex; gap:14px; align-items:center; flex-wrap:wrap; margin:10px 0; }
  .ink { font-size:15px; }
  .ink b { color:var(--gold); }
  #cards { display:flex; gap:8px; flex-wrap:wrap; }
  .mini { padding:8px 10px; border-radius:8px; border:1px solid var(--line);
          background:var(--panel2); cursor:pointer; font-size:13px; min-width:70px; text-align:center; }
  .mini.sel { border-color:var(--gold); background:#3a2f1f; }
  .mini .c { color:var(--gold); font-weight:bold; }
  #banner { font-size:20px; color:var(--gold); min-height:26px; }
</style></head><body>
<header><h1>PAPERBOUND</h1><span class="sub" id="title">live board</span>
  <a class="btn" href="/" style="margin-left:auto">← home</a></header>
<div class="wrap">
  <div id="banner"></div>
  <div id="stage"></div>
  <div id="hud">
    <div class="ink">Bottom ink: <b id="ink">0</b></div>
    <div class="ink">Top ink: <b id="aiink">0</b></div>
    <button class="btn" id="claim" onclick="claimSeat()">Claim a seat to play</button>
    <span class="pill" id="seatlbl">spectating</span>
    <span class="pill" id="watching">👁 …</span>
    <button class="btn" id="mute" onclick="toggleMute()" style="padding:6px 10px">🔊</button>
    <span class="muted">tap a card, then tap your half of the board to deploy</span>
  </div>
  <div id="cards"></div>
</div>
<script>
const GEO_DEFAULT = {left:330,top:36,right:950,bottom:676,w:620,h:640,river:356};
let geo = GEO_DEFAULT, selected = null, three = null, use3d = false;
// head-to-head seat: myTeam is null when spectating, 0=bottom, 1=top
let myTeam = null;
const TOKEN = 'pb-'+Math.random().toString(36).slice(2)+Date.now();
async function claimSeat(){
  try{
    const r = await fetch('/api/game/seat',{method:'POST',headers:{'Content-Type':'application/json'},
      body:JSON.stringify({token:TOKEN})});
    const j = await r.json();
    if(j.ok && j.team!==null && j.team!==undefined){
      myTeam = j.team;
      document.getElementById('seatlbl').textContent = myTeam===0?'you: BOTTOM':'you: TOP';
      document.getElementById('claim').style.display='none';
    } else {
      document.getElementById('seatlbl').textContent = 'both seats taken — spectating';
    }
  }catch(e){}
}

// ── card palette (fetched from server) ──
let cards = [];
async function loadCards(){
  try{ const r = await fetch('/api/cards'); const j = await r.json();
    if(j.ok){ cards = j.cards; renderCards(); } }catch(e){}
}
function renderCards(){
  const box = document.getElementById('cards'); box.innerHTML='';
  cards.forEach(c=>{
    const d = document.createElement('div'); d.className='mini'+(selected===c.key?' sel':'');
    d.innerHTML = c.name+'<br><span class="c">'+c.cost+'</span>';
    d.onclick = ()=>{ selected = (selected===c.key?null:c.key); renderCards(); };
    box.appendChild(d);
  });
}

// ── world→screen helpers shared by both renderers ──
function boardToNorm(x,y){ return [(x-geo.left)/geo.w, (y-geo.top)/geo.h]; }

// ── try three.js; fall back to canvas ──
function initThree(){
  return new Promise((resolve)=>{
    const s = document.createElement('script');
    s.src = 'https://cdnjs.cloudflare.com/ajax/libs/three.js/r128/three.min.js';
    s.onload = ()=>resolve(true); s.onerror = ()=>resolve(false);
    document.head.appendChild(s);
  });
}

let scene, camera, renderer, unitMeshes=[], towerMeshes=[], boardMesh;

// map the 21 card families to folded-paper silhouettes (mirrors the native recipes)
const ARCH = {
  bird:'wing', wave:'wing',
  feline:'cat', canine:'dog', mustelid:'weasel',
  ape:'ape', tusker:'beast', rabbit:'beast', armored:'beast',
  fish:'fish', cephalopod:'fish', jelly:'fish', amphibian:'fish', shelled:'fish',
  insect:'bug',
  tree:'tower', monolith:'tower', golem:'tower',
  blob:'blob', wisp:'wisp', figure:'figure'
};
// family idle-motion flavour (mirrors the native _ANIM_STYLE)
const ARCH_STYLE = {
  cephalopod:'undulate', wave:'undulate', jelly:'pulse', amphibian:'hop',
  wisp:'flicker', blob:'wobble', insect:'skitter', fish:'swim'
};

// build a low-poly, flat-shaded folded-paper mesh for a unit
function makePaperMesh(fam, size, colInt, fly){
  const arch = ARCH[fam] || 'figure';
  const s = Math.max(8, size);
  const mat = new THREE.MeshStandardMaterial({color:colInt, roughness:0.9,
                metalness:0.0, flatShading:true, side:THREE.DoubleSide});
  const grp = new THREE.Group();
  const body = (geo, y, ry) => { const m=new THREE.Mesh(geo, mat);
        m.position.y=y; if(ry)m.rotation.y=ry; grp.add(m); return m; };
  if(arch==='wing'){
    body(new THREE.ConeGeometry(s*0.32, s*1.5, 4), s*0.75, Math.PI/4);
    const wing = new THREE.BufferGeometry();
    const w = s*1.5, d = s*0.7;
    wing.setAttribute('position', new THREE.Float32BufferAttribute(
      [0,0,0,  w,0,-d*0.3,  w*0.3,0,d], 3));
    wing.computeVertexNormals();
    const lw=new THREE.Mesh(wing, mat); lw.position.set(0,s*0.7,0); grp.add(lw);
    const rw=new THREE.Mesh(wing, mat); rw.position.set(0,s*0.7,0); rw.scale.x=-1; grp.add(rw);
  } else if(arch==='beast'){
    body(new THREE.OctahedronGeometry(s*0.7,0), s*0.8).scale.set(1.3,0.8,1.0);
    for(const sx of [-1,1]) for(const sz of [-1,1]){
      const leg=new THREE.Mesh(new THREE.ConeGeometry(s*0.12,s*0.7,3), mat);
      leg.position.set(sx*s*0.4, s*0.32, sz*s*0.3); leg.rotation.x=Math.PI; grp.add(leg);
    }
  } else if(arch==='cat'){
    // sleek body, triangular ears, long curved tail
    const b=body(new THREE.OctahedronGeometry(s*0.6,0), s*0.6); b.scale.set(1.5,0.6,0.8);
    const head=body(new THREE.OctahedronGeometry(s*0.32,0), s*0.7); head.position.x=s*0.7;
    for(const sx of [-1,1]){ const ear=new THREE.Mesh(new THREE.ConeGeometry(s*0.1,s*0.25,3),mat);
      ear.position.set(s*0.7+sx*s*0.08, s*0.95, sx*s*0.12); grp.add(ear); }
    for(const sx of [-1,1]) for(const sz of [-1,1]){ const leg=new THREE.Mesh(new THREE.ConeGeometry(s*0.09,s*0.55,3),mat);
      leg.position.set(sx*s*0.35, s*0.26, sz*s*0.22); leg.rotation.x=Math.PI; grp.add(leg); }
    const tail=new THREE.Mesh(new THREE.ConeGeometry(s*0.08,s*1.0,3),mat);
    tail.position.set(-s*0.8, s*0.7, 0); tail.rotation.z=-0.7; grp.add(tail);
  } else if(arch==='dog'){
    // pointed snout, alert ear, bushy tail up
    const b=body(new THREE.OctahedronGeometry(s*0.62,0), s*0.65); b.scale.set(1.4,0.7,0.85);
    const snout=new THREE.Mesh(new THREE.ConeGeometry(s*0.25,s*0.7,4),mat);
    snout.position.set(s*0.8,s*0.7,0); snout.rotation.z=-Math.PI/2; grp.add(snout);
    const ear=new THREE.Mesh(new THREE.ConeGeometry(s*0.11,s*0.3,3),mat); ear.position.set(s*0.55,s*1.0,0); grp.add(ear);
    for(const sx of [-1,1]) for(const sz of [-1,1]){ const leg=new THREE.Mesh(new THREE.ConeGeometry(s*0.1,s*0.6,3),mat);
      leg.position.set(sx*s*0.35, s*0.28, sz*s*0.25); leg.rotation.x=Math.PI; grp.add(leg); }
    const tail=new THREE.Mesh(new THREE.ConeGeometry(s*0.22,s*0.6,4),mat);
    tail.position.set(-s*0.75, s*0.95, 0); tail.rotation.z=0.6; grp.add(tail);
  } else if(arch==='weasel'){
    // long low tube body, stub legs, round ears (r128-safe geometry only)
    const b=body(new THREE.CylinderGeometry(s*0.35,s*0.35,s*1.4,6), s*0.4);
    b.rotation.z=Math.PI/2; b.scale.set(1,1,0.8);
    const head=body(new THREE.OctahedronGeometry(s*0.3,0), s*0.45); head.position.x=s*0.85;
    for(const sx of [-1,1]){ const ear=new THREE.Mesh(new THREE.SphereGeometry(s*0.1,6,5),mat);
      ear.position.set(s*0.85, s*0.7, sx*s*0.15); grp.add(ear); }
    for(const sx of [-1,1]){ const leg=new THREE.Mesh(new THREE.ConeGeometry(s*0.08,s*0.35,3),mat);
      leg.position.set(sx*s*0.4, s*0.16, 0); leg.rotation.x=Math.PI; grp.add(leg); }
  } else if(arch==='ape'){
    // broad torso, long low arms, small head
    body(new THREE.OctahedronGeometry(s*0.75,0), s*0.85).scale.set(1.1,1.2,0.9);
    for(const sx of [-1,1]){ const arm=new THREE.Mesh(new THREE.CylinderGeometry(s*0.15,s*0.12,s*1.0,4),mat);
      arm.position.set(sx*s*0.7, s*0.6, 0); arm.rotation.z=sx*0.3; grp.add(arm); }
    const head=body(new THREE.OctahedronGeometry(s*0.3,0), s*1.5);
  } else if(arch==='figure'){
    // robed humanoid: tapering body, hood, sleeves
    body(new THREE.CylinderGeometry(s*0.28,s*0.55,s*1.4,5), s*0.75);
    const hood=new THREE.Mesh(new THREE.ConeGeometry(s*0.4,s*0.6,5),mat); hood.position.y=s*1.55; grp.add(hood);
    for(const sx of [-1,1]){ const sl=new THREE.Mesh(new THREE.ConeGeometry(s*0.16,s*0.7,4),mat);
      sl.position.set(sx*s*0.45, s*0.8, 0); sl.rotation.z=sx*1.2; grp.add(sl); }
  } else if(arch==='wisp'){
    // ethereal floating flame, no ground contact
    const flame=new THREE.Mesh(new THREE.ConeGeometry(s*0.55,s*1.5,6),
      new THREE.MeshStandardMaterial({color:colInt, roughness:0.5, emissive:colInt, emissiveIntensity:0.3, flatShading:true}));
    flame.position.y=s*1.1; grp.add(flame);
    const core=new THREE.Mesh(new THREE.SphereGeometry(s*0.25,6,6),
      new THREE.MeshStandardMaterial({color:0xfff0c0, emissive:0xffd080, emissiveIntensity:0.6}));
    core.position.y=s*0.9; grp.add(core);
    grp.userData.float=true;   // gentle bob applied in animate
  } else if(arch==='fish'){
    const b=body(new THREE.OctahedronGeometry(s*0.7,0), s*0.55); b.scale.set(1.4,0.6,0.9);
    const tail=new THREE.Mesh(new THREE.ConeGeometry(s*0.5,s*0.7,3), mat);
    tail.position.set(-s*0.9, s*0.55, 0); tail.rotation.z=Math.PI/2; grp.add(tail);
  } else if(arch==='bug'){
    body(new THREE.OctahedronGeometry(s*0.55,0), s*0.6).scale.set(1,0.7,1.3);
    for(const sx of [-1,1]){ const a=new THREE.Mesh(new THREE.ConeGeometry(s*0.05,s*0.6,3),mat);
      a.position.set(sx*s*0.2, s*1.0, s*0.5); a.rotation.x=-0.5; grp.add(a); }
  } else if(arch==='tower'){
    body(new THREE.CylinderGeometry(s*0.5,s*0.75,s*1.6,5), s*0.8);
    body(new THREE.ConeGeometry(s*0.6,s*0.7,5), s*1.8);
  } else if(arch==='blob'){
    const b=body(new THREE.IcosahedronGeometry(s*0.7,0), s*0.7); b.scale.set(1,0.85,1);
  } else {
    body(new THREE.CylinderGeometry(s*0.25,s*0.4,s*1.2,5), s*0.7);
    body(new THREE.OctahedronGeometry(s*0.4,0), s*1.4);
  }
  if(fly){ grp.position.y = 50; }
  return grp;
}
function setup3d(){
  const stage = document.getElementById('stage');
  scene = new THREE.Scene(); scene.background = new THREE.Color(0x0c0a07);
  const asp = stage.clientWidth/stage.clientHeight;
  camera = new THREE.PerspectiveCamera(46, asp, 0.1, 4000);
  camera.position.set(0, 620, 560); camera.lookAt(0, 0, 0);
  renderer = new THREE.WebGLRenderer({antialias:true});
  renderer.setSize(stage.clientWidth, stage.clientHeight);
  renderer.setPixelRatio(Math.min(2, window.devicePixelRatio||1));
  stage.appendChild(renderer.domElement);
  scene.add(new THREE.AmbientLight(0xffe9c0, 0.7));
  const key = new THREE.PointLight(0xffca80, 1.1, 3000); key.position.set(300,700,200); scene.add(key);
  // the paper board (a plane sized to the play area)
  const bw = geo.w, bh = geo.h;
  const geoP = new THREE.PlaneGeometry(bw, bh);
  const matP = new THREE.MeshStandardMaterial({color:0xd9cbb0, roughness:0.95});
  boardMesh = new THREE.Mesh(geoP, matP); boardMesh.rotation.x = -Math.PI/2; scene.add(boardMesh);
  // the ink river (a darker strip across the middle)
  const riv = new THREE.Mesh(new THREE.PlaneGeometry(bw, 60),
                new THREE.MeshStandardMaterial({color:0x2a3550, roughness:0.4}));
  riv.rotation.x = -Math.PI/2; riv.position.set(0, 1, (geo.river-(geo.top+bh/2)));
  scene.add(riv);
  renderer.domElement.addEventListener('pointerdown', on3dTap);
  animate3d();
}
function worldToLocal(x,y){ // board world (px) → three plane coords centred at 0
  return [ x-(geo.left+geo.w/2), y-(geo.top+geo.h/2) ];
}
// ── persistent, interpolated, animated units ─────────────────────────────────
function rgbInt(c){ return (c&&c.length===3)?(c[0]<<16|c[1]<<8|c[2]):0xb09060; }
// Snapshots arrive ~10Hz but we render at 60fps, so each unit mesh persists
// across snapshots and we tween its position between the last two. Walk-bob and
// attack-lunge are procedural, driven by how fast it's actually moving and a
// fire flag from the server — the same feel the native client has.
const uState = new Map();   // id -> {mesh, x0,z0, x1,z1, tSnap, fam, col, size, fly, team, face, fire, walk, seen}
let lastSnapT = 0, snapDT = 0.1;

function animate3d(){
  if(!use3d) return; requestAnimationFrame(animate3d);
  const now = performance.now()*0.001;
  const tw = snapDT>0 ? Math.min(1.4,(now-lastSnapT)/snapDT) : 1;  // interp factor
  uState.forEach(u=>{
    if(!u.mesh) return;
    // interpolate world position between the last two snapshots
    const ix = u.x0 + (u.x1-u.x0)*Math.min(1,tw);
    const iz = u.z0 + (u.z1-u.z0)*Math.min(1,tw);
    u.mesh.position.x = ix; u.mesh.position.z = iz;
    // measured speed drives the walk cycle
    const moved = Math.hypot(u.x1-u.x0, u.z1-u.z0);
    u.walk = u.walk*0.85 + Math.min(1,moved/6)*0.15;
    const wob = Math.sin(now*10 + u.phase)*u.walk;
    // base height: flyers hover (with a slow bob), ground units bob on the walk
    let baseY = u.fly ? (50 + Math.sin(now*3+u.phase)*6) : Math.abs(wob)*u.size*0.18;
    // family idle flavour so a standing unit still has life (mirrors native)
    const style = ARCH_STYLE[u.fam] || '';
    let sqz = 1, sway = 0;
    if(style==='pulse'){ sqz = 1 + Math.sin(now*3+u.phase)*0.12; }          // jellies
    else if(style==='undulate'){ sway = Math.sin(now*3.5+u.phase)*0.12; baseY += Math.sin(now*4+u.phase)*u.size*0.1; }
    else if(style==='flicker'){ baseY += Math.sin(now*9+u.phase)*u.size*0.06; sway = Math.sin(now*8+u.phase)*0.06; }
    else if(style==='wobble'){ sway = Math.sin(now*2.2+u.phase)*0.08; }
    else if(style==='skitter'){ baseY += Math.abs(Math.sin(now*16+u.phase))*u.size*0.03; }
    u.mesh.position.y = baseY;
    u.mesh.scale.y = sqz;
    // a gentle body roll while walking, plus family sway & face direction
    u.mesh.rotation.z = wob*0.15 + sway;
    u.mesh.rotation.y = (u.team===0?0:Math.PI) + (u.face<0?0.0:0.0);
    // attack lunge: a quick forward jab toward the enemy when firing
    if(u.fireT>0){
      u.fireT = Math.max(0, u.fireT - (now-(u._lt||now)));
      const lp = Math.sin((1-u.fireT/0.25)*Math.PI);   // 0→1→0 over 0.25s
      const dir = (u.team===0)? -1 : 1;                // toward the far side
      u.mesh.position.z += lp*u.size*0.5*dir;
      u.mesh.scale.setScalar(1 + lp*0.12);             // a little anticipation pop
    } else {
      u.mesh.scale.setScalar(1);
    }
    u._lt = now;
  });
  // fade & flatten dying corpses (persisted separately)
  dyingMeshes.forEach(m=>{ if(m.userData.float){ m.position.y=50+Math.sin(now*3+m.position.x*0.05)*6; } });
  renderer.render(scene,camera);
}

let dyingMeshes=[];
function sync3d(snap){
  const now = performance.now()*0.001;
  snapDT = Math.max(0.03, now-lastSnapT); lastSnapT = now;
  // ── units: persist by id, tween position, refresh cheap state ──
  const live = new Set();
  (snap.units||[]).forEach(u=>{
    live.add(u.id);
    const [lx,lz]=worldToLocal(u.x,u.y);
    let st = uState.get(u.id);
    if(!st || st.fam!==(u.fam||'figure') || st.size!==u.size){
      // (re)build the mesh when new or its shape-defining fields changed
      if(st && st.mesh) scene.remove(st.mesh);
      const mesh=makePaperMesh(u.fam||'figure', u.size, rgbInt(u.col), u.fly);
      scene.add(mesh);
      st={mesh, x0:lx,z0:lz, x1:lx,z1:lz, phase:(u.id*1.7)%6.28,
          fam:u.fam||'figure', size:u.size, fly:u.fly, walk:0, fireT:0};
      uState.set(u.id, st);
    }
    // shift the tween window: last target becomes the new origin
    st.x0=st.mesh.position.x; st.z0=st.mesh.position.z;
    st.x1=lx; st.z1=lz;
    st.team=u.team; st.face=u.face; st.fly=u.fly;
    if(u.fire && st.fireT<=0) st.fireT=0.25;   // trigger a lunge on a fresh fire
  });
  // retire meshes for units that died / left
  uState.forEach((st,id)=>{ if(!live.has(id)){ if(st.mesh) scene.remove(st.mesh); uState.delete(id); } });
  // ── towers (rebuilt each frame; they don't move) ──
  towerMeshes.forEach(m=>scene.remove(m)); towerMeshes=[];
  dyingMeshes.forEach(m=>scene.remove(m)); dyingMeshes=[];
  (snap.towers||[]).forEach(t=>{
    const [lx,lz]=worldToLocal(t.x,t.y);
    const g=new THREE.Group();
    const base=new THREE.Mesh(new THREE.CylinderGeometry(24,30,40,6),
      new THREE.MeshStandardMaterial({color: t.team===0?0x53627f:0x7f4a4a, roughness:0.9, flatShading:true}));
    base.position.y=20; g.add(base);
    const roof=new THREE.Mesh(new THREE.ConeGeometry(30,34,6),
      new THREE.MeshStandardMaterial({color: t.team===0?0x6c86c6:0xc65a5a, roughness:0.85, flatShading:true}));
    roof.position.y=54; g.add(roof);
    g.position.set(lx,0,lz); scene.add(g); towerMeshes.push(g);
    const frac=Math.max(0,t.hp/t.max);
    if(frac<0.999){
      const ring=new THREE.Mesh(new THREE.RingGeometry(30,36, 16, 1, 0, frac*6.2832),
        new THREE.MeshBasicMaterial({color:0xd8b45a, side:THREE.DoubleSide}));
      ring.rotation.x=-Math.PI/2; ring.position.set(lx,74,lz); scene.add(ring); towerMeshes.push(ring);
    }
  });
  // ── dying corpses: crush toward the page (squash + shrink + sink + fade) ──
  (snap.dying||[]).forEach(d=>{
    const [lx,lz]=worldToLocal(d.x,d.y);
    const p=d.p||0;
    const mesh=makePaperMesh(d.fam||'figure', d.size*(1-0.4*p), rgbInt(d.col), false);
    mesh.position.set(lx,0,lz);
    mesh.scale.y=Math.max(0.05,1-0.85*p);
    mesh.rotation.z=(d.tip||1)*p*0.9;
    mesh.rotation.y=(d.team===0)?0:Math.PI;
    const a=Math.max(0,1-Math.max(0,(p-0.4)/0.6));
    mesh.traverse(o=>{ if(o.material){ o.material.transparent=true; o.material.opacity=a; } });
    scene.add(mesh); dyingMeshes.push(mesh);
  });
}

function on3dTap(ev){
  if(!selected) return;
  const rect = renderer.domElement.getBoundingClientRect();
  const nx = ((ev.clientX-rect.left)/rect.width)*2-1;
  const ny = -((ev.clientY-rect.top)/rect.height)*2+1;
  const ray = new THREE.Raycaster(); ray.setFromCamera({x:nx,y:ny}, camera);
  const hit = ray.intersectObject(boardMesh);
  if(hit.length){
    const p = hit[0].point;
    const wx = p.x + (geo.left+geo.w/2), wy = p.z + (geo.top+geo.h/2);
    deploy(selected, wx, wy);
  }
}

// ── canvas-2d fallback renderer (interpolated + animated, mirrors the 3d path) ──
let ctx2d=null, cv=null, snap2d=null;
const u2 = new Map();          // id -> {x,z, x0,z0, x1,z1, walk, fireT, ...}
let last2dT=0, snap2dDT=0.1;
function setup2d(){
  const stage=document.getElementById('stage'); stage.innerHTML='';
  cv=document.createElement('canvas'); cv.style.width='100%'; cv.style.height='100%';
  stage.appendChild(cv); ctx2d=cv.getContext('2d');
  cv.addEventListener('pointerdown', on2dTap);
  resize2d(); window.addEventListener('resize', resize2d);
  animate2d();               // its own 60fps loop so motion is smooth
}
function resize2d(){ if(!cv) return; cv.width=cv.clientWidth; cv.height=cv.clientHeight; }
function sync2d(snap){
  const now=performance.now()*0.001;
  snap2dDT=Math.max(0.03, now-last2dT); last2dT=now; snap2d=snap;
  const live=new Set();
  (snap.units||[]).forEach(u=>{ live.add(u.id);
    let st=u2.get(u.id);
    if(!st){ st={x:u.x,z:u.y, x0:u.x,z0:u.y, x1:u.x,z1:u.y, walk:0, fireT:0, phase:(u.id*1.7)%6.28}; u2.set(u.id,st); }
    st.x0=st.x; st.z0=st.z; st.x1=u.x; st.z1=u.y;
    st.team=u.team; st.size=u.size; st.col=u.col; st.fly=u.fly; st.hp=u.hp; st.max=u.max;
    if(u.fire && st.fireT<=0) st.fireT=0.25;
  });
  u2.forEach((st,id)=>{ if(!live.has(id)) u2.delete(id); });
}
function animate2d(){
  if(use3d) return; requestAnimationFrame(animate2d);
  if(!ctx2d||!snap2d){ return; }
  const now=performance.now()*0.001;
  const tw=snap2dDT>0?Math.min(1.4,(now-last2dT)/snap2dDT):1;
  const W=cv.width,H=cv.height;
  const sx=W/geo.w, sy=H/geo.h, S=(x,y)=>[(x-geo.left)*sx,(y-geo.top)*sy];
  ctx2d.fillStyle='#d9cbb0'; ctx2d.fillRect(0,0,W,H);
  const [,ry]=S(geo.left,geo.river); ctx2d.fillStyle='#2a3550'; ctx2d.fillRect(0,ry-30*sy,W,60*sy);
  (snap2d.towers||[]).forEach(t=>{ const [px,py]=S(t.x,t.y);
    ctx2d.fillStyle=t.team===0?'#6c86c6':'#c65a5a';
    ctx2d.beginPath(); ctx2d.arc(px,py,16,0,7); ctx2d.fill(); });
  // interpolated, walking, lunging units
  u2.forEach(st=>{
    st.x=st.x0+(st.x1-st.x0)*Math.min(1,tw); st.z=st.z0+(st.z1-st.z0)*Math.min(1,tw);
    const moved=Math.hypot(st.x1-st.x0,st.z1-st.z0);
    st.walk=st.walk*0.85+Math.min(1,moved/6)*0.15;
    let dz=0;
    if(st.fireT>0){ st.fireT=Math.max(0,st.fireT-(now-(st._lt||now)));
      const lp=Math.sin((1-st.fireT/0.25)*Math.PI); dz=(st.team===0?-1:1)*lp*st.size*0.5; }
    st._lt=now;
    const bob=Math.abs(Math.sin(now*10+st.phase))*st.walk*st.size*0.25;
    const [px,py]=S(st.x, st.z+dz);
    const c=st.col&&st.col.length===3?`rgb(${st.col[0]},${st.col[1]},${st.col[2]})`:'#b09060';
    ctx2d.fillStyle=c; const r=Math.max(4,st.size*0.5*sx);
    ctx2d.beginPath(); ctx2d.arc(px,py-bob,r,0,7); ctx2d.fill();
    ctx2d.strokeStyle='rgba(0,0,0,.5)'; ctx2d.stroke();
    // wounded pip
    if(st.hp<st.max){ const f=Math.max(0,st.hp/st.max);
      ctx2d.fillStyle=st.team===0?'#8fd0f0':'#f0a080';
      ctx2d.fillRect(px-r, py-bob-r-6, r*2*f, 3); }
  });
  // dying corpses squash + fade
  (snap2d.dying||[]).forEach(d=>{ const [px,py]=S(d.x,d.y); const p=d.p||0;
    const c=d.col&&d.col.length===3?`rgba(${d.col[0]},${d.col[1]},${d.col[2]},${Math.max(0,1-p)})`:'rgba(176,144,96,.5)';
    ctx2d.fillStyle=c; const r=Math.max(3,d.size*0.5*sx*(1-0.5*p));
    ctx2d.beginPath(); ctx2d.ellipse(px,py,r,r*(1-0.7*p),0,0,7); ctx2d.fill(); });
}
function on2dTap(ev){
  if(!selected) return; const rect=cv.getBoundingClientRect();
  const wx=geo.left+((ev.clientX-rect.left)/rect.width)*geo.w;
  const wy=geo.top+((ev.clientY-rect.top)/rect.height)*geo.h;
  deploy(selected, wx, wy);
}

async function deploy(key,x,y){
  const team = (myTeam===null)?0:myTeam;    // spectators drop for bottom (casual)
  // clamp to the player's own half of the page (can't deploy past the river)
  if(team===0 && y < geo.river+10) y = geo.river+10;
  if(team===1 && y > geo.river-10) y = geo.river-10;
  try{ await fetch('/api/game/deploy',{method:'POST',headers:{'Content-Type':'application/json'},
    body:JSON.stringify({key:key,x:x,y:y,team:team})}); }catch(e){}
}

// ── browser sound cues (Web Audio, synthesized — no asset files needed) ──
let audioCtx=null, muted=(localStorage.getItem('pb_mute')==='1');
function toggleMute(){
  muted=!muted; localStorage.setItem('pb_mute', muted?'1':'0');
  document.getElementById('mute').textContent = muted?'🔇':'🔊';
}
function blip(freq, dur, type, gain){
  if(muted) return;
  try{
    if(!audioCtx) audioCtx=new (window.AudioContext||window.webkitAudioContext)();
    const o=audioCtx.createOscillator(), g=audioCtx.createGain();
    o.type=type||'sine'; o.frequency.value=freq;
    g.gain.value=0; o.connect(g); g.connect(audioCtx.destination);
    const t=audioCtx.currentTime;
    g.gain.setValueAtTime(0,t); g.gain.linearRampToValueAtTime(gain||0.15, t+0.01);
    g.gain.exponentialRampToValueAtTime(0.0001, t+(dur||0.15));
    o.start(t); o.stop(t+(dur||0.15)+0.02);
  }catch(e){}
}
const SND = {
  deploy: ()=>blip(330,0.12,'triangle',0.12),
  death:  ()=>{ blip(160,0.18,'sawtooth',0.10); setTimeout(()=>blip(110,0.16,'sawtooth',0.08),40); },
  win:    ()=>{ [523,659,784].forEach((f,i)=>setTimeout(()=>blip(f,0.22,'triangle',0.16), i*120)); },
  lose:   ()=>{ [392,330,262].forEach((f,i)=>setTimeout(()=>blip(f,0.24,'sine',0.14), i*140)); },
};
// event detection by diffing snapshots
let prevUnits=0, prevDying=0, prevResult=null;
function soundFromSnapshot(s){
  const nu=(s.units||[]).length, nd=(s.dying||[]).length;
  if(nu>prevUnits) SND.deploy();                 // a unit appeared → deploy
  if(nd>prevDying) SND.death();                  // a new corpse → death thud
  if(s.result && s.result!==prevResult){
    if(s.result==='win') SND.win(); else if(s.result==='lose') SND.lose();
  }
  prevUnits=nu; prevDying=nd; prevResult=s.result;
}

// ── poll loop ──
async function poll(){
  try{
    const r = await fetch('/state?t='+encodeURIComponent(TOKEN)); const s = await r.json();
    if(s.ok){
      if(s.geo) geo = s.geo;
      document.getElementById('ink').textContent = s.ink;
      document.getElementById('aiink').textContent = s.aiink;
      document.getElementById('title').textContent = s.title||'live board';
      if(s.event){ const b=document.getElementById('banner');
        if(!b.dataset.ev){ b.dataset.ev='1'; }
        document.getElementById('title').textContent = (s.event.emoji||'')+' '+(s.title||'live board'); }
      if(s.watching!==undefined) document.getElementById('watching').textContent = '👁 '+s.watching;
      document.getElementById('banner').textContent =
        s.result==='win'?'★ Bottom team wins ★':s.result==='lose'?'Top team wins':'';
      soundFromSnapshot(s);
      if(use3d) sync3d(s); else sync2d(s);
    }
  }catch(e){}
  setTimeout(poll, 100);
}

(async ()=>{
  document.getElementById('mute').textContent = muted?'🔇':'🔊';
  await loadCards();
  // fetch geo once up front so the board is sized right
  try{ const r=await fetch('/state'); const s=await r.json(); if(s.geo) geo=s.geo; }catch(e){}
  const ok = await initThree();
  if(ok && window.THREE){ use3d=true; setup3d(); } else { use3d=false; setup2d(); }
  poll();
})();
</script></body></html>""".replace("__STYLE__", _STYLE)


# ─────────────────────────────────────────────────────────────────────────────
#  CONTROL — tournament organiser panel
# ─────────────────────────────────────────────────────────────────────────────
PAGE_CONTROL = """<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1">
<title>Paperbound — Tournament Control</title>__STYLE__
<style>
  .bracket { display:flex; gap:22px; overflow-x:auto; padding:8px 2px; }
  .round { min-width:200px; }
  .round h4 { color:var(--gold); margin:6px 0; font-size:14px; letter-spacing:1px; }
  .match { background:var(--panel2); border:1px solid var(--line); border-radius:8px;
           padding:8px; margin:8px 0; }
  .slot { display:flex; justify-content:space-between; align-items:center; padding:5px 6px;
          border-radius:5px; cursor:pointer; }
  .slot:hover { background:#33291e; }
  .slot.win { background:#2b3a24; color:#cfe6b6; }
  .slot.bye { opacity:.5; cursor:default; }
  .vs { text-align:center; color:#6f6250; font-size:11px; }
  .champ { color:var(--gold); font-size:18px; }
  table { border-collapse:collapse; width:100%; }
  td,th { border:1px solid var(--line); padding:6px 10px; text-align:left; font-size:14px; }
  th { color:#c7b79b; }
</style></head><body>
<header><h1>PAPERBOUND</h1><span class="sub">tournament control</span>
  <a class="btn" href="/" style="margin-left:auto">← home</a></header>
<div class="wrap">
  <div class="card">
    <h3 style="margin-top:0">New tournament</h3>
    <label>Players (one per line)</label>
    <textarea id="players" rows="5" style="width:100%" placeholder="Ana&#10;Bo&#10;Cy&#10;Di"></textarea>
    <div class="row">
      <div><label>Format</label>
        <select id="style">
          <option value="single">Single elimination</option>
          <option value="double">Double elimination</option>
          <option value="roundrobin">Round robin</option>
          <option value="swiss">Swiss</option>
        </select></div>
      <div><label>Seeding</label>
        <select id="seed_style">
          <option value="as_entered">As entered</option>
          <option value="random">Random</option>
        </select></div>
      <div><label>Arena (for cast matches)</label>
        <select id="arena">
          <option value="study">Binder's Study</option>
          <option value="volcano">Volcano</option>
          <option value="clockwork">Clockwork</option>
          <option value="moth">Moth Hour</option>
          <option value="frost">Frost</option>
        </select></div>
      <div><label>&nbsp;</label>
        <button class="btn primary" onclick="createT()">Build bracket</button></div>
    </div>
    <p class="muted" id="err"></p>
  </div>

  <div class="card" id="live" style="display:none">
    <div class="row" style="justify-content:space-between">
      <h3 style="margin:0">Bracket</h3>
      <div id="champ" class="champ"></div>
    </div>
    <div id="board"></div>
    <div id="standings"></div>
  </div>
</div>
<script>
let T = null;
async function createT(){
  const players = document.getElementById('players').value.split('\\n');
  const style = document.getElementById('style').value;
  const seed_style = document.getElementById('seed_style').value;
  const r = await fetch('/api/tournament/create',{method:'POST',
    headers:{'Content-Type':'application/json'},
    body:JSON.stringify({players:players, style:style, seed_style:seed_style})});
  const j = await r.json();
  if(!j.ok){ document.getElementById('err').textContent = j.err||'error'; return; }
  document.getElementById('err').textContent='';
  T=j; render();
}
async function report(match, winner){
  if(!winner) return;
  const r = await fetch('/api/tournament/report',{method:'POST',
    headers:{'Content-Type':'application/json'},
    body:JSON.stringify({match:match, winner:winner})});
  const j = await r.json(); if(j.ok){ T=j; render(); }
}
function render(){
  if(!T||!T.active){ document.getElementById('live').style.display='none'; return; }
  document.getElementById('live').style.display='block';
  document.getElementById('champ').textContent = T.champion?('★ Champion: '+T.champion):'';
  // bracket
  const board = document.getElementById('board');
  if(T.style==='roundrobin'||T.style==='swiss'){
    board.className=''; board.innerHTML = matchesFlat();
  } else {
    board.innerHTML = '<div class="bracket">'+T.rounds.map((rr,i)=>
      '<div class="round"><h4>Round '+(i+1)+'</h4>'+rr.map(m=>matchHTML(m)).join('')+'</div>'
    ).join('')+'</div>';
  }
  // standings for rr/swiss
  const st = document.getElementById('standings');
  if(T.standings && (T.style==='roundrobin'||T.style==='swiss')){
    const rows = Object.entries(T.standings).sort((a,b)=>b[1].w-a[1].w||a[1].l-b[1].l);
    st.innerHTML = '<h4 style="color:var(--gold)">Standings</h4><table><tr><th>Player</th><th>W</th><th>L</th></tr>'+
      rows.map(([p,s])=>'<tr><td>'+p+'</td><td>'+s.w+'</td><td>'+s.l+'</td></tr>').join('')+'</table>';
  } else st.innerHTML='';
}
function matchHTML(m){
  const a=m.a, b=m.b, w=m.winner;
  const bye = (!a||!b);
  const slot=(name,other)=> name? ('<div class="slot '+(w===name?'win':'')+(bye?' bye':'')+'" '+
       (!bye&&!w?('onclick="report('+m.id+',\\''+name.replace(/'/g,"\\\\'")+'\\')"'):'')+'>'+
       '<span>'+name+'</span>'+(w===name?'<span>✓</span>':'')+'</div>') : '<div class="slot bye"><span>—</span></div>';
  // a "cast" control on ready, unresolved matches: launch the live board for
  // this pairing; the winner auto-reports back here when the game ends.
  let cast='';
  if(!bye && !w){
    cast = '<div style="margin-top:6px;text-align:center">'+
      '<button class="btn" style="padding:5px 10px;font-size:12px" '+
      'onclick="castMatch('+m.id+',\\''+ (a||'').replace(/'/g,"\\\\'") +'\\',\\''+ (b||'').replace(/'/g,"\\\\'") +'\\')">▶ Play live</button> '+
      '<a class="btn" style="padding:5px 10px;font-size:12px" href="/play" target="_blank">👁 watch</a></div>';
  }
  return '<div class="match" id="m'+m.id+'">'+slot(a,b)+'<div class="vs">vs</div>'+slot(b,a)+cast+'</div>';
}
async function castMatch(mid, a, b){
  const arena = (document.getElementById('arena')||{}).value || 'study';
  const r = await fetch('/api/tournament/cast',{method:'POST',
    headers:{'Content-Type':'application/json'},
    body:JSON.stringify({match:mid, a:a, b:b, arena:arena, ai:true})});
  const j = await r.json();
  if(j.ok){ showLive(a, b); }
}
function showLive(a, b){
  // embed the live board inline rather than opening a new tab, so the organiser
  // watches the cast match right on the control page
  let panel = document.getElementById('livepanel');
  if(!panel){
    panel = document.createElement('div'); panel.id='livepanel'; panel.className='card';
    document.getElementById('live').appendChild(panel);
  }
  panel.innerHTML = '<div class="row" style="justify-content:space-between">'+
    '<h3 style="margin:0">▶ Live: '+(a||'?')+' vs '+(b||'?')+'</h3>'+
    '<div><a class="btn" href="/play" target="_blank" style="padding:5px 10px;font-size:12px">⤢ full board</a> '+
    '<button class="btn" style="padding:5px 10px;font-size:12px" onclick="hideLive()">✕ close</button></div></div>'+
    '<iframe src="/play" style="width:100%;height:460px;border:1px solid var(--line);'+
    'border-radius:10px;margin-top:8px;background:#0c0a07"></iframe>';
  panel.scrollIntoView({behavior:'smooth'});
}
function hideLive(){ const p=document.getElementById('livepanel'); if(p) p.remove(); }
function matchesFlat(){
  let all=[]; T.rounds.forEach(rr=>all=all.concat(rr));
  return '<div style="display:flex;flex-wrap:wrap;gap:8px">'+all.map(m=>matchHTML(m)).join('')+'</div>';
}
async function poll(){ try{ const r=await fetch('/api/tournament'); const j=await r.json();
  if(j.active){ T=j; render(); } }catch(e){} setTimeout(poll,2000); }
poll();
</script></body></html>""".replace("__STYLE__", _STYLE)
