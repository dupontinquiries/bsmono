//! Render a scene as a comic page with the auto mode, through Studio's own code.
//!
//!     cargo run --release -p studio-video --example comic_render -- WIDE LEFT RIGHT OUT
//!
//! Each of WIDE/LEFT/RIGHT is `path@start`; 25 s from each. The dialogue lines
//! and their sides are the rooftop's (C0035 30-55 s), hard-coded here for the
//! demo: in Studio they come from the transcript and a side per line.
use std::path::PathBuf;
use studio_video::comic::{self, Line, Region, Side, Style};
use studio_video::framepipe::{render_multi, Pipe, Size};

fn pipe(spec: &str) -> Pipe {
    let (p, at) = spec.rsplit_once('@').expect("path@start");
    Pipe { input: PathBuf::from(p), range: Some((at.parse().unwrap(), 25.0)), size: Size { w: 960, h: 540 }, fps: 24.0 }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (wide, left, right, out) = (pipe(&a[0]), pipe(&a[1]), pipe(&a[2]), PathBuf::from(&a[3]));
    let lines = [(0.0, 7.86, Side::Right), (9.56, 13.10, Side::Right), (13.48, 14.58, Side::Right),
                 (15.32, 17.12, Side::Right), (18.96, 20.72, Side::Right), (21.58, 22.84, Side::Right),
                 (23.04, 23.46, Side::Right), (23.82, 25.0, Side::Left)]
        .map(|(s, e, side)| Line { start: s, end: e, side });
    let shots = comic::plan(&lines, 25.0, comic::Rules::default());
    for s in &shots { println!("{:5.2}-{:5.2}s {:?}", s.start, s.end, s.layout); }
    let style = Style::default();
    let page = comic::paper(960, 540, &style);
    // the women sit on the left of C0033; the men fill C0035
    let left_region = Region { x0: 0.0, y0: 0.1, x1: 0.52, y1: 0.9 };
    let t0 = std::time::Instant::now();
    let n = render_multi(&[wide, left, right], &out, 18, 2, |i, f| {
        let t = i as f64 / 24.0;
        let layout = shots.iter().find(|s| t >= s.start && t < s.end).map(|s| s.layout).unwrap_or(comic::Layout::Wide);
        comic::compose(layout, &f[0], (&f[1], left_region), (&f[2], Region::WHOLE), &page, &style, (t * 1.5) as f32)
    }).expect("renders");
    println!("{n} frames in {:.1?} -> {}", t0.elapsed(), out.display());
}
