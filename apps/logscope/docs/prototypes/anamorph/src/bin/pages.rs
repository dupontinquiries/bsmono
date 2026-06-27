//! Paginated-by-rotation: a long message split into PAGES, each assigned a reveal axis at a distinct
//! (yaw,pitch) on the sphere. Scroll → rotation over BOTH spherical axes; a 2D clarity search snaps to the
//! nearest page detent, where that page's surface text COALESCES into clean text while all other pages stay
//! DISPERSED in the cracked-facet body. clarity(yaw,pitch)=max_i dot(camdir,axis_i)^k; detents = the axis_i.
use fontdue::{Font, FontSettings};
const W: usize=1000; const H: usize=1000; const R: f32=1.0;
const GX: usize=64; const GY: usize=64;
type V3=[f32;3];
fn add(a:V3,b:V3)->V3{[a[0]+b[0],a[1]+b[1],a[2]+b[2]]} fn mul(a:V3,s:f32)->V3{[a[0]*s,a[1]*s,a[2]*s]}
fn dot(a:V3,b:V3)->f32{a[0]*b[0]+a[1]*b[1]+a[2]*b[2]} fn nrm(a:V3)->V3{let l=dot(a,a).sqrt().max(1e-6);mul(a,1.0/l)}
fn cross(a:V3,b:V3)->V3{[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]}
fn hue(h:f32)->V3{let h=h.rem_euclid(1.0);let k=|n:f32|{let t=(n+h*6.0)%6.0;(t.min(4.0-t)).clamp(0.0,1.0)};[k(5.0),k(3.0),k(1.0)]}
fn camdir(yaw:f32,pitch:f32)->V3{[yaw.sin()*pitch.cos(),pitch.sin(),yaw.cos()*pitch.cos()]}
fn main(){
    let font=Font::from_bytes(std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf").unwrap(),FontSettings::default()).unwrap();
    let msg=std::env::args().nth(1).unwrap_or_else(||"Here is a starting point that uses std io and a seek cursor to follow the file as it grows. | It watches the inode and reopens the path on rotation so you never read a stale handle. | Track the byte offset across reads and reset to zero whenever the file shrinks underneath you. | Stream the lines instead of buffering the whole file so memory stays bounded on large logs. | Add a small debounce on change events so a burst of writes coalesces into one read pass.".into());
    // PAGES: split on '|' (or chunk), each gets a reveal axis on the sphere (distinct yaw,pitch detents)
    let pages:Vec<String>=msg.split('|').map(|s|s.trim().to_string()).filter(|s|!s.is_empty()).collect();
    let np=pages.len();
    let axes:Vec<(f32,f32)>=(0..np).map(|i|{ if i==0 {(0.0,0.0)} else {
        let a=(i as f32-1.0)/((np-1) as f32)*std::f32::consts::TAU; (0.72*a.cos(), 0.42*a.sin()) } }).collect(); // spread in yaw, alternate pitch
    let mut rng=0x7A1Eu32;let mut rnd=|s:&mut u32|{*s^=*s<<13;*s^=*s>>17;*s^=*s<<5;*s as f32/u32::MAX as f32};

    // interior cracked facets (shared body)
    let mut node:Vec<V3>=Vec::new();let mut gidx=vec![usize::MAX;GX*GY];
    for gy in 0..GY{for gx in 0..GX{let x=(gx as f32/(GX-1)as f32*2.0-1.0)*0.92;let y=(gy as f32/(GY-1)as f32*2.0-1.0)*0.92;
        let rho=(x*x+y*y).sqrt();if rho>R{continue;}let hz=(R*R-rho*rho).sqrt();
        gidx[gy*GX+gx]=node.len();node.push([x,y,(rnd(&mut rng)*2.0-1.0)*hz*0.95]);}}
    let mut fbeams:Vec<(usize,usize)>=Vec::new();
    for gy in 0..GY{for gx in 0..GX{let i=gidx[gy*GX+gx];if i==usize::MAX{continue;}
        for(dx,dy)in[(1i32,0i32),(0,1),(1,1)]{let(nx,ny)=(gx as i32+dx,gy as i32+dy);
            if nx>=0&&ny>=0&&(nx as usize)<GX&&(ny as usize)<GY{let j=gidx[ny as usize*GX+nx as usize];if j!=usize::MAX{fbeams.push((i,j));}}}}}

    // per-page surface text built in the plane PERPENDICULAR to that page's axis (reads from that axis)
    struct S{home:V3,scat:V3,page:usize}
    // place a glyph point at screen (gx,gy) for a page's view by inverse-rotating into world space
    let invrot=|a:f32,b:f32,d:f32,yaw:f32,pitch:f32|->V3{let(cy,sy)=(yaw.cos(),yaw.sin());let(cp,sp)=(pitch.cos(),pitch.sin());
        let y=b*cp+d*sp;let z1=-b*sp+d*cp;let x=a*cy+z1*sy;let z=-a*sy+z1*cy;[x,y,z]};
    let mut surf:Vec<S>=Vec::new();
    for (pi,page) in pages.iter().enumerate(){
        let (yaw,pitch)=axes[pi];
        // rasterize page (wrap)
        let px=26.0;let wrap=30usize;let lh=px*1.26;let mut lines=vec![];let mut cur=String::new();
        for w in page.split_whitespace(){if !cur.is_empty()&&cur.len()+w.len()+1>wrap{lines.push(std::mem::take(&mut cur));}if !cur.is_empty(){cur.push(' ');}cur.push_str(w);}
        if !cur.is_empty(){lines.push(cur);}
        let mut pix:Vec<(f32,f32)>=vec![];
        for(li,l) in lines.iter().enumerate(){let mut pen=0.0f32;let base=li as f32*lh+px;
            for ch in l.chars(){let(m,bmp)=font.rasterize(ch,px);
                for yy in 0..m.height{for xx in 0..m.width{if bmp[yy*m.width+xx]>110{pix.push((pen+m.xmin as f32+xx as f32,base-(m.ymin as f32+m.height as f32)+yy as f32));}}}
                pen+=m.advance_width;}}
        let(mut mnx,mut mny,mut mxx,mut mxy)=(f32::MAX,f32::MAX,f32::MIN,f32::MIN);
        for &(x,y) in &pix{mnx=mnx.min(x);mny=mny.min(y);mxx=mxx.max(x);mxy=mxy.max(y);}
        let(cx,cy)=((mnx+mxx)*0.5,(mny+mxy)*0.5);let sc=R*0.82/(((mxx-mnx).max(mxy-mny))*0.5).max(1.0);
        for &(x,y) in pix.iter(){
            let gx=(x-cx)*sc; let gy=-(y-cy)*sc;
            let home=invrot(gx,gy,0.0,yaw,pitch);                // upright in this page's camera plane
            let scat=loop{let q=(rnd(&mut rng)*2.0-1.0,rnd(&mut rng)*2.0-1.0,rnd(&mut rng)*2.0-1.0);if q.0*q.0+q.1*q.1+q.2*q.2<=1.0{break [q.0*R,q.1*R,q.2*R];}};
            surf.push(S{home,scat,page:pi});
        }
    }
    eprintln!("{} pages, axes={:?}, {} facet beams, {} surface pts",np,axes,fbeams.len(),surf.len());

    // render at several (yaw,pitch) scroll positions: each near a page detent (+ one between)
    let mut views:Vec<(String,f32,f32)>=(0..np).map(|i|(format!("p{}",i+1),axes[i].0,axes[i].1)).collect();
    views.push(("overview".into(),1.6,0.7));
    for (tag,yaw,pitch) in &views{ let (yaw,pitch)=(*yaw,*pitch);
        let cam=camdir(yaw,pitch);
        let(cyaw,syaw)=(yaw.cos(),yaw.sin());let(cp,sp)=(pitch.cos(),pitch.sin());
        let scale=W as f32*0.42;let(ox,oy)=(W as f32*0.5,H as f32*0.5);
        let proj=|p:V3|->(f32,f32,f32){let x1=p[0]*cyaw-p[2]*syaw;let z1=p[0]*syaw+p[2]*cyaw;let y1=p[1]*cp-z1*sp;let z2=p[1]*sp+z1*cp;(ox+x1*scale,oy-y1*scale,z2)};
        let mut fb=vec![0.0f32;W*H*3];
        for &(a,b) in &fbeams{let(ax,ay,az)=proj(node[a]);let(bx,by,_)=proj(node[b]);let depth=(az+R)/(2.0*R);
            line(&mut fb,ax,ay,bx,by,hue(0.55+0.22*node[a][1]),0.11*(0.4+0.6*depth));}
        // per-page coalesce by alignment of camera dir with that page's axis (the 2D clarity field)
        let cs:Vec<f32>=axes.iter().map(|&(y,p)| dot(cam,camdir(y,p)).max(0.0).powf(7.0)).collect();
        for s in &surf{let c=cs[s.page];
            let pos=[s.scat[0]+(s.home[0]-s.scat[0])*c,s.scat[1]+(s.home[1]-s.scat[1])*c,s.scat[2]+(s.home[2]-s.scat[2])*c];
            let(sx,sy,z)=proj(pos);let depth=(z+R)/(2.0*R);let bw=hue(0.55+0.22*pos[1]);
            let col=[bw[0]+(1.0-bw[0])*c,bw[1]+(1.0-bw[1])*c,bw[2]+(1.0-bw[2])*c];let inten=(0.10+0.85*c)*(0.5+0.5*depth);
            splat(&mut fb,sx,sy,1.4,[col[0]*inten,col[1]*inten,col[2]*inten]);}
        let n=200;for i in 0..n{let t=i as f32/n as f32*std::f32::consts::TAU;line(&mut fb,ox+t.cos()*scale,oy+t.sin()*scale,ox+(t+0.04).cos()*scale,oy+(t+0.04).sin()*scale,[0.5,0.5,0.5],0.5);}
        let mut out=format!("P6\n{W} {H}\n255\n").into_bytes();for v in &fb{let t=(v/(1.0+v)).clamp(0.0,1.0);out.push((t.powf(1.0/2.2)*255.0)as u8);}
        std::fs::write(format!("/home/claude/anamorph/pg_{tag}.ppm"),out).unwrap();eprintln!("wrote pg_{tag} (yaw{yaw:.2} pitch{pitch:.2})");
    }
}
fn splat(fb:&mut[f32],cx:f32,cy:f32,r:f32,col:V3){if cx< -r||cy< -r||cx>W as f32+r||cy>H as f32+r{return;}
    let(x0,x1)=((cx-r).floor().max(0.0)as usize,(cx+r).ceil().min(W as f32-1.0)as usize);let(y0,y1)=((cy-r).floor().max(0.0)as usize,(cy+r).ceil().min(H as f32-1.0)as usize);
    for y in y0..=y1{for x in x0..=x1{let d2=(x as f32-cx).powi(2)+(y as f32-cy).powi(2);let a=(-d2/(r*r*0.5)).exp();if a>0.003{let i=(y*W+x)*3;fb[i]+=col[0]*a;fb[i+1]+=col[1]*a;fb[i+2]+=col[2]*a;}}}}
fn line(fb:&mut[f32],x0:f32,y0:f32,x1:f32,y1:f32,col:V3,inten:f32){let n=((x1-x0).abs().max((y1-y0).abs())as usize).max(1);
    for s in 0..=n{let t=s as f32/n as f32;let x=(x0+(x1-x0)*t)as isize;let y=(y0+(y1-y0)*t)as isize;if x>=0&&y>=0&&(x as usize)<W&&(y as usize)<H{let i=(y as usize*W+x as usize)*3;fb[i]+=col[0]*inten;fb[i+1]+=col[1]*inten;fb[i+2]+=col[2]*inten;}}}
