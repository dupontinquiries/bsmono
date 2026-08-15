#!/usr/bin/env python3
"""Attachment check: do staggered children stay joined to their parents?

Per-part stagger animated every part in WORLD space, so a part that
lagged its neighbours visibly detached and the fold read as a Lego
build. Parts are now animated in LOCAL space under a parent, but that
is only half the fix -- it is worthless if a part's declared parent is
not the thing it is actually mounted to. This measures the gap between
each child's box and its parent's box across the whole fold, at low and
high stagger, and names the worst offender so a wrong parent shows up
as a number instead of as a vibe.

Regenerates from the current HTML every run. An earlier version of this
check lived in /tmp and reported pre-edit numbers for post-edit code.
"""
import os, subprocess, sys, tempfile
HERE=os.path.dirname(os.path.abspath(__file__))
HTML=os.path.join(HERE,"foldviewer.html")
CHK=os.path.join(HERE,"foldviewer-check.py")

TAIL=r"""
function gap(a,b){
  const dx=Math.max(0, Math.max(a.lo[0]-b.hi[0], b.lo[0]-a.hi[0]));
  const dy=Math.max(0, Math.max(a.lo[1]-b.hi[1], b.lo[1]-a.hi[1]));
  const dz=Math.max(0, Math.max(a.lo[2]-b.hi[2], b.lo[2]-a.hi[2]));
  return Math.hypot(dx,dy,dz);
}
let fail=0;
for(const stag of ['0.0','0.7']){
  store['stag'].value=stag;
  console.log('\n=== part stagger '+stag+' ===');
  for(let r=0;r<global.__rigs.length;r++){
    global.__pick(r);
    let worst=0, who='';
    for(let i=0;i<=20;i++){
      global.__sample(i/20);
      const ps=global.__prims();
      for(const p of ps){ if(p.parent<0) continue;
        const g=gap(p.bb, ps[p.parent].bb);
        if(g>worst){ worst=g; who=p.name+'\u2192'+ps[p.parent].name+' @t='+(i/20).toFixed(2); } }
    }
    const ok = worst<=0.30;
    if(!ok) fail++;
    console.log(global.__rigs[r].name.padEnd(22)+' worst child gap '+worst.toFixed(3)+
                (ok?'  [attached]':'  DETACHED ['+who+']'));
  }
}
console.log('');
console.log(fail===0?'ALL PARTS STAY ATTACHED':fail+' rig/stagger combo(s) detach');
process.exit(fail===0?0:1);
"""

def main():
    html=open(HTML,encoding="utf-8").read()
    js=html[html.index("<script>")+8:html.rindex("</script>")]
    js=js.replace("requestAnimationFrame(loop);\n",
      "requestAnimationFrame(loop);\n"
      "global.__pick=pickRig; global.__sample=sampleGeom; global.__rigs=RIGS; global.__prims=()=>prims;\n",1)
    stub=open(CHK,encoding="utf-8").read().split('STUB = r"""')[1].split('"""')[0]
    with tempfile.NamedTemporaryFile("w",suffix=".mjs",delete=False,encoding="utf-8") as f:
        f.write(stub+js+TAIL); path=f.name
    try:
        r=subprocess.run(["node",path],capture_output=True,text=True)
        sys.stdout.write(r.stdout)
        if r.stderr.strip(): sys.stderr.write(r.stderr)
        return r.returncode
    finally: os.unlink(path)

if __name__=="__main__": sys.exit(main())
