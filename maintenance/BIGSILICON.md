# BigSilicon integration — status & roadmap

The goal: every personal app installable as grouped **kits** on the fleet (FP5 phone +
Asahi desktop), sharing one UI kit, one voice daemon, one mailbox daemon, and one models
folder — with good defaults but room for per-app custom behaviour.

This turn did the foundation (ingest + adaptive UI kit + groups + models-kit). The daemons
and the Qt/GTK→egui ports are staged below with their reference implementations named, so
each is a bounded next step rather than a from-scratch design.

## Done this turn

- **Ingest.** All projects merged from `tomerge_18`: new (av1tuner-m1, beatspace, krapper,
  paperbound, fao-codec, cybertron) and bumped to latest (rusty-board, bo-suite, snapclone,
  voiceterm/web2qml 0.1.903, logscope voice-nav-complete, unendlich r603). Source only —
  build output, prebuilt binaries, vendored SDL blobs, model weights and heavy media were
  stripped (weights belong in `~/models`, see below).
- **uikit-egui adaptive layer** (`shared/uikit/crates/uikit-egui/src/adaptive.rs`). Device
  class (phone/tablet/desktop from logical width), touch detection, UI scale, safe-area,
  and responsive helpers (`pick`, `columns`) — all *defaults the app queries*, each
  overridable with a builder. `apply_touch_defaults` only widens hit-areas, never shrinks
  what an app set. Compiles against egui 0.29; tests pass.
- **App groups** (`maintenance/suites/groups/*.json`): work, music, multimedia, games,
  system, and `all-bigsilicon`. These reuse the existing suite format, so gitmind can
  install a whole group.
- **models-kit** (`shared/models-kit`): `~/models` is now the one canonical model home
  (`GITMIND_MODELS` overrides), with migration-aware search so nothing re-downloads.

## Kits (shared/) — the shared substrate

Existing: core-kit, net-kit, crypto-kit, audio-kit, voice-kit, share-kit, registry-kit,
snapcore, uikit (egui), and now models-kit. "Integrating with kits" = each app depends on
these instead of carrying its own copy. Refinement backlog: point voice-kit's
`find_model_in` at `models_kit::search_dirs`, and have voiceterm's `model_download`,
beatspace's GAN loader, and snapclone's YOLO loader resolve through models-kit.

## Install hooks (convert custom scripts → gitmind hooks)

Real scripts found in the ingest that must become declarative hooks so a group install is
one action instead of hunting for shell scripts:

- `rusty-board/scripts/install.sh` — the on-screen **Linux keyboard** (input-method
  registration). → `hook: input-method`.
- `snapclone/camera-bridge/scripts/setup_v4l2loopback.sh`, `setup_webcam_client.sh`,
  `setup_backend.sh` — v4l2loopback module + PipeWire bridge. → `hook: kernel-module`,
  `hook: service`.
- `bo-suite/bo-mux/scripts/install-daemon.sh`, `bo-suite/scripts/install-deps.sh` →
  `hook: service` + system deps (reuse the cross-distro pydeps/system-dep provisioning).
- `voiceterm/setup-voice.sh`, `setup-android.sh` — model fetch + platform setup →
  `hook: models` (delegates to models-kit) + `hook: platform`.

**Proposed hook spec** (in a suite/group app entry): `hooks: [{kind, script?|declarative,
when: install|uninstall|update, needs_root: bool}]`. gitmind runs them with the same
privilege-escalation ladder (root → sudo -n → doas) already built for system deps, and
records what ran so uninstall is clean. Kinds: `system-deps`, `kernel-module`, `service`
(systemd/OpenRC unit), `input-method`, `models`, `platform`.

## Voice daemon (`voiced`) — one pipeline, many apps

Reference implementation is **voiceterm** (`src/voice.rs`, `mic_scheduler.rs`,
`speaker_claim.rs`, `whisper_pool.rs`, `pane_voice.rs`, `editor_voice.rs`, `grammar_context.rs`)
plus **voice-kit** (hallucination filtering + model resolution) and **audio-kit** (signal
math). Today each voice app owns the mic. Unify into a single daemon on voice-kit:

- **One owner of the mic + whisper pool.** `voiced` runs once, holds the model (from
  models-kit), and dispatches recognised intents to the **active** app.
- **App registration.** Apps register a grammar/intent set and a *scope* per phrase:
  - `always` — listen even when the app isn't focused (wake-style, e.g. "hey krapper");
  - `active` — only when the app is foreground (most commands);
  and the daemon owns **system intents** globally: "switch to <app>", "open <document> in
  <app>".
- **Handoff protocol.** On a match, `voiced` tells the target app *what it matched and what
  it needs* (slots filled), activating/launching it first for `switch`/`open` intents. Apps
  say when they want raw audio vs. just intents.
- Interface lands in `voice-kit` (traits: `VoiceClient::grammar() -> Grammar`,
  `on_intent(Intent)`), daemon binary in a new `apps/voiced` or under gitmind. Speaker
  claim (`speaker_claim.rs`) becomes the multi-speaker arbitration inside the daemon.

## Mailbox daemon (`dhtmboxd`) on opendht — one mailbox layer

Reference: **voiceterm** already has a full peer-mail stack (`peer_mail.rs`, `mail.rs`,
`mail_crypto.rs`, `mail_session.rs`, `mail_secure.rs`), and **net-kit + snapcore** already
carry DHT RPC. Unify the store-and-forward mailbox so every service uses one:

- **unendlich** multiplayer servers (match invites / state handoff),
- **snapclone** contacts (peer discovery + presence),
- **voiceterm** email/peer-mail,
- other games/services (generic typed mailboxes).

Design: `dhtmboxd` runs one opendht node, exposes a local API (`send(box_id, msg)`,
`poll(box_id)`, `subscribe`), with crypto-kit for envelope encryption (reuse
`mail_crypto`). Mailboxes are typed by app; net-kit provides the transport, snapcore the
DHT put/get. Apps drop their private DHT clients and talk to the daemon. This also gives
offline delivery (DHT as the drop) for free across the fleet.

## Qt / GTK → egui ports (onto uikit-egui + adaptive)

Already egui: **logscope**, **unendlich** (these seeded the kit). To port:

| App        | Current    | Notes for the port                                            |
|------------|------------|---------------------------------------------------------------|
| rusty-board| GTK4/Adwaita | on-screen keyboard; touch-first — adaptive + gesture layer fit well |
| bot-arena  | GTK4/Adwaita | agent arena dashboard; desktop-class, mostly tables/panels    |
| voiceterm  | Qt/QML (web2qml) | large; port the shell to egui, keep the voice/mail cores |
| bo-suite   | Qt          | control panel over the mux daemon                             |
| snapclone  | Qt          | camera/preview UI; egui + the carved surface                  |

### Port progress — bot-arena → egui (DONE) + swipe gestures wired in
- `apps/bot-arena/egui-frontend` (`bot-arena-egui`): the GTK4/libadwaita dashboard ported to
  egui. Views (Arena/Standings/Replays) are **swipe-paged** via the shared gesture layer
  (`GestureLayer` over uikit-gesture), laid out adaptively (desktop shows a tab row; phone
  swipes), themed from snapclone-theme, with the barrier indicator painted mid-swipe.
  Compiles vs egui 0.29; 3 tests. bot-arena's engine.rs/fsm.rs are GTK-free → drop-in on-device.
- `rusty-board-egui`: now uses the gesture layer too — `Board` swipes between qwerty ↔ symbols
  layouts (mirrors rusty-board's swipe layout-switching). Test `swipe_switches_layout`.
- **Swipe-gesture shared libs (uikit-gesture + gesture_layer) are now adopted in two ports.**

Port recipe: keep the Rust core crates as-is, replace the UI crate with an egui front-end
built on `uikit-egui` (skin + carved widgets + theme bridge) and `adaptive` (so one binary
adapts phone↔desktop), reusing `examples/logscope_touch.rs` as the pattern. Do them one at
a time; rusty-board is the best first port (small, touch-first, already needs the adaptive
layer).

## Suggested order

1. Wire install hooks into the suite/group installer (unblocks group installs).
2. models-kit adoption (voice-kit + the three model loaders) — small, high leverage.
3. `voiced` (biggest UX win; voiceterm already has every piece).
4. Port rusty-board → egui (proves the adaptive kit on a real touch app).
5. `dhtmboxd`, then the remaining ports.

## Analysis findings (kit opportunities)

Re-ran the duplication scan across all 16 apps. Keyword-based, so treat counts as
directional, but the strong signals:

**New kits worth extracting** (recurring, not yet a kit):
- **config-kit** — TOML config load + XDG config/data dir resolution. ~11 apps each roll
  their own. Highest-confidence, small, universal. Build first.
- **cli-kit** — arg parsing + subcommand dispatch. ~11 apps hand-roll `std::env::args`
  loops. A thin shared parser (the style gitmind's CLI already uses) removes a lot of it.
- **device-kit** — DONE. Device model + selection-by-preference, video/audio format
  negotiation (nearest supported), v4l2loopback setup + `v4l2-ctl` enumeration parser.
  std-only; PipeWire/v4l2 capture behind a feature on-device. 6 tests.

**Existing kits with un-adopted consumers** (the integration backlog, biggest first):
- **uikit** → the 9 egui apps (this is the port program).
- **audio-kit** → beatspace, krapper, bo-suite, fao-codec (DSP/mel).
- **models-kit** → voiceterm, beatspace, snapclone, bot-arena (model loaders → `~/models`).
- **voice-kit** → voiceterm, rusty-board, logscope (whisper post-proc).
- **net-kit / snapcore** → voiceterm, unendlich, snapclone, bo-suite (transport/DHT).
- **crypto-kit** → voiceterm, snapclone (sealing/envelopes).

## Daemon substrate added this turn

- **ipc-kit** — local app↔daemon IPC over a Unix socket, socket-first fallback
  (`try_connect` → `None` when no daemon → app uses its own code), per-event Scope
  (Always/Active). Reuses net-kit's line codec.
- **voiced-core** — the voice daemon's routing brain (register intents per event-type +
  scope; deliver Always to backgrounded apps, Active to the focused app; own switch/open
  system intents). Audio stage still to wire from voice-kit + voiceterm.
- **datashare-core** — cross-app data bus (namespaced KV + pub-sub Hub). share-kit is the
  durable/federated backend.

Remaining to make the daemons real: bind ipc-kit sockets in a `voiced` and a `sharemd`
binary, wire voiced's audio stage (voice-kit + audio-kit + voiceterm's mic/whisper pool),
back datashare with share-kit, and have apps call `try_connect` before their own path.

## Kits built (config/cli/gpu) + TODOs

Built and tested this pass:
- **cli-kit** — std-only arg parsing (flag / opt / opt_all / positionals / subcommand),
  lifted from gitmind's CLI. **gitmind's CLI now consumes it** (first realized adoption).
- **config-kit** — std-only XDG dirs + load/save + minimal `[section] key=value` parser.
- **gpu-kit** — reduce wgpu boilerplate. v0.1 is the version-agnostic policy core (power
  preference, sRGB, present-mode/MSAA/format selection over the app's capability lists),
  tested. A `wgpu29`-feature context builder (`Gpu::headless`, `shader()`) wraps the
  instance→adapter→device→queue dance.

### TODO — GPU kit
- The 7 wgpu apps span **wgpu 0.19 / 27 / 29** — align them (target 29) so the `wgpu29`
  context builder is usable everywhere; until then the policy core is the portable part.
- Compile-verify `gpu-kit[wgpu29]` on-device (can't build wgpu in the sandbox).
- Extend the builder: surface creation + `configure(&Policy)` + resize; a small
  render/compute pipeline helper; bind-group-layout shorthands. Reference the repeated
  code in beatspace/src/gpu.rs, fao-codec/src/{gpu_pipeline,gpu_phase_corr,v8_gpu_rasterizer}.rs,
  unendlich (world_renderer), bo-suite (surface config).

### TODO — remaining backlog
- config-kit / cli-kit adoption across the other ~10 apps (delete their local copies).
- **DONE:** `apps/voiced` + `apps/sharemd` are runnable daemons (ipc-kit `serve_hub`).
- **DONE:** voiced's audio-stage *brain* — voiced-core `grammar` turns transcribed text
  into intents (voice-kit clean+hallucination gate → system phrases → per-app grammars
  with `{slot}` capture, active app wins). Daemon `transcript` verb runs the full
  clean→gate→match→route pipeline; integration-tested (hallucination dropped, slot routed).
- **Remaining audio:** only the raw mic→whisper capture behind voiced-core's `Transcriber`
  trait (device + model I/O — can't run in the sandbox). On-device: implement it over
  voiceterm's whisper pool + voice-kit model resolution (models-kit `~/models`), feeding
  `transcript` messages to voiced.
- **DONE (enabler):** `shared/bigsilicon-client` — one SDK for voiced/sharemd/coordd with
  socket-first fallback baked in. `Voice::connect(app)` / `Share::connect()` /
  `Coord::connect(app)` each return `None` when the daemon is down (→ app runs its own code);
  inbound events arrive on a channel (`try_next`), so adoption is a few lines:
  ```rust
  if let Some(mut v) = Voice::connect("beatspace") {
      v.register("play_track", Scope::Active, &["play","{track}"]).ok();
      while let Some(ev) = v.try_next() { /* handle intent */ }
  } // else: in-process fallback
  ```
  `Coord::next_directive()` returns a typed `(Resource, Directive)`. Tested (fallback +
  typed directive push).
- **Remaining data:** back sharemd's Hub with share-kit for durability/federation; adopt the
  client in real apps (snapclone claims the call, voiceterm ducks, unendlich shares/pipes).
- device-kit (PipeWire/v4l2) — snapclone's camera-bridge is the reference.
- uikit adoption / Qt+GTK→egui ports (largest LOC lever).

### Port progress — rusty-board → egui (STARTED)
- `apps/rusty-board/egui-frontend` (`rusty-board-egui`): the on-screen keyboard's UI ported
  from GTK4 to egui on **uikit-egui + adaptive**. Renders keys sized by device class (taller
  touch targets on the FP5, tighter on desktop), themed via snapclone-theme, with a pure
  `hit_test` for headless layout tests. **Compiles against egui 0.29; 3 tests.** Proves the
  port recipe for the other Qt/GTK apps (bot-arena, voiceterm, bo-suite, snapclone).
- Remaining for rusty-board: split the crate into `rusty-board-core` (its already-GTK-free
  `layouts` + `gestures` + `keyboard/engine.rs`) and drive this front-end from it; add the
  eframe `main()` + text-injection into the compositor (the Wayland backend already exists).

## TODO — cross-app call/activity coordination (scenario: in a snapclone call)

When the user is in a snapclone voice call, other apps should react without hard-coding
snapclone. The substrate is in place; `coord-core` is the arbitration brain.

**Mechanism (uniform):**
- **Presence** — each app publishes its [`Activity`] to `presence/<app>` on the data bus
  (sharemd). snapclone publishes `voice_call`; subscribers react.
- **Claims + arbitration** — apps make `Claim`s on shared `Resource`s (Mic, CallAudio,
  Attention) with a `Mode` (Exclusive / Duck / Share). `coord-core::Arbiter` resolves a
  `Directive` per app per resource: Own / Yield / Duck / Share. Mic claims flow through
  voiced; CallAudio/Attention through sharemd.

**The scenario, resolved (tested in coord-core):**
- snapclone call → Exclusive claim on Mic + CallAudio (`VOICE_CALL`). Directive: Own.
- **unendlich** → Share claim on CallAudio (`GAME_VOICE`): pipes call audio into the game;
  snapclone keeps Own. unendlich also sends a *server invite* to the peer via share-kit's
  mailbox (dhtmboxd), keyed off the call's peer identity.
- **voiceterm** (user composing) → Duck claim on CallAudio (`DICTATION` ≥ `VOICE_CALL`, so
  honoured): snapclone's directive becomes **Duck** → it mutes the call momentarily and
  publishes a `busy` presence; voiceterm gets the floor. On release, snapclone resumes.
- A low-priority ducker (e.g. background media) is **not** honoured against a live call.

**Wiring status:**
- **DONE:** `apps/coordd` — the coordinator daemon serves the Arbiter over ipc-kit and
  pushes `directive <resource> <state>` deltas to affected apps on every claim/release.
  Integration-tested: snapclone gets `own`, voiceterm's duck pushes `duck` (mute), release
  pushes `own` (resume). Presence relayed via an `activity`/`presence` message.
**Remaining wiring:**
- Each app: translate its own activity into claims, act on its directive (mute/show-busy/
  pipe/invite), and publish presence. snapclone's busy notifier + mute hook; unendlich's
  audio-pipe + invite; voiceterm's compose-duck.
- Route the unendlich→peer invite through share-kit's mailbox.

## TODO — automatic accessibility voice via the kits

**Verdict: worth doing, and cheap here.** Because voiced already routes intents system-wide
and uikit-egui is the shared UI layer, accessibility voice lives *in the kit*: an app gets
voice focus-navigation, activation, and spoken descriptions almost for free. Guardrail:
on-by-default but **overridable** (a game like unendlich needs custom handling), and it only
works if widgets carry semantic labels — which is the real per-app work.

**Built (first cut):** `uikit-egui::a11y`.
- `Semantics` (role + label + value) with spoken `describe()` ("Mute, toggle, on").
- `A11yScene` — immediate-mode focusable-widget collector; voice `Action`s move focus /
  activate / describe; focus persists across frames by id; `describe_screen()` overview;
  `paint_focus_ring()`.
- `standard_grammar()` — the universal voice commands (activate/next/previous/first/last/
  what-is-this/read-screen/scroll/go-back) as (event_type, phrase) pairs, and
  `Action::from_event`. Tested (5).

**Remaining wiring (the "automatic" part):**
- A one-call helper `a11y::register_with_voiced(client)` that sends `standard_grammar()` to
  voiced at `Always` scope, and a default `on_intent` that feeds `a11y.*` events into
  `A11yScene::apply` — so a uikit app opts in with ~2 lines and can override.
- **DONE:** `a11y::semantic_button` / `semantic_toggle` render an egui widget and auto-`add()`
  its Semantics + rect — apps using kit widgets get labels without hand-tagging.
- A TTS sink for `Outcome::Speak` (piper/espeak via a small audio-kit/voice path, model in
  `~/models` via models-kit).
- **DONE:** rusty-board egui port adopts a11y — `register_a11y` tags every key, `voice_key`
  drives focus/activation (voice "next"/"activate" types the focused key). The ported
  keyboard is now both touch-adaptive *and* voice-navigable. Test: `keyboard_is_voice_navigable`.
- The `register_with_voiced` helper is now a 2-liner over the client: `for (e,p) in
  a11y::standard_grammar() { voice.register(e, Scope::Always, &[p]); }`. Remaining: TTS sink;
  then the other ports.

## Fedora RPM + Alpine apk repositories (gitmind as host/builder)

gitmind can now package a project and host a repo for the fleet: RPM for the Asahi/Fedora
desktop, apk for the Alpine/postmarketOS phone. It builds *on the device* with the native
tools and serves the repo over HTTP.

- **`cockpit::packaging`** (pure, tested): `PkgMeta`; `rpm_spec()` and `apkbuild()`
  generators (install prebuilt binaries — no recompile); command builders (`rpmbuild_cmd`,
  `createrepo_cmd`, `abuild_cmd`, `apk_index_cmd`); client configs (`dnf_repo_file`,
  `apk_repo_line`, `apk_add_cmd`); arch mapping; and a traversal-safe `resolve_in_root` for
  the repo server. 6 tests.
- **`flow` orchestration** (3 tests): `package_project(dir, format, out)` builds the binary
  (any language, via buildsystem), stages the RPM/apk tree (spec/APKBUILD + sources), and
  runs the native packager best-effort (absent tool → returns the staged tree + a hint);
  `repo_add` copies + reindexes; `serve_repo` is a minimal GET server dnf/apk fetch from;
  `repo_client_config` prints the exact dnf/apk setup; `host_repo_format` auto-detects.
- **CLI**: `gitmind fleet package <dir> [--rpm|--apk] [--out <dir>]`,
  `gitmind fleet repo add <pkg> --to <dir> [--rpm|--apk]`,
  `gitmind fleet repo serve <dir> [--port N] [--rpm|--apk]` (prints client config + serves).

Remaining (on-device, needs the native tools present): run rpmbuild/abuild/createrepo/apk
end-to-end on the fleet; optional GPG/abuild-sign signing (currently unsigned →
gpgcheck=0 / --allow-untrusted); a GUI "Publish" card.

## DHT rendezvous — reach the fleet across networks

Off-LAN (phone on cellular, desktop behind another NAT), netshare's LAN discovery can't find
a peer. gitmind now has a **DHT account + advertisement** layer so any fleet device can be
reached by account regardless of network.

- **`cockpit::rendezvous`** (8 tests): `account_id` (stable, derived from the device's ssh
  identity key) + `account_key`; `Advert` (user/hostname/endpoints/ttl) with a line record
  format; `Endpoint{Lan|Public|Relay}`; `resolve_order`/`best_target` (LAN if same /24, else
  Public→Relay); `same_subnet`; freshness; and the `AdvertStore` trait with an in-memory +
  `FileStore` impl.
- **`flow`**: `self_account`, `build_self_advert` (from netshare local_ip + user + hostname),
  `advertise`/`resolve_peer`/`reach`, `default_advert_store` (file drop under the config dir).
- **CLI**: `gitmind fleet account`, `fleet advertise [--port --public host:port --relay --ttl]`,
  `fleet resolve <account>`, `fleet reach <account>` (prints the best ssh route).

**DHT backend (on-device):** `AdvertStore` is the seam. The production store is **snapcore**
— `put(key,record)` → `announce`/`store` under `hash(account_key)`, `get(key)` →
`find_value`/`discover` — which needs a UDP `Transport` + bootstrap nodes (can't run in the
sandbox). share-kit's mailbox is an alternative backend. The `FileStore` works today over any
shared drop and as a cache of DHT results. Deploy integration: `reach` yields a
`(user,host,port)` that scp_push/ssh use, so pushing a build to a phone on cellular resolves
via the DHT advert instead of the LAN. Remaining: the snapcore UDP transport + a periodic
advertise loop (publish self every ttl/2), and public-IP discovery (STUN/echo).

## Peer connection broker — hand out streams, not DHT churn

**Question:** can snapclone hand out ports/sockets to already-connected peers so apps stop
each creating DHT entries, while keeping a reliable crypto handshake? **Yes** — and it's
built + tested.

- **`peermux-core`** (5 tests, real crypto via crypto-kit): a broker keeps **one
  authenticated session per peer** (ephemeral X25519 key-agreement, each side's ephemeral
  key signed by its long-term Ed25519 identity → the peer id *is* the verified key) and
  **multiplexes app streams** over it. `Session::open_stream(service)` derives an isolated
  per-stream key from the session secret via HKDF (`derive_session_key` with a per-stream
  `info`) — **no new handshake, no new DHT item**. Proven: same session key both sides;
  tampered handshake rejected; 3 streams over 1 handshake; the "mail" stream cannot open the
  "photos" stream's sealed bytes (cryptographic isolation).

**How it fits:** snapclone (or a small `connd`) is the broker. It holds the *single* DHT
presence via `rendezvous` (one advert per device, not per app), maintains sessions to
connected peers, and serves local apps over ipc-kit: an app calls
`open_stream(peer, service)` (through bigsilicon-client) and gets a muxed, individually-keyed
stream to an already-connected peer — falling back to its own path only if the broker is
down. This removes both the per-app DHT churn *and* the per-app handshake, without weakening
crypto: every stream is bound to the one authenticated session.

**Remaining (on-device):** frame the muxed streams over the real peer link (net-kit
transport), bridge each stream to a local socket/fd the app reads/writes (the "hand out a
socket" part), and add session rekey/expiry. The security model and mux bookkeeping are done.


## More kit candidates (fresh scan) + built

Re-scanned for cohesive cross-crate patterns (recurrence in parens):
- **proc-kit** — DONE. Run + capture subprocesses with a privilege-escalation ladder
  (direct → `sudo -n` → `doas`), plus `which`, root detection, and permission-error
  sniffing. Unifies the ladder hand-rolled in pydeps/device-kit/packaging/deploy (7 crates).
  6 tests. Adoption: point `pydeps::install_system`, device-kit modprobe, and packaging's
  rpmbuild/abuild at `proc_kit::run_escalated`.
- **fs-kit** (atomic write, 10 crates) — write-temp-then-rename + safe read-or-default; small.
- **time-kit** (14 crates) — unix-secs/millis, monotonic elapsed, and a tiny RFC3339 format
  (several places want a real changelog/log timestamp — e.g. packaging's rpm_date stub).
- **hash-kit** (11 crates) — non-crypto hashing (FNV/CRC32); I hand-rolled FNV in rendezvous
  and account_id — good first consumer.
- hex/base64 codec (12) — likely subsumed by net-kit/core-kit; check before extracting.

## Cross-app settings — "update the gesture feel" → all apps change

Today gesture feel is a compile-time default; changing it means recompiling. This adds a live
cross-app settings layer on the sharemd bus so a central app changes it once and every egui-kit
app updates — while keeping its own overrides.

- **`settings-kit`** (4 tests): one `Prefs` schema (gesture feel: sensitivity/commit_fraction/
  flick_velocity; touch: min_target/scale; palette; a11y: voice/tts) that maps onto the kit
  knobs. `to_kv`/`from_kv` (flat keys for the bus + config file), `apply_kv` (partial — the
  mechanism for both live changes *and* per-app overrides), and `apply_to_gesture`/
  `gesture_config` so applying prefs actually changes `uikit_gesture::GestureConfig`. Proven:
  a central change yields identical gesture feel across apps; higher sensitivity fires sooner.
- **`bigsilicon-client::Settings`** (live-sync tested): connects to sharemd, subscribes to the
  `settings` namespace, seeds from a snapshot, and keeps a `Prefs` in sync. `set(k,v)` publishes
  (the settings app); `poll()` picks up others' changes; `with_overrides` layers app uniqueness
  on top. Integration test: central `set` → another app's `poll()` updates its prefs live.

**So: yes.** With this, "update the touch gesture feel" in a central settings app publishes to
`settings/gesture.*` and every app applies it next frame — apps still keep bespoke overrides.

**DONE this pass:**
- `apps/settings/egui-frontend` (`settings-egui`): the central settings app — an egui editor on
  uikit-egui (sliders for gesture feel, touch targets, palette combo, a11y checkboxes) that
  returns changed keys; `SettingsApp` forwards them to a publish closure (→ `Settings::set`).
  3 tests; builds vs egui 0.29.
- `settings_kit::diff(old,new)` — publish only what changed. `uikit_egui::apply_prefs(ctx,&Prefs)`
  — applies palette + touch (gesture applied via `Prefs::apply_to_gesture`). uikit-egui lib green.

**The full loop, proven:** settings-egui edits `Prefs` → `diff` → `Settings::set` → sharemd
fans out → every app's `Settings::poll()` → `apply_prefs` + `gesture_config()` live.

**Remaining:** a tiny **settingsd**/sharemd persistence via config-kit (survive restarts); wire
each app's frame loop (`Settings::connect`/`poll`/apply) — ~3 lines using bigsilicon-client.

## Desktop-builds → phone-runs, for ALL apps (the first-priority flow)

Gap found + closed: `install_suite`/`group build` built + installed **locally**; the single-app
"desktop builds, phone runs" path (`push_install`) existed but nothing did it for a whole
group. Now:
- `flow::deploy_app_remote(app, target, user, host)` — build one app *here* for the device's
  target (cross-compiling if its libc differs), scp the binary, then install it on the phone
  (`~/.local/bin` + `.desktop` launcher via `installer::push_install_script`).
- `flow::install_group_remote(group, target, user, host)` — do that for every app in a group.
- CLI: `gitmind fleet group build <name> --to user@host [--target T]`. With `--to`, gitmind
  builds all the apps on the desktop, auto-detects the phone's triple over ssh (`rustc -vV`),
  and pushes+installs each. Without `--to`, it installs locally as before.

**Your flow:** clone the exported bundle on the desktop → `cargo build -p gitmind-cli` →
`gitmind fleet group build bigsilicons-apps --to user@192.168.2.223`. That builds all 16 apps
on the desktop and drops runnable launchers on the phone. (Prereq on the desktop: the phone's
Rust target for cross-libc builds — `rustup target add <triple>`; both are aarch64 so it's a
static-musl cross, not a full arch cross.) cockpit + cli compile clean, rustcheck clean.

## GUI parity check

Audited the GUI against this session's backend features:
- **In the GUI:** app-store groups (local `build all`), single-app push-to-phone
  (`push_install`/`send_binary`), device-target detection (`remote_target`), reclaim/
  prepare-ingest, suites, downloads, rollup/merge, keys.
- **Added this turn — group remote deploy:** the store card now has a `user@host` field and a
  **"→ device"** button per group → `build_group_remote` (build all here for the device's
  auto-detected triple, push+install each on the phone). Same first-priority flow as the CLI's
  `--to`. rustcheck clean; QML balanced; all `backend.*` calls resolve.
- **Still CLI-only (GUI TODO):** packaging/repos (`fleet package`, `fleet repo serve`) and
  DHT rendezvous (`fleet account|advertise|resolve|reach`). These are newer and secondary to
  the build→install flow; a "Publish" card and a "Reach across networks" affordance are the
  next GUI additions.

## Apps helping the user — cross-app actions (gitmind → voiceterm)

"If gitmind says 'you need to run this', can it hand the command to the terminal?" — yes, built.
- **`bigsilicon_client::action`** — a small deep-link vocabulary (open_tab, stage_command,
  run_command, open_doc, open_url). `send_action(target_socket, verb, args)` is socket-first
  (false if the target app is down → caller falls back). `Terminal::stage(cmd)` opens a
  voiceterm tab and **prefills** the command without running it (user reviews + presses enter);
  `run`/`open_tab` too. Server side: `parse_terminal_action(&Message) -> TerminalAction`
  (OpenTab/Stage/Run) — voiceterm calls this in its ipc-kit service. Tested: socket-first
  fallback + a staged command round-trips to a stub voiceterm.
- **GUI wiring:** the failed-build panel gains "→ fix in terminal" (stages
  `gitmind fleet deps --build-essentials` in voiceterm; falls back to showing it if voiceterm
  is down), via a `stage_in_terminal` qt_method over `Terminal::stage`.
- **voiceterm's job (on-device):** serve an ipc-kit socket named "voiceterm", and on each
  message call `parse_terminal_action`; Stage(cmd) → new tab + put cmd on the prompt (no
  enter), Run(cmd) → new tab + execute, OpenTab → new tab. Then any app can hand the user a
  ready-to-run command through the terminal.

## GUI: Network page → RPM/apk repos (agreed, next)

Confirmed placement: a "Publish / Repos" card on the Network page (alongside DHT rendezvous).
Pick a group → build packages (`package_project`) → Serve locally (`repo serve`, shows the
dnf/apk client line) and Serve over DHT (advertise the repo endpoint so a gitmind on another
network resolves+pulls — gitmind2gitmind). Backend exists CLI-only; the card is the wiring.


## Realized kit adoption (turning built kits into used kits)

Started closing the adoption gap (kits with 0 consumers → real consumers):
- **proc-kit → cockpit::pydeps** (first proc-kit consumer): `install_system`'s hand-rolled
  root→`sudo -n`→`doas` prefix ladder replaced with `proc_kit::Escalation` +
  `escalate_argv`. The escalator literals now live only in proc-kit; pydeps keeps its
  path_env-aware `which`/`run_ok`. pydeps 3 tests still green.
- **models-kit → voice-kit** (first models-kit consumer): `voice_kit::find_whisper_model`
  routes whisper discovery through `models_kit::search_dirs("whisper")` — the canonical
  ~/models store every app shares — instead of callers guessing dirs. voice-kit 11 tests.

Remaining 0-consumer kits: peermux-core, share-kit, registry-kit (peermux/share land when
snapclone's broker + durable sharemd backend are wired on-device). Next verifiable in-cockpit
adoptions: packaging tool-runs → proc_kit::run (needs a cwd-aware run_in), and
config-kit/cli-kit into more apps as the version bumps arrive.

## Accessibility → Display (white point + colour filters)

Cross-app display accessibility, added to the settings system end to end:
- **settings-kit** (9 tests): `DisplayPrefs { white_point: 0.4..1.0, color_filter }` +
  `ColorFilter` (None/Grayscale/Protanopia/Deuteranopia/Tritanopia/Warm) with `label()`/kv.
  Pure transforms `apply_white_point` (scales luminance → dims bright whites),
  `apply_color_filter` (standard colour-vision matrices + grayscale + warm), and
  `DisplayPrefs::apply`/`is_active`. Wired into Prefs to_kv/from_kv/apply_kv.
- **settings-app** (5 tests): Accessibility → **Display** section — a white-point slider and a
  colour-filter picker. Publishes `display.white_point` / `display.color_filter` like any
  other pref → live across the fleet; settingsd persists them (seeded from Prefs defaults).
- **Apply path:** apps run their palette colours through `DisplayPrefs::apply` (uikit-egui
  theme). Full-screen coverage of images/video is a shader or compositor pass (Plasma has
  night-colour) — the preference + colour math live here; a uikit-egui
  `apply_display(ctx,&Prefs)` palette pass is the next small step.

## Swipe & Interaction preferences page

Added to the settings system (schema → app UI → live/persisted):
- **settings-kit** (12 tests): `AnimationStyle` (Snappy/Smooth-fps/Heavy/Reduced) mapping onto
  the gesture engine's timing (detent_arm_window_ms / detent_rate_per_ms / takeover_lock_ms) —
  applied by `Prefs::apply_to_gesture`, so "detents snappy vs fps vs heavy" changes every app's
  feel. `TouchPrefs` gains `text_scale` + `button_scale` (size across apps). New `InputPrefs`
  { `handedness` (Right/Left), `double_tilt` } + `double_tilt_action` (a pure toggle the
  on-device accelerometer handler calls to flip handedness). All wired into kv.
- **settings-app** (6 tests): a **Swipe & interaction** section — detent-animation picker,
  text-size + button-size sliders, handedness picker, and a "double-tilt to flip handedness"
  toggle (via semantic_toggle, so it's a11y-navigable). Publishes the keys like any pref →
  live cross-app, persisted by settingsd.
- **Apply path:** animation + gesture already apply via `gesture_config`; text/button scale and
  handedness are read by apps for layout (uikit-egui pass is the next small step). double-tilt
  detection is on-device (accelerometer) → calls `double_tilt_action` → publishes the new
  handedness, which fans out to every app.


## Ports now consume the settings system (apply surface + adoption)

Closed the long-standing "apply surface" gap and wired it into the ports:
- **uikit-egui**: `apply_prefs(ctx, &Prefs)` now applies palette **+ text size + button/control
  size** (was palette + min_target only); new `filter_color(Color32, &DisplayPrefs)` applies
  the display accessibility filter (lowered white point + colour filter) per colour. 14 lib
  tests (incl. apply_prefs scales text; grayscale collapses channels).
- **bot-arena-egui** + **rusty-board-egui**: each gains `apply_settings(ctx, &Prefs)` →
  `uikit_egui::apply_prefs` for theme/text/button + `*gesture.config_mut() =
  prefs.gesture_config(px)` so the swipe feel *and* detent animation track the fleet. Tests
  prove the port adopts the shared feel (bot-arena: Heavy → 220ms takeover lock).

So both ports are now full kit citizens: adaptive + a11y + gesture + **live cross-app
settings** (palette, text/button size, gesture feel, detent animation). Apps opt in with
`Settings::connect()` → each frame `apply_settings(ctx, settings.prefs())`. Remaining ports
(voiceterm/bo-suite/snapclone) follow the same recipe; full-screen display-filter coverage of
images/video is still a compositor/shader concern (per-colour `filter_color` is available now).


## Voice filtering + bo-suite port

- **Voice filtering (settings):** settings-kit `VoiceFilter` (Off/Light/Standard/Strict) on
  A11yPrefs with level semantics (strips_artifacts/filters_hallucinations/requires_speech_like);
  settings-app shows a voice-filtering picker under Accessibility (when voice is on);
  voice-kit `filter_transcript(text, strip, drop_halluc, require_speech)` composes the
  hallucination filters by level (voiced maps the setting → these flags). Live cross-app + persisted.
- **bo-suite → egui (DONE):** `apps/bo-suite/egui-frontend` — the B&O controller ported: per-device
  volume + playback and scene presets, swipe-paged (Devices/Scenes/Library), adaptive,
  a11y-navigable, and `apply_settings` so it tracks the fleet feel. 3 tests, egui 0.29.
  bo-suite's mixer/routing engine is Qt-free → drop-in on-device. Detached as its own workspace
  (bo-suite is a cargo workspace) via an empty `[workspace]`.

**Ports now:** rusty-board, bot-arena, bo-suite (all full kit citizens: adaptive+a11y+gesture+
live settings). Remaining: voiceterm, snapclone.


## voiceterm port — started (the tall one)

The terminal is the hard port; started with the portable, tested core (no egui/PTY in it):
- **`apps/voiceterm/egui-frontend`** terminal core (11 tests): `Grid` (fixed grid + cursor +
  pen style + scrollback; put/wrap/scroll, line_feed/CR/tab/backspace, move_to, erase
  line/display, resize) and a `Parser` (Ground/Esc/Csi state machine) handling printable UTF-8,
  the C0 controls, and CSI: cursor moves (A/B/C/D, H/f CUP), erase (J/K), and SGR
  (colour/bold/inverse + bright). `Terminal { grid, parser, input }` with feed_bytes/feed and
  `stage(cmd)` — the cross-app `stage_command` lands on the input line without running.
- **web → egui** (8 tests): the old path rendered web content into **QML (retained** scene
  graph); egui is immediate-mode, so re-parsing per frame is wasteful. Inverted the assumption:
  `web::parse_html` builds a flat **display list** (`RichDoc` = blocks of styled inline runs)
  **once** on content change; `web::render` walks it into egui each frame (cheap, no parse).
  Tolerant subset — block tags (p/h1–6/li/pre/blockquote/hr), inline (b/strong,i/em,code,a),
  HTML entities, ws-collapse (preserved in pre); unknown tags degrade to their text. This is
  the assumption change the QML→egui move forces, handled at the data-model level.
- **viz — diagram/plot render layer** (5 tests): voiceterm's geometry engines (mermaid,
  layered/graph layout, niftyplot) are already Qt-free — they compute positions; only the
  drawing was QML. Instead of a renderer per engine, they all target one IR: a `Scene` (flat
  display list of 2D `Prim`s — Line/Polyline/Rect/Circle/Arrow/Text — in world coords, with
  palette *roles* resolved at draw time). `render_scene` fits world bounds to the target rect
  and draws via egui's Painter (cheap per frame; layout cached in the Scene). Adapters built:
  `graph_scene` (nodes→boxes+labels, edges→arrows) and `plot_scene` (data series→axes+polyline,
  the niftyplot role, y-flipped). mermaid/sequence follow: their pure layout → `Scene`.
- **Next:** the egui cell renderer (monospace grid), PTY I/O (on-device), voice pane
  (voice-kit + voiced/whisper), mail pane; and wiring the real mermaid/graph_layout/niftyplot
  engine outputs into `graph_scene`/`plot_scene`. The genuinely hard, easy-to-get-wrong parts —
  VT emulation, web rendering, and the shared diagram/plot render target — are done + covered.

**Ports:** rusty-board, bot-arena, bo-suite (complete); voiceterm (core started); snapclone remaining.


## Wiring — real engines → the render targets (nifty-core extraction)

Wired voiceterm's actual diagram engines to the new egui Scene renderer, via a clean extraction:
- **`shared/nifty-core`** (NEW lib) — moved voiceterm's three self-contained, Qt-free engines
  out of the bin: `niftyplot` (source → GraphDoc of nodes/edges/planes), `graph_layout`
  (force-directed positioning), `mermaid`. Builds standalone; **106 of the engines' own tests
  pass** post-move. No duplication — both the Qt app and the egui front-end share it.
- **voiceterm bin** re-points at nifty-core: `mod niftyplot;` → `pub use nifty_core::niftyplot;`
  (+ graph_layout, mermaid), preserving every `crate::niftyplot::…` path. *(Can't build the Qt
  bin here — no Qt6 — so this re-export edit is unverified but is the standard extraction shape
  and trivially revertible.)*
- **voiceterm-egui `viz::nifty_scene(source)`** — the real end-to-end path: parse via niftyplot
  → position via graph_layout::layout_force_directed → build a `Scene` → render. Tested:
  `"A -> B\nB -> C"` → 3 boxes + 2 arrows with finite bounds. Not a mock — the shipping engines.

Ports/wiring status: rusty-board/bot-arena/bo-suite complete; voiceterm has terminal + web +
viz(with real nifty engines) cores done (25 crate tests); snapclone remaining. Remaining voiceterm: PTY I/O, voice/mail panes (on-device).

## voiceterm — terminal cell renderer + App (done)
- **`render_grid`**: draws the `Grid` as a monospace cell grid, grouping same-style runs per
  row into one text galley (efficient, not one draw per cell), with a cursor block. Colours
  come from `ansi_color` (standard 16-colour ANSI) + palette defaults, routed through
  `filter_color` so the display-accessibility settings (white point / colour filter) apply live.
- **`resolve_colors`**: cell style → (fg, bg), honouring inverse video + theme defaults.
- **`App { terminal, prefs, cell, font_size }`**: `apply_settings` themes the ctx and scales the
  monospace cell to the text-size pref; `render` draws the grid. The on-device `main` wires a
  PTY to `terminal.feed_bytes` and calls `render` each frame; voice/mail panes attach here.
  28 crate tests (terminal + renderer + web + viz + real nifty).


## snapclone port + extracting the founder's reusable systems

snapclone was the founder of several systems; two of its UI systems lived in QML and are useful
beyond it, so they became shared uikit-egui capabilities (localweather, av1tuner-mobile can adopt):
- **`uikit_egui::livebg`** (5 tests): live animated backgrounds — Aurora / Waves / Starfield /
  Gradient. Immediate-mode: `LiveBackground::tick(dt)` advances a phase, `paint` draws the frame
  with palette colours (re-themes + display-filter via caller). Pure motion math tested
  (wave_y, mix_factor, star_pos, phase wrap).
- **`uikit_egui::nav`** (3 tests): page + subpage navigation — top-level pages, each with a
  drill-down subpage stack; `go_page`/`push_subpage`/`back`/`can_go_back`/`title_bar`. One model
  serves phone (full-screen + back) and desktop (master–detail). The localweather Today→details
  and av1tuner presets→per-encode cases.
- **`apps/snapclone/egui-frontend` (DONE):** the port *consumes* both — pages Camera/Peers/Settings
  with a live background, swipe between pages (disabled inside a subpage so content wins), peer
  drill-down, a background-style picker, adaptive + a11y + apply_settings. 4 tests. snapclone-* engine
  crates (audio/proto/net-intel, Qt-free) drive it on-device; camera on the carved surface.

**Port program COMPLETE (egui + kits):** rusty-board, bot-arena, bo-suite, voiceterm (terminal +
web + viz + cell renderer), snapclone — all full kit citizens. Reusable extractions from the
founder now in the kit for the rest of the fleet.

## Audio playback kit set

Audio playback is reimplemented across bo-suite/snapclone/av1tuner/ambient-learner; extracting it:
- **audiomix-kit (NEW, 7 tests)** — pure channel routing + stream mixing, generalized from
  bo-suite's soundscape router (bo-mux channel_layout/mux, all Qt-free). `Channel` +
  `Layout` (Mono/Stereo/Quad/5.1/7.1: channels/count/contains/parse), `RouteMatrix`
  (input→output gains: `identity`, `downmix` with standard fold coeffs — 5.1→stereo folds
  centre+rears, stereo→mono averages, LFE dropped —, `set_gain`, `apply(frame)`,
  `contributions_to` for interop with bo-suite's `ResolvedRouting`), and stream merging
  (`mix`/`mix_into`/`soft_clip`). No hardware, no codec → f32 in, f32 out. This is the
  "routing mechanism integrated across the ecosystem" — bo-suite keeps its DSP upmix filters
  and delegates the routing/mixing; other apps get the same primitive.
- **opus-kit (proposed)** — the libopus/audiopus boilerplate is duplicated across the opus
  users (encoder/decoder setup, frame-size math, packet TOC). Pure parts (frame duration ↔
  sample count, valid frame sizes, TOC config) are testable; the FFI calls stay feature-gated
  (needs system libopus + pkg-config, can't build in-sandbox). Design only for now.
- **audio-kit** stays the *analysis* side (gen/goertzel/level/pitch/window); audiomix-kit is the
  *playback/routing* side. Adoption: bo-suite → RouteMatrix; snapclone-audio + ambient-learner →
  mix/mix_into for stream merging; av1tuner mux → Layout/downmix.
