fn clamp01(v:f32)->f32{ if v.is_nan(){0.0}else{v.max(0.0).min(1.0)} }
const BACK_FLOOR:f32=0.25;
fn belly_weight(ny:f32)->f32{ let d=clamp01(-ny); BACK_FLOOR+(1.0-BACK_FLOOR)*d }
fn apply_grime(base:[f32;3], ny:f32, grime:f32, tint:[f32;3])->[f32;3]{
    let a=clamp01(grime*belly_weight(ny));
    let mut o=[0.0;3]; for c in 0..3 { o[c]=base[c]+(tint[c]*0.6-base[c])*a; } o
}
fn main(){
    let (sw,sh,pad)=(120usize,200usize,12usize);
    let levels=[0.0f32,0.25,0.5,0.75,1.0];
    let w=pad+(sw+pad)*levels.len(); let h=sh+2*pad;
    let mut img=vec![18u8; w*h*4]; for p in img.chunks_mut(4){p[3]=255;}
    let base=[0.62,0.58,0.50]; let mud=[0.34,0.22,0.12];
    for (li,&g) in levels.iter().enumerate(){
        let x0=pad+li*(sw+pad);
        for yy in 0..sh {
            let t=yy as f32/(sh-1) as f32; let ny=1.0-2.0*t;
            let col=apply_grime(base,ny,g,mud);
            for xx in 0..sw { let px=((pad+yy)*w+(x0+xx))*4;
                img[px]=(col[0]*255.0)as u8; img[px+1]=(col[1]*255.0)as u8; img[px+2]=(col[2]*255.0)as u8; }
        }
    }
    write_png("creature_grime_swatch.png",w,h,&img).unwrap();
    println!("wrote creature_grime_swatch.png {}x{}",w,h);
}
// ── dependency-free PNG encoder (8-bit RGBA, stored zlib) ──
fn write_png(path: &str, w: usize, h: usize, rgba: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    chunk(&mut out, b"IDAT", &zlib_store(&raw));
    chunk(&mut out, b"IEND", &[]);
    std::fs::File::create(path)?.write_all(&out)
}
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = 0xffff_ffffu32;
    for b in kind.iter().chain(data.iter()) {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    out.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
}
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let mut i = 0;
    while i < data.len() {
        let n = (data.len() - i).min(0xffff);
        let fin = i + n >= data.len();
        z.push(if fin { 1 } else { 0 });
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&data[i..i + n]);
        i += n;
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    z
}
