//! Stills of Studio's film tools on the cartoon fixture.
//!
//!     cargo run -p studio-app --example cartoon_showcase -- FIXTURE_DIR OUT_DIR [PINK_REFERENCE_VIDEO@SECONDS]
//!
//! Each still goes through the same code the app uses: the comic plan and
//! compositor (sides from the close mics), the kaleidoscope, style transfer,
//! particles riding a tracked hand, and the follow-motion punch-in.
use std::path::{Path, PathBuf};
use studio_core::*;
use studio_video::comic::{self, Side};

const W: u32 = 320;
const H: u32 = 180;

fn save(px: &[u8], w: u32, h: u32, out: &Path) {
    let ppm = out.with_extension("ppm");
    let mut b = format!("P6\n{w} {h}\n255\n").into_bytes();
    b.extend_from_slice(px);
    std::fs::write(&ppm, b).unwrap();
    // upscale 2x with nearest: the cartoon stays crisp
    std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-i"]).arg(&ppm)
        .args(["-vf", "scale=iw*2:ih*2:flags=neighbor"]).arg(out).status().unwrap();
    let _ = std::fs::remove_file(&ppm);
    println!("wrote {}", out.display());
}

fn frame(dir: &Path, file: &str, t: f64) -> Vec<u8> {
    studio_video::frame_rgb(&dir.join(file), Ticks::from_seconds(t.max(0.0)), W, H).expect("frame")
}

fn to_frame(px: Vec<u8>) -> studio_video::framepipe::Frame { studio_video::framepipe::Frame { w: W, h: H, px } }

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (dir, out) = (PathBuf::from(&a[0]), PathBuf::from(&a[1]));
    std::fs::create_dir_all(&out).unwrap();
    let bytes = std::fs::read(dir.join("fragile-cartoon.studio")).unwrap();
    let p = studio_collab::file::SaveFile::decode(&bytes).unwrap().to_project();
    let group = GroupId(1);
    let (angles, start, end) = studio_video::multicam::from_group(&p, group).unwrap();
    let span = (end.0 - start.0) as f64 / TICKS_PER_SECOND as f64;
    // file names by angle, and each angle's roll (its offset from the group start)
    let files: Vec<String> = angles.iter().map(|a| p.sources[&a.source].path.clone()).collect();
    let file_t = |k: usize, t: f64| start.to_seconds() + angles[k].offset + t;   // group time -> that file's time

    // ── 1. the comic auto mode: lines from the transcripts, sides from the close mics
    let words = studio_app::transcripts::load(&p, &dir).words;
    let aud: Vec<_> = files.iter().map(|f| studio_video::extract_audio(&dir.join(f)).unwrap().unwrap()).collect();
    let rms = |k: usize, a: f64, b: f64| {
        let au = &aud[k];
        let sr = au.sample_rate as f64;
        let c = &au.channels[0];
        let (i0, i1) = ((file_t(k, a) * sr) as usize, ((file_t(k, b) * sr) as usize).min(c.len()));
        if i1 <= i0 { return 0.0f32; }
        (c[i0..i1].iter().map(|v| v * v).sum::<f32>() / (i1 - i0) as f32).sqrt()
    };
    let side_at = |a: f64, b: f64| -> Option<Side> {
        let (l, r) = (rms(1, a, b), rms(2, a, b));
        let db = 20.0 * (l.max(1e-6) / r.max(1e-6)).log10();
        if db > 3.0 { Some(Side::Left) } else if db < -3.0 { Some(Side::Right) } else { None }
    };
    let lines = studio_app::comic_job::lines(&p, group, start, span, &words, &side_at);
    println!("comic: {} lines -> {:?}", lines.len(), lines.iter().map(|l| (l.start as f32, l.side)).collect::<Vec<_>>());
    let shots = comic::plan(&lines, span, comic::Rules::default());
    for s in &shots { println!("  {:5.2}-{:5.2} {:?}", s.start, s.end, s.layout); }
    let style = comic::Style::default();
    let page = comic::paper(W, H, &style);
    let mut seen = std::collections::HashSet::new();
    for s in &shots {
        let kind = format!("{:?}", s.layout).split(' ').next().unwrap().to_string();
        if !seen.insert(kind.clone()) { continue; }
        let t = (s.start + s.end) / 2.0;
        let f: Vec<_> = (0..3).map(|k| to_frame(frame(&dir, &files[k], file_t(k, t)))).collect();
        let img = comic::compose(s.layout, &f[0], (&f[1], comic::Region::WHOLE), (&f[2], comic::Region::WHOLE), &page, &style, (t * 1.5) as f32);
        save(&img.px, W, H, &out.join(format!("1-comic-{}.png", kind.to_lowercase())));
    }

    // ── 2. the kaleidoscope: six wedges, turned, on the wide
    let wide = frame(&dir, &files[0], file_t(0, 3.0));
    // centred on the men's heads: centred on the empty couch it was just a grey star
    let k = studio_video::kaleido::Kaleido { segments: 6, angle: 30.0, cx: 0.62, cy: 0.45 };
    save(&studio_video::kaleido::apply(&wide, W, H, &k), W, H, &out.join("2-kaleidoscope-6-turned.png"));
    let k8 = studio_video::kaleido::Kaleido { segments: 8, angle: 0.0, cx: 0.5, cy: 0.5 };
    save(&studio_video::kaleido::apply(&wide, W, H, &k8), W, H, &out.join("2-kaleidoscope-8.png"));

    // ── 3. style transfer: a real look (the pink of "All My Life") onto the cartoon
    if let Some(spec) = a.get(2) {
        let (path, at) = spec.rsplit_once('@').unwrap();
        let reference = studio_video::frame_rgb(Path::new(path), Ticks::from_seconds(at.parse().unwrap()), W, H).unwrap();
        let t = studio_video::style::Transfer::fit(&studio_video::style::stats(&wide), &studio_video::style::stats(&reference), 1.0);
        let mut styled = wide.clone();
        studio_video::style::apply_lut(&mut styled, &t.lut(studio_video::style::LUT_N), studio_video::style::LUT_N);
        save(&styled, W, H, &out.join("3-style-pink-look.png"));
        // at 60%: a night scene pulled all the way to a bright pink one lifts the
        // shadows so far that the encode's ringing shows as dark fringes
        let gentle = studio_video::style::Transfer { strength: 0.6, ..t };
        let mut soft = wide.clone();
        studio_video::style::apply_lut(&mut soft, &gentle.lut(studio_video::style::LUT_N), studio_video::style::LUT_N);
        save(&soft, W, H, &out.join("3-style-pink-look-60.png"));
        save(&reference, W, H, &out.join("3-style-reference.png"));
    }
    save(&wide, W, H, &out.join("0-wide-plain.png"));

    // ── 4. particles: dust riding Jude's hand (tracked on the close right), smoke from the lamp
    let cr = &files[2];
    let (t0, t1) = (4.0f64, 6.4f64);   // scene 5.0-7.4 s is the gesture; group time = scene - 0.9
    let fps = 24.0;
    let grey = studio_video::framepipe::Pipe { input: dir.join(cr), range: Some((file_t(2, t0), t1 - t0)),
        size: studio_video::framepipe::Size { w: W, h: H }, fps }.gray().unwrap();
    // points on the hand and forearm in the first frame (close right: hand near x 240, y 115)
    let pts = studio_video::tracker::pick_points(&grey[0], (245.0, 112.0), 22.0, 4, 4);
    let solved = studio_video::tracker::track(&grey, &pts, 4, 8, 22.0);
    println!("particles: {} points on the hand; rig held in {} of {} frames", pts.len(),
             solved.iter().filter(|s| s.held).count(), solved.len());
    let mut dust = studio_video::particles::System::new(studio_video::particles::Params::space_dust(), 7);
    let mut smoke = studio_video::particles::System::new(studio_video::particles::Params {
        size: (3.0, 12.0), swirl_scale: 25.0, ..studio_video::particles::Params::creepy_smoke() }, 8);
    // The rig above HOLDS on this cartoon hand (a small flat-colour circle moving
    // across a changing background: the patch around each point never matches its
    // first appearance again). The motion path -- the same one the punch-in
    // follows -- tracks the centre of everything moving on Jude (his talking bob
    // and his hand), so the dust rides that here, and is labelled so.
    let centroids = studio_video::action::motion_centroid(&grey, 8);
    let path = studio_video::action::follow_path(&centroids, fps, 0.15, 1.0);
    let mut last = Vec::new();
    for i in 0..grey.len() {
        let (fx, fy) = path[i.min(path.len() - 1)];
        dust.step(1.0 / fps as f32, (fx * W as f32, fy * H as f32), 0.6);
        smoke.step(1.0 / fps as f32, (35.0, 170.0), 0.5);
        if i == grey.len() - 1 {
            let mut f = to_frame(frame(&dir, cr, file_t(2, t0) + i as f64 / fps));
            smoke.draw(&mut f);
            dust.draw(&mut f);
            last = f.px;
        }
    }
    let _ = &solved;
    save(&last, W, H, &out.join("4-particles-dust-follows-the-motion.png"));

    // ── 5. rescue a take: punch in on what moves and follow it
    let path = studio_video::action::follow_path(&centroids, fps, 0.5, 2.0);
    let i = path.len() * 2 / 3;
    let src = frame(&dir, cr, file_t(2, t0) + i as f64 / fps);
    let (cx, cy) = path[i];
    let f = to_frame(src).warp(W as f32 / 2.0 - cx * W as f32, H as f32 / 2.0 - cy * H as f32, 2.0);
    save(&f.px, W, H, &out.join("5-follow-the-motion-2x.png"));
}
