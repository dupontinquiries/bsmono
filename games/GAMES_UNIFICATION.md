# Games — landing + unification plan

## Landed (in-tree, standalone workspaces under games/)
| game | lang/build | engine | multiplayer | LOC |
|---|---|---|---|---|
| clawbound | Rust/cargo | wgpu + winit | enet, WebRTC, netcode | 4.4K |
| soldier (Soldier) | Rust/cargo | wgpu + winit | enet, netcode | 45K |
| unendlich | Rust/cargo (workspace) | custom | enet, netcode | 326K |
| blackstar (Blackstar) | C++/CMake | Vulkan + OpenGL + SDL2 | enet, netcode | 137K |

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

## Built: unified profiles + micro-DHT (games/shared/netcode, 11 tests)
- `profile`: Profile{id,name,presence,friends} + Presence{Offline,Online,InGame{game,session}},
  versioned length-prefixed wire (round-trips through any backend as bytes).
- `microdht`: Kademlia-lite fallback — 256-bit NodeId, XOR distance, k-buckets, closest-node
  selection, local key→value store. Pure routing math, tested.
- `store`: ProfileBackend trait; MicroDhtStore (always-on) + OpenDHT (FFI, feature `opendht`);
  ProfileService composes them with the fleet's graceful floor — try OpenDHT (global), fall back to
  micro-DHT (LAN), always mirror writes locally so same-network friends resolve instantly. Reach
  {OpenDht,MicroDht,Miss} surfaced. friends_of() resolves a roster across both.

## Built: bsgames centre (games/bsgames — backend 2 tests + egui GUI)
Backend: registry() (the 4 games + launch kind), launch_argv() (cargo run / cmake bin), friend_rows()
(presence → joinable rows, online-first). GUI (bsettings mould): game grid with launch, friends
roster over the profile service, snapclone chat panel. I/O seams marked (spawn, presence publish,
join transport, snapclone deliver).

## Sequence
1. ✅ land the 4 games in-tree
2. ✅ shared netcode: unified profiles over OpenDHT + micro-DHT fallback
3. ✅ bsgames centre (launch + friends/presence + chat scaffold)
4. ✅ wire the seams to EXISTING fleet code (not reinvented):
   - **presence** → coord-core `Activity::InGame` (the bus every fleet app already publishes to);
     `wiring::activity_of` bridges netcode Presence → Activity; bsgames publishes on launch.
   - **chat** → share-kit `Mailbox` over **snapcore's `Store`** (snapcore IS the fleet Kademlia DHT —
     `Key = NodeId`, RoutingTable, node.rs). `wiring::{dm_mailbox,dm_key,send_chat,recv_chat}`;
     messages sealed (crypto-kit) + content-addressed; GUI chat panel uses the real path.
   - **cross-application tests** (bsgames × coord-core × share-kit × snapcore × crypto-kit): presence
     bridge; chat round-trips through the Mailbox+Store attributed + ordered. 5 tests.
   - remaining seams (I/O, on-device): OpenDHT FFI primary; enet session join; SenderChain ratchet
     for chat forward-secrecy (currently a fixed per-DM key); coord-core publish via the Coord client.

   *Note (shared-code find):* snapcore already provides a Kademlia DHT — netcode's `microdht` now
   overlaps it. Follow-up: back netcode's ProfileService fallback with snapcore's Store/node so
   profiles ride the same DHT as chat, and keep `microdht` only as the zero-dep option.
5. render abstraction + backends — the hard one, last
