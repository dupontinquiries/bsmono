# Fleet voice mesh — plan

Goal: **speech across the whole fleet.** Any device's mic can hear you; heavy speech compute is
**offloaded to a trusted, capable device**; multiple mics recognize **who** is speaking; and a
recognized command is **routed to the device that should perform it**. Every app gets voice, and
bsettings controls the mesh.

## What already exists (we federate it, we don't rebuild it)
- **voiced daemon** (`apps/voiced` + `voiced-core`): binds the `voiced` socket, owns an **intent
  Registry** — apps `register(event, scope, phrase)`, feed `transcript(text)`, and voiced matches
  + routes to the focused/owning app. Today this is *single-device*.
- **Voice client** (`bigsilicon-client::Voice`): `connect / register / focus / transcript / events`
  — the API apps already use to get voice. **The fleet-awareness can live behind this, so apps
  barely change.**
- **coord-core** (`Arbiter / Claim / Directive / presence_key`) + **peermux-core**: cross-device
  arbitration + multiplexing — the substrate for federating voiced across devices (peermux-core
  finally gets a consumer).
- **mic-kit**: profile-gated encrypted capture + `FallbackChain` (graceful: read never blocks,
  `Live/Fallback/Silent`). **This is how offload degrades to local without stalling.**
- **voice-kit**: `pipeline` (Transcriber/Recognizer/FastpathTranscriber/VoicePipeline), `context`
  (ContextModel — the learned fastpath DB), `hallucination` filter, `model`.
- **crypto-kit** (`seal/open`): encrypt audio frames device→device.
- **Trust + discovery** (this session, in gitmind): `deploy::scan_ssh / host_trusted / trust_host`
  — the trusted-device set the mesh is allowed to use.

## Architecture — the utterance lifecycle
```
[mic on device A] --capture--> mic-kit FallbackChain
      │                               │ (never blocks; Silent floor)
      │  mesh::recog_path(A)          ▼
      ├─ Offloaded ──> seal frames ──> trusted Heavy host B ──> whisper ──> transcript
      ├─ LocalHeavy ─> whisper on A ─────────────────────────────────────> transcript
      └─ LocalFastpath > fastpath recognizer on A ───────────────────────> transcript
                                                                              │
                          voiced Registry (fleet-federated) <── transcript + speaker id
                                                                              │
                                    mesh::command_target(owner) ──> execute on device C
```
- **Where STT runs** = `voice_kit::mesh::stt_host` / `recog_path` (built): offload to a trusted,
  reachable **Heavy** device; else local heavy; else the **fastpath floor**. Offload is a step-up,
  never required (autonomy invariant).
- **Who is speaking** = speaker-id tags each transcript; the ContextModel keys fastpath/priors per
  speaker so the mesh can personalize + arbitrate (whose command wins).
- **Where it runs** = `mesh::command_target` (built): the trusted+reachable device that owns the
  intent. voiced's Registry becomes fleet-wide (federated over peermux/coord).

## Built this session (verifiable cores)
- **`voice_kit::mesh`** (5 tests): `SttClass{None,Fast,Heavy}`, `VoiceDevice`, `stt_host` /
  `can_offload` / `recog_path{Offloaded,LocalHeavy,LocalFastpath}` / `command_target`. Pure trust-
  and reachability-gated decisions with the local-fastpath floor. The routing/offload heart.
- **`settings_kit::VoicePrefs`** (round-trip tested): `mic_enabled`, `offload_enabled`,
  `compute_host` ("" = auto), `speaker_id` — kv round-trips, so it flows through settingsd to every
  app and shows in bsettings.

## Phases
1. **Offload STT (P1).** voiced (or a `voice-relay` companion) accepts sealed audio from a trusted
   origin, runs whisper, returns the transcript. Origin uses `mesh::stt_host` to choose the host and
   `mic-kit` fallback to stay live. *Cores done; needs the relay endpoint + wire.*
2. **Federate the intent Registry (P2).** A transcript captured on A resolves against intents
   registered on any device; `command_target` routes execution. Uses coord-core presence + peermux.
3. **Multi-mic speaker id (P3).** Speaker embeddings tag transcripts; ContextModel keyed per
   speaker; the Arbiter resolves simultaneous mics / whose-command-wins.
4. **App integration (P4).** Apps keep using `bigsilicon-client::Voice` unchanged — the mesh lives
   under voiced. A tiny shared helper registers each app's intents on start.
5. **bsettings controls (P5).** VoicePrefs UI: mic-on toggle, offload toggle, compute-host picker
   (from the trusted-device list), speaker-id toggle. *Pref layer done; picker UI next.*

## Design invariants (fleet ethos)
- **Trust-gated:** only SSH-trusted devices run our audio or receive our commands (mesh checks
  `trusted`).
- **Graceful floor:** no network → local fastpath still works (mesh returns `LocalFastpath`).
- **Offload for efficiency, not austerity:** send heavy STT to the device that has the compute;
  don't cripple the phone (the corrected survival-tool principle).

## Open questions
- Speaker-id model: on-device embeddings vs a shared model on the Heavy host?
- Arbitration policy when two mics hear the same command (nearest? focused device? explicit owner?).
- Relay transport: extend `voiced`'s ipc socket over the net (peermux) vs a dedicated TCP relay.
