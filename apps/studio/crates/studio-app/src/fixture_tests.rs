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

fn fixture(name: &str) -> Option<PathBuf> {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(name);
    d.exists().then_some(d)
}

fn ffmpeg_here() -> bool { std::process::Command::new("ffmpeg").arg("-version").output().is_ok() }

/// Pull the numbers after `"key": ` for every occurrence (the truth files are
/// small and written by our own generator; no JSON dependency needed).
fn json_numbers(text: &str, key: &str) -> Vec<f64> {
    text.split(&format!("\"{key}\": ")).skip(1)
        .filter_map(|s| s.split(|c: char| c == ',' || c == '}' || c == '\n').next()?.trim().parse().ok()).collect()
}

#[test]
fn the_cut_detector_finds_every_cartoon_cut_on_its_frame() {
    let Some(d) = fixture("cartoon-allmylife") else { eprintln!("skip: no fixture"); return };
    if !ffmpeg_here() { eprintln!("skip: no ffmpeg"); return; }
    let truth = std::fs::read_to_string(d.join("truth.json")).unwrap();
    let frames_truth: Vec<usize> = json_numbers(&truth, "frame").into_iter().map(|f| f as usize).collect();
    let kinds: Vec<&str> = truth.split("\"kind\": \"").skip(1).map(|s| s.split('"').next().unwrap()).collect();
    let pipe = studio_video::framepipe::Pipe { input: d.join("allmylife-cartoon.mp4"), range: None,
        size: studio_video::framepipe::Size { w: 160, h: 90 }, fps: 24.0 };
    let ev = studio_video::cuts::detect(&pipe.rgb().unwrap());
    let found = studio_video::cuts::cut_frames(&ev);
    for (f, k) in frames_truth.iter().zip(&kinds) {
        match *k {
            "fade" => assert!(ev.iter().any(|e| matches!(e, studio_video::cuts::Event::Fade { start, to_black: true, .. } if start.abs_diff(*f) <= 2)),
                              "the fade at frame {f} is reported as a fade: {ev:?}"),
            _ => assert!(found.iter().any(|c| c.abs_diff(*f) <= 1), "the {k} cut at frame {f} is found within a frame: {found:?}"),
        }
    }
    let hard_or_glitch = kinds.iter().filter(|k| **k != "fade").count();
    assert_eq!(found.len(), hard_or_glitch, "no extra cuts -- not in the spinning kaleidoscopes, and the four-frame glitch once: {found:?}");
}

#[test]
fn the_cut_detector_agrees_with_the_analysis_of_the_real_edit() {
    let Some(d) = fixture("real-proxies") else { eprintln!("skip: no fixture"); return };
    if !ffmpeg_here() { eprintln!("skip: no ffmpeg"); return; }
    let text = std::fs::read_to_string(d.join("allmylife-analysis.json")).unwrap();
    let est: Vec<f64> = text.split("\"cuts_estimated\": [").nth(1).unwrap().split(']').next().unwrap()
        .split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let pipe = studio_video::framepipe::Pipe { input: d.join("allmylife-256.mp4"), range: None,
        size: studio_video::framepipe::Size { w: 160, h: 90 }, fps: 30.0 };
    let found: Vec<f64> = studio_video::cuts::cut_frames(&studio_video::cuts::detect(&pipe.rgb().unwrap()))
        .into_iter().map(|f| f as f64 / 30.0).collect();
    // the estimates are estimates (the script's own detector, and the credits fade
    // together into one "shot"): agreement, not identity, is the bar
    let hit = est.iter().filter(|t| found.iter().any(|f| (f - *t).abs() <= 2.0 / 30.0)).count();
    let extra = found.iter().filter(|f| !est.iter().any(|t| (*f - t).abs() <= 2.0 / 30.0)).count();
    eprintln!("real edit: {hit}/{} estimated cuts found, {extra} found that the estimate lacks", est.len());
    assert!(hit as f64 >= 0.85 * est.len() as f64, "{hit}/{} of the analysis's cuts", est.len());
}

#[test]
fn every_fixture_project_opens_and_finds_its_media() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    if !root.exists() { eprintln!("skip: no fixtures"); return; }
    let found = crate::open_project::find(&[root.clone()], 3, 100);
    let names: Vec<String> = found.iter().map(|p| p.strip_prefix(&root).unwrap().display().to_string()).collect();
    for want in ["cartoon-fragile/fragile-cartoon.studio", "cartoon-allmylife/allmylife-cartoon.studio",
                 "real-proxies/allmylife.studio", "real-proxies/fragile.studio"] {
        assert!(names.iter().any(|n| n == want), "{want} is openable: {names:?}");
    }
    for p in &found {
        let bytes = std::fs::read(p).unwrap();
        let proj = studio_collab::file::SaveFile::decode(&bytes).unwrap_or_else(|| panic!("{} opens", p.display())).to_project();
        assert!(!proj.clips.is_empty(), "{} has something on it", p.display());
        let dir = p.parent().unwrap();
        for s in proj.sources.values() {
            assert!(!std::path::Path::new(&s.path).is_absolute(), "{}: {} is relative, so it opens anywhere", p.display(), s.path);
            assert!(dir.join(&s.path).exists(), "{}: {} is beside it", p.display(), s.path);
        }
    }
    // the cartoon's markers are its shots, from the truth
    let aml = found.iter().find(|p| p.ends_with("cartoon-allmylife/allmylife-cartoon.studio")).unwrap();
    let proj = studio_collab::file::SaveFile::decode(&std::fs::read(aml).unwrap()).unwrap().to_project();
    assert_eq!(proj.markers.len(), 11);
    assert!(proj.markers.iter().any(|m| m.name.contains("kaleido(6)")) && proj.markers.iter().any(|m| m.name.contains("(glitch)")),
            "{:?}", proj.markers.iter().map(|m| &m.name).collect::<Vec<_>>());
}
