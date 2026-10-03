//! Try single-frame "virtual bracket" fusion on a folder of JPEGs: three exposures made from one frame
//! (black point + white balance from `autotone`, then -1 / 0 / +1 stop) blended by Mertens exposure
//! fusion, written as `<name>.fused.jpg`. A way to judge what software alone can do for a photo whose
//! highlights are already in the frame.
//!
//!     cargo run --no-default-features --release --example fusion_roll -- /path/to/roll [N]

use snapshot_cam::autotone;
use snapshot_cam::develop::{render, Params};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("a folder of .jpg files");
    let limit: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(usize::MAX);
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jpg") && !p.to_string_lossy().ends_with(".tone.jpg") && !p.to_string_lossy().ends_with(".fused.jpg")).collect();
    files.sort();
    for f in files.into_iter().take(limit) {
        let img = image::open(&f).unwrap().to_rgba8();
        let (w, h) = img.dimensions();
        // exactly what the shutter does: develop (black point, white balance, bias), then fuse
        let t = autotone::measure(img.as_raw(), w, h);
        let mut p = Params::identity(w, h);
        p.flare = [t.black; 3];
        p.gains = t.total_gains();
        let developed = render(img.as_raw(), w, h, &p);
        let out = snapshot_cam::fusion::fuse_brackets(&developed, w, h);
        let rgb: Vec<u8> = out.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        image::RgbImage::from_raw(w, h, rgb).unwrap().save(f.with_extension("fused.jpg")).unwrap();
        let scenario = snapshot_cam::fusion::stats(img.as_raw(), w, h).map(|s| snapshot_cam::fusion::judge(&s).label()).unwrap_or("?");
        println!("{}: {scenario}", f.file_name().unwrap().to_string_lossy());
    }
}
