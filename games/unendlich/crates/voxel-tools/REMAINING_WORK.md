# What's left to type in a world and see a structure

> **Status after a stepwise pass (2026-09-05).** Steps 1, 2 and 3 are
> written into the tree and exported as patches; step 3's unknown is
> resolved and turned out to hide a performance bug worth more than the
> original question. Steps 4-9 are unchanged.
>
> **I could not build or run the game.** Evidence, not assumption: this
> sandbox has rustc 1.75.0 with no rustup, `static.rust-lang.org` is
> blocked, and the workspace needs egui 0.34 / wgpu 29 whose transitive
> deps declare `resolver = "3"` — cargo 1.75 refuses to parse them
> (`redox_syscall-0.9.4/Cargo.toml: resolver setting 3 is not valid`).
> The headless-egui route in `docs/agents/HEADLESS_GUI_TESTING.md` is
> real but needs a modern toolchain, which I cannot install here.
>
> What I did instead: `cargo run --example bindcheck` in `voxel/`
> exercises the entire pipeline against stubs whose signatures were
> read out of the real engine source. It prints:
>
> ```text
> preview: 258 blocks
> commit: 258 blocks, 6 batches
> chunks that will be cloned: 4
> flushed: applied=254 skipped=0 clone_for_edit_calls=4
> world now: (0,65,0)=Air  (3,66,0)=StoneBricks
> reveal: 11 frames
> ```
>
> That verifies the shapes and the arithmetic. It does not verify that
> `BlockId`, `ChunkManager` and `raycast` behave as their signatures
> suggest — only a build in your tree does that.

Everything in `voxel/` is written and tested (106 tests). Nothing in it
has been compiled *against unendlich*, because this sandbox has no GPU,
no Qt, and no way to open a world. So the remaining work is all seam
work, and it is small but real.

Ordered so each step is verifiable before the next.

---

## 0. Prerequisite: an inference server (30 min, no code)

The paintbrush needs something to talk to. Any OpenAI-compatible
endpoint works:

```bash
llama-server -m ~/models/qwen2.5-7b-instruct-q4_k_m.gguf \
    -c 16384 --port 8080 --host 127.0.0.1
curl -s localhost:8080/v1/models     # sanity
```

Pick a model that fits: `cargo run -- models` in `voxel/` lists what is
in `~/models` with sizes and quant tags. A 7B at Q4 with 16k context is
the sweet spot for this workload — the system prompt is ~1.5k tokens
and stable, so with prefix caching only the per-turn context is new.

**Verify:** `cargo run -- models` prints your real models.

---

## 1. Add the crate to the workspace (10 min)

```toml
# games/unendlich/Cargo.toml
[workspace]
members = [..., "crates/voxel-tools"]
```

Copy `voxel/` to `games/unendlich/crates/voxel-tools/`. It has no
dependencies, so this cannot break your dependency tree.

**Verify:** `cargo test -p voxel-tools` — 106 tests, no network.

---

## 2. Apply the parser patch (10 min)

```bash
git apply patches/unendlich-chat-verbs.patch
cargo test -p unendlich-engine        # NOT --lib; see CLAUDE.md
```

Adds `/build`, `/plan`, `/commit`, `/undo`, `/focus`, `/anim`,
`/model`, and `/build enableVoice`. The module's own `prop_round_trip`
property test covers the new variants.

**Verify:** its 12 tests pass in the real crate, not just in isolation.

---

## 3. The `VoxelWorld` impl — RESOLVED, and it found a bug (done)

**The `&self` question.** `ChunkManager` holds
`chunks: Arc<DashMap<ChunkCoord, Arc<Chunk>>>`, so the interior
mutability is in the DashMap and `set_block` correctly takes `&self`.
**No trait change is needed:** a newtype holding `&ChunkManager` can
expose `&mut self` methods that call through. The `&mut` lives on the
wrapper. That is `voxel_bind::WorldView`.

**The bug behind it.** Reading `set_block` before wiring to it:

```text
let mut new_chunk = entry.value().clone_for_edit();   // clones 32^3 BlockIds
new_chunk.set(lx, ly, lz, block);
self.replace_edited_chunk(new_chunk, ...);            // + octree rebuild
...invalidate_for_edit... bump_neighbor_if_edge x3
```

Copy-on-write **per block**. Right for a player breaking one block;
catastrophic for a generated structure. A 500-block tower spanning
four chunks would clone 500 chunks — 16.4M BlockId copies — to change
500 cells. There is no batch entry point in the tree.

So the patch adds two things:

- `ChunkManager::set_blocks(&[(x,y,z,BlockId)]) -> (applied, skipped)`
  — groups by chunk, one `clone_for_edit` each, dedupes the neighbour
  bumps so a wall along a chunk edge bumps each neighbour once instead
  of once per block. Mirrors the single-block logic including the
  wild-coordinate watchdog.
- `voxel_tools::staged::StagedWorld` — the tools write here, not to
  the world. Reads fall through; writes buffer; `drain_by_chunk`
  hands back edits grouped for one clone each.

Measured on the bindcheck run: **254 blocks, 4 clone_for_edit calls**
instead of 254. For the 500-block case that is 125x fewer BlockId
copies. Three things also fall out free: dry runs never touch the
world, undo is the overlay's recorded previous values, and the
animator gets its block list without a second pass.

**Verify in your tree:** `cargo test -p unendlich-engine` (all
targets), then a unit test that stages a few blocks and flushes.

## 3-old. The `VoxelWorld` impl — was the unknown

```rust
impl voxel_tools::VoxelWorld for ChunkManager {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 { self.block_at(x, y, z) as u16 }
    fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { /* ... */ }
}
```

`ChunkManager::set_block` takes `&self` (chunk_manager.rs ~2298).
Either route the trait's `set` through the interior mutability it
already uses, or change the trait to `&self` — a one-line edit in
`voxel-tools/src/lib.rs`. **This is the only place I could not
resolve without building the engine**, and everything downstream
depends on it, which is why it is step 3 and not step 8.

Also needed: `u16` → `BlockId`. If there is no `from_u16`, a
`match` over the enum is fine; `resolve_block` in `plan.rs` already
has the full numbering.

**Verify:** a unit test that places one block through `VoxelTools`
and reads it back through `ChunkManager`.

---

## 4-9. Written, and run end to end against stubs (done)

`voxel_chat::Paintbrush` in the engine patch owns the chat box, the
session, the model registry and the voice link, and routes every
`Command`. `harness/` runs the whole thing:

```bash
cd voxel/harness && cargo run
```

It drives keystrokes -> Commands -> a scripted model -> preview ->
commit -> flush -> reveal, against stubs whose signatures were read
out of the engine source. Output:

```text
model loaded: qwen2.5-7b-instruct-q4_k_m  ctx 16384  in 3.2s
Plan 'watchtower' - 2 operations
220 blocks planned -- /commit to build
built 220 blocks in 4 batches
220 blocks, 4 chunk(s) remeshed, 9 frames
world: (3,66,0)=StoneBricks  interior (0,66,0)=Air
9 frames at 4 fps ~= 2.2s
pass inferred: detail, referent present
70 blocks planned -- /commit to build
door carved: (3,66,0)=Air
undid last build: 70 blocks restored across 2 ops
registering 17 grammar rules
spoken build -> Model("a stone wall")
foreign event -> None
```

**Running it found three bugs that reading did not.** All three are
fixed with regression tests, and all three would have been visible
only as wrong behaviour in-game:

1. **Dry runs staged 220 no-op writes.** `receive_plan_for` applies a
   plan and reverses it, but reversing calls `set()`, which staged
   *more* edits rather than cancelling them. Values were right;
   `pending()` was 220, and a flush would have cloned chunks for
   nothing. Caught by a `debug_assert` in the binding. Fixed: writing a
   cell back to its base value drops the entry.
2. **Undo restored 0 blocks.** The edit log lives on `VoxelTools`, and
   the game builds fresh tools every dispatch -- so by the time
   `/undo` arrived, the ids referred to a log that no longer existed.
   Fixed: the session captures the inverse block list at commit, so
   undo is independent of tool lifetime.
3. **The door was carved in the sky.** `sphere block=air` with no `y`
   measured from `find_surface` at its own column -- which, once the
   tower exists, is the parapet ten blocks up. Fixed: carving measures
   from the plan's focus anchor, which is the ground the player is
   looking at.

What remains is the part stubs cannot cover: that `BlockId`,
`ChunkManager` and `raycast` behave as their signatures suggest. Apply
both patches and run `cargo test -p unendlich-engine`.

## 4-old. Gaze → focus (20 min)

Map `raycast::RayHit` onto `focus::GazeHit` (see INTEGRATION.md §2).
Check `RayKind`'s variant names — I saw the enum but not its members.

**Verify:** print `Focus::from_gaze(&g).read_hint(None)` each frame in
a debug overlay. Look at a block: the anchor should be the cell in
front of the face you are looking at, and the tier should change as
you look further away.

---

## 5. Chat box in egui (1–2 h)

`chatbox::ChatBox` is a pure state machine; INTEGRATION.md §4b has the
egui key mapping. Two things that are easy to get wrong:

- **Capture the keyboard on `Action::Opened`** and release on
  `Closed`, or WASD will walk you around while typing.
- Draw `current_card()` beside the box and `matching_cards()` as a
  dropdown. That is the whole discoverability story.

**Verify:** type `/`, see eight cards; Tab cycles; Enter submits.

---

## 6. The model round-trip (1–2 h)

The one piece with no code here, because it depends on your HTTP
client:

```rust
let system = voxel_tools::prompt::system_prompt();          // cache this prefix
let (turn, _pass) = session.user_turn(&tools, &gaze, surface_name, &prose, None);
let ir = post_chat(&system, &turn)?;                        // your client
session.receive_plan_for(&mut tools, &gaze, &ir, Some(player_feet));
```

Stop tokens: `\n\n`. Max tokens: 512 is plenty for a plan.
Temperature low (0.2–0.4) — this is structured output, not prose.

**Verify:** log the raw IR next to the parsed op count. If the model
emits prose around the plan, the parser drops those lines as unknown
ops and says so, so you will see it immediately.

---

## 7. Reveal (1 h)

`session.reveal(25)` → frames → `mesher.mark_dirty(chunk)` per frame.
INTEGRATION.md §5 has the loop and what `drop_from` means for the
physics profile.

**Verify:** `/anim instant` vs `/anim ground-up` should look obviously
different.

---

## 8. Model loading (1 h)

`/model` lists, `/model <name>` loads. `ModelRegistry` handles scan,
autocomplete, ambiguity and the `LoadState` messages; the game supplies
the actual load (spawn `llama-server`, or POST to a running one).

```rust
Command::Model(None) => show(registry.listing()),
Command::Model(Some(name)) => {
    match registry.begin_load(&name) {
        Ok(m) => {
            let path = m.path.clone();
            chat_log(registry.state.message());          // "loading ..."
            let t0 = Instant::now();
            match spawn_or_switch_server(&path) {
                Ok(ctx) => chat_log(registry.finish_load(t0.elapsed(), ctx)),
                Err(e) => chat_log(registry.fail_load(e.to_string())),
            }
        }
        Err(e) => chat_log(e),                            // ambiguous or missing
    }
}
```

After a scan, feed the names to the box so Tab works:

```rust
chatbox.set_dynamic_args("/model",
    registry.models.iter().map(|m| m.name.clone()).collect());
```

**Verify:** `/model` lists your `~/models`; `/model qw` + Tab completes;
loading prints `model loaded: … ctx 16384 in 3.2s`.

---

## 9. Voice (optional, 1 h)

`/build enableVoice` → register `voice::grammar()` with the `voiced`
socket, poll for `event`, route through `session.voice(...)`.
INTEGRATION.md §4c. Check whether `ipc_kit::Client` has a non-blocking
receive — the daemon's tests only show blocking `recv()`, and a
blocking call in the frame loop will stall the game.

---

## Realistic total

Steps 0–8 are roughly **a day**, and step 3 dominates the risk.
Steps 0, 1, 2 and 8 are mostly mechanical; 3 is the one that could
turn into an afternoon if `ChunkManager`'s mutability fights you.

A useful shortcut for the first run: skip steps 5–7 and drive it from
a debug key. Bind F7 to a hardcoded `/build a stone tower` with a
canned IR string, commit it instantly, and remesh everything. That
proves steps 3 and 4 in isolation, which is where the real risk is —
the chat box and the model are comparatively easy once blocks appear
where you look.
