// S1 headless proof: dynamic point lights on a dark plane (attenuation + colour mix).
#[derive(Clone,Copy)] struct PL{pos:[f32;3],col:[f32;3],i:f32,r:f32}
fn d2(a:[f32;3],b:[f32;3])->f32{let d=[a[0]-b[0],a[1]-b[1],a[2]-b[2]];d[0]*d[0]+d[1]*d[1]+d[2]*d[2]}
fn atten(l:&PL,p:[f32;3])->f32{let r=l.r.max(1e-4);let dd=d2(l.pos,p);let r2=r*r; if !(dd<r2){return 0.0;} let w=1.0-dd/r2;(1.0/(1.0+dd))*(w*w).max(0.0)}
fn apply(p:[f32;3],ls:&[PL])->[f32;3]{let mut a=[0.0;3];for l in ls{let k=atten(l,p)*l.i.max(0.0);for c in 0..3{a[c]+=l.col[c].max(0.0)*k;}}[a[0].min(1.0),a[1].min(1.0),a[2].min(1.0)]}
fn main(){
    let (w,h)=(512usize,512usize);
    // a 20x20 world plane; lights placed in it
    let span=20.0f32;
    let lights=[
        PL{pos:[5.0,0.0,5.0],col:[1.0,0.55,0.15],i:6.0,r:9.0},   // forge glow (warm)
        PL{pos:[14.0,0.0,7.0],col:[0.2,0.5,1.0],i:5.0,r:8.0},    // eye (cool)
        PL{pos:[10.0,0.0,15.0],col:[1.0,0.2,0.1],i:7.0,r:7.0},   // fire (red)
    ];
    let mut img=vec![0u8;w*h*4]; for p in img.chunks_mut(4){p[3]=255;}
    for y in 0..h { for x in 0..w {
        let wx=x as f32/(w-1)as f32*span; let wz=y as f32/(h-1)as f32*span;
        let c=apply([wx,0.0,wz],&lights);
        let amb=0.04; let px=(y*w+x)*4;
        img[px]=(((c[0]+amb).min(1.0))*255.0)as u8;
        img[px+1]=(((c[1]+amb).min(1.0))*255.0)as u8;
        img[px+2]=(((c[2]+amb).min(1.0))*255.0)as u8;
    }}
    write_png("dynamic_lights_swatch.png",w,h,&img).unwrap();
    println!("wrote dynamic_lights_swatch.png {}x{} — 3 emissive point lights, inverse-square+radius window",w,h);
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
