//! Laser-splitting array: a 3D lattice of light BEAMS filling a sphere. Lattice nodes sit at glyph (x,y) in
//! the reveal plane but at RANDOM DEPTHS across the sphere, so from the reveal axis the on-stroke beams align
//! into a near-grid that spells the text, while from ANY OTHER angle the depth parallax scatters them — true
//! obfuscation. The whole sphere stays lit (off-stroke + body beams glow dimly, hue-shifted) from all angles.
use fontdue::{Font, FontSettings};
const W: usize = 1100; const H: usize = 1100; const R: f32 = 1.0;
const GX: usize = 150; const GY: usize = 150;  // finer square lattice for a paragraph
type V3=[f32;3];
fn hue(h:f32)->V3{let h=h.rem_euclid(1.0);let k=|n:f32|{let t=(n+h*6.0)%6.0;(t.min(4.0-t)).clamp(0.0,1.0)};[k(5.0),k(3.0),k(1.0)]}
fn main(){
    let word=std::env::args().nth(1).unwrap_or_else(||"BUILD".into());
    let bytes=std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf").unwrap();
    let font=Font::from_bytes(bytes,FontSettings::default()).unwrap();
    // glyph coverage in a square target
    const GW:usize=512;const GH:usize=512;let mut glyph=vec![false;GW*GH];
    {   // wrap the text to lines, stack, fit the block into the square target centered
        let px=42.0; let wrap=24usize; let line_h=px*1.30;
        let mut lines:Vec<String>=Vec::new(); let mut cur=String::new();
        for w in word.split_whitespace(){ if !cur.is_empty()&&cur.len()+w.len()+1>wrap{lines.push(std::mem::take(&mut cur));}
            if !cur.is_empty(){cur.push(' ');} cur.push_str(w); }
        if !cur.is_empty(){lines.push(cur);} if lines.is_empty(){lines.push(word.clone());}
        let mut pix:Vec<(f32,f32)>=Vec::new();
        for (li,line) in lines.iter().enumerate(){ let mut pen=0.0f32; let base=li as f32*line_h+px;
            for ch in line.chars(){ let (m,bmp)=font.rasterize(ch,px);
                for yy in 0..m.height{for xx in 0..m.width{ if bmp[yy*m.width+xx]>110{
                    pix.push((pen+m.xmin as f32+xx as f32, base-(m.ymin as f32+m.height as f32)+yy as f32)); }}}
                pen+=m.advance_width; } }
        let (mut mnx,mut mny,mut mxx,mut mxy)=(f32::MAX,f32::MAX,f32::MIN,f32::MIN);
        for &(x,y) in &pix{mnx=mnx.min(x);mny=mny.min(y);mxx=mxx.max(x);mxy=mxy.max(y);}
        let sc=(GW as f32*0.94/(mxx-mnx).max(1.0)).min(GH as f32*0.94/(mxy-mny).max(1.0));
        let offx=(GW as f32-(mxx-mnx)*sc)*0.5; let offy=(GH as f32-(mxy-mny)*sc)*0.5;
        for &(x,y) in &pix{ let xx=((x-mnx)*sc+offx) as isize; let yy=((y-mny)*sc+offy) as isize;
            if xx>=0&&yy>=0&&(xx as usize)<GW&&(yy as usize)<GH{glyph[yy as usize*GW+xx as usize]=true;} }
        eprintln!("{} lines wrapped", lines.len());
    }
    // neighborhood test so the lattice catches thin paragraph strokes
    let onstroke=|x:f32,y:f32|->bool{let cx=((x*0.5+0.5)*GW as f32)as isize;let cy=((0.5-y*0.5)*GH as f32)as isize;
        for dy in -2..=2{for dx in -2..=2{let gx=cx+dx;let gy=cy+dy;
            if gx>=0&&gy>=0&&(gx as usize)<GW&&(gy as usize)<GH&&glyph[gy as usize*GW+gx as usize]{return true;}}} false};

    let mut rng=0xBEEF1u32;let mut rnd=|s:&mut u32|{*s^=*s<<13;*s^=*s>>17;*s^=*s<<5;*s as f32/u32::MAX as f32};
    // text lattice nodes: (x,y) on a grid spanning the disk, z = random full-depth in the sphere → obfuscation
    let mut node:Vec<V3>=Vec::new();let mut gidx=vec![usize::MAX;GX*GY];
    for gy in 0..GY{for gx in 0..GX{
        let x=(gx as f32/(GX-1)as f32*2.0-1.0)*0.9; let y=(gy as f32/(GY-1)as f32*2.0-1.0)*0.9;
        let rho=(x*x+y*y).sqrt(); if rho>R{continue;}
        let hz=(R*R-rho*rho).sqrt(); let z=(rnd(&mut rng)*2.0-1.0)*hz*0.95; // FULL depth spread
        gidx[gy*GX+gx]=node.len(); node.push([x,y,z]);
    }}
    let n_text=node.len();
    // body nodes: fill the rest of the sphere so it stays lit from every angle
    let mut made=0;while made<900{let(a,b,c)=(rnd(&mut rng)*2.0-1.0,rnd(&mut rng)*2.0-1.0,rnd(&mut rng)*2.0-1.0);
        if a*a+b*b+c*c<=1.0{node.push([a*R,b*R,c*R]);made+=1;}}
    // edges: text lattice neighbours (the near-grid) + body kNN; classify on-stroke by reveal-plane midpoint
    struct Beam{a:usize,b:usize,text:bool,col:V3}
    let mut beams:Vec<Beam>=Vec::new();
    for gy in 0..GY{for gx in 0..GX{let i=gidx[gy*GX+gx];if i==usize::MAX{continue;}
        for (dx,dy) in [(1i32,0i32),(0,1),(1,1),(1,-1)]{let(nx,ny)=(gx as i32+dx,gy as i32+dy);
            if nx>=0&&ny>=0&&(nx as usize)<GX&&(ny as usize)<GY{let j=gidx[ny as usize*GX+nx as usize];if j==usize::MAX{continue;}
                let mx=(node[i][0]+node[j][0])*0.5;let my=(node[i][1]+node[j][1])*0.5;
                let on=onstroke(mx,my);
                // laser palette: mostly white on strokes, occasional colored splits
                let col=if on{ if rnd(&mut rng)<0.18 {hue(rnd(&mut rng))} else {[1.0,1.0,1.0]} } else {hue(0.55+0.2*node[i][1])};
                beams.push(Beam{a:i,b:j,text:on,col});
            }}}}
    // body kNN beams (lit sphere)
    for i in n_text..node.len(){let mut ds:Vec<(f32,usize)>=(0..node.len()).filter(|&j|j!=i).map(|j|{
        let d=[node[i][0]-node[j][0],node[i][1]-node[j][1],node[i][2]-node[j][2]];(d[0]*d[0]+d[1]*d[1]+d[2]*d[2],j)}).collect();
        ds.sort_by(|a,b|a.0.partial_cmp(&b.0).unwrap());
        for &(_,j) in ds.iter().take(3){beams.push(Beam{a:i,b:j,text:false,col:hue(0.55+0.2*node[i][1])});}}
    eprintln!("{} nodes ({} text-lattice), {} beams, {} on-stroke",node.len(),n_text,beams.len(),beams.iter().filter(|b|b.text).count());

    for (tag,yaw,pitch) in [("reveal",0.0_f32,0.04_f32),("turn",0.55,0.18),("side",1.2,0.30)]{
        let(cyaw,syaw)=(yaw.cos(),yaw.sin());let(cp,sp)=(pitch.cos(),pitch.sin());
        let scale=W as f32*0.42;let(ox,oy)=(W as f32*0.5,H as f32*0.5);
        let proj=|p:V3|->(f32,f32,f32){let x1=p[0]*cyaw-p[2]*syaw;let z1=p[0]*syaw+p[2]*cyaw;
            let y1=p[1]*cp-z1*sp;let z2=p[1]*sp+z1*cp;(ox+x1*scale,oy-y1*scale,z2)};
        let mut fb=vec![0.0f32;W*H*3];
        // external god-ray shafts (the hatching in the sketch)
        for k in 0..5{let t=k as f32; let sx=ox-360.0+t*38.0; line(&mut fb,sx,40.0,sx+90.0,200.0,[0.5,0.5,0.5],0.5);
            line(&mut fb,ox-120.0+t*44.0,H as f32-220.0,ox-60.0+t*44.0,H as f32-30.0,[0.45,0.45,0.45],0.5);}
        for bm in &beams{let(ax,ay,az)=proj(node[bm.a]);let(bx,by,_)=proj(node[bm.b]);let depth=(az+R)/(2.0*R);
            let inten=if bm.text{0.95*(0.5+0.5*depth)}else{0.13*(0.4+0.6*depth)};
            line(&mut fb,ax,ay,bx,by,bm.col,inten);}
        // sphere outline
        let n=200;for i in 0..n{let t=i as f32/n as f32*std::f32::consts::TAU;
            line(&mut fb,ox+t.cos()*scale,oy+t.sin()*scale,ox+(t+0.04).cos()*scale,oy+(t+0.04).sin()*scale,[0.6,0.6,0.6],0.5);}
        let mut out=format!("P6\n{W} {H}\n255\n").into_bytes();
        for v in &fb{let t=(v/(1.0+v)).clamp(0.0,1.0);out.push((t.powf(1.0/2.2)*255.0)as u8);}
        std::fs::write(format!("/home/claude/anamorph/lz_{tag}.ppm"),out).unwrap();eprintln!("wrote lz_{tag}");
    }
}
fn line(fb:&mut[f32],x0:f32,y0:f32,x1:f32,y1:f32,col:V3,inten:f32){
    let n=((x1-x0).abs().max((y1-y0).abs())as usize).max(1);
    for s in 0..=n{let t=s as f32/n as f32;let x=(x0+(x1-x0)*t)as isize;let y=(y0+(y1-y0)*t)as isize;
        for(ox,oy)in[(0,0),(1,0),(0,1)]{let xx=x+ox;let yy=y+oy;
            if xx>=0&&yy>=0&&(xx as usize)<W&&(yy as usize)<H{let i=(yy as usize*W+xx as usize)*3;fb[i]+=col[0]*inten;fb[i+1]+=col[1]*inten;fb[i+2]+=col[2]*inten;}}}}
