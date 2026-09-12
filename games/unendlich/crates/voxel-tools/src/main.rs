//! voxel-demo — run the tool layer without a game attached.
//!
//!   cargo run -- schemas          print the JSON tool schemas
//!   cargo run -- palettes         list palettes and their block ids
//!   cargo run -- demo             build a small scene, print a slice
//!   cargo run -- demo --ascii     same, with an ASCII cross-section
//!
//! The point of `demo` is that you can watch the primitives compose
//! before wiring them to unendlich: it runs against a flat in-memory
//! world, so what you see is exactly what the tools would do to a
//! chunk.
use std::collections::BTreeMap;
use voxel_tools::math::{check_math, explain_plan, Note};
use voxel_tools::plan::{parse_plan, run_plan, validate, Severity};
use voxel_tools::{palettes, tool_schemas, tool_names, VoxelTools, VoxelWorld};

/// Flat world: solid stone to y=64, air above. Stands in for
/// ChunkManager so the tools can be exercised with no engine.
struct Flat {
    cells: BTreeMap<(i32, i32, i32), u16>,
}
impl Flat {
    fn new() -> Self { Self { cells: BTreeMap::new() } }
}
impl VoxelWorld for Flat {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
        if y <= 64 { 1 } else { 0 }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
        self.cells.insert((x, y, z), b);
        true
    }
}

/// Reverse lookup for display. Derived from the resolver rather than
/// a second hand-written table -- the earlier duplicate table drifted
/// and printed "?" for blocks the resolver knew perfectly well.
fn name_of(id: u16) -> String {
    const NAMES: &[&str] = &[
        "air","stone","grass","dirt","cobblestone","water","lava","sand","gravel",
        "oak_log","oak_leaves","birch_log","birch_leaves","cactus","clay","moss",
        "red_sandstone","flower","tall_grass","fern","dead_bush","vine","dripstone",
        "ice","snow","mushroom","cobweb","lichen","limestone","marble","basalt","tuff",
        "granite","slate","calcite","andesite","schist","bone","amber","crystal",
        "sandstone","obsidian","quartz","oak_planks","stone_bricks","bricks","glass",
        "iron_block","gold_block","bookshelf","pumpkin","snow_block","mossy_cobblestone",
        "polished_stone","smooth_sandstone","terracotta","chiseled_stone_bricks",
        "dark_oak_planks","polished_basalt","selenite","gypsum","dolomite","sulfur",
        "magma","glow_crystal",
    ];
    for n in NAMES {
        if voxel_tools::plan::resolve_block(n) == Some(id) {
            return (*n).to_string();
        }
    }
    format!("id{id}")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("demo");

    match cmd {
        "schemas" => {
            println!("{}", tool_schemas());
            eprintln!("\n{} tools", tool_names().len());
        }
        "palettes" => {
            for (name, p) in palettes() {
                println!("{name:9} primary={:<3}({:<12}) secondary={:<3}({:<12}) \
                          accent={:<3}({:<12}) foliage={:<3}({})",
                    p.primary, name_of(p.primary), p.secondary, name_of(p.secondary),
                    p.accent, name_of(p.accent), p.foliage, name_of(p.foliage));
            }
        }
        "demo" => {
            let ascii = args.iter().any(|a| a == "--ascii");
            let mut world = Flat::new();
            let mut t = VoxelTools::new(&mut world);

            // This is the sequence a model would emit. Read it as the
            // worked example of the intended call order: look, then
            // place, then verify.
            println!("== 1. look before building");
            println!("   {}", t.probe_column(0, 0, 200, 6));
            let surface = t.find_surface(0, 0, 200).expect("ground");
            println!("   surface y = {surface}");

            println!("\n== 2. a tower on the surface");
            let pal = palettes();
            let stone = pal["stone"].clone();
            let r = t.cylinder((0, surface + 1, 0), 4, 12, stone.primary, true);
            println!("   {}", r.summary);
            let r2 = t.sphere((0, surface + 13, 0), 5, stone.accent, true);
            println!("   {}", r2.summary);

            println!("\n== 3. trees around it, each found on its own ground");
            for (dx, dz, sp) in [(-12, -8, "oak"), (14, 6, "birch"), (-9, 13, "oak")] {
                if let Some(sy) = t.find_surface(dx, dz, 200) {
                    let r = t.place_tree((dx, sy + 1, dz), sp, 8, (1, 0));
                    println!("   {}", r.summary);
                }
            }

            println!("\n== 4. decorate, reproducibly");
            let r = t.scatter((-16, -16), (16, 16), 200, 25, 60, 1337);
            println!("   {}", r.summary);

            println!("\n== 5. verify, then restyle");
            println!("   {}", t.read_region((-16, surface, -16), (16, surface + 20, 16)));
            let r = t.replace((-6, surface, -6), (6, surface + 20, 6),
                              stone.primary, pal["polished"].primary);
            println!("   {}", r.summary);

            println!("\n== 6. undo the restyle (edit {})", r.edit_id.unwrap());
            println!("   {}", t.undo_edit(r.edit_id.unwrap()).summary);

            println!("\n== 7. corpus-derived ops: terraform, path, enclose, copy/paste, restyle");
            let r = t.terraform((20, -10), (34, 4), 0, true, 3, 200);
            println!("   {}", r.summary);
            let r = t.connect_path((0, 6), (27, -3), 3, 4, 200);
            println!("   {}", r.summary);
            let r = t.enclose((20, -10), (34, 4), 2, pal["oak"].secondary, 200);
            println!("   {}", r.summary);
            let (clip, msg) = t.copy_region((-2, surface, -2), (2, surface + 6, 2));
            println!("   {msg}");
            if let Some(cid) = clip {
                let r = t.paste_region(cid, (44, surface + 1, -20), 90, false, true);
                println!("   {}", r.summary);
                let r = t.restyle((44, surface, -20), (52, surface + 8, -12),
                                  "stone", "volcanic");
                println!("   {}", r.summary);
            }
            let r = t.scatter_clustered((-20, -20), (20, 20), 200,
                                        pal["oak"].foliage, 6, 10, 3, 7);
            println!("   {}", r.summary);

            if ascii {
                println!("\n== cross-section at z=0, x -16..16 ==");
                for y in (surface..surface + 20).rev() {
                    let row: String = (-16..=16)
                        .map(|x| match t.world.get(x, y, 0) {
                            0 => ' ', 1 => '#', 17 | 19 => '|', 18 | 20 => '*',
                            25 => ',', 37 => 'O', _ => '.',
                        })
                        .collect();
                    println!("   y={y:3} |{row}|");
                }
                println!("   legend: # stone  O marble  | trunk  * leaves  , flower");
            }
        }
        "plan" => {
            // The full ingest path, minus the model: prose would
            // produce this IR, then everything downstream is
            // deterministic and inspectable.
            let src = if let Some(p) = args.get(1) {
                std::fs::read_to_string(p).unwrap_or_else(|e| { eprintln!("{p}: {e}"); std::process::exit(1) })
            } else {
                "# hilltop camp\n\
                 terraform 20,-10 34,4 smooth block=stone\n\
                 path 0,6 27,-3 w=3 block=cobblestone\n\
                 cylinder 27,-3 r=4 h=12 block=stone hollow\n\
                 tree -12,-8 oak h=8 lean=1,0\n\
                 scatter -20,-20 20,20 block=oak_leaves clusters=6 per=10 spread=3 seed=7\n"
                 .to_string()
            };

            println!("== 1. parse");
            let (plan, parse_issues) = parse_plan(&src);
            println!("   plan '{}': {} ops, {} parse issues", plan.name, plan.ops.len(), parse_issues.len());
            for i in &parse_issues { println!("   !  op {}: {}", i.op, i.message); }

            println!("\n== 2. validate (no world access)");
            let issues = validate(&plan);
            if issues.is_empty() { println!("   clean"); }
            for i in &issues {
                let tag = if i.severity == Severity::Error { "ERROR" } else { "warn " };
                println!("   {tag} op {}: {}", i.op, i.message);
            }
            if issues.iter().any(|i| i.severity == Severity::Error) {
                println!("\n   errors present: would not commit");
                return;
            }

            println!("\n== 3. arithmetic echo (send this back before committing)");
            print!("{}", explain_plan(&plan, 200_000));
            let math = check_math(&plan, 200_000);
            let blockers = math.iter().filter(|n| n.level == Note::Error).count();
            if blockers > 0 {
                println!("   {blockers} arithmetic errors: would not commit");
                return;
            }

            println!("\n== 4. dry run (world untouched)");
            let mut world = Flat::new();
            let dry = { let mut t = VoxelTools::new(&mut world);
                        run_plan(&mut t, &plan, 200, 32, true) };
            println!("   {} ops applied, {} failed, {} blocks would change",
                     dry.applied, dry.failed, dry.blocks_changed);
            for l in &dry.log { println!("   {l}"); }

            println!("\n== 5. commit, batched for the mesher");
            let run = { let mut t = VoxelTools::new(&mut world);
                        run_plan(&mut t, &plan, 200, 32, false) };
            let chunks: std::collections::BTreeSet<_> = run.batches.iter().map(|b| b.chunk).collect();
            println!("   {} blocks in {} batches across {} chunks",
                     run.blocks_changed, run.batches.len(), chunks.len());
            let ticks: std::collections::BTreeSet<_> = run.batches.iter().map(|b| b.tick).collect();
            println!("   animation ticks: {} (one per op, batches within a tick can remesh in parallel)",
                     ticks.len());
            for b in run.batches.iter().take(6) {
                println!("   tick {:2} chunk {:?}  {} blocks", b.tick, b.chunk, b.blocks.len());
            }
            if run.batches.len() > 6 { println!("   ... {} more", run.batches.len() - 6); }
        }
        "resolve" => {
            // Sanity-check that prose material names map to real ids.
            for name in args.iter().skip(1) {
                match voxel_tools::plan::resolve_block(name) {
                    Some(id) => println!("  {name:24} -> {id}"),
                    None => println!("  {name:24} -> UNKNOWN"),
                }
            }
        }
        "session" => {
            // The in-game experience without the game: three turns of a
            // player building, with the passes inferred, the previews
            // shown, and the reveal scheduled.
            use voxel_tools::chat::{ChatSession, Reply};
            use voxel_tools::focus::{Gaze, GazeHit};
            use voxel_tools::anim;
            let mut world = Flat::new();
            let mut s = ChatSession::new(32, 200);
            let gaze = Gaze { eye: (0.0, 70.0, -12.0), dir: (0.0, -0.4, 1.0),
                hit: Some(GazeHit { block: (0, 64, 0), face_normal: (0, 1, 0), distance: 14.0 }) };
            let feet = (0, 65, -12);

            // What the model would have replied for each turn. In the
            // game these come from the LLM; here they are the worked
            // examples from the prompt, so the demo exercises exactly
            // what the prompt teaches.
            let turns: [(&str, &str); 3] = [
                ("a small watchtower on this hill",
                 "# watchtower\nterraform @-5,-5 @5,5 smooth block=stone\ncylinder @0,0 r=3 h=11 block=stone hollow\n"),
                ("make it look old and mossy",
                 "restyle @-4,-1,-4 @4,15,4 stone ruined\n"),
                ("add a door and a path down to the road",
                 "sphere @3,0 r=1 block=air\npath @3,0 @18,12 w=2 block=cobble\nscatter @-8,-8 @8,8 block=flower clusters=3 per=5 spread=2 seed=11\n"),
            ];
            for (i, (prose, ir)) in turns.iter().enumerate() {
                println!("\n==== turn {} : player says: {prose}", i + 1);
                let mut tools = VoxelTools::new(&mut world);
                let (turn, pass) = s.user_turn(&tools, &gaze, Some("grass"), prose, None);
                println!("  inferred pass: {}", pass.name());
                println!("  context sent to model ({} chars), first line: {}",
                         turn.len(), turn.lines().next().unwrap_or(""));
                match s.receive_plan_for(&mut tools, &gaze, ir, Some(feet)) {
                    Reply::Preview { echo, blocks, .. } => {
                        println!("  preview: {blocks} blocks would change");
                        for l in echo.lines().filter(|l| l.contains("CHECK") || l.contains("warn")) {
                            println!("  {l}");
                        }
                    }
                    Reply::Say(m) => { println!("  {m}"); continue; }
                    _ => {}
                }
                match s.commit(&mut tools, &gaze) {
                    Reply::Committed { blocks, batches, .. } => {
                        println!("  committed: {blocks} blocks in {batches} chunk batches");
                    }
                    Reply::Say(m) => println!("  {m}"),
                    _ => {}
                }
                if let Some(lb) = &s.last_build {
                    println!("  last build '{}': {:?}..{:?}, {} blocks", lb.name, lb.min, lb.max, lb.blocks);
                }
            }
            // reveal schedule from the LAST COMMIT, as the animator sees it
            println!("\n==== reveal schedule for the last turn (what the animator gets)");
            let (ops, batches) = s.last_commit_batches();
            let frames = anim::schedule(ops, batches, (0, 65, 0), 10, 32);
            let total: usize = frames.iter().map(|f| f.block_count()).sum();
            let fps = anim::pacing(total, 10, 2.0);
            println!("  {} blocks over {} frames at {:.0} fps ~= {:.1}s",
                     total, frames.len(), fps, frames.len() as f32 / fps);
            let mut last = None;
            for f in &frames {
                if last != Some(f.phase) {
                    println!("  frame {:3}: phase -> {:?}  ({} chunks)", f.index, f.phase, f.chunks.len());
                    last = Some(f.phase);
                }
            }
        }
        "models" => {
            // Scan the real ~/models (or a path given as an argument)
            // and show exactly what the chat box would show.
            use voxel_tools::model::{ModelRegistry, default_models_dir};
            use std::time::Duration;
            let dir = args.get(1).map(std::path::PathBuf::from).unwrap_or_else(default_models_dir);
            let mut r = ModelRegistry::scan(dir);
            println!("{}", r.listing());
            if let Some(first) = r.models.first().map(|m| m.name.clone()) {
                println!("== simulating a load of '{}'", first);
                println!("  {}", { r.begin_load(&first).ok(); r.state.message() });
                println!("  {}", r.finish_load(Duration::from_millis(3200), 16384));
                println!("\n== listing now marks the loaded model");
                print!("{}", r.listing());
            }
            println!("\n== autocomplete behaviour");
            for probe in ["", "q", "7b", "nope"] {
                let c = r.complete(probe);
                println!("  '{probe}' -> {} match(es): {}", c.len(),
                         c.iter().take(3).map(|m| m.name.as_str()).collect::<Vec<_>>().join(", "));
            }
        }
        "keys" => {
            // The control surface: keystrokes into the chat box, the
            // actions it emits, and what a voice event does alongside.
            use voxel_tools::chatbox::{ChatBox, Key, Action, CARDS};
            use voxel_tools::voice::{parse_event, to_command_line, INTENT_BUILD, INTENT_COMMIT};
            let mut b = ChatBox::new();
            let press = |b: &mut ChatBox, k: Key| { let a = b.key(k); (a, b.buffer.clone()) };

            println!("== cards a player sees after typing '/'");
            for c in CARDS { println!("  {:9} {:40} {}", c.command, c.signature, c.summary); }

            println!("\n== keystrokes");
            for (label, k) in [("Enter", Key::Enter), ("'/'", Key::Char('/')), ("'a'", Key::Char('a')),
                               ("Tab", Key::Tab), ("Tab", Key::Tab), ("' '", Key::Char(' ')),
                               ("'p'", Key::Char('p')), ("Tab", Key::Tab), ("Enter", Key::Enter)] {
                let (a, buf) = press(&mut b, k);
                let card = b.current_card().map(|c| c.command).unwrap_or("-");
                println!("  {label:6} -> {a:<24?} buffer={buf:<18} card={card}");
            }
            println!("\n== hotkeys with the box closed");
            for (label, k) in [("Ctrl+Z", Key::CtrlZ), ("PageUp", Key::PageUp), ("'w'", Key::Char('w'))] {
                println!("  {label:6} -> {:?}", b.key(k));
            }
            println!("\n== voice events from voiced (same commands, different keyboard)");
            for (verb, args) in [("event", vec![INTENT_BUILD.to_string(), "a mossy well".to_string()]),
                                 ("event", vec![INTENT_COMMIT.to_string()]),
                                 ("event", vec!["play_track".to_string(), "not ours".to_string()])] {
                match parse_event(verb, &args) {
                    Some(i) => println!("  {args:?} -> {i:?} -> {:?}", to_command_line(&i)),
                    None => println!("  {args:?} -> ignored"),
                }
            }
            let _ = Action::Nothing;
        }
        _ => {
            eprintln!("usage: voxel-demo [schemas|palettes|demo [--ascii]|plan [file.plan]|resolve NAME...|session|keys]");
            std::process::exit(2);
        }
    }
}
