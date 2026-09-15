//! real_frame_workflow — the ONE piece `playhead_sim.rs` deliberately
//! leaves untested: does a real frame actually come back from a real file
//! through the real decode path.
//!
//! Reproduces every step of `viewport::show` up to (not including) the
//! `egui::Ui` texture upload: `video_at` -> `layers_at` -> real
//! `studio_video::frame_rgb` (an actual ffmpeg subprocess) -> real
//! `studio_fx::compositor::composite`. That boundary is deliberate and
//! stated, not glossed over: if a bug turns out to live specifically in the
//! `ui.ctx().load_texture` step or egui's own repaint scheduling, this test
//! will pass and tell you the bug is there, not here -- which is still
//! useful information.
//!
//! Needs a real video file, so it's gated behind an env var rather than run
//! unconditionally: `STUDIO_TEST_VIDEO=/path/to/file.mkv cargo test --test
//! real_frame_workflow`. Skips (not fails) when unset, so a normal
//! `cargo test` run elsewhere never breaks on a missing fixture.

use std::path::PathBuf;
use studio_app::playhead_sim::SimProject;
use studio_app::viewport::{layers_at, video_at};
use studio_core::Ticks;

fn test_video_path() -> Option<PathBuf> {
    std::env::var_os("STUDIO_TEST_VIDEO").map(PathBuf::from)
}

#[test]
fn a_real_frame_decodes_and_composites_at_the_resolved_playhead() {
    let Some(video_path) = test_video_path() else {
        eprintln!("skipping: set STUDIO_TEST_VIDEO=/path/to/a/real/clip.mkv to run this");
        return;
    };
    assert!(video_path.exists(), "STUDIO_TEST_VIDEO points at {}, which doesn't exist", video_path.display());

    // The project references the file by a name relative to `base` --
    // exactly how `show()`'s `base.join(&l.path)` resolves it -- so the
    // fixture doesn't need copying into the repo; `base` just points at
    // wherever the real file already lives.
    let base = video_path.parent().unwrap_or(std::path::Path::new("."));
    let filename = video_path.file_name().unwrap().to_string_lossy().to_string();

    let mut sp = SimProject::new();
    let track = sp.add_video_track();
    // 5s clip starting 2s into the source -- exercises trim math against a
    // real file's real timestamps, not a synthetic one.
    sp.place_clip(track, &filename, 0.0, 5.0, 2.0);

    // Three probe points: start, middle, and right at the far edge minus a
    // frame -- the boundary is where an off-by-one is most likely to show.
    for t_secs in [0.1, 2.5, 4.9] {
        let playhead = Ticks::from_seconds(t_secs);
        let (resolved_path, media_time) = video_at(sp.project(), playhead)
            .unwrap_or_else(|| panic!("video_at found nothing at t={t_secs}s -- clip selection is broken, not decode"));
        assert_eq!(resolved_path, filename);

        let layers = layers_at(sp.project(), playhead);
        assert_eq!(layers.len(), 1, "expected exactly one layer at t={t_secs}s");

        let full_path = base.join(&resolved_path);
        let px = studio_video::frame_rgb(&full_path, media_time, studio_app::viewport::W, studio_app::viewport::H)
            .unwrap_or_else(|e| panic!("frame_rgb failed at t={t_secs}s (media_time={:.3}s): {e:?}", media_time.to_seconds()));

        let expected_len = (studio_app::viewport::W * studio_app::viewport::H * 3) as usize;
        assert_eq!(px.len(), expected_len, "frame_rgb returned the wrong buffer size at t={t_secs}s");

        // A real decoded frame from real footage should not be uniformly
        // one value -- that's what a black-frame/decode-failure-that-
        // didn't-error would look like, and it's the "preview isn't
        // loading" symptom made assertable instead of eyeballed.
        let first = px[0];
        assert!(px.iter().any(|&b| b != first), "frame at t={t_secs}s is a single flat colour ({first}) -- likely a black/blank frame, not real content");

        let stack = [studio_fx::compositor::Layer {
            rgb: &px,
            w: studio_app::viewport::W,
            h: studio_app::viewport::H,
            transform: studio_fx::Transform::default(),
            mode: studio_fx::compositor::BlendMode::Normal,
            opacity: 1.0,
        }];
        let composited = studio_fx::compositor::composite(studio_app::viewport::W, studio_app::viewport::H, [0, 0, 0], &stack);
        assert_eq!(composited.len(), expected_len);

        println!("t={t_secs:.1}s -> media {:.3}s: decoded + composited {} bytes, sample px[0..3]={:?}", media_time.to_seconds(), composited.len(), &composited[..3]);
    }
}
