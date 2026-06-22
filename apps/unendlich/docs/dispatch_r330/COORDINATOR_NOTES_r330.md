# COORDINATOR NOTES — r330

## This round
Coordinator took **multiplayer** in-house and **dispatched six sections**.

### Built in-house (multiplayer) — VERIFIED in-sandbox
**`sim::net_channels`** — `crates/unendlich-engine/src/sim/net_channels.rs` (+ one `pub mod`
line in `sim/mod.rs`). Roadmap **item 5** of `docs/sections/SECTION_multiplayer.md`; grounds
the **§6 QoS channel layout** of `handoff/MULTIPLAYER_WORLD_JOIN.md` as pure data + a
budgeter. This is "needed before any real transport" and unblocks the entity-snapshot codec
(item 7) and the bin transport (item 9).

- `ChannelId` {Control, Snapshots, Input, Voice, Bulk, Chat}, `ChannelMode`
  {ReliableOrdered, UnreliableSequenced, Unreliable}, `ChannelConfig
  { id, mode, priority, max_bytes_per_tick, keepalive_ms }`.
- `DEFAULT_CHANNELS` = the §6 table as concrete data; snapshots are unreliable-sequenced;
  **Bulk is lowest priority so the priority-ordered budgeter only ever feeds it leftover** —
  the "bulk throttled below snapshots, never starves the hot loop" rule, enforced structurally.
- `budget_tick(pending, tick_budget) -> Vec<Grant>` honors per-channel caps + priority, sum
  never exceeds the tick budget. `keepalive_due` carries the §6 15 s punch keepalive on the
  control channel. Cadence consts: `ADVERT_REANNOUNCE_MS` 5 min, `DHT_TTL_MS` 10 min,
  `PUNCH_KEEPALIVE_MS` 15 s.
- Pure: no IO/clock/RNG/`unwrap`/`expect`/`panic!`; saturating arithmetic.
- **Verify:** `rustc --edition 2021 --test net_channels.rs` → **13 passed / 0 failed**
  (standalone, self-contained — the documented fast gate). Compile-clean.

**MP state after this round** (roadmap `SECTION_multiplayer.md`):
built earlier — `net_candidates`, `net_rendezvous`, `session_mode`, `player_views`,
`net/{identity, dht_mailbox, session, transport, wire}`, `world_authority`/`world_catalog`/
`world_launch`/`world_prober`. **Built now** — `net_channels` (item 5) **and
`net::entity_snapshot` (item 7, §C — the transform-replication codec + newest-wins gate +
host authority stamping; rides the `Snapshots` channel net_channels defines).**
**Still open** (good next coordinator self-tasks): `world_advert` wire codec (item 1),
`net_transport_select` (item 2), `world_presence` (item 3), `world_join_requests` (item 4 —
partly covered by `net/identity` Mailbox), chunk-stream seed-vs-delta codec (item 6, §B).
Items 8–9 (run_world seam + DHT/UDP IO glue) are bin/render, compile-on-end, after the pure layer.

### Dispatched (six sections) — see `DISPATCH_PACKET_r330.md` + `SHARED_RULES.md`
| WID | Section | Task (pure, in-sandbox-validatable) | Grounded in |
|---|---|---|---|
| r330-items | items | `sim::item_tooltip` lore/affix tooltip composition | design/item_armor_lore_tooltips |
| r330-mobs | mobs | `sim::roamer_cap` spawn-admission (NO despawn — r300 history) | §4d + r300 |
| r330-world | world | `sim::surface_scan` top-solid-Y from FP32 voxels (the float bug) | §4a |
| r330-gpu | GPU | mesher foliage **sway-tag** + `chunk.vert` gate | §4b |
| r330-menus | menus | graphics-settings model + Camera/Gameplay page (pure) | §4e |
| r330-bughunt | bughunting | floating-trunk decay + `unwrap`/`expect` panic audit | §4e + SHARED_RULES §1 |

**Merge order on return:** `items → bughunt → mobs → menus → world → gpu`
(new-file/self-contained cores first; world's `surface_y_at` neighbourhood and the gpu
mesher+shader gated last). Gate each (`rustc --test` or
`cargo test --features "render-core egui" --lib <module>`), roll `unendlich-r331-merged.zip`,
add `## r331` ledger entries.

### Addendum dispatch — three more threads (see DISPATCH_PACKET_r330_addendum.md)
| WID | kind | task | grounded in |
|---|---|---|---|
| r330-refactor-math | maintenance | new `util::mathx` exact scalar helpers + migrate ≥10 local-`clamp`/`lerp`/`smoothstep` dup sites (visual/post-fx cluster) | 161 local `clamp`, 34 `lerp`, 20 `smoothstep`; `fastmath` is approx-only |
| r330-refactor-batch | maintenance | new `util::batch_kernel` (`map_into`) + collapse ≥6 `batch_*` map-loop / `nonneg` dups | 67 `batch_*` mirrors; `batch_aggro_decay` exemplar |
| r330-swords | content | `ItemKind::Legendary(LegendarySpec)` + `sim::legendary_loot` drop core + loot hook; render/equip proposed in md | `legendary_weapon::forge` exists but no weapon `ItemKind` variant |

**Disjointness:** refactor-math owns root-level visual `*.rs`; refactor-batch owns `sim/batch_*`
(and never the scalar sibling); swords owns `item.rs`/`legendary_loot`/`loot.rs`-hook. Each brief
carries a FILE ALLOW-LIST + DO-NOT-TOUCH covering the other two and the original six.
**Addendum merge order:** `refactor-math → refactor-batch → swords` (helper libs as new files
first; swords touches `item.rs` exhaustive matches, gated last — then T-items' `item_tooltip`
rebases onto the new variant). Outstanding threads this round: **9** (6 original + 3 addendum).

## Honesty
- `net_channels`: **VERIFIED** (logic actually executed, 13/13 standalone). No bin/GPU/visual
  claims — it's pure data + a budgeter.
- Did **not** run a full-workspace `cargo build` this round (cold link on a ~1031-module,
  7-crate tree overruns a turn; the standalone gate is the documented substitute for a
  dependency-free `sim::` module). The added `pub mod net_channels;` line is the only
  shared-file touch and is a single declaration.
- Dispatch briefs are **doc-grounded only** (no invented scope); every anchor was checked
  against this tree. The mobs brief was corrected to spawn-admission-only after reading the
  r300 ledger note that the operator declined population despawn.
