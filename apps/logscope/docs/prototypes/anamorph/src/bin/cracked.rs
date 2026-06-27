//! Cracked-pearl: depth-spread beam lattice = the INTERNAL FACETS (dim, hue-shifted, always-lit body that
//! obscures from every angle), plus a thin SURFACE MODULATOR layer carrying the crisp text — sharp white
//! glyph coverage on the front shell, alignment-gated so it resolves only on the reveal axis. Interior =
//! obfuscation + life; surface = legibility. Decouples the two (no more white-out / lattice-noise tradeoff).
use fontdue::{Font, FontSettings};
const W: usize = 1100; const H: usize = 1100; const R: f32 = 1.0;
const GX: usize = 70; const GY: usize = 70;
type V3=[f32;3];
fn hue(h:f32)->V3{let h=h.rem_euclid(1.0);let k=|n:f32|{let t=(n+h*6.0)%6.0;(t.min(4.0-t)).clamp(0.0,1.0)};[k(5.0),k(3.0),k(1.0)]}
fn main(){
    let word=std::env::args().nth(1).unwrap_or_else(||"BUILD".into());
    let bytes=std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf").unwrap();
    let font=Font::from_bytes(bytes,FontSettings::default()).unwrap();
    // glyph coverage (wrapped to lines, fit to square) → also gives us crisp surface points
    const GW:usize=560;const GH:usize=560;let mut glyph=vec![false;GW*GH];
    {   let px=44.0; let wrap=22usize; let line_h=px*1.30;
        let mut lines:Vec<String>=Vec::new(); let mut cur=String::new();
        for w in word.split_whitespace(){ if !cur.is_empty()&&cur.len()+w.len()+1>wrap{lines.push(std::mem::take(&mut cur));}
            if !cur.is_empty(){cur.push(' ');} cur.push_str(w);} if !cur.is_empty(){lines.push(cur);} if lines.is_empty(){lines.push(word.clone());}
        let mut pix:Vec<(f32,f32)>=Vec::new();
        for (li,line) in lines.iter().enumerate(){let mut pen=0.0f32;let base=li as f32*line_h+px;
            for ch in line.chars(){let (m,bmp)=font.rasterize(ch,px);
                for yy in 0..m.height{for xx in 0..m.width{if bmp[yy*m.width+xx]>110{
                    pix.push((pen+m.xmin as f32+xx as f32,base-(m.ymin as f32+m.height as f32)+yy as f32));}}}
                pen+=m.advance_width;}}
        let(mut mnx,mut mny,mut mxx,mut mxy)=(f32::MAX,f32::MAX,f32::MIN,f32::MIN);
        for &(x,y) in &pix{mnx=mnx.min(x);mny=mny.min(y);mxx=mxx.max(x);mxy=mxy.max(y);}
        let sc=(GW as f32*0.94/(mxx-mnx).max(1.0)).min(GH as f32*0.94/(mxy-mny).max(1.0));
        let(offx,offy)=((GW as f32-(mxx-mnx)*sc)*0.5,(GH as f32-(mxy-mny)*sc)*0.5);
        for &(x,y) in &pix{let xx=((x-mnx)*sc+offx)as isize;let yy=((y-mny)*sc+offy)as isize;
            if xx>=0&&yy>=0&&(xx as usize)<GW&&(yy as usize)<GH{glyph[yy as usize*GW+xx as usize]=true;}}
        eprintln!("{} lines",lines.len());
    }
    let mut rng=0x5A5Au32;let mut rnd=|s:&mut u32|{*s^=*s<<13;*s^=*s>>17;*s^=*s<<5;*s as f32/u32::MAX as f32};

    // INTERIOR FACETS: depth-spread lattice nodes (dim, hue body) — the cracked-crystal backbone
    let mut node:Vec<V3>=Vec::new();let mut gidx=vec![usize::MAX;GX*GY];
    for gy in 0..GY{for gx in 0..GX{let x=(gx as f32/(GX-1)as f32*2.0-1.0)*0.92;let y=(gy as f32/(GY-1)as f32*2.0-1.0)*0.92;
        let rho=(x*x+y*y).sqrt();if rho>R{continue;} let hz=(R*R-rho*rho).sqrt();
        gidx[gy*GX+gx]=node.len();node.push([x,y,(rnd(&mut rng)*2.0-1.0)*hz*0.95]);}}
    let mut beams:Vec<(usize,usize)>=Vec::new();
    for gy in 0..GY{for gx in 0..GX{let i=gidx[gy*GX+gx];if i==usize::MAX{continue;}
        for (dx,dy) in [(1i32,0i32),(0,1),(1,1)]{let(nx,ny)=(gx as i32+dx,gy as i32+dy);
            if nx>=0&&ny>=0&&(nx as usize)<GX&&(ny as usize)<GY{let j=gidx[ny as usize*GX+nx as usize];
                if j!=usize::MAX{beams.push((i,j));}}}}}

    // SURFACE TEXT: crisp white glyph points on the FRONT shell (z>0), sampled from coverage
    let mut surf:Vec<(V3,V3)>=Vec::new();   // (home=clean text pos, scatter=dispersed-into-body pos)
    for yy in (0..GH).step_by(2){for xx in (0..GW).step_by(2){ if glyph[yy*GW+xx]{
        let x=(xx as f32/GW as f32*2.0-1.0); let y=-(yy as f32/GH as f32*2.0-1.0);
        let rho=(x*x+y*y).sqrt(); if rho<R{ let z=(R*R-rho*rho).sqrt()*0.75;
            let sc=loop{let a=(rnd(&mut rng)*2.0-1.0,rnd(&mut rng)*2.0-1.0,rnd(&mut rng)*2.0-1.0);
                if a.0*a.0+a.1*a.1+a.2*a.2<=1.0{break [a.0*R,a.1*R,a.2*R];}};
            surf.push(([x,y,z],sc)); }
    }}}
    eprintln!("{} facet nodes, {} facet beams, {} surface text points",node.len(),beams.len(),surf.len());

    for (tag,yaw,pitch) in [("reveal",0.0_f32,0.04_f32),("turn",0.6,0.18),("side",1.2,0.30)]{
        let(cyaw,syaw)=(yaw.cos(),yaw.sin());let(cp,sp)=(pitch.cos(),pitch.sin());
        let align=(cyaw*cp).abs();                                  // 1 looking down reveal axis
        let scale=W as f32*0.42;let(ox,oy)=(W as f32*0.5,H as f32*0.5);
        let proj=|p:V3|->(f32,f32,f32){let x1=p[0]*cyaw-p[2]*syaw;let z1=p[0]*syaw+p[2]*cyaw;
            let y1=p[1]*cp-z1*sp;let z2=p[1]*sp+z1*cp;(ox+x1*scale,oy-y1*scale,z2)};
        let mut fb=vec![0.0f32;W*H*3];
        // interior facets: dim hue beams (always lit, obfuscating body)
        for &(a,b) in &beams{let(ax,ay,az)=proj(node[a]);let(bx,by,_)=proj(node[b]);let depth=(az+R)/(2.0*R);
            let c=hue(0.55+0.22*node[a][1]); let inten=0.12*(0.4+0.6*depth);
            line(&mut fb,ax,ay,bx,by,c,inten);}
        // surface text: crisp white, alignment-gated → resolves only near the reveal axis
        // SURFACE MODULATOR: coalesce into clean text at the reveal detent (align→1), DISPERSE points back
        // into the facet body off-axis (align→0). Reconfiguration, not just dimming → real obfuscation.
        let c=align.powf(3.0);
        for (home,scat) in &surf{
            let pos=[scat[0]+(home[0]-scat[0])*c, scat[1]+(home[1]-scat[1])*c, scat[2]+(home[2]-scat[2])*c];
            let(sx,sy,z)=proj(pos); let depth=(z+R)/(2.0*R);
            let bw=hue(0.55+0.22*pos[1]);                          // dispersed = dim body hue
            let col=[bw[0]+(1.0-bw[0])*c, bw[1]+(1.0-bw[1])*c, bw[2]+(1.0-bw[2])*c]; // → white as it coalesces
            let inten=(0.11+0.85*c)*(0.5+0.5*depth);
            splat(&mut fb,sx,sy,1.7,[col[0]*inten,col[1]*inten,col[2]*inten]);
        }
        // sphere outline
        let n=200;for i in 0..n{let t=i as f32/n as f32*std::f32::consts::TAU;
            line(&mut fb,ox+t.cos()*scale,oy+t.sin()*scale,ox+(t+0.04).cos()*scale,oy+(t+0.04).sin()*scale,[0.5,0.5,0.5],0.5);}
        let mut out=format!("P6\n{W} {H}\n255\n").into_bytes();
        for v in &fb{let t=(v/(1.0+v)).clamp(0.0,1.0);out.push((t.powf(1.0/2.2)*255.0)as u8);}
        std::fs::write(format!("/home/claude/anamorph/ck_{tag}.ppm"),out).unwrap();eprintln!("wrote ck_{tag}");
    }
}
fn splat(fb:&mut[f32],cx:f32,cy:f32,r:f32,col:V3){ if cx< -r||cy< -r||cx>W as f32+r||cy>H as f32+r{return;}
    let(x0,x1)=((cx-r).floor().max(0.0)as usize,(cx+r).ceil().min(W as f32-1.0)as usize);
    let(y0,y1)=((cy-r).floor().max(0.0)as usize,(cy+r).ceil().min(H as f32-1.0)as usize);
    for y in y0..=y1{for x in x0..=x1{let d2=(x as f32-cx).powi(2)+(y as f32-cy).powi(2);let a=(-d2/(r*r*0.5)).exp();
        if a>0.003{let i=(y*W+x)*3;fb[i]+=col[0]*a;fb[i+1]+=col[1]*a;fb[i+2]+=col[2]*a;}}}}
fn line(fb:&mut[f32],x0:f32,y0:f32,x1:f32,y1:f32,col:V3,inten:f32){let n=((x1-x0).abs().max((y1-y0).abs())as usize).max(1);
    for s in 0..=n{let t=s as f32/n as f32;let x=(x0+(x1-x0)*t)as isize;let y=(y0+(y1-y0)*t)as isize;
        if x>=0&&y>=0&&(x as usize)<W&&(y as usize)<H{let i=(y as usize*W+x as usize)*3;fb[i]+=col[0]*inten;fb[i+1]+=col[1]*inten;fb[i+2]+=col[2]*inten;}}}
