# RULESET — invariants the harness enforces

Convention adopted from a reference harness: every class of breakage we hit
becomes a numbered rule with an automated check, so it can't recur silently.
Sandbox tier = builds + ctest here (headless); integration tier = the user plays
on-device and reports. Add rules freely as new breakage classes appear.

| # | Rule | The break it prevents | Check |
|---|------|-----------------------|-------|
| R1 | HUD/VGA text only prints inside the 32x16 grid (rows 0-15, cols 0-31). | Off-grid prints spilled onto the world floor (debug overlay, feel tuner). | review + grid-bounds |
| R2 | The VGA overlay binds the DEFAULT framebuffer (0) before drawing. | HUD baked into the scene FBO and smeared across the map. | `ex_vga_render` binds fb 0 |
| R3 | Vehicle steering rotates the velocity vector directly at the turn rate; accel governs only speed. | Low-accel/heavy vehicles were accel-bottlenecked and couldn't turn (dino-on-sand). | `test_movesim::run_turn180` |
| R4 | Transform fold: a part whose fade < 0.05 collapses to exactly 0 scale. | Parts hovered at the cull threshold and flickered appear/disappear mid-fold. | review (deterministic hide) |
| R5 | Every proportion gene stays inside its declared [lo,hi] for any seed; spread 0 = species default; seeds are deterministic. | Genome drift could break the rig or make bots non-reproducible. | `test_genome` |
| R6 | The game and the tuning harness call the SAME movement core (`move.h`). | A harness that measures a copy tunes the wrong thing. | shared `ex_move_horizontal` |
| R7 | Persistent pools (debris) are bounded by a ring buffer even with ~infinite lifetime. | Never-expiring litter would exhaust the pool / leak. | `g_debris_next` ring |
| R8 | Pure logic modules (feel, material, move, genome) include no rendering/GL. | Keeps them unit-testable headless. | header includes |

Run everything: `./run_harnesses.sh`
