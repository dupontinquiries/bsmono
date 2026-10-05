# C21_TODO — open threads from the c21 session

Same convention as `C9`–`C20UNIFIED`: every item says how far it actually got. Session base
`7b2633a5e` (the `c9-001-fp5-eyes-thin-bar-gaze` bundle's HEAD, branch
`feat/gpu-deltanet-carry-integration-test`), all commits by the repo's last author with the
`Claude-Session` trailer. Shipped as a zip of `git format-patch` files (the chat upload cap is
20 MB; a full bundle is 422 MiB) and an incremental bundle on `7b2633a5e`. Per-commit detail is in
the messages; this file is the index, the "verified how", and what is open.

The operator's order was **logscope, then the apps, then the games**, with "performance, bughunt,
feature rollout and TODOs all good". Everything here ran on **lavapipe** (llvmpipe) in a 1-core,
3 GB, ~20 GB-disk sandbox. **No number in this session is a hardware speed.** The commits that
change what runs on the GPU say so and name the ledger row that decides them.

---

## 1. logscope (Ornith-1.5 / Qwen3.5 hybrid on the wgpu engine)

- `986941adc` **fused decode kernels on the V2 word-per-thread pattern** — RMSNORM_MATVEC_Q8,
  MATVEC_Q8_0_ADD and SWIGLU_MATVEC_Q8_0_ADD get V2Wg128's loop (every layer projection, not just
  the LM head). Default ON, `LOGSCOPE_FUSED_V2=0` restores the originals. `--gate-fused-v2`: 75
  kernel cases vs V1 and a CPU reference, plus a whole-model half that REFUSES to pass unless all
  three kernels were dispatched under both levers. **Open: the first M2 ledger row** —
  `./loop.sh bench fusedv2` against `LOGSCOPE_FUSED_V2=0` decides whether the default stays.
- `294bda73e` the fused norm+matvec "bit-identical" claim corrected (1.9e-6; it never was).
- `1de10afa3` **`--bench` counts instead of quoting**: dispatches/submits per token, prefill tok/s
  through `prefill_chunked`, bytes the engine reads, and per-kernel GPU timestamps
  (`LOGSCOPE_GPU_TIMESTAMPS=1`). **Open:** `LOGSCOPE_GPU_TIMESTAMPS=1 kernel-validate --bench
  Ornith-1.5-9B-Q4_K_M.gguf 64 512` on the M2 — the per-kernel table says where the token goes.
- `4f6dbddbd` `f9e1267ea` `fc0436784` `5355d2284` `c96a1f8f5` **tool calling and templates**:
  the taint gate scans every tool-call shape the parser reads; the grammar and the gate agree on
  where the run token goes (the fence); the agent prompt teaches the XML-parameter grammar
  Ornith/Qwen3.5/Qwen3-Coder are trained on (`LOGSCOPE_TOOL_SYNTAX=xml|json`); a reply that only
  closes a think block is reasoning up to the close, `enable_thinking` reaches the Jinja template
  (`LOGSCOPE_THINKING=0`); the agent transcript reaches the engine rendered in the model's chat
  template (`agent_render.rs`, `LOGSCOPE_AGENT_TEMPLATE=0`). **Open:** a real agent run on the M2
  with Ornith — every piece is unit-tested, none has met the model.
- `6585fb8eb` **IQ2_XS** (ggml type 17) reads, checked against gguf-py 0.19.0. `545a0fefb`
  **IQ4_XS held natively** (4.25 bpw resident, decoded in the matvec, widened to int8 on the GPU for
  prefill; fused q‖k‖v, gate‖up and the DeltaNet in-projection included; `--gate-iq4xs`;
  `LOGSCOPE_IQ4XS_NATIVE=0`). **Open:** the gated q‖gate row-gather and the MoE expert stacks still
  convert to int8 on load; `--bench <Ornith-IQ4_XS.gguf> 64 512` with and without the lever.
- `d8a35e236` `--gate-deltanet-carry` recovered (the tag `delta-carry-proven` was never an
  ancestor of HEAD); passes, max |Δ| 4.77e-7.
- `1bed58e0c` **dupscan ratchet 393 → 422, audited** — red on arrival with the c20 landing (the
  hive crate's vocabulary); all 29 are two ideas under one name. The pair to watch: fan `enlist::
  Capability` (what a machine offers) vs hive `matrix::Capability` (what a box measured).
- **The gates that read FAIL on lavapipe, classified** (`run-all-gates.sh` now carries the list):
  `714c35ab1` `--gate-q4resident` was a stale gate (it handed the loader a budget
  `gpu_buffer_budget_uma` halved below q4); `8225f2870` `--gate-qkv`'s end-to-end "bit-exact" was
  never true on a per-shape tile policy, now rel ≤ 1e-5, and `--gate-matvec` reports the subgroup
  rows as informational where the engine's self-check already distrusts the kernel;
  **`54b2bb8d1` a real kernel bug** — the subgroup matvec's partials array overflowed at tile 8 on
  any subgroup narrower than 16 (lavapipe's 8; Intel at SIMD8; a phone's Mali is 16, Adreno 64),
  and the self-check only ever tried tile 4; `0162addd7` `--gate-moe-raw`'s "1e20" was its own
  fixture (random f16 scales to 1e4, three near-identical expert tensors), the file-view path is
  right. Still red: `--gate-q4k` (two tie-straddles) and `gate-bidir-attn` column 1.
- **Not done, deliberately:** chunked-parallel DeltaNet prefill. `DELTANET_STEP_T` runs one
  workgroup per head, sequential over tokens; whether it is the prefill bottleneck is a question
  for the timestamp table above, not for a rewrite. BATCH 0 of `apps/logscope/TODO.md` is done in
  the code (repeat_penalty, the EOG set, the backend init) and its marks are stale.

## 2. apps

- `515f3e2ab` **jotter CLI** — the manifest declared `[[bin]] jotter` since 5780b2ff4 and the file
  was never committed, so every all-targets build of the crate failed. `id`/`devices`/`notes`/
  `pair [--serve]`/`scan [--pair|--save]`/`show [--png]`/`serve`/`sync`. End to end over loopback:
  pair, sync, a stranger refused; a 7-frame note as two mosaic PNGs scanned back byte-identical
  (rqrr reads version-40 symbols at 3 px a module). **Open:** a real camera photo; the LAN-address
  guess (`local_ip()` found no route here).
- Fleet sweep of `apps/` (every root manifest and every `crates/*`; `fleet_build.sh` died at
  studio when the session paused, the rest ran by hand) — the failures it found, all fixed:
  `9836670b5` settingsd's two tests raced on XDG_CONFIG_HOME; `c662dd0b9` fao-codec's damaged-file
  harness found a u64 overflow in `frame_data_end` (3 of 46,330 cases); `1fb7272aa` gitmind-core's
  delta-chunk length became a 4 GiB preallocation when corrupt; `e0160f8cf` studio-app's TCP voice
  test depended on the scheduler; `917e31044` uikit-egui's callout now falls back to a readable
  ink (slate's text is 3.3:1 on its OWN card — a palette-level finding, recorded, not fixed);
  `84950db27` `d3314b2a3` `b6f09d7ec` stale/missing locks (voiceterm's proton-srp tree, uikit-term,
  gaze-trainer); `14fc46790` gaze-trainer's phantom bin; `1db192831` drivemind; `90f12ee62`
  rusty-board's nine `llm-swipe-ffi` arms; `b90a5d5e2` board-compositor's `atspi` feature declares
  the glib it uses (found by checking the non-default configuration — the default one needs
  gtk4-layer-shell, which apt does not carry).
- **Environment, not code** (install before you gate — CLAUDE.md §3 updated): gitmind's cockpit
  tests need `openssh-client openssh-server` and `mkdir -p /run/sshd`; vidkit-player needs
  `libavutil-dev libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev libswscale-dev
  libswresample-dev clang libclang-dev` (then 239 tests pass — its first run in a sandbox);
  card-diagnoser needs `libpcsclite-dev`; rusty-board's default features need gtk4-layer-shell
  (absent); voiceterm's root crate is the Qt frontend (skip).
- **`float_literal_f32_fallback`** (rust-lang/rust#154024, `future_incompatible` → a hard error
  one day, and every egui frontend here would stop compiling together): `54359106d` the tool
  `maintenance/tools/f32_fallback_fix.py` applies rustc's own suggestions and nothing else;
  `295511bdf` `8f4918f7b` `bd79b3016` `710837433` 161 sites suffixed across shared/, apps/ and two
  games. **Open:** `games/echelon/client` (one site, behind a bevy build the sweep's 20-minute
  budget could not finish), `system/`, `tools/`, and any crate whose sites hide under a non-default
  feature.
- **C20UNIFIED §9 (point the damaged-file method at other readers):** done for gitmind
  (serde/text readers, the one find was the store's delta length) and depot (tar+gzip+serde, the
  truncation cases already handled); eink-kit's parsers are defensive text parsing. Not every
  reader rewards the harness; these did not.

## 3. games

- `8cbecdcb1` `f0315e73e` `6f41f203c` `4953d3b6f` **blackstar-rs**: `rig.rs` reads
  `gltf::scene::Transform::Matrix` again (the vocabulary sweep had rewritten a library's enum to
  `the Charge`; the census floor for `Matrix` is 1 with the reason); the joints tests stop sharing
  one fixture file; **a hurt left leg reaches the gait machine** (C20_TODO's "never limps": the
  signed `limp()` was clamped to 0..1 before the sign was read); the station tests store into a
  per-thread sandbox — `cargo test` had been writing a real `blackstar.save` into the crate root,
  the player's own save if the game is run from there. 1792 tests pass.
- `c40d74388` **voxelkit**: the plan's voxel size never reached the rasterisation (every
  half-extent halved at voxel 0.5; the crate held two tests that disagreed about which semantics
  was meant, one of them never green). No consumer in the tree yet.
- `60ae7a3c4` creature-rig: the strike-overlay test read the timer backwards (never green).
- `7b857c9a1` clawbound-battleground's `mapgen_eval` example matches the six new scenery `Item`s.
- `36026f555` four games/shared locks committed.
- `6bc5378c5` **unendlich**: the subgroup cull shader leaves `src/shaders` again (08ffc680f had
  moved it to `shaders_proposed/`; the blackstar-rs rigkit merge 87795e9cd brought it back and
  both `shader_artifacts` tests failed since). `57a73b2ab` **the plant stamp budget is charged
  where a stamp is drawn** — it was charged per block VISITED, before the air-above and
  probability checks, so buried grass and the 55% the probability skips burned it and the
  surface went without tufts (c20's unconfirmed hypothesis in `docs/KNOWN-TEST-FAILURES.md`,
  confirmed). Engine lib: 9501 passed, 2 failed, 19 ignored — the two documented ones (the sag
  test, a soft-block question; the uncalibrated 90K triangle tripwire, worst chunk 155,304 at
  (-1,2,-1) seed 1, terrain-dominated per the c20 breakdown). **Open:** those two, and the
  tripwire wants calibrating against the weathered-top subdivision it keeps finding.
- Games sweep results (same commands as fleet_build.sh): see §5.

### 3b. The "you wouldn't release a game without ___" pass (c21, second half)

Three read-only surveys (multiplayer, campaign, menus, audio, performance, wired-but-unused
modules) across every game, then the cheapest verified fixes first. Every commit carries its
test and a §5b break test; nothing below was seen on a screen or heard.

- **bsgames hub** `a63e0c94b` `f77bd9091` `9fd396081` `4927e74e9` `d5c3a950b`: the hub built
  its presence service over an in-process `SnapcoreStore` while every game publishes to
  `FileStore::dev_default_path()` — no friend could ever appear (two-process test). Its launch
  argv was refused by cargo for soldier (four bins), unendlich (a virtual workspace) and EVERY
  quick match (`--quick-match` before `--`); clawbound launched the headless render. `CargoRun
  { bin, features, args }` per entry, a `cargo metadata --no-deps` test against the real
  manifests, skydribble and blackstar-rs registered, launches from the monorepo root wherever
  the hub was started. shellkit's `Net` takes the game id: blackstar-rs announced as skydribble.
- **clawbound** `c9b1b7834`: the Battleground rows launch `games/clawbound-battleground`
  (they printed to stderr); `mp_status` is drawn (it never was).
- **clawbound-battleground** `0fd1d1da1` the Esc panel pauses a local match (the sim ran under
  it). Then its 16 failing lib tests, never seen because the `mapgen_eval` example broke the
  sweep's build first: `44125e839` a Time trophy at zero stacks underflowed — in RELEASE it
  wrapped to four billion stacks, Beacon of Ages granted +1000 INT at purchase (the manifest
  said "there is a bug where it applies right away all 1k"); `558752b3e` `is_sentinel` was
  "has a field", so Twofang's new Face Death dressed him in a permanent slow aura; `daa61fba1`
  the flick glide never coasted (pad units vs the glide's px/s floor; the plumbing commit said
  its tests were never run); `470e45b3d` `OUTER_SCALE` cancelled MAP_SCALE out (e4c7a60aa's
  message says `/ 135`, the code said `/ (95*MAP_SCALE)`) — at the user's 3.75 the rim began
  inside the floor, the rivers were dry, the ground ended short of the intro camera;
  `964d6b240` `lane_clearance` now counts the fountain approach minions walk; `8b55c240c`
  three tests pinned 1.42 coordinates. **Open:** `the_lanes_are_open_going_and_the_jungle_is_not`
  still fails at 3.75 and should — build_world's grove/scatter counts are fixed numbers for the
  135 u arena, so 3% of the 5.4x floor is rough. Scaling ~20 counts by area is a perf decision
  (~5x instances) that belongs to the MAP_SCALE experiment; the assertion says so.
- **blackstar-rs** `37077878f` a dead body was never repaired (one death and no later sortie
  could be lost); `0213e3764` HUD hull bar + rebuild countdown; `770288240` Esc twice to abandon
  a sortie; `4a5e2879d` the flown sortie/raid now banks the warden's arm, the nightfall's chi
  and kit — only the CLI sims ever rewarded anyone; `f90bfce0c` combat sounds (seven synthesized
  cues; unheard here). 1798 tests.
- **unendlich** `54552a7b3` the dedicated server persists (`--world-dir`, the fullgame's
  `worlds/<name>-<seed hex>` rule lifted to `server_config::world_dir_for`), loads the world's
  whitelist, saves on Ctrl-C/SIGTERM (`libc::signal` into a static atomic) and autosaves;
  `3cb8980a3` admission is the only way in -- data from a non-admitted address is dropped by
  the pump and the fullgame's inline loop (anyone could edit the world before), and a joiner
  re-Joins when its host goes quiet; `c77c34d27` per-world `player.json` (position, view,
  inventory slot for slot incl. forged weapons, hotbar, clocks) written at Save & Quit, the
  autosave and the world switch, loaded over the seed's spawn; `4ef8b0a0c` music under play
  (the title's pad stem alone at a quarter: `cue_for(InGame)` was None); `9053d76e6` a death is
  a moment (the veil with YOU DIED, 2.5 s, then the respawn -- at THIS world's spawn: `spawn_pos`
  was the startup world's). The fullgame bin compiles here with `--features ui-overlay` (its
  first compile in this sandbox; the sweep's `--all-targets` skips it).
- **soldier** `72220a86f` footsteps (the DSP existed, nothing emitted one); `6a70a77da` the
  `--features app` test build had not compiled since d854f8ebc.
- **road-warrior** `e7410f3ca` a title screen with the car select (unlocks were undriveable), a
  pause, a campaign-complete screen (was a toast).
- **netcode/clawbound** `052b03bf9` a standalone player is the login name, not every player
  "cat" (one shared profile on the fleet store).
- **clawbound-battleground LAN** `4b0bbc507` `2f5cc5091` `6ab5659d7`: the local player is
  `Game::viewer` (22 `gods[0]` sites); `lanmatch` puts the tested lockstep + netplay on the
  den's UDP socket (loopback test: two sockets, two copies, 600 frames, bit-identical); the host
  STARTs the match from the den (plan over the wire, members build the same match). Not seen
  across two windows. Limits in the commit: a leap's wire direction is the move direction,
  ancestors are drawn by the seed, Esc does not pause a LAN match.
- **The overnight batch (c21, third stretch):**
  - **unendlich** `713e860fc` an effects-volume slider, persisted (the Audio tab said the
    setting would be audible "after the audio backend lands"; it had landed).
  - **skydribble** `8652878d2` 1539 lines of dead second copies of nine sims (a merge kept
    both sides; `--sim platform`'s second block was a different check that could never run);
    `62d53b751` the 37 fast sims run under `cargo test` (`tests/sims.rs`: the PASS line, no
    FAIL line, AND the exit status -- `sim::verdict` remembers a FAIL and `main` exits 1;
    before, a failed sim exited 0); `c391100b6` the first run found `--sim planets` red: the
    planet it watches spawned 2.5 m inside a KUIPER rock and jittered there for the whole run
    (c15's note in blackstar-rs's TODO.md had stopped between "intended" and "tuning";
    `541b0e13a` resolves it). `World::unbury_drifters` lifts a buried drifter clear.
  - **blackstar-rs** `a4cd28e00` the mission slides are shown -- `content::Mission.intro` on
    the brief, `outro` on a won debrief (Enter turns, Esc skips); only `campsim` ever printed
    them. `245853b28` the wheel cycles the hotbar (`Hotbar::cycle` had no caller), consoles
    and the holster click.
  - **clawbound-battleground** `f2144b762` `teamplay` wired: `Game::team_call` decided per
    tick from `ai::team_view`, bound bots take the call's goal or lane (home lane restored
    after). The first headless ten-bot runs showed the standoff a team layer makes stable --
    five a side at one inner tower for forty minutes -- so: a tied or DEFENDED lane is no lane
    to push, Defend binds the lane's cats and the near ones (the team only against 3+), and
    the grouped siege waives the tower discipline when the quorum is at the tower with two
    more of us than of them. Measured at the authored scale: the match ends at 9:18 with the
    layer, 17:01 without. **At MAP_SCALE 3.75 no bot match ends either way** (40 min, both):
    the AI's radii (siege 16 u, tower range 24 u, "near" 12-16 u) are absolute, the inner
    towers sit off the lane paths by more than a bot looks, and the lane path ends AT a
    shielded titan that kills whoever arrives. The match test asserts progress at 3.75 and
    the end at 1.42. Also seen and left: a bot healed at the fountain beelines to its old far
    waypoint through the middle; bots stand at a shielded titan.
  - **road-warrior** `502b42211` `cues` (the car's moments named: launch, landing clean or
    rough, hit, ram, nitro, dry tank; the engine note); `39eada491` sound -- an engine drone, a
    voice per cue, two stings, `M` mutes; cpal behind a default-on `sound` feature.
  - **clawbound** `3e991f0fb` settings and keybinds persist (`~/.config/clawbound/settings.cfg`;
    every launch began at the defaults); `b410bd5ad` `cues` for the hunt and the prowl;
    `c0d6868b8` sound -- wind/rain/thunder/snow beds, a tension drone with a heartbeat, a voice
    per cue, the Settings master volume finally read. (`soundscape.rs` is a mix for the in-crate
    battleground the window never plays.)
  - **netcode/shellkit/soldier** `5b1dacddf` `af466d3fc` join by the host's address: the fleet
    store resolves a name only between machines sharing its temp-dir path, so across two
    machines a name resolved nowhere. HOST lines show `ip:port`, FRIEND takes one (the field
    cap of 16 cut every address short), skydribble/blackstar-rs/soldier.
  - **unendlich** `fa78357b8` `9b21c5216` WORLD HISTORY restores: a point was `save_all` plus
    a marker (the live files overwritten in place; nothing read the RESTORE marker) -- now a
    copy of the region tree in the world root, staged restore applied on open by the game and
    the server, the replaced state kept as a point. `876619520` `a5035513e` world edits over
    a reliable-ordered link (seq/ack/resend per peer; they went out in one raw datagram, and
    a dropped one was a silent divergence) in the inline loop and `HostPump`.
  - **clawbound-battleground** `9dc21783f` the LAN plan carries the host's rules and every
    player's ancestor pick (`picks::deal_seated`; the den's HELLO says the pick; the LAN
    screen has the arrows and the roster); `dc444e56a` `07bfdcdce` the match has music: a
    synthesized bed per map (root/mode/tempo/colour each) when no `music/<map>` file is in the
    bank, rendered off the frame thread; `da5144d76` the announcer's calls are read on screen
    (no recording exists, so they reached nobody).
  - **soldier** `7653ba5fd` `app` implies `sound`: the README's run line and the hub built a
    silent game.
  - **echelon** `1801b8d9f` `812a2bbf6` the UDP transport the README named as remaining
    (`echelon_net::udp::UdpLink`, re-send until acked, proven through a relay dropping every
    third datagram) and the runner's `--lan-host`/`--lan-join`: the first Echelon match across
    two processes (4620 turns, 41 checkpoints, no desync, identical hashes). The Bevy client
    still plays human vs bot; wiring it to the session is the next step there.
- **The overnight batch, fourth stretch:**
  - **soldier** `9d8a76aad` the settings page is saved -- nothing on it but the renderer
    toggle survived a relaunch (sensitivity, FOV, preset, both detail sliders, the team
    colours, all 22 bindings). `db::Options` is the record (`opt.*`/`bind.*` lines in the same
    save.txt; its defaults ARE the game's -- `Settings::default()` derives from it), tolerant
    of an older save, NaN, off-scale numbers and unknown keys; written once a slider drag
    ends (`Context::is_using_pointer`), never on an untouched launch. The colour pickers lost
    their alpha slider: `Color32` is premultiplied and the tint never read alpha.
  - **road-warrior** `176a045df` the mute key is remembered (`SaveState.muted`,
    `serde(default)` for older saves; the master gain is set before the first mixed frame).
  - **clawbound-battleground** `60c6e26c9` a bot whose home lane has no enemy tower left goes
    to the lane with the fewest (`lane_to_push`; ties to the nearest, home again once every
    tower is down) instead of standing at the shielded titan; a healed bot walks its lane
    back out from the nearest waypoint instead of beelining to the far one. At 1.42 the ten-bot
    match now ends at 12:11 with every tower down (9:18 before, towers left); at 3.75 still no
    end (6/12 towers at 20 min) -- the scale question stands.
  - **unendlich** `ffe2245a7` `games/unendlich/README.md` was the README of a docs zip; a
    README for the directory (build/test commands, the module map, where the claims live),
    the zip companion moved under `docs/claude-notes/`.
  - **echelon** `fb1578063` the Command Deck plays over the LAN: `Deck` holds a `Driver`
    (`Local` bot or `Lan` = `Lockstep` + `UdpLink`), `--lan-host`/`--lan-join` on `tape` and
    `play`; the world steps only on turns with both players' commands, one submission per
    executed turn, a silent peer freezes it (owed ticks capped). The crate's FIRST tests (the
    README had called tape mode "CI-proof" over zero tests): a tape, two decks over loopback
    (one world, both orders land, hashes identical, no running ahead), the silent peer.
  - **clawbound** `0abbdd5ca` the suite's one-in-six desync in `wire::two_peers...` was the
    terrain race: the battleground build dropped its guard mid-build, and world.rs's own
    tests set `mountain_bias` 1.7 and re-baked with no lock -- every bake and knob flip is
    under `TERRAIN_LOCK` now, the lockstep/wire/Prowl-soak tests hold it across stepping (18
    green runs after; the global grid is still the design). `8a2eb0a47` a person at the
    hunter's controls (`Game::set_hunter_input`: the stick for movement, the pounce for the
    catch; the FSM keeps the senses). `b6d591c72` the hunt over the link: `hunt_net`, host-
    authoritative state (316 B, 30 Hz) with the joiner playing the hunter; the Header carries
    the TERRAIN POOL (`world::TerrainPool`, `Game::new_match_with_terrain`) because the bake
    reads the global knobs as it finds them; the Multiplayer entry and the bsgames handoff both
    land on it; a stale stick (2 s) hands the hunter back to the AI. Loopback-tested; never two
    machines. `7740910ac` the joiner's wildlife moves (`Game::step_wildlife`). Left: Single
    Player while joined is confused.
  - **Fullscreen, four games:** clawbound `a8e52bcc9` (Settings row + F11, saved),
    clawbound-battleground `c83c8b55c` (panel checkbox + F11, controls.cfg), road-warrior
    `f8161f463` (F11, in the save), soldier `ba54cdf8a` (Settings checkbox + F11, save.txt);
    unendlich `4bfcd99aa` `ToggleFullscreen` (F11) had been bound and on the keybind page with
    NO handler since the table was written -- handled now (glfw monitor at its video mode,
    back to the window where it was). None seen on a screen.
  - **unendlich** `c5ef751f1` the headless test build (`--no-default-features --lib`, the
    documented way) had not COMPILED since c20's canopy-depth commit (an ungated
    `crate::render` import in `world/foliage_depth.rs`); 9130 pass headless now, 0 failed.
    `cd65791fa` `DropItem` (Q) was the other bound-and-listed action with no handler: one of the
    held stack is tossed ahead (`DropField::toss`, a 45-tick grace before pickup so it does not
    come straight back). `b7b65445c` "Screenshot" (F2) off the keybind page -- no readback
    exists -- and a scan test that fails for any page row the fullgame does not handle.
    `e5e15e788` KNOWN-TEST-FAILURES / README counts.
  - **echelon-deck** `a336287af` the match is heard: `cues::Listener` reads built / trained /
    lost / hits (rate-limited) / the banner off the world between frames, Ack on a command;
    `audio` is the house synth behind `sound` (off by default; the tape and CI stay silent).
    Unheard.
- **Decided, not wired:** blackstar-rs `dispatch.rs` models voyages (`voyage`/`damage`/
  `survival`/`loot`, orbits and delta-v) that nothing in the shipboard game reaches -- wiring
  it is a game-layer design, not a bug; `suitability.rs` would tell the player "wheels have
  6% grip on Io" while the car physics scales nothing with gravity -- an advisor the physics
  does not back is a lie, so it waits on TODO.md's "Wire traction and mass_bias into real
  handling". The other five battleground "done" mechanics (markers/daylight/freshkill/
  shadowrealm/camps) are the Warriors design (`docs/WARRIORS_MECHANICS.md`), a different
  fantasy from the 5v5 the sim plays; wiring them is a rules change, not a fix.
- **Still open from the surveys** (ranked): unendlich `settings_model` unwired (the renderer's
  `ui_*` fields ARE the settings; the model's draft/apply/revert is a Cancel button nobody has
  asked for) / `gpu_mesher_mode` hard-coded (documented decision) / the inline host loop vs
  `HostPump` duplication (both now carry admission and the reliable link; the dedupe is a
  refactor of a 20k-line bin); clawbound-battleground: the AI radii vs MAP_SCALE (above), the
  narrator/announcer VOICE (the calls are text now; speech cannot be synthesized here);
  echelon's Bevy client on the session (the deck is; the Bevy `client/` is a detached
  workspace and still human vs bot); soldier's "play with a friend" (host/join over the
  fleet DHT, then each end plays its own bots match -- the sim has one player; a co-op
  second player needs a second actor the enemies target, enemy genomes on the wire for the
  joiner to draw them, and client-authoritative positions: a day, not a fix); unendlich's
  F2 screenshot (a swapchain readback the sandbox cannot run).
  Everything synthesized this batch (road-warrior, clawbound, the battleground's beds) is
  unheard: the voices are first passes by their numbers.
- **Not touched:** blackstar-rs's visual/playtest backlog (`TODO.md` §"Found, NOT fixed" needs
  eyes on a screen), unendlich's BACKLOG §B pure cores (octree re-merge, rebuild coalescer,
  `MoistureField::recenter`) — sandbox-verifiable but each is a feature, not a fix, and the
  engine's test crate is a 10-minute, ~3 GB build here (its sweep step hit the 20-minute budget
  with the check included; `cargo test -p unendlich-engine --lib` alone, one job, is the way).

## 4. Repo tooling

- **A split script that opens a file for writing before it finishes computing the text
  truncates the file when it fails.** `open(p,'w').write(f(x))` with an assertion inside `f`
  left `blackstar-rs/src/main.rs` EMPTY, and `cargo test` then reported "0 tests, ok" against
  a 1 MB binary — twice, read as a stale-build problem. Compute every output first, assert on
  its length, then write; and a `cargo test` that finishes in 0.1 s with 0 tests is a missing
  source, not a cache.
- **A background job launched in a Bash call that then sleeps past the tool timeout dies with
  the shell** (the process group is killed): a cargo link interrupted that way leaves a fresh-
  looking, truncated binary. Launch, return at once, poll in separate short calls.

- `maintenance/tools/f32_fallback_fix.py` (above). Resolves relative diagnostic paths against the
  WORKSPACE root — the first run on a member crate opened `crates/uikit-egui/crates/uikit-egui/…`.
- The `maintenance/tools/session_capture.sh` hook is installed in this clone; six early commits
  were made from a shell that had not sourced it and carry no notes (`90f12ee6` `c96a1f8f`
  `1db19283` `d8a35e23` `b6f09d7e` `14fc4679`).

## 5. The sweep, as it stands

`apps/`: every root manifest and the four crates no parent workspace covers (hatch-core, the three
rusty-board crates) built and tested; the per-crate `apps/*/crates/*` entries of the apps before
studio were not run separately (their parent workspaces were). Failures found are all fixed above;
`bo-suite` (Qt) and `voiceterm`'s Qt root skipped; `rusty-board` tested without `wayland`.
`games/`: every root manifest and every crate no workspace covers, check + test. Green: blackstar-
demo, blackstar/rust_fsm, bsgames, clawbound (362; `--features app` checks clean too), echelon
(root + 7 members), road-warrior (+core, +shell), skydribble, soldier, speedrace, games/shared ×
13 (after the voxelkit and creature-rig fixes). Red and fixed: blackstar-rs (4 commits, 1792
pass), clawbound-battleground (the example), voxelkit, creature-rig, unendlich (shader + budget;
2 documented failures remain). Not finished here: `echelon/client` (bevy; its test build passed
the sweep's 20-minute budget — check is clean) and `unendlich`'s test build under the sweep's
budget (run by hand afterwards: engine 9501/2/19, the other eight members green).
`shared/`, `system/`, `tools/`: not swept this session.

## 6. Hardware follow-ups for the M2, in the order they decide things

1. `./loop.sh bench fusedv2` vs `LOGSCOPE_FUSED_V2=0` — keeps or drops the fused-V2 default.
2. `LOGSCOPE_GPU_TIMESTAMPS=1 kernel-validate --bench Ornith-1.5-9B-Q4_K_M.gguf 64 512` — the
   per-kernel table; decides whether DeltaNet prefill is worth a chunked kernel.
3. `kernel-validate --gate-iq4xs`, then `--bench <Ornith IQ4_XS> 64 512` with and without
   `LOGSCOPE_IQ4XS_NATIVE=0`.
4. `LOGSCOPE_MATVEC_SG=1` is worth a try now that the subgroup kernel's partials array is right —
   the Asahi guard predates both the packer fix and this one.
5. An agent run with Ornith through the chat tab: tool calls in XML, a `<tool_response>` turn, a
   think block — the pieces are tested apart and never together.
