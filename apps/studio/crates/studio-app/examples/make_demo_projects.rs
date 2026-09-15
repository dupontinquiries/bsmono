//! Generate the debug/demo project set, instead of every session starting
//! from either an empty project or whatever was left open last.
//!
//!     cargo run -p studio-app --example make_demo_projects --no-default-features -- out_dir/
//!
//! Each project is built from the SAME construction this crate's own tests
//! already use and pass against -- not hand-crafted separately, so a demo
//! file can't drift from what "correct" means here. Loading one of these
//! back into the app one command from now is exactly what
//! `sink_resync_tests`/`curve_shell_tests` already prove works at the model
//! level; the difference is just doing it through `save_to`/`load_from`
//! (the real file I/O path) instead of an in-memory `Studio` value, and
//! actually eyeballing it in the running app.

use studio_core::{Clip, ClipId, Op, Project, Source, SourceId, Ticks, Track, TrackId, TrackKind};
use studio_app::Studio;
use studio_collab::{session::Team, SiteId};

fn team() -> Team {
    Team::from_contacts("solo", &["sc-local".into()])
}

/// Clean project: a few tracks, a few clips, nothing exotic. The
/// "everything works, start here" baseline every debugging session should
/// have instead of an empty timeline needing setup first.
fn clean_project() -> Studio {
    Studio::demo()
}

/// Reproduces the loop bug from earlier in this project's history (fixed,
/// but kept as a regression fixture): a video clip sitting entirely OUTSIDE
/// an arrangement loop range, so "does video keep advancing while the beat
/// pattern loops" is one button-press away from being checked by eye,
/// instead of only checkable via `playhead_sim`'s headless tests.
///
/// `Studio::save_to` only persists `project` + history + rack (see its own
/// doc) -- NOT `Transport` (so no `loop_range`) and NOT `status`. Neither
/// survives a save/load round trip, so this scenario is set up by naming
/// the PROJECT itself descriptively (which does persist) and printing the
/// manual step here, rather than pretending the loop or a status message
/// will already be armed when the file is opened.
fn loop_scenario() -> Studio {
    let mut p = Project { name: "loop scenario -- enable a 0-2s loop, then play; video (clip 5-15s) must keep advancing".into(), ..Default::default() };
    p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "drums".into(),
        muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).ok();
    p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Video, name: "footage".into(),
        muted: false, locked: false, gain: 1.0, route: None }, at: 1 }).ok();
    p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "takes/beat.wav".into(),
        len: Ticks::from_seconds(2.0), sample_rate: 48_000, channels: 2, has_video: false, kind: studio_core::SourceKind::File } }).ok();
    p.apply(&Op::AddSource { source: Source { id: SourceId(2), path: "footage/clip.mp4".into(),
        len: Ticks::from_seconds(30.0), sample_rate: 48_000, channels: 0, has_video: true, kind: studio_core::SourceKind::File } }).ok();
    // The beat pattern: a 2-second drum loop tiled across the first 8
    // seconds, matching how a pattern actually gets "tiled in the timeline"
    // rather than looped by the transport.
    for i in 0..4u64 {
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(i + 1), source: SourceId(1), track: TrackId(1),
            pos: Ticks::from_seconds(i as f64 * 2.0), src_in: Ticks::ZERO, len: Ticks::from_seconds(2.0),
            gain: 1.0, effects: vec![] } }).ok();
    }
    // The video clip: 5s-15s, entirely OUTSIDE a 0-2s loop range -- the
    // exact case `a_video_clip_outside_the_loop_range_becomes_unreachable_while_looping`
    // in `playhead_sim.rs` exercises headlessly. Loading this project and
    // enabling a 0-2s loop should show video continuing to advance past 5s.
    p.apply(&Op::AddClip { clip: Clip { id: ClipId(10), source: SourceId(2), track: TrackId(2),
        pos: Ticks::from_seconds(5.0), src_in: Ticks::ZERO, len: Ticks::from_seconds(10.0),
        gain: 1.0, effects: vec![] } }).ok();

    let mut s = Studio::new(p, SiteId(1), team());
    s
}

/// The multi-composition scenario: clip 4 sits in BOTH "hook" and "bridge"
/// -- `a_clip_shared_between_two_compositions_pulls_in_both_when_moved`'s
/// exact setup, so the union-of-groups behaviour (drag the shared clip,
/// watch both compositions move) can be seen, not just asserted. `status`
/// does not survive save/load (see `loop_scenario`'s note); the
/// explanation goes in the project name instead, which does.
fn grouping_scenario() -> Studio {
    let mut s = Studio::demo();
    s.session.doc.project.name = "grouping scenario -- clip 4 is in both 'hook' and 'bridge'; drag it and watch both move".into();
    s.edit(Op::SetGroup { id: studio_core::GroupId(1), name: "hook".into(),
        clips: vec![ClipId(1), ClipId(4)] }, "grouped");
    s.edit(Op::SetGroup { id: studio_core::GroupId(2), name: "bridge".into(),
        clips: vec![ClipId(4), ClipId(5)] }, "grouped");
    s
}

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| "demo_projects".into());
    let out_dir = std::path::PathBuf::from(out_dir);
    std::fs::create_dir_all(&out_dir).expect("create output dir");

    let projects: Vec<(&str, Studio)> = vec![
        ("clean", clean_project()),
        ("loop-scenario", loop_scenario()),
        ("grouping-scenario", grouping_scenario()),
    ];

    for (name, mut s) in projects {
        let path = out_dir.join(format!("{name}.studio"));
        s.save_to(&path).unwrap_or_else(|e| panic!("saving {name}: {e}"));
        // Round-trip immediately: a demo file nobody can load is worse than
        // no demo file, so this is checked here rather than assumed.
        let reloaded = Studio::load_from(&path, SiteId(1), team())
            .unwrap_or_else(|e| panic!("{name} was written but does not reload: {e}"));
        assert_eq!(reloaded.session.doc.project.clips.len(), s.session.doc.project.clips.len(), "{name}: clip count changed across a save/load round trip");
        println!("wrote and verified {} ({} clips, {} groups)", path.display(), reloaded.session.doc.project.clips.len(), reloaded.session.doc.project.groups.len());
        if !reloaded.session.doc.project.groups.is_empty() {
            println!("  grouping report:\n{}", reloaded.session.doc.project.grouping_report().lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n"));
        }
    }
}
