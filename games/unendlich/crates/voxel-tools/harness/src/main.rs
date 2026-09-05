mod stubs;
mod bind;
mod paintbrush;
mod armcheck;
mod router;
mod focusguard;

use stubs::ChunkManager;
use bind::{flush, WorldView};
use voxel_tools::staged::StagedWorld;
fn main() {
    scenario();
    focus_scenario();
    async_scenario();
    macro_scenario();
}

/// Drives Paintbrush exactly as the game's input handler would:
/// keystrokes in, Commands out, model round-trip on a worker, commit,
/// flush, reveal. Nothing here knows it is a test.
fn scenario() {
    use paintbrush::{Paintbrush, Pending};
    use voxel_tools::chatbox::Key;
    use voxel_tools::llm::{Completer, FakeCompleter};

    let cm = ChunkManager::new();
    let mut pb = Paintbrush::new(32);
    let gaze = Paintbrush::gaze_from([0.0, 70.0, -12.0], [0.0, -0.35, 1.0],
                                     Some(([0, 64, 0], [0, 1, 0], 13.0)));
    let feet = Some((0, 65, -12));

    // the model the game would spawn; scripted here
    let mut llm = FakeCompleter::new(vec![
        "Sure! Here's the plan:\n```text\n# watchtower\n         terraform @-5,-5 @5,5 smooth block=stone\n         cylinder @0,0 r=3 h=11 block=stone_bricks hollow\n```\nHope that works!".into(),
        "sphere @3,0 r=1 block=air\npath @3,0 @16,9 w=2 block=cobblestone\n".into(),
    ]);

    let mut type_line = |pb: &mut Paintbrush, line: &str| -> Option<stubs::Command> {
        pb.key(Key::Enter);
        for c in line.chars() { pb.key(Key::Char(c)); }
        pb.key(Key::Enter)
    };

    println!("== boot");
    for l in pb.drain_log() { println!("  {l}"); }

    println!("\n== /build with no model loaded");
    // The default provider is the fixture, which needs no model, so this
    // is ALLOWED now. Readiness is a question for the provider, not for
    // the model registry -- an ollama or an engine with its own path
    // does not care what /model says.
    pb.provider = paintbrush::ProviderChoice::Engine { model_path: String::new() };
    if let Some(cmd) = type_line(&mut pb, "/build a watchtower") {
        let p = pb.dispatch(&cm, &gaze, cmd);
        assert_eq!(p, Pending::None, "the engine with no model must refuse");
    }
    pb.provider = paintbrush::ProviderChoice::Fixture;
    for l in pb.drain_log() { println!("  {l}"); }

    println!("\n== pretend a model loaded");
    pb.models.begin_load("").ok();          // no real files here
    pb.models.state = voxel_tools::model::LoadState::Loaded {
        name: "qwen2.5-7b-instruct-q4_k_m".into(),
        took: std::time::Duration::from_millis(3200), ctx: 16384 };
    println!("  {}", pb.models.state.message());

    println!("\n== turn 1: /build (massing)");
    let cmd = type_line(&mut pb, "/build a small watchtower on this hill").unwrap();
    let prose = match pb.dispatch(&cm, &gaze, cmd) {
        Pending::Model(p) => p,
        o => panic!("expected a model round-trip, got {o:?}"),
    };
    // worker thread: build the prompt, call the model
    let (sys, user) = pb.prompt_for(&cm, &gaze, Some("grass"), &prose);
    assert!(sys.contains("BUILD IN PASSES") && user.contains("READ FIRST"));
    let raw = llm.complete(&sys, &user).unwrap();
    // back on the frame thread
    pb.finish_turn(&cm, &gaze, &raw, feet);
    for l in pb.drain_log() { println!("  {}", l.lines().next().unwrap_or("")); }

    println!("\n== /commit");
    let cmd = type_line(&mut pb, "/commit").unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    for l in pb.drain_log() { println!("  {l}"); }
    println!("  world: (3,66,0)={:?}  interior (0,66,0)={:?}",
             cm.block_at(3, 66, 0), cm.block_at(0, 66, 0));

    println!("\n== reveal");
    let (frames, fps) = pb.take_reveal();
    println!("  {} frames at {:.0} fps ~= {:.1}s", frames.len(), fps, frames.len() as f32 / fps);

    println!("\n== turn 2: follow-up becomes a detail pass on the referent");
    let cmd = type_line(&mut pb, "/build add a door and a path").unwrap();
    let prose = match pb.dispatch(&cm, &gaze, cmd) { Pending::Model(p) => p, o => panic!("{o:?}") };
    let (sys, user) = pb.prompt_for(&cm, &gaze, Some("grass"), &prose);
    assert!(user.contains("PASS: detail"), "should infer detail: {}",
            user.lines().rev().take(3).collect::<Vec<_>>().join(" | "));
    assert!(user.contains("LAST BUILD"), "referent must be in the context");
    println!("  pass inferred: detail, referent present");
    let raw = llm.complete(&sys, &user).unwrap();
    pb.finish_turn(&cm, &gaze, &raw, feet);
    let cmd = type_line(&mut pb, "/commit").unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    for l in pb.drain_log() { println!("  {}", l.lines().next().unwrap_or("")); }
    println!("  door carved: (3,66,0)={:?}", cm.block_at(3, 66, 0));

    println!("\n== /anim physics, then /undo");
    let cmd = type_line(&mut pb, "/anim physics").unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    let cmd = type_line(&mut pb, "/undo").unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    for l in pb.drain_log() { println!("  {l}"); }
    println!("  after undo, path block (10,65,6)={:?}", cm.block_at(10, 65, 6));

    println!("\n== voice");
    let cmd = type_line(&mut pb, "/build enableVoice").unwrap();
    match pb.dispatch(&cm, &gaze, cmd) {
        Pending::VoiceGrammar(g) => println!("  registering {} grammar rules", g.len()),
        o => panic!("{o:?}"),
    }
    for l in pb.drain_log() { println!("  {l}"); }
    let p = pb.voice_event(&cm, &gaze, "event",
                           &["vox_build".to_string(), "a stone wall".to_string()]);
    println!("  spoken build -> {p:?}");
    let p = pb.voice_event(&cm, &gaze, "event", &["play_track".to_string()]);
    println!("  foreign event -> {p:?}");
}

fn old_main() {
    use voxel_tools::chat::{ChatSession, Reply};
    use voxel_tools::focus::{Gaze, GazeHit};
    use voxel_tools::VoxelTools;

    let cm = ChunkManager::new();
    let gaze = Gaze { eye: (0.0, 70.0, -10.0), dir: (0.0, -0.3, 1.0),
        hit: Some(GazeHit { block: (0, 64, 0), face_normal: (0, 1, 0), distance: 12.0 }) };
    let mut session = ChatSession::new(32, 200);

    // read through the live world, stage every write
    let view = WorldView(&cm);
    let mut staged = StagedWorld::new(&view);
    let ir = "terraform @-4,-4 @4,4 smooth block=stone\ncylinder @0,0 r=3 h=10 block=stone_bricks hollow\npath @3,0 @14,8 w=2 block=cobblestone\n";
    {
        let mut tools = VoxelTools::new(&mut staged);
        match session.receive_plan_for(&mut tools, &gaze, ir, Some((0, 65, -10))) {
            Reply::Preview { blocks, .. } => println!("preview: {blocks} blocks"),
            r => println!("unexpected: {r:?}"),
        }
        match session.commit(&mut tools, &gaze) {
            Reply::Committed { blocks, batches, .. } => println!("commit: {blocks} blocks, {batches} batches"),
            r => println!("unexpected: {r:?}"),
        }
    }
    println!("staged pending: {}", staged.pending());
    println!("chunks that will be cloned: {}", staged.chunks_touched(32));
    let (applied, skipped, chunks) = flush(&cm, &mut staged, 32);
    println!("flushed: applied={applied} skipped={skipped} clone_for_edit_calls={chunks}");
    println!("world now: (0,65,0)={:?}  (3,66,0)={:?}",
             cm.block_at(0, 65, 0), cm.block_at(3, 66, 0));
    let frames = session.reveal(25);
    println!("reveal: {} frames", frames.len());
}


/// Dvorak `.oeu` sits on physical E/S/D/F; space is jump. Typing must
/// not move the player, including in the frame Enter opened the box.
fn focus_scenario() {
    use focusguard::{Ev, Sim};
    println!("\n==== focus guard");

    // one long frame: Enter then a burst of typing, all in one batch
    let mut s = Sim::default();
    s.frame(&[Ev::Enter,
              Ev::Key('e'), Ev::Char('.'),
              Ev::Key('s'), Ev::Char('o'),
              Ev::Key('d'), Ev::Char('e'),
              Ev::Key('f'), Ev::Char('u'),
              Ev::Key(' '), Ev::Char(' ')]);
    println!("  same frame as Enter: to_movement={:?} to_chat={:?} release_all={}",
             s.to_movement, s.to_chat.iter().collect::<String>(), s.released_all);
    assert!(s.to_movement.is_empty(), "typing must not reach movement");
    assert_eq!(s.to_chat.iter().collect::<String>(), ".oeu ");

    // steady state across frames
    let mut s2 = Sim::default();
    s2.frame(&[Ev::Enter]);
    s2.frame(&[Ev::Key('s'), Ev::Char('o'), Ev::Key(' '), Ev::Char(' ')]);
    assert!(s2.to_movement.is_empty(), "steady-state typing must not move");
    println!("  next frame:          to_movement={:?} to_chat={:?}",
             s2.to_movement, s2.to_chat.iter().collect::<String>());

    // and movement works again after Escape, in the same frame
    let mut s3 = Sim::default();
    s3.frame(&[Ev::Enter]);
    s3.frame(&[Ev::Esc, Ev::Key('s'), Ev::Key(' ')]);
    println!("  after Esc:           to_movement={:?}", s3.to_movement);
    assert_eq!(s3.to_movement, vec!['s', ' ', 's', ' '],
               "both drains resume once focus is released");
    println!("  OK");
}

/// A turn must not stop the frame loop. Uses a provider that sleeps,
/// standing in for the 22.5-second generation in the field log.
fn async_scenario() {
    use paintbrush::Paintbrush;
    use voxel_tools::focus::{Gaze, GazeHit};
    println!("\n==== off-thread turn");

    let cm = stubs::ChunkManager::new();
    let mut pb = Paintbrush::new(32);
    pb.provider = paintbrush::ProviderChoice::Script(vec![
        "sh".into(), "-c".into(),
        "cat >/dev/null; sleep 1; printf 'cylinder @0,0 r=2 h=6 block=stone\\n'".into()]);
    let gaze = Paintbrush::gaze_from([0.0, 70.0, -12.0], [0.0, -0.35, 1.0],
                                     Some(([0, 64, 0], [0, 1, 0], 13.0)));
    let t0 = std::time::Instant::now();
    pb.start_turn(&cm, &gaze, None, "a tower", Some((0, 65, -12)));
    let start_cost = t0.elapsed();
    println!("  start_turn returned in {:.0}ms (must be ~0)", start_cost.as_millis());
    assert!(start_cost.as_millis() < 300, "start_turn blocked for {start_cost:?}");

    // "render" frames while the worker generates
    let mut frames = 0;
    loop {
        pb.poll_turn(&cm);
        if pb.in_flight.is_none() { break; }
        frames += 1;
        std::thread::sleep(std::time::Duration::from_millis(16));
        assert!(t0.elapsed().as_secs() < 20, "worker never answered");
    }
    println!("  {frames} frames rendered while it worked (would have been 0)");
    assert!(frames > 10, "the frame loop must keep running");
    for l in pb.drain_log() { println!("  {}", l.lines().next().unwrap_or("")); }

    // a second /build while one is in flight is refused, not raced
    pb.provider = paintbrush::ProviderChoice::Script(vec![
        "sh".into(), "-c".into(), "cat >/dev/null; sleep 2; echo 'tree @0,0 oak'".into()]);
    pb.start_turn(&cm, &gaze, None, "one", None);
    pb.start_turn(&cm, &gaze, None, "two", None);
    let refused = pb.drain_log().iter().any(|l| l.contains("already asking"));
    println!("  second build while busy refused: {refused}");
    assert!(refused);
    assert!(pb.cancel_turn(), "cancel drops it");
    println!("  cancelled cleanly");
}

/// "fortify my base" with no model, and a shot of the result.
fn macro_scenario() {
    use paintbrush::{Paintbrush, Pending};
    use voxel_tools::chatbox::Key;
    println!("\n==== macros and shots");
    let cm = stubs::ChunkManager::new();
    let mut pb = Paintbrush::new(32);
    // The harness lives in /tmp, so the fixture is named by absolute path.
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../providers/fixture.sh");
    let fixture = std::fs::canonicalize(fixture).map(|p| p.display().to_string())
        .unwrap_or_else(|_| "/home/claude/bundle/voxel/providers/fixture.sh".into());
    pb.provider = paintbrush::ProviderChoice::Script(vec![fixture]);
    let gaze = Paintbrush::gaze_from([0.0, 70.0, -12.0], [0.0, -0.35, 1.0],
                                     Some(([0, 64, 0], [0, 1, 0], 13.0)));
    let feet = Some((0, 65, -12));
    let mut line = |pb: &mut Paintbrush, l: &str| -> Option<stubs::Command> {
        pb.key(Key::Enter); for c in l.chars() { pb.key(Key::Char(c)); } pb.key(Key::Enter)
    };
    // a base first (the fixture builds a tower)
    let cmd = line(&mut pb, "/build a base").unwrap();
    if let Pending::Model(p) = pb.dispatch(&cm, &gaze, cmd) {
        pb.start_turn(&cm, &gaze, None, &p, feet);
        while pb.in_flight.is_some() { pb.poll_turn(&cm); std::thread::sleep(std::time::Duration::from_millis(10)); }
    }
    let cmd = line(&mut pb, "/commit").unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    for l in pb.drain_log() { println!("  after commit: {}", l.lines().next().unwrap_or("")); }
    println!("  last_build: {:?}", pb.session.last_build.as_ref().map(|b| (b.name.clone(), b.min, b.max)));
    println!("  looking at it: {}", pb.session.focus_on_last_build(&gaze));

    // now the macro: no provider will be called
    pb.provider = paintbrush::ProviderChoice::Script(vec!["/bin/false".into()]);
    pb.start_turn(&cm, &gaze, None, "fortify my base", feet);
    let log = pb.drain_log();
    assert!(log.iter().any(|l| l.contains("expanded without the model")), "{log:?}");
    let planned = log.iter().find(|l| l.contains("blocks planned")).cloned().unwrap_or_default();
    println!("  fortify: {planned}");
    let cmd = line(&mut pb, "/commit").unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    for l in pb.drain_log() { if l.contains("built") { println!("  {l}"); } }

    // and a shot of it
    let cmd = line(&mut pb, "/shot reveal").unwrap();
    match pb.dispatch(&cm, &gaze, cmd) {
        Pending::Camera(poses) => {
            let total: f32 = poses.iter().map(|p| p.seconds).sum();
            println!("  reveal: {} poses over {total:.1}s, from {:.0?} to {:.0?}",
                     poses.len(), poses[0].eye, poses.last().unwrap().eye);
            assert!(poses.last().unwrap().eye[1] > poses[0].eye[1], "pulls up and back");
        }
        o => panic!("{o:?}"),
    }
    let cmd = line(&mut pb, "/shot orbit").unwrap();
    if let Pending::Camera(poses) = pb.dispatch(&cm, &gaze, cmd) {
        println!("  orbit: {} poses", poses.len());
    }

    // the refinement ladder: small passes, no model
    println!("\n==== refinement ladder");
    let cmd = line(&mut pb, "/refine").unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    for l in pb.drain_log() { for m in l.lines() { println!("  {m}"); } }
    for rung in ["vary", "trim", "settle"] {
        let cmd = line(&mut pb, &format!("/refine {rung}")).unwrap();
        pb.dispatch(&cm, &gaze, cmd);
        let planned = pb.drain_log().iter().find(|l| l.contains("blocks planned")).cloned();
        let cmd = line(&mut pb, "/commit").unwrap();
        pb.dispatch(&cm, &gaze, cmd);
        let built = pb.drain_log().iter().find(|l| l.contains("built")).cloned();
        println!("  {rung:8} {} -> {}", planned.unwrap_or_default().trim(),
                 built.unwrap_or_default().trim());
    }
}
