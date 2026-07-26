# Porting the engine to Rust — scope and method

## The honest answer: not all of it

    game.c            11,932 lines   745 static globals   171 SDL calls
    exengine (logic)   6,123 lines   no SDL, no GL, no Vulkan
    exengine (gfx/os)    628 lines
    third_party        Vulkan backend

**The 6,123 pure-logic lines are worth porting.** They take data in and return data
out, so a port can be *differentially tested* against the original — the C and the
Rust run the same inputs and their output is compared byte for byte. That is a real
guarantee, not a claim.

**game.c is not.** 745 interdependent static globals driving SDL input and a Vulkan
backend is an architectural rewrite rather than a translation, and none of it can be
verified here: there is no display, so "it compiles" would be the only evidence, and
this project has repeatedly shown what unverified graphics code is worth. A rewrite
that cannot be run is a rewrite that cannot be trusted.

## Method: differential testing, not inspection

`combat.rs` is the worked example. The C and the Rust each run 200 random ray casts
from a *shared xorshift stream* — same seed, same sequence — and print results:

    C lines: 400   Rust lines: 400
    IDENTICAL output — the port is bit-for-bit faithful

Both `ray_aabb` (slab method) and `ray_sphere` agree exactly, including the edge
cases: rays parallel to a slab, roots behind the origin, hits beyond `max_dist`.

No module counts as ported until it passes this. Reading the C and writing
equivalent Rust is how subtle differences in float ordering and early-exit
conditions survive into a "faithful" port.

## Candidates, in order of value

    mapgen.c        332   world generation — pure, well-defined output
    robot_parts.c   348   part assembly rules
    signature.c     234   the signature system
    encounter.c     253   encounter selection
    enemy.c         288   enemy behaviour tables
    lore.c          299   text generation
    combat_json.c   293   combat data loading
    commander_ai.c  151   AI decisions
    hive_ai.c       178
    mapfsm.c        186   map state machine

Each is self-contained enough to port and diff-test in isolation, and each can be
called from the C via FFI once ported, so the engine keeps running throughout —
no big-bang switchover.

## What stays in C, permanently

Rendering, input, windowing, the Vulkan backend, and game.c's frame loop and state
machine. Those are where SDL lives and where a port buys least: they are not the
parts that benefit from Rust's guarantees, and they are the parts that cannot be
tested here.
