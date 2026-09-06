mod stubs;
mod bind;
mod paintbrush;
mod armcheck;
mod router;
mod focusguard;

use stubs::ChunkManager;
use bind::{flush, WorldView};
use voxel_tools::staged::StagedWorld;
/// The frame loop's polling, condensed: wait for the drone's dry run.
fn settle(pb: &mut paintbrush::Paintbrush) {
    let t0 = std::time::Instant::now();
    while pb.drone.is_some() {
        pb.poll_drone();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(t0.elapsed().as_secs() < 30, "drone never returned");
    }
}

fn main() {
    scenario();
    focus_scenario();
    async_scenario();
    macro_scenario();
    hive_scenario();
    connect_scenario();
    plant_scenario();
    repair_scenario();
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
    // back on the frame thread: the dry run now runs on a DRONE, and
    // the frame collects it when it lands
    pb.finish_turn(&cm, &gaze, &raw, feet);
    settle(&mut pb);
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
    settle(&mut pb);
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
        while pb.in_flight.is_some() || pb.drone.is_some() { pb.poll_turn(&cm); pb.poll_drone(); std::thread::sleep(std::time::Duration::from_millis(10)); }
    }
    let cmd = line(&mut pb, "/commit").unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    for l in pb.drain_log() { println!("  after commit: {}", l.lines().next().unwrap_or("")); }
    println!("  last_build: {:?}", pb.session.last_build.as_ref().map(|b| (b.name.clone(), b.min, b.max)));
    println!("  looking at it: {}", pb.session.focus_on_last_build(&gaze));

    // now the macro: no provider will be called
    pb.provider = paintbrush::ProviderChoice::Script(vec!["/bin/false".into()]);
    pb.start_turn(&cm, &gaze, None, "fortify my base", feet);
    settle(&mut pb);
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
        settle(&mut pb);
        let planned = pb.drain_log().iter().find(|l| l.contains("blocks planned")).cloned();
        let cmd = line(&mut pb, "/commit").unwrap();
        pb.dispatch(&cm, &gaze, cmd);
        let built = pb.drain_log().iter().find(|l| l.contains("built")).cloned();
        println!("  {rung:8} {} -> {}", planned.unwrap_or_default().trim(),
                 built.unwrap_or_default().trim());
    }
}

/// The hive: a big commit goes to the comb, the queen applies it within
/// her budget, and a slow frame gets nothing.
fn hive_scenario() {
    use paintbrush::{Paintbrush, Pending};
    use voxel_tools::chatbox::Key;
    println!("\n==== hive");
    let cm = stubs::ChunkManager::new();
    let mut pb = Paintbrush::new(32);
    let fixture = std::fs::canonicalize(concat!(env!("CARGO_MANIFEST_DIR"), "/../providers/fixture.sh"))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "/home/claude/bundle/voxel/providers/fixture.sh".into());
    pb.provider = paintbrush::ProviderChoice::Script(vec![fixture]);
    let gaze = Paintbrush::gaze_from([0.0, 70.0, -12.0], [0.0, -0.35, 1.0],
                                     Some(([0, 64, 0], [0, 1, 0], 13.0)));
    let mut line = |pb: &mut Paintbrush, l: &str| -> Option<stubs::Command> {
        pb.key(Key::Enter); for c in l.chars() { pb.key(Key::Char(c)); } pb.key(Key::Enter)
    };
    // a big landscape plan, straight in as IR (no model)
    let ir = "brush raise @0,0 by=20 r=60 hill\nlayer boreal @-50,-50 @50,50 density=0.6\n";
    {
        let view = bind::WorldView(&cm);
        let mut staged = voxel_tools::staged::StagedWorld::new(&view);
        let mut tools = voxel_tools::VoxelTools::new(&mut staged);
        let r = pb.session.receive_plan_for(&mut tools, &gaze, ir, None);
        pb.push(r);
    }
    let planned = pb.drain_log().iter().find(|l| l.contains("blocks planned")).cloned().unwrap_or_default();
    println!("  {}", planned.trim());
    let cmd = line(&mut pb, "/commit").unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    let after = pb.drain_log();
    let queued = after.iter().find(|l| l.contains("chunks queued")).cloned().unwrap_or_default();
    println!("  commit -> {}", queued.trim());
    assert!(!queued.is_empty(), "a large commit must go to the comb: {after:?}");
    let before_world = cm.block_count();
    println!("  blocks in world right after commit: {before_world} (nothing applied yet)");

    // frames: fast ones apply cells; a slow one hibernates
    let mut now = 0.0;
    let mut applied_frames = 0;
    let mut total_cells = 0;
    for frame in 0..400 {
        let slow = frame % 50 == 25;
        let dt = if slow { 40.0 } else { 8.0 };
        let t = pb.tick_hive(&cm, [0.0, 70.0, -12.0], [0.0, -0.35, 1.0], dt, now);
        now += dt as f64 / 1000.0;
        if slow { assert!(t.hibernated || t.cells == 0, "a 40 ms frame must get nothing"); }
        if t.cells > 0 { applied_frames += 1; total_cells += t.cells; }
        assert!(t.spent_ms <= pb.queen.budget.max_ms + pb.queen.budget.cell_cost_ms * 2.0 + 1.0,
                "frame {frame}: spent {:.2} ms against max {:.2}", t.spent_ms, pb.queen.budget.max_ms);
        if pb.comb.is_empty() { println!("  drained after {} frames", frame + 1); break; }
        if frame == 3 {
            let cmd = line(&mut pb, "/hive").unwrap();
            pb.dispatch(&cm, &gaze, cmd);
            for l in pb.drain_log() { println!("  /hive: {l}"); }
        }
    }
    println!("  {total_cells} cells over {applied_frames} frames; world now has {} blocks",
             cm.block_count());
    assert!(cm.block_count() > before_world + 10_000, "the landscape actually arrived");
    assert!(applied_frames > 5, "spread over many frames, not one");
}


/// "connect the mill to the barn" across a river, no model.
fn connect_scenario() {
    use paintbrush::Paintbrush;
    println!("\n==== connect by name");
    let cm = stubs::ChunkManager::new();
    // a river between them
    let edits: Vec<_> = (30..40).flat_map(|x| (-40..40).map(move |z| (x, 64, z, stubs::BlockId::Water)))
        .collect();
    cm.set_blocks(&edits);
    let mut pb = Paintbrush::new(32);
    pb.provider = paintbrush::ProviderChoice::Script(vec!["/bin/false".into()]);
    pb.land.claim("Old Mill", "kess", (0, 64, 0), (8, 70, 8));
    pb.land.claim("Red Barn", "kess", (60, 64, 20), (70, 72, 30));
    let gaze = Paintbrush::gaze_from([4.0, 70.0, 4.0], [1.0, -0.3, 0.3], Some(([4, 64, 4], [0, 1, 0], 8.0)));
    pb.start_turn(&cm, &gaze, None, "connect the mill to the barn", Some((4, 65, 4)));
    settle(&mut pb);
    let log = pb.drain_log();
    let planned = log.iter().find(|l| l.contains("blocks planned")).cloned().unwrap_or_default();
    println!("  {}", planned.trim());
    assert!(log.iter().any(|l| l.contains("road planned without the model")), "{log:?}");
    assert!(planned.contains("blocks planned"), "{log:?}");
    let cmd = { pb.key(voxel_tools::chatbox::Key::Enter);
                for c in "/commit".chars() { pb.key(voxel_tools::chatbox::Key::Char(c)); }
                pb.key(voxel_tools::chatbox::Key::Enter) }.unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    let built = pb.drain_log();
    println!("  {}", built.first().cloned().unwrap_or_default().trim());
    // The proof is in the world: a deck over the water, not a trench
    // through it, and water still under the deck.
    // along the road's own line from (4,4) to (65,25): z = 4 + (x-4)*21/61
    let on_line = |x: i32| 4 + (x - 4) * 21 / 61;
    let over = (30..40).filter(|&x| {
        let z = on_line(x);
        (z - 2..=z + 2).any(|zz| cm.block_at(x, 65, zz) != stubs::BlockId::Air)
    }).count();
    let wet = (30..40).filter(|&x| cm.block_at(x, 64, on_line(x)) == stubs::BlockId::Water).count();
    println!("  deck over the river at y=65: {over}/10 columns; water still beneath: {wet}/10");
    assert!(over >= 8, "a deck");
    assert!(wet >= 8, "the river survives under it");
}


/// "plant tall ancient oaks": bred, placed at the phenotype, and handed
/// to the flora store with the genome.
fn plant_scenario() {
    use paintbrush::Paintbrush;
    println!("\n==== living plants");
    let cm = stubs::ChunkManager::new();
    let mut pb = Paintbrush::new(32);
    let gaze = Paintbrush::gaze_from([0.0, 70.0, -12.0], [0.0, -0.35, 1.0], Some(([0, 64, 0], [0, 1, 0], 13.0)));
    let ir = "plant @0,0 oak tall ancient seed=3\nplant @14,0 pine hardy alpine seed=4\nplant @-14,0 birch small fruit seedling\n";
    {
        let view = bind::WorldView(&cm);
        let mut staged = voxel_tools::staged::StagedWorld::new(&view);
        let mut tools = voxel_tools::VoxelTools::new(&mut staged);
        let r = pb.session.receive_plan_for(&mut tools, &gaze, ir, None);
        pb.push(r);
    }
    for l in pb.drain_log() { for line in l.lines() { if line.contains("living") { println!("  {}", line.trim()); } } }
    let cmd = { pb.key(voxel_tools::chatbox::Key::Enter);
                for c in "/commit".chars() { pb.key(voxel_tools::chatbox::Key::Char(c)); }
                pb.key(voxel_tools::chatbox::Key::Enter) }.unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    let log = pb.drain_log();
    assert!(log.iter().any(|l| l.contains("3 living plant(s) registered")), "{log:?}");
    let mut flora = stubs::FloraStore::default();
    let n = pb.drain_plants(&mut flora, 1000);
    println!("  {n} plants in the flora store");
    assert_eq!(flora.len(), 3);
    for (pos, fc) in &flora.cells {
        let g = voxel_tools::flora::PlantGenome::from_array(fc.genome);
        println!("  {:?} block {} growth {:.2} stage {} -- {}", pos, fc.block, fc.growth01, fc.stage, g.describe());
    }
    let oak = flora.cells.iter().find(|(p, _)| p[0] == 0).unwrap().1;
    assert!(oak.genome[voxel_tools::flora::MAX_HEIGHT] > 0.75 && oak.genome[voxel_tools::flora::TRUNK_GIRTH] > 0.75,
            "tall ancient: {:?}", oak.genome);
    let pine = flora.cells.iter().find(|(p, _)| p[0] == 14).unwrap().1;
    assert!(pine.genome[voxel_tools::flora::HARDINESS] > 0.8, "hardy: {:?}", pine.genome);
    let birch = flora.cells.iter().find(|(p, _)| p[0] == -14).unwrap().1;
    assert!(birch.growth01 < 0.1 && birch.stage == 0, "a seedling grows from nothing");
    assert!(pb.drain_plants(&mut flora, 1001) == 0, "drained once");
}


/// /repair runs on the worker and its reply is merged into the plan.
fn repair_scenario() {
    use paintbrush::Paintbrush;
    use voxel_tools::chatbox::Key;
    println!("\n==== repair on the worker");
    let cm = stubs::ChunkManager::new();
    let mut pb = Paintbrush::new(32);
    // a provider that answers any prompt with one fixed line
    let script = "/tmp/fix_provider.sh";
    std::fs::write(script, "#!/bin/sh\ncat >/dev/null\necho 'tree @6,6 birch h=6'\n").unwrap();
    std::process::Command::new("chmod").args(["+x", script]).status().unwrap();
    pb.provider = paintbrush::ProviderChoice::Script(vec![script.into()]);
    let gaze = Paintbrush::gaze_from([0.0, 70.0, -12.0], [0.0, -0.35, 1.0], Some(([0, 64, 0], [0, 1, 0], 13.0)));
    // a plan with one bad line, previewed on the drone
    pb.launch_drone(&cm, &gaze, "enclose @-3,-3 @3,3 h=4\nfrobnicate\n".into(), None, "");
    settle(&mut pb);
    let log = pb.drain_log();
    assert!(log.iter().any(|l| l.contains("SKIPPED 1")), "{log:?}");
    // /repair must NOT run the model on this thread: it returns at once
    let t0 = std::time::Instant::now();
    let cmd = { pb.key(Key::Enter); for c in "/repair".chars() { pb.key(Key::Char(c)); } pb.key(Key::Enter) }.unwrap();
    pb.dispatch(&cm, &gaze, cmd);
    assert!(t0.elapsed().as_millis() < 50, "dispatch returned in {:?}: the model ran inline", t0.elapsed());
    assert!(pb.in_flight.is_some(), "a turn is in flight");
    while pb.in_flight.is_some() || pb.drone.is_some() {
        pb.poll_turn(&cm); pb.poll_drone(); std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let log = pb.drain_log();
    let planned = log.iter().find(|l| l.contains("blocks planned")).cloned().unwrap_or_default();
    println!("  after repair: {}", planned.trim());
    assert!(!log.iter().any(|l| l.contains("SKIPPED")), "the bad line was replaced: {log:?}");
    assert!(log.iter().any(|l| l.contains("Plan") && l.contains("2 operations")), "wall + the fixed tree: {log:?}");
}
