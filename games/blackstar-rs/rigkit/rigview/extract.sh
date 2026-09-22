#!/bin/sh
# Pull the stream branch out of a built page by evaluating its own voxHook.
node -e "
const fs=require('fs');
const s=fs.readFileSync(process.argv[1],'utf8');
global.THREE={Vector3:class{constructor(){this.x=this.y=this.z=0;}}, Vector4:class{constructor(x=0,y=0,z=0,w=0){this.x=x;this.y=y;this.z=z;this.w=w;}}};
const a=s.indexOf('  const voxHook = (sh, holder) => {');
eval(s.slice(a, s.indexOf('\n  };\n', a)+5).replace('const voxHook','global.voxHook'));
const sh={uniforms:{},vertexShader:'#include <begin_vertex>\n'}; voxHook(sh,{userData:{}});
const g=sh.vertexShader, hash=g.slice(g.indexOf('float vhash'), g.indexOf('\n', g.indexOf('float vhash')));
const i=g.indexOf('if (uMorph')>=0?g.indexOf('if (uMorph'):g.indexOf('if (uStream'), j=g.indexOf('} else if', g.indexOf('if (uStream'));
fs.writeFileSync(process.argv[2], '// EXTRACTED from the page by evaluating its voxHook.\n'+hash+'\n//@@BRANCH\n'+g.slice(i,j)+'}\n');
" "$1" "$2"
