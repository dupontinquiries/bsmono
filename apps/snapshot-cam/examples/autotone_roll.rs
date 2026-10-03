//! Run `autotone` over a folder of JPEGs and write `<name>.tone.jpg` beside each, printing what was
//! found: the way to judge the tone on a real roll without a phone.
//!
//!     cargo run --no-default-features --release --example autotone_roll -- /path/to/roll [N]
//!
//! It mirrors `Cam::save_frame`'s developed path: black point as the flare, white balance and the
//! exposure bias folded into the gains, `develop::render` at native size.

use snapshot_cam::autotone;
use snapshot_cam::develop::{render, Params};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("a folder of .jpg files");
    let limit: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(usize::MAX);
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jpg") && !p.to_string_lossy().ends_with(".tone.jpg")).collect();
    files.sort();
    for f in files.into_iter().take(limit) {
        let img = image::open(&f).unwrap().to_rgba8();
        let (w, h) = img.dimensions();
        let t = autotone::measure(img.as_raw(), w, h);
        let mut p = Params::identity(w, h);
        p.flare = [t.black; 3];
        p.gains = t.total_gains();
        let out = render(img.as_raw(), w, h, &p);
        let rgb: Vec<u8> = out.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        let dst = f.with_extension("tone.jpg");
        image::RgbImage::from_raw(w, h, rgb).unwrap().save(&dst).unwrap();
        println!("{}: black {:.3} gains [{:.2} {:.2} {:.2}] ev {:+.2}", f.file_name().unwrap().to_string_lossy(), t.black, t.gains[0], t.gains[1], t.gains[2], t.ev);
    }
}
