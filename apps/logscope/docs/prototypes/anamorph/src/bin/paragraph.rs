//! Density experiment: a full paragraph in one cluster, rendered two ways —
//!  DENSE  = one splat per sampled stroke pixel (many particles), and
//!  GROWN  = far fewer particles but each a brighter soft emitter + a bloom/scatter "grow" pass
//!           (light spreading) so sparse points coalesce into legible strokes.
use fontdue::{Font, FontSettings};

const W: usize = 1200;
const H: usize = 1200;

fn main() {
    let bytes = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf").unwrap();
    let font = Font::from_bytes(bytes, FontSettings::default()).unwrap();
    let para = "Here's a starting point using std::io and a seek cursor. It watches \
the inode; on change it reopens the path. Track the offset and reset to zero when the file \
shrinks, and stream rather than buffer for bounded memory on huge logs.";

    // wrap to lines, rasterize a block of glyph stroke pixels at modest px
    let px = 30.0_f32;
    let wrap = 34usize;
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in para.split_whitespace() {
        if cur.len() + w.len() + 1 > wrap { lines.push(std::mem::take(&mut cur)); }
        if !cur.is_empty() { cur.push(' '); } cur.push_str(w);
    }
    if !cur.is_empty() { lines.push(cur); }

    // collect all stroke pixels (x,y) in block space
    let mut strokes: Vec<(f32, f32)> = Vec::new();
    let line_h = px * 1.25;
    for (li, line) in lines.iter().enumerate() {
        let mut pen = 0.0;
        for ch in line.chars() {
            let (m, bmp) = font.rasterize(ch, px);
            for yy in 0..m.height { for xx in 0..m.width {
                if bmp[yy * m.width + xx] > 110 {
                    let gx = pen + m.xmin as f32 + xx as f32;
                    let gy = -(li as f32 * line_h) + m.ymin as f32 + (m.height as f32 - yy as f32);
                    strokes.push((gx, gy));
                }
            }}
            pen += m.advance_width;
        }
    }
    eprintln!("paragraph: {} lines, {} stroke pixels", lines.len(), strokes.len());

    // normalize block to fit the canvas
    let (mut mnx, mut mny, mut mxx, mut mxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in &strokes { mnx = mnx.min(x); mny = mny.min(y); mxx = mxx.max(x); mxy = mxy.max(y); }
    let s = (W as f32 * 0.86) / (mxx - mnx);
    let (ox, oy) = (W as f32 * 0.07, H as f32 * 0.5 - (mxy + mny) * 0.5 * s);
    let map = |x: f32, y: f32| ((x - mnx) * s + ox, oy - (y - mxy) * s);

    // ── DENSE: a splat per stroke pixel ──
    let mut dense = vec![0.0f32; W * H * 3];
    for &(x, y) in &strokes { let (sx, sy) = map(x, y); splat(&mut dense, sx, sy, 1.3, [0.55, 1.0, 0.95]); }
    write_png(&dense, "para_dense", strokes.len());

    // ── GROWN: keep ~1 of every N stroke pixels, brighter big soft emitters, then a scatter/bloom grow ──
    let keep = 9; // ~9× fewer particles
    let mut grown = vec![0.0f32; W * H * 3];
    let mut kept = 0;
    for (i, &(x, y)) in strokes.iter().enumerate() {
        if i % keep == 0 { let (sx, sy) = map(x, y); splat(&mut grown, sx, sy, 2.4, [0.6, 1.0, 0.95]); kept += 1; }
    }
    // optical "grow": light scatter (blur) added back, then re-emphasise bright cores → sparse dots coalesce
    let blur = box_blur(&grown, 2);
    for i in 0..grown.len() { grown[i] = grown[i] * 0.7 + blur[i] * 1.6; }
    write_png(&grown, "para_grown", kept);
}

fn splat(fb: &mut [f32], cx: f32, cy: f32, r: f32, col: [f32; 3]) {
    if cx < -r || cy < -r || cx > W as f32 + r || cy > H as f32 + r { return; }
    let (x0, x1) = ((cx - r).floor().max(0.0) as usize, (cx + r).ceil().min(W as f32 - 1.0) as usize);
    let (y0, y1) = ((cy - r).floor().max(0.0) as usize, (cy + r).ceil().min(H as f32 - 1.0) as usize);
    for y in y0..=y1 { for x in x0..=x1 {
        let d2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
        let a = (-d2 / (r * r * 0.5)).exp();
        if a > 0.003 { let i = (y * W + x) * 3; fb[i] += col[0] * a; fb[i + 1] += col[1] * a; fb[i + 2] += col[2] * a; }
    }}
}
fn box_blur(src: &[f32], rad: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; src.len()];
    for y in 0..H { for x in 0..W { for c in 0..3 {
        let mut acc = 0.0; let mut n = 0.0;
        for dx in -(rad as isize)..=rad as isize {
            let xx = x as isize + dx; if xx < 0 || xx >= W as isize { continue; }
            acc += src[(y * W + xx as usize) * 3 + c]; n += 1.0;
        }
        tmp[(y * W + x) * 3 + c] = acc / n;
    }}}
    let mut out = vec![0.0f32; src.len()];
    for y in 0..H { for x in 0..W { for c in 0..3 {
        let mut acc = 0.0; let mut n = 0.0;
        for dy in -(rad as isize)..=rad as isize {
            let yy = y as isize + dy; if yy < 0 || yy >= H as isize { continue; }
            acc += tmp[(yy as usize * W + x) * 3 + c]; n += 1.0;
        }
        out[(y * W + x) * 3 + c] = acc / n;
    }}}
    out
}
fn write_png(fb: &[f32], tag: &str, n: usize) {
    let mut out = format!("P6\n{W} {H}\n255\n").into_bytes();
    for v in fb { let t = (v / (1.0 + v)).clamp(0.0, 1.0); out.push((t.powf(1.0 / 2.2) * 255.0) as u8); }
    std::fs::write(format!("/home/claude/anamorph/{tag}.ppm"), out).unwrap();
    eprintln!("wrote {tag} ({} particles)", n);
}
