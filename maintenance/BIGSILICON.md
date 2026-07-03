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
- **device-kit** — PipeWire / v4l2 / ALSA device bridging. ~7 apps (snapclone, bo-suite,
  voiceterm, krapper, logscope, rusty-board, beatspace). snapclone's camera-bridge is the
  reference; generalize the device plumbing.

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
