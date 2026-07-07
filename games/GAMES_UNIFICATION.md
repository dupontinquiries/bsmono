# Games — landing + unification plan

## Landed (in-tree, standalone workspaces under games/)
| game | lang/build | engine | multiplayer | LOC |
|---|---|---|---|---|
| clawbound | Rust/cargo | wgpu + winit | enet, WebRTC, netcode | 4.4K |
| early-fps | Rust/cargo | wgpu + winit | enet, netcode | 45K |
| unendlich | Rust/cargo (workspace) | custom | enet, netcode | 326K |
| cybertron | C++/CMake | Vulkan + OpenGL + SDL2 | enet, netcode | 137K |

*Note:* unendlich `docs/art-targets` (29M reference renders) omitted to keep git lean —
`docs/ART_TARGETS_OMITTED.txt` marks it; re-add from the upload if wanted.

## Unify multiplayer (all four already speak enet + netcode)
The common substrate is **enet-based netcode** — every game has it. Target: a shared
`games/shared/netcode` layer (session/lobby/replication/transport) the Rust games consume as a
crate and cybertron links as a C lib, so they share one wire protocol + lobby model. clawbound's
WebRTC becomes an alternate transport behind the same seam.

## Abstract the graphics engines ("abstract all, pick best of each")
Split stack: wgpu (clawbound/early-fps), a custom Vulkan/OpenGL C++ engine (cybertron), a custom
Rust engine (unendlich). Target: a thin **render abstraction** (device/surface/pipeline/mesh/
material/frame-graph) with backends — wgpu backend for the Rust games, an FFI backend bridging
cybertron's Vulkan engine — picking the strongest primitive from each (e.g. cybertron's Vulkan
frame-graph, wgpu's portability, unendlich's material system). Large, cross-language; do it last
with all four in view.

## Sequence
1. ✅ land the 4 games in-tree
2. shared netcode crate (enet seam) — the tractable unification first
3. render abstraction + backends — the hard one, last
