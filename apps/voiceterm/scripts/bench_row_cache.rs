use std::time::Instant;
fn render_row_html(out: &mut String, cells: &[(char,u16,u16,u8)]) {
    let mut cur_fg = u16::MAX;
    for &(ch,fg,_,_) in cells {
        if fg != cur_fg { out.push_str("<span style='color:#"); out.push_str(&format!("{:06x}", fg as u32*0x010101)); out.push_str("'>"); cur_fg=fg; }
        out.push(ch);
    }
    out.push_str("</span>\n");
}
fn main() {
    let rows=50; let cols=200; let frames=10_000;
    let mut grid: Vec<Vec<(char,u16,u16,u8)>> = (0..rows).map(|r| (0..cols).map(|c| ((b'a'+((r+c)%26) as u8) as char,7u16,0u16,0u8)).collect()).collect();

    // OLD: full serialize
    let t0=Instant::now(); let mut s=0usize;
    for f in 0..frames { grid[f%rows][0].0 = if f&1==0 {'X'} else {'Y'};
        let mut out=String::with_capacity(rows*cols*8); out.push_str("<pre>");
        for r in &grid { render_row_html(&mut out,r); } out.push_str("</pre>"); s=s.wrapping_add(out.len()); }
    let old=t0.elapsed();

    // NEW: parser hands us the dirty row set (here: the 1 row it wrote).
    // We serialize ONLY those, never touch the rest.
    let t1=Instant::now(); let mut s2=0usize;
    for f in 0..frames { let dirty = f%rows; grid[dirty][0].0 = if f&1==0 {'X'} else {'Y'};
        // patch just the dirty row
        let mut out=String::with_capacity(cols*8); render_row_html(&mut out,&grid[dirty]); s2=s2.wrapping_add(out.len()); }
    let new=t1.elapsed();

    println!("frames={frames} grid={rows}x{cols}, parser-dirty=1 row/frame");
    println!("OLD full-serialize  : {:?} ({:.2} us/frame)", old, old.as_micros() as f64/frames as f64);
    println!("NEW dirty-row patch : {:?} ({:.2} us/frame)", new, new.as_micros() as f64/frames as f64);
    println!("speedup             : {:.1}x", old.as_secs_f64()/new.as_secs_f64());
    println!("(sinks {} {})", s&0xff, s2&0xff);
}
