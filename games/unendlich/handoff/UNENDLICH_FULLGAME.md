# unendlich-fullgame — main menu → singleplayer / host / join → gameplay

The target binary. It is `unendlich-game` (r63-07) plus multiplayer host/join. All the
pieces now exist in the tree; this is the integration map (compile on your end — render+
window + the `run_world` extraction).

## Composes (all merged, in `src/sim/`)
- **Menu spine:** `game_flow` (MainMenu↔Loading↔InGame↔Paused), `title_flow`, `menu_screen_layout`, `menu_nav`.
- **Worlds:** `world_catalog` (saved worlds), `new_world_config` (→ `WorldSpec`), `settings_model`, `pause_overlay`.
- **Multiplayer (world-join port):** `net_candidates` (LAN-first candidate ordering + transport pick), `net_rendezvous` (the join FSM/codec). Design: `handoff/MULTIPLAYER_WORLD_JOIN.md` (DHT off in SP; on only when "Open as server"; identity = unendlich username).
- **Gameplay loop:** `app::world_session::run_world(&WorldSpec,&Settings)->ExitReason` (r63-07 seam).

## Top-level flow (extend `game_flow`)
```
MainMenu
 ├─ Singleplayer → world_catalog (Load) | new_world_config (New) → Loading → run_world(local)
 ├─ Host Server  → pick world (catalog/new) → run_world(host=true): on enter, announce world to
 │                 the DHT (net_rendezvous publish), admit joiners (RequestMailbox→admit), serve snapshots
 └─ Join Server  → server browser (poll DHT advert list) → pick → resolve host candidates
                   (net_candidates::order_targets) → connect (transport_order) → run_world(client)
```
`game_flow::AppCmd` gains `HostWorld(WorldSpec)` and `JoinWorld(WorldAdvert)`; `AppEvent` gains
`EnterHosting`/`EnterJoining`. The menu uses `menu_screen_layout` for the new "Multiplayer" screen
(Host / Join / Back) and a server-browser layout (a list of `WorldAdvert`s).

## run_world signature for MP
Extend the r63-07 seam to carry a session role:
```rust
pub enum SessionRole { Local, Host { advert: WorldAdvert }, Client { target: ServerEndpoint } }
pub fn run_world(spec: &WorldSpec, settings: &Settings, role: SessionRole) -> ExitReason;
```
- `Local`: today's behavior (DHT untouched).
- `Host`: spin the DHT announce loop + the join-admit mailbox + a snapshot broadcaster (the QoS
  channels from MULTIPLAYER_WORLD_JOIN.md §6); otherwise the same loop.
- `Client`: connect via the chosen transport, receive entity snapshots, send input; render remotely-
  authoritative world. (Client world-streaming is the largest new subsystem — stage it behind the
  pure `net_*` modules + a thin transport glue.)

## A small pure anchor — sim::session_mode (BUILT r67, gated green)
`sim::session_mode`: `enum SessionRole { Local, Host, Client }`, the Host/Join sub-FSM (browsing →
selected → connecting → connected | failed→relay), and a `ServerBrowser` model over `[WorldAdvert]`
(sort by ping/recency, dedupe by username). Keeps the menu/MP decision logic pure + tested while the
socket/DHT glue stays bin-side. (Could be cut as a dispatch thread alongside the bending set.)

## Build order
1. Land r63-07 (`unendlich-game`, `run_world`) on your toolchain — singleplayer menu→gameplay first.
2. Add `SessionRole` to `run_world`; wire Host (announce + admit) using `net_rendezvous`/`net_candidates`.
3. Add the server browser screen + Join path (resolve candidates → transport → connect).
4. Rename/alias the bin to `unendlich-fullgame` once host+join work; keep `unendlich-world` as the
   thin local launcher.
Verify each step with `--features render,window`; the engine lib + HARNESS stay green throughout
(all the new decision logic is in pure `sim::` modules; only sockets/DHT/clock are bin-side).

---

## Wiring (r69) — music + session_mode glue, concrete menu loop

Everything below is **pure + merged + gated**; the bin just calls it. Music + the MP decision
layer are now built (`sim::music_director`, `sim::session_mode`), so the menu loop is fully
specified — only the renderer/input/socket calls (marked `// BIN`) are device-side.

### Audio (title music)
The track ships at `assets/music/title_koyopper.opus` (Opus stereo 48 kHz). The bin needs ONE audio
backend dependency — `rodio` (with its Opus/symphonia feature) or `kira`. Then per frame:
```rust
let cue   = music_director::cue_for(flow.state());      // pure
music     = music.step(&cue, dt_ms);                    // pure fader (music: MusicState)
match music.asset() {                                   // BIN: reflect onto the backend
    Some(path) if backend.current() != Some(path) => backend.play_looping(path),
    None if music.volume01 == 0.0                 => backend.stop(),
    _ => {}
}
backend.set_volume(music.volume01);                     // BIN
```
That alone gives: full theme on the menu, ducked while loading/paused, faded out into gameplay.

### Per-frame main-menu loop (composing the merged modules)
```rust
let mut flow    = GameFlow::new(has_save, slot_count);  // sim::game_flow
let mut music   = music_director::MusicState::new();
let mut browser = session_mode::ServerBrowser::new();
let mut join    = session_mode::JoinFlow::new();
let mut host    = session_mode::HostFlow::new();

loop {
    let dt_ms = /* BIN: frame dt */;
    let input = /* BIN: gather keyboard/mouse/gamepad */;

    // 1) music follows app state (see above)
    let cue = music_director::cue_for(flow.state());
    music = music.step(&cue, dt_ms);
    /* BIN: apply music to backend */

    match flow.state() {
        AppState::MainMenu => {
            // 2) lay out + navigate the active screen
            let screen = menu_screen_layout::layout(/* current screen, viewport */); // sim
            let nav    = menu_nav::resolve(&screen, &input);                          // sim
            /* BIN: draw `screen`; read which item nav highlighted/activated */

            match nav.activated {
                Some(MenuItem::Singleplayer) => { /* go to world picker (world_catalog/new_world_config) */ }
                Some(MenuItem::HostWorld)    => { host.open(); /* BIN: start DHT announce w/ HostFlow::make_advert(username, name, max, candidates, now) */ }
                Some(MenuItem::JoinServer)   => { /* enter server-browser screen */ }
                Some(MenuItem::Quit)         => flow.apply(AppCmd::Quit),
                _ => {}
            }
        }
        // server browser screen (Join):
        // BIN polls the DHT → browser.observe(advert) for each; browser.prune(now, ttl);
        // draw browser.by_recent(); on pick: browser.select(key);
        // on connect: let plan = join.connect(browser.selected().unwrap(), self_pub, is_cgnat, has_relay);
        //   BIN: drive net_rendezvous::HolePunch over plan.targets in plan.transports order;
        //   on success join.on_connected() → run_world(spec, settings, SessionRole::Client{..});
        //   on failure join.on_failed() → fall to relay/mailbox (plan.dead_end) or back_to_browse().
        AppState::Loading => { /* BIN: stream world; when ready → flow.apply(AppCmd::WorldReady) */ }
        AppState::InGame  => {
            let role = /* Local | Host{advert} | Client{endpoint} per the path taken */;
            match run_world(&spec, &settings, role) {   // app::world_session (extended seam)
                ExitReason::ToMenu => flow.apply(AppCmd::LeaveWorld),
                ExitReason::Quit   => flow.apply(AppCmd::Quit),
            }
        }
        AppState::Paused  => { /* sim::pause_overlay drives Resume/Settings/Quit-to-menu */ }
        AppState::Exiting => break,
    }
    /* BIN: present frame */
}
```

### The one engine-side change: `run_world` gains a role
```rust
pub fn run_world(spec: &WorldSpec, settings: &Settings, role: session_mode::SessionRole) -> ExitReason;
```
- `Local`: today's loop, DHT untouched.
- `Host`: also run the announce loop (re-publish `make_advert` on a timer) + admit joiners (net_rendezvous
  mailbox) + broadcast entity snapshots.
- `Client`: connect via the punched transport, receive snapshots, send input; render remote-authoritative.
Client world-streaming is the only large new subsystem; everything that *decides* (role, target order,
browser, music) is pure and done.
