// Boot a built rig-viewer page in node: real three.js, fake browser only.
const fs=require('fs');
global.THREE=require('three/build/three.js');
class FakeEl{ constructor(id){ this.id=id; this.value='0'; this.textContent=''; this.innerHTML='';
  this.style={}; this.children=[]; this.dataset={}; this.clientWidth=1200; this.clientHeight=800;
  this.classList={add(){},remove(){},toggle(){},contains(){return false;}}; }
  addEventListener(){} removeEventListener(){} appendChild(c){this.children.push(c);return c;} prepend(){}
  setAttribute(){} getAttribute(){return null;} querySelector(){return new FakeEl();} querySelectorAll(){return [];}
  getBoundingClientRect(){return {left:0,top:0,width:1200,height:800};} focus(){} remove(){} }
const els={};
global.document={ getElementById:id=>(els[id]=els[id]||new FakeEl(id)), createElement:t=>new FakeEl(t),
  querySelector:()=>new FakeEl(), querySelectorAll:()=>[], body:new FakeEl('body'), addEventListener(){} };
global.window={ addEventListener(){}, devicePixelRatio:1, innerWidth:1200, innerHeight:800,
  matchMedia:()=>({matches:false,addEventListener(){}}) };
global.matchMedia=global.window.matchMedia;
// the page uses these bare, as browser globals
global.devicePixelRatio=1; global.innerWidth=1200; global.innerHeight=800;
global.addEventListener=()=>{}; global.removeEventListener=()=>{};
global.location={search:'',hash:''}; global.navigator={userAgent:'node'};
global.requestAnimationFrame=()=>0;
global.ResizeObserver=class{observe(){}};
global.performance=global.performance||{now:()=>Date.now()};
THREE.WebGLRenderer=class{ constructor(){ this.domElement=new FakeEl('canvas'); this.shadowMap={};
  this.capabilities={isWebGL2:true}; this.outputEncoding=0; }
  setPixelRatio(){} setSize(){} render(){} setClearColor(){} getContext(){return {};} compile(){} dispose(){} };
module.exports=async function boot(path){
  const html=fs.readFileSync(path,'utf8');
  const a=html.indexOf('<script>', html.indexOf('three.min.js'))+8, b=html.indexOf('</script>', a);
  // the page wraps its script in try{...}: export from INSIDE the block,
  // just before its own loadCar() call, or nothing is in scope
  let src=html.slice(a,b);
  const call=src.lastIndexOf('loadCar().then(refresh)');
  if(call<0) throw new Error('page has no loadCar().then(refresh)');
  src=src.slice(0,call)+
    'global.__rt={refresh, carMeshes, scene, tIn:document.getElementById("t"), setDyn:v=>{dynOn=v;}, RIG, bodyMesh, poseAt, solve, spinAt, smoothLift, groundLift, MOTION:(typeof MOTION!=="undefined"?MOTION:null), setMotion:(c,t0)=>{MOTION_ON=c;MOTION_T0=t0;}, getFx:()=>MOTION_FX, resetLift:()=>{LIFT=null;}, getRanged:()=>({hubBarrel,palmBarrel,shotPool,flashPart,flashPalm,slugMat})};\n'+
    src.slice(call);
  (0,eval)(src);
  for(let i=0;i<200 && !Object.keys(global.__rt.carMeshes).length;i++) await new Promise(r=>setTimeout(r,50));
  await new Promise(r=>setTimeout(r,200));
  return {rt:global.__rt, els};
};
