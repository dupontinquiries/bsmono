# combat_json -- the LLM/Blackstar combat boundary

A clean JSON seam between the game's live combat state and an external "brain"
(intended for later LLM integration: Blackstar reading how the player fights and
issuing directives back). Module: `engine_src/exengine/combat_json.{h,c}`.
Pure C, no external JSON dependency, fully unit-tested (`tests/test_combat_json.c`).

## The loop (this is what "completable" means)

```
  game state ──► ex_cj_build_snapshot ──► ex_cj_write_snapshot ──► JSON
                                                                     │
                                                            (brain / LLM: Blackstar)
                                                                     │
  apply to units ◄── ex_cj_orders_t ◄── ex_cj_read_orders ◄──── JSON reply
```

Every piece except the brain itself is implemented and tested. Wiring the brain
is a matter of shipping the snapshot string out (file, socket, or the netcode
transport) and feeding the reply back into `ex_cj_read_orders`.

## SNAPSHOT (game -> brain)

`ex_cj_write_snapshot` emits:

- `tick` -- sequence counter for ordering (echo it back in the orders).
- `player` -- `pos`, `in_vehicle`.
- `signals` -- the ENGAGEMENT INPUT VARIABLES a roleplaying brain reacts to:
  - `aggression` 0..1 -- how hard the player is pushing (fire rate + closing speed)
  - `recent_dmg` -- damage taken lately
  - `pressure` 0..1 -- how surrounded/threatened the player is
  - `kills`, `enemies_alive`, `player_hp_frac`
- `allies[]` -- bots following the player: `pos`, `hp`, `mode` (0 follow / 1
  engage / 2 down), `target` enemy id.
- `units[]` -- troops/enemies: `pos`, `hp`, `zone` (0 front / 1 mid / 2 back
  owned-space line), `kind`, `element`, `alive`.

`ex_cj_signal_tracker_t` + `ex_cj_signals_tick` smooth per-frame fire/closing/
near-count into the 0..1 `aggression`/`pressure`. `ex_cj_build_snapshot` fills a
snapshot from the game's parallel arrays in one call.

## ORDERS (brain -> game)

`ex_cj_read_orders` parses a reply into `ex_cj_orders_t` -- a list of commands,
each targeting a GROUP (by `zone` 0/1/2, or -1 = all):

| verb           | meaning                                        |
|----------------|------------------------------------------------|
| `flock_to`     | move a group toward `point` (cohesive)         |
| `flock_spread` | spread a group around `point` (encircle)       |
| `hold`         | group holds position                           |
| `focus`        | group presses the attack on the player         |
| `retreat`      | group falls back                               |
| `ally_follow`  | set ally behavior (`param0`: 0 follow/1 engage)|

Each command carries `point[3]` and `param0/param1` (verb-specific: radius,
urgency, mode).

### Tolerant by design

The parser is deliberately forgiving so a chatty LLM reply decodes cleanly:
prose before/after the JSON, unknown keys (`reasoning`, `mood`, ...), and
unknown verbs are all skipped; missing fields take defaults. An observe-only
reply (no `commands`) is valid and yields zero commands. Field parsing is
windowed per-command so values don't bleed between commands.

## Example

Snapshot out:
```json
{"tick":42,"player":{"pos":[10,1,-5],"in_vehicle":1},
 "signals":{"aggression":0.80,"recent_dmg":12.5,"pressure":0.40,"kills":7,"enemies_alive":5,"player_hp_frac":0.65},
 "allies":[{"id":900,"pos":[3,1,3],"hp":80,"max_hp":100,"mode":1,"target":103}],
 "units":[{"id":100,"pos":[5,0,0],"hp":50,"max_hp":100,"zone":0,"kind":0,"element":1,"alive":1}]}
```

Blackstar replies (prose + JSON both fine):
```
The guardian over-commits. Encircle and punish.
{"speaker":"blackstar","tick":42,"reasoning":"punish the reckless push",
 "commands":[
   {"type":"flock_spread","zone":0,"point":[10,0,-5],"param0":0.9},
   {"type":"focus","zone":2,"param1":0.7}
 ]}
```

`ex_cj_read_orders` yields 2 commands with fields intact.

## Not yet wired (future work)

- The game-side call sites: build a snapshot each decision tick (throttled --
  the brain doesn't need 60Hz), send it out, apply parsed orders to `g_enc`
  units (translate `flock_to`/`focus`/... into zone standoff + rally-point
  nudges) and to `g_ally` (mode).
- Transport: reuse `net_transport` or a simple file/pipe for a local LLM.
- The commands map naturally onto existing systems: `focus`/`hold`/`retreat`
  are zone-standoff changes; `flock_to`/`flock_spread` are rally points layered
  on the existing boid separation; `ally_follow` sets `g_ally.mode`.
