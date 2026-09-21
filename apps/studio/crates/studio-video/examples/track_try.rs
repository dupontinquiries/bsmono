//! Track hand-placed points through a real shot and print the rig per second.
//!     cargo run -p studio-video --example track_try -- SHOT START_S SECONDS x,y x,y ...
use studio_video::framepipe::{Pipe, Size};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (start, secs): (f64, f64) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let size = Size { w: 960, h: 540 };
    let pts: Vec<(f32, f32)> = a[3..].iter().map(|p| { let (x, y) = p.split_once(',').unwrap();
        (x.parse::<f32>().unwrap() * size.w as f32, y.parse::<f32>().unwrap() * size.h as f32) }).collect();
    let fps = 25.0;
    let grey = Pipe { input: a[0].clone().into(), range: Some((start, secs)), size, fps }.gray().unwrap();
    let patch = ((size.w / 80).max(4)) as i32;
    let solved = studio_video::tracker::track(&grey, &pts, patch, (size.w / 40).max(6) as i32, 22.0);
    let c0 = (pts.iter().map(|p| p.0).sum::<f32>() / pts.len() as f32, pts.iter().map(|p| p.1).sum::<f32>() / pts.len() as f32);
    for (i, s) in solved.iter().enumerate().step_by(25) {
        let c = s.rig.apply(c0);
        println!("{:4.1}s  centre ({:5.0},{:5.0})  seen {}/{}  held {}  rot {:+.2}  scale {:.2}",
            i as f64 / fps, c.0, c.1, s.points.iter().filter(|p| p.is_some()).count(), pts.len(), s.held, s.rig.rot, s.rig.scale);
    }
}
