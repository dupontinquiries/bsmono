//! Beam-network text: instead of points on/off, run a random-walk × A* (ant-colony) traversal connecting
//! input edges → output edges across a node field, then BRIGHTEN the beams whose segments align with glyph
//! strokes and optimise for legibility. Beams are line segments (few primitives, laser/fiber aesthetic) and
//! double as the flow connections. Glyph stays the explicit objective — guided, not emergent.
use fontdue::{Font, FontSettings};

const W: usize = 1100;
const H: usize = 360;
const GX: usize = 92;   // node grid
const GY: usize = 30;

fn main() {
    // 1) glyph coverage target
    let bytes = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf").unwrap();
    let font = Font::from_bytes(bytes, FontSettings::default()).unwrap();
    let word = std::env::args().nth(1).unwrap_or_else(|| "BUILD".into());
    let mut glyph = vec![false; W * H];
    {
        let px = 230.0; let mut pen = 60.0;
        for ch in word.chars() {
            let (m, bmp) = font.rasterize(ch, px);
            for yy in 0..m.height { for xx in 0..m.width {
                if bmp[yy*m.width+xx] > 110 {
                    let gx = (pen + m.xmin as f32 + xx as f32) as isize;
                    let gy = (H as f32*0.5 - (m.ymin as f32 + (m.height as f32 - yy as f32)) + 40.0) as isize;
                    if gx>=0 && gy>=0 && (gx as usize)<W && (gy as usize)<H { glyph[gy as usize*W + gx as usize]=true; }
                }
            }}
            pen += m.advance_width;
        }
    }
    let covered = glyph.iter().filter(|&&b| b).count();

    // 2) node field + 8-connected edges
    let nid = |gx: usize, gy: usize| gy*GX+gx;
    let npos = |gx: usize, gy: usize| (gx as f32/(GX-1) as f32*(W as f32-40.0)+20.0, gy as f32/(GY-1) as f32*(H as f32-40.0)+20.0);
    let mut edges: Vec<(usize,usize)> = Vec::new();
    let mut nbr: Vec<Vec<usize>> = vec![Vec::new(); GX*GY]; // edge indices per node
    for gy in 0..GY { for gx in 0..GX {
        for (dx,dy) in [(1i32,0i32),(0,1),(1,1),(1,-1)] {
            let (nx,ny)=(gx as i32+dx, gy as i32+dy);
            if nx>=0 && ny>=0 && (nx as usize)<GX && (ny as usize)<GY {
                let e = edges.len(); edges.push((nid(gx,gy), nid(nx as usize, ny as usize)));
                nbr[nid(gx,gy)].push(e); nbr[nid(nx as usize,ny as usize)].push(e);
            }
        }
    }}
    // glyph alignment per edge = fraction of samples along it that land on a stroke
    let mut galign = vec![0.0f32; edges.len()];
    for (i,&(a,b)) in edges.iter().enumerate() {
        let (ax,ay)=npos(a%GX,a/GX); let (bx,by)=npos(b%GX,b/GX);
        let mut hit=0; let n=8;
        for s in 0..=n { let t=s as f32/n as f32; let x=(ax+(bx-ax)*t) as usize; let y=(ay+(by-ay)*t) as usize; if glyph[y.min(H-1)*W+x.min(W-1)] { hit+=1; } }
        galign[i] = hit as f32/(n as f32+1.0);
    }
    let other = |e: usize, from: usize| { let (a,b)=edges[e]; if a==from {b} else {a} };

    // 3) ant-colony: input edge = left column, output = right column. Walk biased by pheromone × glyph × rightward.
    let mut pher = vec![0.10f32; edges.len()];
    let entries: Vec<usize> = (0..GY).map(|gy| nid(0,gy)).collect();
    let mut rng = 0xC0FFEEu32;
    let mut rand = |s: &mut u32| { *s^=*s<<13; *s^=*s>>17; *s^=*s<<5; *s as f32/u32::MAX as f32 };
    let iters = 60;
    for _it in 0..iters {
        for _ant in 0..420 {
            let mut cur = entries[(rand(&mut rng)*GY as f32) as usize % GY];
            let mut visited = vec![false; GX*GY]; visited[cur]=true;
            let mut path: Vec<usize> = Vec::new();
            for _ in 0..(GX*2) {
                // candidate edges to unvisited neighbors
                let mut cand: Vec<(usize,f32)> = Vec::new();
                for &e in &nbr[cur] { let o=other(e,cur); if visited[o] {continue;}
                    let rightward = ((o%GX) as f32 - (cur%GX) as f32).max(0.0); // A*-ish pull toward exits
                    let w = pher[e].powf(1.1) * (1.0 + 6.0*galign[e]) * (0.3 + rightward);
                    cand.push((e,w));
                }
                if cand.is_empty() { break; }
                let tot: f32 = cand.iter().map(|c|c.1).sum();
                let mut pick = rand(&mut rng)*tot; let mut chosen = cand[0].0;
                for (e,w) in &cand { if pick<=*w {chosen=*e;break;} pick-=*w; }
                path.push(chosen); cur=other(chosen,cur); visited[cur]=true;
                if cur%GX==GX-1 { break; } // reached an output edge
                if rand(&mut rng) < 0.05 { /* laser split: drop a small deposit and continue */ }
            }
            // deposit weighted by glyph alignment of the path (reinforces letter-tracing beams)
            let score: f32 = path.iter().map(|&e| galign[e]).sum::<f32>() / path.len().max(1) as f32;
            for &e in &path { pher[e] += 0.04 + 1.2*galign[e]*(0.3+score); }
        }
        for p in pher.iter_mut() { *p *= 0.90; } // evaporate
    }

    // 4) legibility metric + active-path fraction
    let pmax = pher.iter().cloned().fold(0.0f32,f32::max);
    let active = pher.iter().filter(|&&p| p > 0.35*pmax).count();
    // correlation of bright-on-glyph vs bright-off-glyph
    let mut on=0.0f32; let mut off=0.0f32;
    for (i,&p) in pher.iter().enumerate() { if p>0.35*pmax { if galign[i]>0.4 {on+=1.0} else {off+=1.0} } }
    eprintln!("glyph px={covered}  edges={}  active beams={active} ({:.2}% of edges)  legible: {:.0}% of bright beams on-stroke",
              edges.len(), 100.0*active as f32/edges.len() as f32, 100.0*on/(on+off).max(1.0));

    // 5) render: dim connective web + bright glyph-aligned beams (the text)
    let mut fb = vec![0.0f32; W*H*3];
    for (i,&(a,b)) in edges.iter().enumerate() {
        let (ax,ay)=npos(a%GX,a/GX); let (bx,by)=npos(b%GX,b/GX);
        let p = pher[i]/pmax;
        let glyphy = galign[i] > 0.4;
        let (col, base) = if glyphy { ([0.55,1.0,0.95], 0.15) } else { ([0.10,0.35,0.40], 0.02) };
        let inten = base + p*if glyphy {1.4} else {0.25};
        line(&mut fb, ax,ay,bx,by, [col[0]*inten,col[1]*inten,col[2]*inten]);
    }
    let mut out = format!("P6\n{W} {H}\n255\n").into_bytes();
    for v in &fb { let t=(v/(1.0+v)).clamp(0.0,1.0); out.push((t.powf(1.0/2.2)*255.0) as u8); }
    std::fs::write("/home/claude/anamorph/beams.ppm", out).unwrap();
    eprintln!("wrote beams.ppm");
}

fn line(fb: &mut [f32], x0:f32,y0:f32,x1:f32,y1:f32,col:[f32;3]) {
    let n = ((x1-x0).abs().max((y1-y0).abs()) as usize).max(1);
    for s in 0..=n { let t=s as f32/n as f32; let x=(x0+(x1-x0)*t) as isize; let y=(y0+(y1-y0)*t) as isize;
        for (ox,oy) in [(0,0),(1,0),(0,1)] { let xx=x+ox; let yy=y+oy;
            if xx>=0 && yy>=0 && (xx as usize)<W && (yy as usize)<H { let i=(yy as usize*W+xx as usize)*3; fb[i]+=col[0]; fb[i+1]+=col[1]; fb[i+2]+=col[2]; }
        }
    }
}
