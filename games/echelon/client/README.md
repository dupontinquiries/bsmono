# echelon-client (M1) — Bevy front-end

Play any race against the scripted bot of any other, on the same
deterministic sim the headless runner and lockstep harness use. The client
never touches game state directly — every click becomes a `Cmd` stepped at
22 Hz through `World::step`, so a remote human can replace the local bot
without touching this crate's game logic.

## Toolchain

This crate is **detached from the parent workspace** (`exclude =
["client"]`) because Bevy needs a current stable Rust, while everything
else in `apps/echelon` stays on distro rustc 1.75. Use rustup here:

```sh
rustup toolchain install stable
cd apps/echelon/client
cargo run --release                    # Genetron (you) vs Keiron (bot)
cargo run --release -- Xayid Genetron 42   # race1=you race2=bot seed
```

Linux notes: needs Vulkan (Asahi's Honeykrisp works; `mesa-vulkan-drivers`
on x86) and the usual Bevy deps (`libwayland-dev`/X11, `libasound2-dev`,
`libudev-dev` on Debian/Ubuntu).

## Controls

| input                    | action                                   |
|--------------------------|------------------------------------------|
| WASD / arrows / Q        | pan (A is attack, Q pans left)           |
| mouse wheel              | zoom                                     |
| left click / drag        | select own units (shift adds)            |
| right click              | move; workers on a resource → harvest    |
| A, then left click       | attack-move (Esc cancels)                |
| S                        | stop                                     |
| 1–9                      | train from selected structure (HUD list) |
| B                        | cycle buildable structure for a selected worker; ghost shows validity; left click places; Esc cancels |

The HUD shows resources, supply, game clock, selection info, the train
hotkey map, and VICTORY/DEFEAT when a side loses its last structure.

## Pinned against `bevy = "0.16"`

If your Bevy differs, these are the only drift points, all one-liners:

- `Query::single()/single_mut()` return `Result` here (0.16). On 0.15 use
  `get_single()/get_single_mut()`.
- `Camera::viewport_to_world_2d(..)` returns `Result` here (0.16); 0.14
  returned `Option` (drop the `.ok()`).
- `Time::delta_secs()` (0.15+) was `delta_seconds()` on 0.14.
- UI text uses the 0.15+ `Text`/`TextFont`/`TextColor`/`Node` components.

## Not here yet (tracked)

Fog of war, health bars, minimap, rally points, sounds, ability buttons —
and the real multiplayer hookup: swap the in-process bot for an
`echelon-net::Lockstep` session over a UDP/relay transport (net-kit), with
the pack hash + seed as the match handshake.
