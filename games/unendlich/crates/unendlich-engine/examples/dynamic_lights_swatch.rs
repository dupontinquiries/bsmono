// S1 headless proof: dynamic point lights on a dark plane (attenuation + colour mix).
use unendlich_engine::pngutil::write_png;
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
