//! Point-cloud sphere with flat text through the middle. The BODY is a dim hue-shifted point cloud (blues/
//! greens/reds). The TEXT is a flat plane of WHITE points in the mid-plane, intensified along the beam
//! trajectory routed from the input edge → through the glyph strokes → out the output edge. Cloud points that
//! would OCCLUDE the text (in front of the mid-plane, over a stroke) are hidden via knobs: dimmed, hue-pushed,
//! and BOUNCED perpendicular to the camera so they slide out of the text's footprint and let it appear.
use fontdue::{Font, FontSettings};

const W: usize = 1000;
const H: usize = 1000;
const R: f32 = 1.0;

type V3 = [f32;3];
fn hue(h: f32) -> V3 { let h=h.rem_euclid(1.0); let k=|n:f32|{let t=(n+h*6.0)%6.0; (t.min(4.0-t)).clamp(0.0,1.0)}; [k(5.0),k(3.0),k(1.0)] }

struct P { p: V3, col: V3, base: f32, text: bool }

fn main() {
    let word = std::env::args().nth(1).unwrap_or_else(|| "BUILD".into());
    let bytes = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf").unwrap();
    let font = Font::from_bytes(bytes, FontSettings::default()).unwrap();

    // flat glyph points in the mid-plane (z≈0), normalized to the sphere disk
    let px=150.0; let mut raw: Vec<[f32;2]> = Vec::new(); let mut pen=0.0;
    for ch in word.chars() { let (m,bmp)=font.rasterize(ch,px);
        for yy in 0..m.height { for xx in 0..m.width { if bmp[yy*m.width+xx]>120 {
            raw.push([pen+m.xmin as f32+xx as f32, m.ymin as f32+(m.height as f32-yy as f32)]); }}}
        pen+=m.advance_width; }
    let (mut mnx,mut mny,mut mxx,mut mxy)=(f32::MAX,f32::MAX,f32::MIN,f32::MIN);
    for q in &raw { mnx=mnx.min(q[0]); mny=mny.min(q[1]); mxx=mxx.max(q[0]); mxy=mxy.max(q[1]); }
    let (cx,cy)=((mnx+mxx)*0.5,(mny+mxy)*0.5); let half=((mxx-mnx).max(mxy-mny))*0.5; let s=R*0.92/half;

    let mut rng=0x51EDu32; let mut rnd=|s:&mut u32|{*s^=*s<<13;*s^=*s>>17;*s^=*s<<5; *s as f32/u32::MAX as f32};
    let mut pts: Vec<P> = Vec::new();

    // TEXT: white points in the mid-plane, thin depth (the flat text through the middle). Stroke skeleton →
    // line-shaped clusters; brightened along the beam trajectory (left→right routing across the strokes).
    let mut text2d: Vec<[f32;2]> = Vec::new();
    for (i,q) in raw.iter().enumerate() { if i%2==0 {
        let x=(q[0]-cx)*s; let y=(q[1]-cy)*s; text2d.push([x,y]);
        let z=(rnd(&mut rng)*2.0-1.0)*0.10*R;                 // thin mid-plane slab
        pts.push(P{ p:[x,y,z], col:[1.0,1.0,1.0], base:0.0, text:true });
    }}
    let xspan=(mxx-mnx)*s;

    // BODY: hue-shifted cloud filling the ball, dim. Hue varies with position (blues/greens/reds).
    let mut made=0; while made<2600 {
        let (a,b,c)=(rnd(&mut rng)*2.0-1.0,rnd(&mut rng)*2.0-1.0,rnd(&mut rng)*2.0-1.0);
        if a*a+b*b+c*c<=1.0 {
            let h=0.55 + 0.25*b + 0.15*a;                     // mostly blue/teal, drifting to green/red
            pts.push(P{ p:[a*R,b*R,c*R], col:hue(h), base:0.12, text:false }); made+=1;
        }
    }
    eprintln!("{} points ({} text)", pts.len(), pts.iter().filter(|p|p.text).count());

    for (tag, yaw, pitch) in [("reveal",0.0_f32,0.0_f32), ("orb",1.0,0.25)] {
        let (cyaw,syaw)=(yaw.cos(),yaw.sin()); let (cp,sp)=(pitch.cos(),pitch.sin());
        // camera basis: view dir (cam→scene) and right (perpendicular, for bounce + reveal gating)
        let view: V3=[ -syaw*cp, -sp, -cyaw*cp ];
        let right: V3=[ cyaw, 0.0, -syaw ];
        let align=(-view[2]).abs();                            // 1 when looking down the reveal (+z) axis
        let scale=W as f32*0.42; let (ox,oy)=(W as f32*0.5,H as f32*0.5);
        let proj=|p:V3|->(f32,f32,f32){ let x1=p[0]*cyaw - p[2]*syaw; let z1=p[0]*syaw + p[2]*cyaw;
            let y1=p[1]*cp - z1*sp; let z2=p[1]*sp + z1*cp; (ox+x1*scale, oy-y1*scale, z2) };

        // helper: is a point's mid-plane projection on/near a text stroke? (to decide occluder handling)
        let on_text=|x:f32,y:f32|->bool{ text2d.iter().any(|t|(t[0]-x).powi(2)+(t[1]-y).powi(2) < (0.035*R).powi(2)) };

        let mut fb=vec![0.0f32; W*H*3];
        for pt in &pts {
            let mut p=pt.p;
            if !pt.text {
                // OCCLUSION KNOBS: a body point in FRONT of the text plane (toward camera) that sits over a
                // stroke would hide the text → bounce it perpendicular to the camera, dim + hue-push it.
                let front = (p[0]*view[0]+p[1]*view[1]+p[2]*view[2]) < 0.0; // between camera and mid-plane
                if front && align>0.4 && on_text(p[0],p[1]) {
                    let push=0.22*R; p=[p[0]+right[0]*push, p[1]+right[1]*push, p[2]+right[2]*push]; // bounce aside
                }
            }
            let (sx,sy,z)=proj(p); let depth=(z+R)/(2.0*R);
            if pt.text {
                // white text only blazes when looked at along the reveal axis; fades into the body off-axis
                let beam = 0.6 + 0.4*((p[0]/xspan)+0.5);        // brighter along the L→R beam routing
                let b = (0.55+0.45*depth)*beam*align.powf(2.0);
                if b>0.02 { splat(&mut fb,sx,sy,2.2,[b,b,b]); }
            } else {
                // body: dim, and extra-dim if it still lands over the text region (so text wins)
                let mut b=pt.base*(0.5+0.7*depth);
                if align>0.4 && on_text(p[0],p[1]) && (p[0]*view[0]+p[1]*view[1]+p[2]*view[2])<0.0 { b*=0.25; }
                splat(&mut fb,sx,sy,1.6,[pt.col[0]*b,pt.col[1]*b,pt.col[2]*b]);
            }
        }
        let mut out=format!("P6\n{W} {H}\n255\n").into_bytes();
        for v in &fb { let t=(v/(1.0+v)).clamp(0.0,1.0); out.push((t.powf(1.0/2.2)*255.0) as u8); }
        std::fs::write(format!("/home/claude/anamorph/cld_{tag}.ppm"),out).unwrap();
        eprintln!("wrote cld_{tag}");
    }
}
fn splat(fb:&mut[f32],cx:f32,cy:f32,r:f32,col:V3){
    if cx < -r||cy < -r||cx>W as f32+r||cy>H as f32+r {return;}
    let (x0,x1)=((cx-r).floor().max(0.0) as usize,(cx+r).ceil().min(W as f32-1.0) as usize);
    let (y0,y1)=((cy-r).floor().max(0.0) as usize,(cy+r).ceil().min(H as f32-1.0) as usize);
    for y in y0..=y1 { for x in x0..=x1 { let d2=(x as f32-cx).powi(2)+(y as f32-cy).powi(2);
        let a=(-d2/(r*r*0.5)).exp(); if a>0.003 { let i=(y*W+x)*3; fb[i]+=col[0]*a; fb[i+1]+=col[1]*a; fb[i+2]+=col[2]*a; }}}
}
