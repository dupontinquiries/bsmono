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
- Bind ipc-kit sockets into real `voiced` + `sharemd` binaries; wire voiced's audio stage
  (voice-kit + audio-kit + voiceterm's mic/whisper pool); back datashare with share-kit.
- device-kit (PipeWire/v4l2) — snapclone's camera-bridge is the reference.
- uikit adoption / Qt+GTK→egui ports (largest LOC lever).
