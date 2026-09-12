# Wiring the paintbrush into unendlich

Four attachment points. Each names the existing module it hooks
into, because the CLAUDE file's rule applies here: a grep hit pointing
at an existing module is worth two minutes before you build a
parallel one. Nothing below is a new UI.

```
player gaze ──raycast::raycast()──> RayHit ──> focus::Gaze ──> Focus
                                                                 │
chat line ──command_parse::parse()──> Command::Build(prose) ──────┤
                                                                 v
                                                      chat::ChatSession
                                                                 │
                              model round-trip (prose + context → line IR)
                                                                 │
                                                                 v
                                        preview ──/commit──> Vec<Batch> ──> mesher
```

## 1. Chat verbs — `sim/command_parse.rs` (patched)

`patches/unendlich-chat-verbs.patch` adds five verbs to the existing
parser, honouring both of its contracts (`opens_gui`, canonical
round-trip). The module's own property test `prop_round_trip` passes
with them. Compiled and tested in isolation with a stub of
`gamemode_traits`; **not compiled in the full crate** — apply and run
`cargo test` (all targets, per the CLAUDE file, not `--lib`).

| verb | Command | opens GUI |
|---|---|---|
| `/build <prose>` | `Build(String)` — prose kept verbatim, case preserved | yes (plan preview) |
| `/plan` | `ShowPlan` | yes |
| `/commit` | `CommitPlan` | no |
| `/undo` | `UndoBuild` | no |
| `/focus [bigger\|smaller]` | `Focus(Option<FocusAdjust>)` | no |

`/build` is parsed before the tokenizer runs so the prose survives
intact; `/buildx` and an empty `/build` are `Unknown`.

**Enter-to-open:** the existing chat entry already routes lines
through `parse`. Nothing new is needed for the key — only the dispatch
arm for the five new variants (below).

## 2. Focus — `raycast.rs` → `focus::Gaze`

Field-for-field from the existing `RayHit`:

```rust
use unendlich_engine::raycast::{raycast, RayQuery, RayKind, OpaqueOnlyFilter};
use voxel_tools::focus::{Gaze, GazeHit};

let hit = raycast(&world, RayQuery { origin: eye, direction: look, max_distance: 128.0,
                                     kind: RayKind::Block }, &OpaqueOnlyFilter);
let gaze = Gaze {
    eye: (eye[0], eye[1], eye[2]),
    dir: (look[0], look[1], look[2]),
    hit: hit.map(|h| {
        let n = h.face.normal();
        GazeHit { block: (h.block[0], h.block[1], h.block[2]),
                  face_normal: (n[0], n[1], n[2]), distance: h.distance }
    }),
};
```

Check the `RayKind` variant name against your enum; I saw the enum
exists but did not read its variants.

`Focus::from_gaze(&gaze)` then gives:

- **anchor** — the empty cell in front of the hit face, i.e. where a
  placed block would go. Top face → one above. This is "here".
- **tier** — from distance: Detail <8, Structure <32, Landscape <96,
  Horizon beyond or sky. Sets the read radius (6 / 16 / 48 / 96).
- **region** — a sphere for near tiers, a ground-hugging box for
  landscape tiers (more above the anchor than below, because you care
  about a footprint and what stands on it, not the sky).
- **feature_id** — hash of (anchor quantised to the tier's cell,
  face normal, tier). Stable under gaze jitter; changes on a tier
  change or a different cell. Looking at sky projects an anchor 48
  blocks out rather than failing, marked `projected`.

## 3. The world binding — `world/chunk_manager.rs`

```rust
impl VoxelWorld for ChunkManager {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { self.block_at(x, y, z) as u16 }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
        self.set_block(x, y, z, BlockId::from_u16(b))   // or however ids convert
    }
}
```

`set_block` takes `&self` in the current tree (line ~2298). Either the
trait impl goes through whatever interior mutability it already uses,
or the trait's `set` becomes `&self` — a one-line change in
`voxel_tools::VoxelWorld`. This is the one integration unknown I could
not resolve without compiling.

## 4. Dispatch — wherever `Command` is matched today

```rust
use voxel_tools::chat::{ChatSession, Reply};
use voxel_tools::VoxelTools;

// per player, persisted across turns
let session = ChatSession::new(CHUNK_SIZE, 200);

match command {
    Command::Build(prose) => {
        let mut tools = VoxelTools::new(&mut chunk_manager);
        let context = session.context(&tools, &gaze, surface_block_name);
        // ONE model round-trip. System prompt: the line IR grammar +
        // 3-4 worked examples (stable prefix, cache it). User turn:
        // context + prose. The reply is line IR.
        let ir = llm.complete(&context, &prose);
        match session.receive_plan(&mut tools, &gaze, &ir) {
            Reply::Preview { echo, blocks, .. } => open_plan_panel(echo, blocks),
            Reply::Say(msg) => chat_log(msg),
            _ => {}
        }
    }
    Command::ShowPlan => show(session.show_plan()),
    Command::CommitPlan => {
        let mut tools = VoxelTools::new(&mut chunk_manager);
        match session.commit(&mut tools, &gaze) {
            Reply::Committed { batches, .. } => animate(batches),   // see below
            Reply::Say(msg) => chat_log(msg),                        // e.g. focus moved
            _ => {}
        }
    }
    Command::UndoBuild => show(session.undo(&mut VoxelTools::new(&mut chunk_manager))),
    Command::Focus(adj) => show(session.adjust_focus(&gaze, adj.map(|a| a == FocusAdjust::Bigger))),
    // existing arms unchanged
}
```

**The stale-focus guard.** `commit` refuses if the feature id has
changed since the plan was previewed, because the plan's `@dx,dz`
coordinates were resolved against the *old* anchor at parse time.
Committing anyway would build where the player *was* looking. The
plan stays pending; looking back at the spot makes it commit-able
again. There is a test for exactly this.

## 4b. Controls — `chatbox::ChatBox`

The box is a pure state machine; the game maps its key events onto
`chatbox::Key` and acts on the returned `Action`. In egui:

```rust
use voxel_tools::chatbox::{ChatBox, Key, Action};

let key = ctx.input(|i| {
    if i.key_pressed(egui::Key::Enter) {
        if i.modifiers.ctrl { Some(Key::CtrlEnter) } else { Some(Key::Enter) }
    } else if i.key_pressed(egui::Key::Escape) { Some(Key::Escape) }
    else if i.key_pressed(egui::Key::Tab) {
        if i.modifiers.shift { Some(Key::ShiftTab) } else { Some(Key::Tab) }
    }
    else if i.key_pressed(egui::Key::ArrowUp) { Some(Key::Up) }
    else if i.key_pressed(egui::Key::ArrowDown) { Some(Key::Down) }
    else if i.key_pressed(egui::Key::PageUp) { Some(Key::PageUp) }
    else if i.key_pressed(egui::Key::PageDown) { Some(Key::PageDown) }
    else if i.modifiers.ctrl && i.key_pressed(egui::Key::Z) { Some(Key::CtrlZ) }
    else if i.key_pressed(egui::Key::Backspace) { Some(Key::Backspace) }
    else { i.events.iter().find_map(|e| match e {
        egui::Event::Text(t) => t.chars().next().map(Key::Char), _ => None }) }
});
if let Some(k) = key {
    match chatbox.key(k) {
        Action::Submit(line) => dispatch(command_parse::parse(&line)),
        Action::Commit => dispatch(Command::CommitPlan),
        Action::Undo => dispatch(Command::UndoBuild),
        Action::FocusBigger => dispatch(Command::Focus(Some(FocusAdjust::Bigger))),
        Action::FocusSmaller => dispatch(Command::Focus(Some(FocusAdjust::Smaller))),
        Action::Opened => capture_keyboard(true),    // stop WASD reaching the player
        Action::Closed => capture_keyboard(false),
        _ => {}
    }
}
// draw: the buffer, and beside it chatbox.current_card() / matching_cards()
```

**Key ownership.** While the box is open, keys go to it and not to
movement — `Opened`/`Closed` are where to flip that. With the box
closed, only Ctrl+Z, PageUp and PageDown are claimed; everything else
returns `Nothing` and falls through to the game.

## 4c. Voice — `voice::grammar` + `ipc_kit::Client`

No audio in the game. On `Command::BuildVoice(true)`:

```rust
let (reply, grammar) = session.set_voice(true);
let mut vc = ipc_kit::Client::connect("voiced")?;
vc.request(&Message::new("hello", vec!["unendlich".into()]))?;
for args in grammar { vc.request(&Message::new("register", args))?; }
// keep `vc` and poll it each frame:
while let Some(msg) = vc.try_recv() {
    if let Some(intent) = voice::parse_event(&msg.verb, &msg.args) {
        match session.voice(&mut tools, &gaze, &intent) {
            VoiceRoute::NeedsModel(prose) => { /* same round-trip as /build */ }
            VoiceRoute::Done(reply) => show(reply),
            VoiceRoute::Ignored => {}
        }
    }
}
```

`Client::try_recv` — check the real method name in `ipc_kit`; the
daemon's tests use blocking `recv()`. Voice is a second keyboard:
every intent becomes the same `Command` the typed path produces, so
nothing downstream knows which one spoke. "build a stone tower" →
preview; "build it" → the guarded commit; "undo" → undo.

## 5. Animation — `session.reveal(blocks_per_frame)` → mesher

`/anim` selects a profile; `session.reveal(n)` returns frames for the
last commit under it. Profiles:

| profile | order | animator hint |
|---|---|---|
| `ground-up` | ground < structure < decoration, walls rise, grows outward | — |
| `physics` | bottom-up per column, columns staggered | `frame.drop_from = Some(12)`: draw each block falling from 12 above |
| `random` | seeded shuffle | — |
| `instant` | one frame | skip the animation |

```rust
let frames = session.reveal(25);
let fps = anim::pacing(frames.iter().map(|f| f.block_count()).sum(), 25, 3.0);
for f in &frames {
    for (chunk, blocks) in &f.chunks {
        if let Some(h) = f.drop_from { animate_drop(blocks, h); }
        mesher.mark_dirty(*chunk);
    }
    mesher.rebuild_dirty();
    sleep(1.0 / fps);
}
```

Frames are grouped by chunk, so each chunk remeshes once per frame
and chunks within a frame can rebuild in parallel.

## 5-old. Animation — `Vec<Batch>` → mesher

Each `Batch` is one chunk's worth of writes for one tick (the tick is
the op index, so the build reveals in authored order). The blocks are
already in the world by the time you get the batches; the animator's
job is to *reveal* them:

```rust
for tick in 0..=max_tick {
    let this_tick: Vec<&Batch> = batches.iter().filter(|b| b.tick == tick).collect();
    // all batches in a tick touch distinct chunks: remesh in parallel
    for b in &this_tick { mesher.mark_dirty(b.chunk); }
    mesher.rebuild_dirty();
    sleep(tick_interval);
}
```

If you want a *progressive* reveal within a tick, hold the batch's
blocks as air until its frame and write them then — `run_plan` gives
you the coordinates; the previous block values are in
`tools.edits[..].before` if you need to reverse the reveal.

## Prompt shape (for the model round-trip)

System (stable, cache it):
- the line IR grammar from `plan.rs`'s module doc
- the `@dx,dz` convention and "Y comes from the ground"
- 3–4 worked examples: a tower, a path to it, a grove, a restyle
- `HECTOR_MODE_BRIEF` if reply budget is 512

User turn: `session.context(...)` followed by the prose. The context
already says READ FIRST and includes the region digest and probe, so
the model has read the file before it is asked to write.

## What is verified and what is not

- `voxel-tools`: 55 tests pass, including focus tiers, feature-id
  stability, relative-coordinate resolution, stale-focus refusal,
  preview/commit equality, and unit undo.
- `command_parse.rs`: all 10 tests pass in isolation against a stub
  mirroring the real `gamemode_traits` surface.
- **Not compiled:** the `VoxelWorld for ChunkManager` impl, the
  dispatch arms, the raycast field mapping. Those are the seams; they
  are small, but they are unverified until `cargo test` runs in the
  engine crate.
