# r80 — bin/render handoff: music, hotbar, title splash (compile-on-end)

The engine-side logic landed this round is pure + verified. The pieces below are the
on-device (glfw/egui/audio = system-lib) wiring that compiles only in the full bin build.

## 1. Title music (small)
The bin already drives `fg_music` via `sim::music_director` and has a TODO at
`unendlich_fullgame.rs:3599`. Add an audio backend (rodio) and play the cued track:

```toml
# Cargo.toml [dependencies] (bin-only; needs system ALSA/CoreAudio)
rodio = "0.19"
```
```rust
// near renderer setup
let _audio = rodio::OutputStream::try_default().ok();
let sink = _audio.as_ref().map(|(_, h)| rodio::Sink::try_new(h).unwrap());
let mut playing_asset: Option<&str> = None;
// in the music block (~3598), after fg_music.step(...):
if let Some(sink) = &sink {
    match (fg_music.asset(), playing_asset) {
        (Some(path), prev) if prev != Some(path) => {
            sink.stop();
            if let Ok(f) = std::fs::File::open(path) {
                if let Ok(src) = rodio::Decoder::new(std::io::BufReader::new(f)) {
                    sink.append(src.repeat_infinite());
                }
            }
            playing_asset = Some(path);
        }
        (None, Some(_)) => { sink.stop(); playing_asset = None; }
        _ => {}
    }
    sink.set_volume(fg_music.volume01);
}
```
`music_director::cue_for(AppState::MainMenu)` already returns the title theme; the cross-fade
volume is in `fg_music.volume01`. Track: `assets/music/title_koyopper.opus` (staged).

## 2. Hotbar with real items + abilities (`sim::hotbar`)
Replace the bin's hardcoded 1–9 BlockId palette (~`unendlich_fullgame.rs:835`/`2282`) with
`sim::hotbar::Hotbar`:

```rust
use unendlich_engine::sim::hotbar::{Hotbar, HeldAbility};
let mut hotbar = Hotbar::with_items(&starter_stacks); // e.g. Stone, Dirt, OakLog, Torch...
// input: Hotbar1..9 → hotbar.select(n); scroll wheel → hotbar.scroll(±1)
// right-click place: if let Some(stk) = hotbar.active_item() { place(stk.item); hotbar.consume_active_one(); }
// HUD readout (egui, bottom bar): for each slot draw the tile (ItemKind::Block(b).display_color());
//   highlight hotbar.active_index(); show hotbar.active_ability().verb() + the item name.
// drag from the inventory screen: hotbar.assign_from(&mut inv_grid, inv_slot, hb_slot)
```
`HeldAbility` (`Place` / `PlaceLight` / `PlaceDecoration` / `Mineable` / `PlaceFluid`) gives the
"what does this do" readout the operator wanted. To carry richer items (spawn eggs / tools), widen
`inventory::ItemStack` from `BlockId` to `ItemKind` later — out of scope for this pass.

## 3. Title splash scene (`sim::title_cinematic`)
All the *motion* is in the module; the renderer samples it. Architecture:

- **3-chunk bake/freeze:** on entering MainMenu, generate a 3-chunk-radius region around a fixed
  showcase origin, mesh it once, and STOP world streaming + simulation ticks (freeze entity
  updates, mesher, chunk loader). Cheap + stable backdrop → headroom for nicer shaders. Re-enable
  streaming on StartWorld/Load. (Gate the existing main-loop `spawn_streamer`/`tick_apply` on a
  `menu_frozen` bool.)
- **Prezi camera:** each frame `let pose = title_path.sample(elapsed_secs);` where
  `title_path = CinematicPath::intro();` then drive the render camera from `pose.eye/target/fov_deg`
  (smootherstep-eased, looping, non-axis-aligned hero angles already baked in).
- **Menu pieces fly to custom locations:** for `MenuPiece::ALL`, `piece_state(piece, elapsed, 0.5)`
  → `pos01` (normalised viewport anchor) + `alpha01`. Place each egui menu window at
  `pos01 * viewport`, fading/sliding in on the staggered `fly_delay_secs` cascade. Anchors are
  deliberately asymmetric (`anchor01()`), with a `theme()` tint per piece.
- **Transparent UI + hover fade:** keep one `HoverFade::new(0.35)` per interactive widget; each
  frame `let a = fade.step(ui.rect_contains_pointer, dt);` and paint the panel with fill/stroke
  alpha `a`. Resting at 0.35 (translucent), easing to 1.0 on hover — frame-rate independent.
- **Shaders:** the frozen scene can afford the AAA stack already merged (PBR/water/sky/volumetrics/
  tonemap+bloom); enable bloom + a shallow DoF on the menu backdrop for the "splash" look.

This is sizeable render work — a good candidate for a dedicated thread; the sim backbone here makes
that thread purely a drawing/wiring job (camera + egui placement + freeze toggle), not motion design.

## 4. Plants / blocks / textures — status
- **Plants:** tree profiles added for the 3 new surface biomes (Savanna sparse, Jungle dense-tall,
  Taiga conifer). Ground flora (Flower/TallGrass/Fern/Mushroom) has no scatter hook in
  `cpu_generator` yet — next step is a per-column decoration pass keyed on biome (verifiable).
- **Blocks:** 59 BlockIds already exist (incl. Granite/Andesite/Slate/Marble/Calcite/Crystal/Amber);
  the new cave biomes now surface Crystal (CrystalCave) and Moss (LushCave). Wiring more stone
  variants into underground strata is a `cpu_generator` change (verifiable).
- **Textures:** the incoming pack is staged at `docs/textures_incoming/` (manifest JSON + gen script
  + PNGs). Loading it is a render/asset-pipeline task (atlas build + sampler), compile-on-end.
