fn clamp01(v:f32)->f32{ if v.is_nan(){0.0}else{v.max(0.0).min(1.0)} }
use unendlich_engine::pngutil::write_png;
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
