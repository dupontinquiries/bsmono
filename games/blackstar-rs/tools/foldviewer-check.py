#!/usr/bin/env python3
"""Headless check for tools/foldviewer.html.

The viewer cannot be eyeballed from a terminal, and the two times it
shipped broken the fault was invisible to every other kind of review:
once a CDN import that the sandbox could not fetch (blank page), once a
loop variable left behind by a function split (ReferenceError on the
first audit call). Both would have been caught by simply RUNNING the
page, so this runs it: a stubbed DOM and a stubbed WebGL context, the
real script, every rig swept through the whole fold.

    python3 tools/foldviewer-check.py

Regenerates from the current file on every run -- an earlier version of
this check was a stale copy in /tmp and reported passing results for
code that had already changed.
"""
import os, re, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
HTML = os.path.join(HERE, "foldviewer.html")

STUB = r"""
const store={};
function el(id){ if(store[id]) return store[id];
  const o={ id, style:{}, value:"0", checked:(id==='play'||id==='showfault'),
    textContent:"", innerHTML:"", selectedIndex:0, children:[],
    addEventListener(){}, appendChild(c){ this.children.push(c); },
    getContext(){ return GL; }, width:1600, height:900 };
  store[id]=o; return o; }
const GLC={COMPILE_STATUS:1,LINK_STATUS:1,VERTEX_SHADER:1,FRAGMENT_SHADER:2,ARRAY_BUFFER:1,
  STATIC_DRAW:1,DYNAMIC_DRAW:2,DEPTH_TEST:1,SCISSOR_TEST:2,COLOR_BUFFER_BIT:1,DEPTH_BUFFER_BIT:2,
  LINES:1,TRIANGLES:4,FLOAT:5126};
let draws=0, panes=0;
const GL=Object.assign({},GLC,{
  createShader:()=>({}), shaderSource(){}, compileShader(){}, getShaderParameter:()=>true,
  getShaderInfoLog:()=>"", createProgram:()=>({}), attachShader(){}, linkProgram(){},
  getProgramParameter:()=>true, getProgramInfoLog:()=>"", useProgram(){},
  getAttribLocation:()=>0, getUniformLocation:()=>({}), enable(t){ if(t===2) panes++; }, disable(){},
  createBuffer:()=>({}), deleteBuffer(){}, bindBuffer(){}, bufferData(){}, viewport(){}, scissor(){},
  clearColor(){}, clear(){}, uniformMatrix4fv(){}, uniform3f(){},
  enableVertexAttribArray(){}, vertexAttribPointer(){},
  drawArrays(m,f,c){ draws++; if(!(c>0)) throw new Error("drawArrays count="+c); }
});
global.document={ getElementById:el, addEventListener(){}, createElement:()=>({style:{}}) };
global.window={ addEventListener(){}, innerWidth:1600, innerHeight:900, devicePixelRatio:1 };
global.performance={ now:()=>0 };
let raf=0; global.requestAnimationFrame=(fn)=>{ if(raf++<2) fn(raf*16); };
global.addEventListener=()=>{};
"""

TAIL = r"""
let fail=0;
const st=store['selftest'].innerHTML;
const parity = st.indexOf('JS matches engine')!==-1;
console.log('parity     :', parity?'JS matches engine':'DIVERGED FROM ENGINE');
if(!parity) fail++;
console.log('panes/frame:', panes, ' draw calls:', draws, ' examples:', store['rig'].children.length);
if(draws<10){ console.log('FAIL: almost nothing drawn'); fail++; }
if(store['rig'].children.length!==global.__rigs.length){ console.log('FAIL: dropdown not populated'); fail++; }
console.log('');
for(let r=0;r<global.__rigs.length;r++){
  global.__pick(r);
  let bad=[];
  for(let i=0;i<=20;i++){ const t=i/20; global.__sample(t);
    const txt=(store['fault'].innerHTML||'').replace(/<[^>]*>/g,'');
    if(txt.indexOf('intersection')!==-1) bad.push(t.toFixed(2)); }
  global.__sample(0); const e0=(store['fault'].innerHTML||'').replace(/<[^>]*>/g,'');
  global.__sample(1); const e1=(store['fault'].innerHTML||'').replace(/<[^>]*>/g,'');
  const c0=e0.indexOf('clean')===0, c1=e1.indexOf('clean')===0;
  if(!c0||!c1) fail++;
  const mbad=global.__prims().filter(p=>p.vratio>3.0||p.vratio<0.28)
                             .map(p=>p.name+' '+p.vratio.toFixed(1)+'x');
  const con=(store['conceal'].innerHTML||'').replace(/<[^>]*>/g,'').split('\n')[0];
  const conOk=con.indexOf('stowed')===0 || con.indexOf('no soft')===0;
  if(mbad.length) fail++;
  if(!conOk) fail++;
  console.log((r+1)+'. '+global.__rigs[r].name.padEnd(22)+
    ' ends='+((c0&&c1)?'CLEAN ':'DIRTY ')+
    ' midfold='+(bad.length?(bad.length+'/21'):'none').padEnd(7)+
    ' mass='+(mbad.length?('OUT: '+mbad.join(', ')):'ok').padEnd(10)+
    ' conceal='+(conOk?'ok':'EXPOSED'));
  if(!c0) console.log('     t=0 '+e0.split('\n').slice(1).join(' | '));
  if(!c1) console.log('     t=1 '+e1.split('\n').slice(1).join(' | '));
}
if(store['err'].style.display==='block'){ console.log('FATAL:',store['err'].textContent); fail++; }
console.log('');
console.log(fail===0 ? 'VIEWER OK' : fail+' PROBLEM(S)');
process.exit(fail===0?0:1);
"""

def main():
    html = open(HTML, encoding="utf-8").read()
    js = html[html.index("<script>")+8 : html.rindex("</script>")]
    # expose the internals the sweep needs, without editing the page
    js = js.replace("requestAnimationFrame(loop);\n",
        "requestAnimationFrame(loop);\n"
        "global.__pick=pickRig; global.__sample=sample; global.__rigs=RIGS; global.__prims=()=>prims;\n", 1)
    with tempfile.NamedTemporaryFile("w", suffix=".mjs", delete=False, encoding="utf-8") as f:
        f.write(STUB + js + TAIL)
        path = f.name
    try:
        r = subprocess.run(["node", path], capture_output=True, text=True)
        sys.stdout.write(r.stdout)
        if r.stderr.strip():
            sys.stderr.write(r.stderr)
        return r.returncode
    finally:
        os.unlink(path)

if __name__ == "__main__":
    sys.exit(main())
