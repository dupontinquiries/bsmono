// Which side of the hip->ankle line each knee sits on, over a clip: in front
// (toward -x, the facing) is a healthy bend; behind is a hyperextended knee.
module.exports=async function(page, clip, T){
  delete require.cache[require.resolve('./boot.js')];
  const {rt}=await require('./boot.js')(page); rt.setDyn(false);
  const M=rt.MOTION, stand=JSON.parse(JSON.stringify(rt.poseAt(1).p));
  const res={l:{worst:1e9,behind:0,n:0}, r:{worst:1e9,behind:0,n:0}};
  for(let k=0;k<=120;k++){ const t=k/120*(T||1.0);
    const s=rt.solve(M.pose(clip,t,stand,rt.solve).p);
    for(const side of ['l','r']){
      const H=s['thigh_'+side].head, K=s['shin_'+side].head, A=s['foot_'+side].head;
      // the knee's offset from the hip-ankle line, in the facing direction (-x)
      const d=[A[0]-H[0],A[1]-H[1],A[2]-H[2]], L2=d[0]**2+d[1]**2+d[2]**2;
      const f=((K[0]-H[0])*d[0]+(K[1]-H[1])*d[1]+(K[2]-H[2])*d[2])/L2;
      const off=-(K[0]-(H[0]+d[0]*f));          // + = knee in front
      const R=res[side]; R.worst=Math.min(R.worst,off); R.n++; if(off<-0.5) R.behind++;
    }
  }
  return res;
};
