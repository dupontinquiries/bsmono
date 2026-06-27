//! Sphere-beam pearl: the beam lattice wrapped onto a sphere. Glyph-aligned beams burn pure WHITE (legible
//! text); every other beam rainbow-SPLITS (prism dispersion) into a dim distorted body so the cluster reads
//! as a glowing orb from a distance. Plus a breathing pulse (rendered at two phases here; relaxation/breathing
//! is a per-frame anim in the live app).
use fontdue::{Font, FontSettings};

const W: usize = 1000;
const H: usize = 1000;
const N: usize = 1500;     // nodes on the sphere (Fibonacci → irregular, not grid-latticy)
const K: usize = 6;        // beams per node (nearest neighbours)
const R: f32 = 1.0;

type V3 = [f32; 3];
fn d2(a: V3, b: V3) -> f32 { (a[0]-b[0]).powi(2)+(a[1]-b[1]).powi(2)+(a[2]-b[2]).powi(2) }

fn hue(h: f32) -> V3 { // h in 0..1 → rgb
    let k = |n: f32| { let t=(n + h*6.0)%6.0; (t.min(4.0-t).clamp(0.0,1.0)) };
    [k(5.0), k(3.0), k(1.0)]
}

fn main() {
    let word = std::env::args().nth(1).unwrap_or_else(|| "BUILD".into());
    // glyph coverage in the reveal plane (xy), centered, scaled to the sphere disk
    let bytes = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf").unwrap();
    let font = Font::from_bytes(bytes, FontSettings::default()).unwrap();
    const GW: usize = 512; const GH: usize = 512;
    let mut glyph = vec![false; GW*GH];
    { let px=150.0; let mut pen=20.0; let mut allw=0.0;
      let metrics: Vec<_> = word.chars().map(|c| font.rasterize(c, px)).collect();
      for (m,_) in &metrics { allw += m.advance_width; }
      let sx = (GW as f32*0.9)/allw; let py = GH as f32*0.5;
      for (ch,(m,bmp)) in word.chars().zip(metrics.iter()) { let _=ch;
        for yy in 0..m.height { for xx in 0..m.width { if bmp[yy*m.width+xx]>110 {
            let gx=((pen + m.xmin as f32 + xx as f32)*sx) as isize;
            let gy=(py - (m.ymin as f32 + (m.height as f32-yy as f32))*sx + 40.0) as isize;
            if gx>=0&&gy>=0&&(gx as usize)<GW&&(gy as usize)<GH { glyph[gy as usize*GW+gx as usize]=true; }
        }}}
        pen += m.advance_width;
      }
    }
    // sample glyph at a reveal-plane coord in [-1,1]
    let gsample = |x: f32, y: f32| -> bool {
        let gx=((x*0.5+0.5)*GW as f32) as isize; let gy=((0.5-y*0.5)*GH as f32) as isize;
        gx>=0&&gy>=0&&(gx as usize)<GW&&(gy as usize)<GH && glyph[gy as usize*GW+gx as usize]
    };

    // Fibonacci sphere nodes
    let ga = std::f32::consts::PI*(3.0-5.0_f32.sqrt());
    let nodes: Vec<V3> = (0..N).map(|i| {
        let y = 1.0 - 2.0*(i as f32+0.5)/N as f32;
        let r = (1.0-y*y).sqrt(); let th = i as f32*ga;
        [th.cos()*r*R, y*R, th.sin()*r*R]
    }).collect();
    // kNN beams
    let mut beams: Vec<(usize,usize)> = Vec::new();
    for i in 0..N {
        let mut ds: Vec<(f32,usize)> = (0..N).filter(|&j| j!=i).map(|j| (d2(nodes[i],nodes[j]),j)).collect();
        ds.sort_by(|a,b| a.0.partial_cmp(&b.0).unwrap());
        for &(_,j) in ds.iter().take(K) { if i<j { beams.push((i,j)); } }
    }
    // classify: positive = front-facing (z>0) beam whose projection lies on a glyph stroke
    let mut pos = vec![false; beams.len()];
    for (bi,&(a,b)) in beams.iter().enumerate() {
        let mid=[ (nodes[a][0]+nodes[b][0])*0.5, (nodes[a][1]+nodes[b][1])*0.5, (nodes[a][2]+nodes[b][2])*0.5 ];
        if mid[2] < 0.15 { continue; } // front hemisphere only
        let mut hit=0; let n=6;
        for s in 0..=n { let t=s as f32/n as f32;
            let x=nodes[a][0]+(nodes[b][0]-nodes[a][0])*t; let y=nodes[a][1]+(nodes[b][1]-nodes[a][1])*t;
            if gsample(x/R, y/R) { hit+=1; } }
        pos[bi] = hit as f32/(n as f32+1.0) > 0.55;
    }
    eprintln!("{} nodes, {} beams, {} positive(text)", N, beams.len(), pos.iter().filter(|&&p|p).count());

    for (tag, yaw, breath) in [("reveal", 0.0_f32, 0.0_f32), ("breathe", 0.0, 1.0), ("orb", 1.1, 0.5)] {
        let (cyaw,syaw)=(yaw.cos(), yaw.sin());
        let pulse = 1.0 + 0.06*(breath*std::f32::consts::PI).sin(); // breathing scale
        let scale=W as f32*0.42; let (ox,oy)=(W as f32*0.5,H as f32*0.5);
        let proj = |p: V3| -> (f32,f32,f32) {
            let x=p[0]*pulse; let y=p[1]*pulse; let z=p[2]*pulse;
            let x1=x*cyaw - z*syaw; let z1=x*syaw + z*cyaw;
            (ox + x1*scale, oy - y*scale, z1)
        };
        let mut fb=vec![0.0f32; W*H*3];
        for (bi,&(a,b)) in beams.iter().enumerate() {
            let (ax,ay,az)=proj(nodes[a]); let (bx,by,_)=proj(nodes[b]);
            let depth=(az+R)/(2.0*R);
            if pos[bi] {
                line(&mut fb,ax,ay,bx,by,[1.0,1.0,1.0], 0.9*(0.5+0.5*depth), 0.0); // pure white text
            } else {
                // rainbow prism split → dim distorted body
                let h=((nodes[a][1]*0.5+0.5)+nodes[a][0]*0.2+0.0).rem_euclid(1.0);
                let c=hue(h); let dim=0.10*(0.4+0.6*depth);
                line(&mut fb,ax,ay,bx,by,[c[0],c[1],c[2]], dim, 2.0); // off=2px lateral dispersion
            }
        }
        let mut out=format!("P6\n{W} {H}\n255\n").into_bytes();
        for v in &fb { let t=(v/(1.0+v)).clamp(0.0,1.0); out.push((t.powf(1.0/2.2)*255.0) as u8); }
        std::fs::write(format!("/home/claude/anamorph/sph_{tag}.ppm"), out).unwrap();
        eprintln!("wrote sph_{tag}");
    }
}

fn line(fb: &mut [f32], x0:f32,y0:f32,x1:f32,y1:f32, col:V3, inten:f32, split:f32) {
    let n=((x1-x0).abs().max((y1-y0).abs()) as usize).max(1);
    // prism split: draw R/G/B passes laterally offset by `split` px
    let passes = if split>0.0 { vec![([col[0],0.0,0.0],-split),([0.0,col[1],0.0],0.0),([0.0,0.0,col[2]],split)] }
                 else { vec![([col[0],col[1],col[2]],0.0)] };
    let (dx,dy)=(x1-x0,y1-y0); let len=(dx*dx+dy*dy).sqrt().max(1e-3); let (px,py)=(-dy/len,dx/len);
    for (c,off) in passes { for s in 0..=n { let t=s as f32/n as f32;
        let x=(x0+dx*t+px*off) as isize; let y=(y0+dy*t+py*off) as isize;
        if x>=0&&y>=0&&(x as usize)<W&&(y as usize)<H { let i=(y as usize*W+x as usize)*3; fb[i]+=c[0]*inten; fb[i+1]+=c[1]*inten; fb[i+2]+=c[2]*inten; }
    }}
}
