//! The cartoon fixture (fixtures/cartoon-fragile, made by
//! `examples/cartoon_fragile`) behaves the way it was designed to, so the
//! film tools can be exercised without the real footage. Skips when the
//! fixture or ffmpeg is absent.

use std::path::PathBuf;
use studio_core::*;

fn dir() -> Option<PathBuf> {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cartoon-fragile");
    d.join("fragile-cartoon.studio").exists().then_some(d)
}

fn project(d: &std::path::Path) -> Project {
    let bytes = std::fs::read(d.join("fragile-cartoon.studio")).unwrap();
    studio_collab::file::SaveFile::decode(&bytes).expect("the fixture project opens").to_project()
}

#[test]
fn the_fixture_project_opens_and_its_camera_group_spans_the_overlap() {
    let Some(d) = dir() else { eprintln!("skip: no fixture"); return };
    let p = project(&d);
    assert_eq!(p.clips.len(), 4, "three angles and a recorder");
    for s in p.sources.values() { assert!(d.join(&s.path).exists(), "{} is beside the project", s.path); }
    let (angles, start, end) = studio_video::multicam::from_group(&p, GroupId(1)).expect("a camera group");
    assert_eq!(angles.len(), 3);
    // scene 0 s is timeline 2 s; the last camera rolled at 0.9 s; the first stops at 14 s
    assert!((start.to_seconds() - 2.9).abs() < 1e-6 && (end.to_seconds() - 16.0).abs() < 1e-6, "{start:?}..{end:?}");
    assert!(p.busses.values().any(|b| b.silent), "a control channel");
}

#[test]
fn every_camera_syncs_to_the_recorder_at_its_roll() {
    let Some(d) = dir() else { eprintln!("skip: no fixture"); return };
    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() { eprintln!("skip: no ffmpeg"); return; }
    let rec = studio_media::decode_bytes(std::fs::read(d.join("R001_recorder.flac")).unwrap(), Some("flac")).expect("the recorder decodes");
    let sounds = vec![studio_video::dualsync::Onsets::of(&rec).expect("recorder onsets")];
    let rolls = [("A001_wide", 0.0), ("A002_close_left", 0.4), ("A003_close_right", 0.9)];
    let pics: Vec<_> = rolls.iter().map(|(n, _)| {
        let a = studio_video::extract_audio(&d.join(format!("{n}.mp4"))).unwrap().expect("camera audio");
        studio_video::dualsync::Onsets::of(&a).expect("camera onsets")
    }).collect();
    let m = studio_video::dualsync::match_pool(&pics, &sounds);
    for ((n, roll), mm) in rolls.iter().zip(&m) {
        let mm = mm.as_ref().unwrap_or_else(|| panic!("{n}: no match"));
        // the recorder rolled at scene -1.5 s: picture 0 is recorder (roll + 1.5)
        let want = -(roll + 1.5);
        assert!((mm.offset_seconds() - want).abs() <= 0.03, "{n}: offset {} want {want}", mm.offset_seconds());
    }
}

#[test]
fn the_transcripts_load_beside_the_media() {
    let Some(d) = dir() else { eprintln!("skip: no fixture"); return };
    let p = project(&d);
    let l = crate::transcripts::load(&p, &d);
    assert_eq!(l.timed, 3, "one per camera: {}", l.summary());
    // and the way the app finds them on opening: beside each source, and the
    // fixture's sources are RELATIVE -- they must resolve against the project's
    // folder, not the working directory (they did not, until this test)
    let auto = crate::transcripts::beside_media(&p, &d);
    assert_eq!(auto.timed, 3, "found beside relative sources: {}", auto.summary());
}

#[test]
fn each_close_mic_is_louder_for_its_own_sides_lines() {
    let Some(d) = dir() else { eprintln!("skip: no fixture"); return };
    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() { eprintln!("skip: no ffmpeg"); return; }
    let rms = |name: &str, roll: f64, a: f64, b: f64| {
        let au = studio_video::extract_audio(&d.join(format!("{name}.mp4"))).unwrap().unwrap();
        let sr = au.sample_rate as f64;
        let c = &au.channels[0];
        let (i0, i1) = (((a - roll) * sr) as usize, (((b - roll) * sr) as usize).min(c.len()));
        (c[i0..i1].iter().map(|v| v * v).sum::<f32>() / (i1 - i0) as f32).sqrt()
    };
    // a men's line (Max, 2.0-4.2 s) and the women's line (10.5-12.5 s), scene time
    let (l_men, r_men) = (rms("A002_close_left", 0.4, 2.2, 4.0), rms("A003_close_right", 0.9, 2.2, 4.0));
    let (l_wom, r_wom) = (rms("A002_close_left", 0.4, 10.7, 12.3), rms("A003_close_right", 0.9, 10.7, 12.3));
    let db = |a: f32, b: f32| 20.0 * (a / b).log10();
    assert!(db(r_men, l_men) > 6.0, "the right mic hears the men louder: {:.1} dB", db(r_men, l_men));
    assert!(db(l_wom, r_wom) > 6.0, "the left mic hears the women louder: {:.1} dB", db(l_wom, r_wom));
}
