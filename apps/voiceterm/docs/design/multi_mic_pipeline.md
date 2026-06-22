# Multi-microphone processing & dynamic voice pipeline

Status: **DESIGN ONLY (filed post-v0.1.519).** A large, phased arc.
This captures the user's vision faithfully and breaks it into stages
with a pure/testable core vs device/hardware parts, and flags the
open questions. NOTHING built yet.

## Implementation status (updated v0.1.523)

- Stage 1 (classifier) — DONE: src/mic_topology.rs (10/10). classify()
  + deviation().
- Stage 2 (scheduler) — DONE: src/mic_scheduler.rs (9/9). schedule().
- Stage 3 (FSM) — DONE: src/mic_pipeline_fsm.rs (8/8, tested composing
  the real stage-1/2 modules). tick() with anti-thrash confirm window +
  budget-change replan.
- Stage 0 (follow active device) — NOT STARTED (OS glue, mostly device).
- Stage 4 (Kalman isolation) — CORE DONE: src/speaker_kalman.rs
  (10/10). ScalarKalman predict/update + SpeakerTdoaBank (per-speaker
  TDOA smoothing). Converges, gain bounded [0,1], no divergence over
  2000 steps, reduces steering jitter. The cross-correlation that
  PRODUCES TDOA obs + the beamformer that consumes the smoothed
  estimate are DSP/device; tuning needs real arrays.
- Wiring (real per-mic stats from the capture stack, whisper_pool
  spawn/reshape, audio-thread buffering) — DEVICE-ONLY, deferred.

The three pure cores now COMPOSE: the FSM calls classify → schedule
every tick and reshapes only on sustained deviation. What remains to
make it live is the stats source + the audio-thread actions, both
device-only.

## The vision (user's words, organized)

Support the full range of microphone topologies and use each optimally:
  - **Phone built-ins** (top+bottom): treat as a PHASE array → phase
    cancellation for noise reduction. Easy default rule.
  - **Studio / podcast**: multiple mics, possibly users in different
    ROOMS → treat as separate ROOM mics, not a phase array.
  - **Mixed / unknown**: use FINGERPRINTING (of speakers AND of the
    sounds/room) to classify whether a given mic set behaves like a
    phase array or independent room mics.

From classification, build a GRAPH of the mics, reduce it to a simple
end-state TREE (the pipeline layout), with a DEVIATION TRIGGER that
rebuilds the tree when the acoustic situation changes enough.

Where the array geometry supports it, use KALMAN FILTERING to achieve
SPEAKER ISOLATION → enable a "raw-stream mode": raw streams → ingest →
derive the phased-array + room-mic layout → feed the voice pipelines.

A RESOURCE SCHEDULER takes known quantities — RAM, N CPUs, N mics —
and decides how many whisper instances to open, or whether to run
ASYNC INSERTION on a many-mic→few-processor (or single-processor)
node. Dynamic: a VAD-gate-assisted FSM samples per-mic noise/activity
in realtime and re-determines the pipeline SHAPE every 1500ms, with a
1500ms buffer on the raw streams so processing can catch up.

Screen-off interaction: normally screen-off → low power (see uclamp
work), BUT a setting for BROADCAST STUDIOS / planning-room servers
keeps the mics HOT while the display is off.

Short-term pragmatic step: just follow whatever the active audio
OUTPUT/input device is, and update when the system changes it.

## Proposed staging (smallest useful → full vision)

### Stage 0 (short-term, shippable soon): follow the active device
Track the system's current default input device; when it changes,
reset the pipeline to single-mic on the new device. No topology logic
yet. This is the "good in the short term" the user named. Mostly OS
glue (PipeWire/PulseAudio device-change events) — device part is
NOT sandbox-verifiable, but the state machine (current device →
pipeline reset) is pure + testable.

### Stage 1 (pure core): mic-topology classifier + pipeline tree
Inputs (computed elsewhere from raw streams): per-mic-PAIR correlation
/ phase coherence + per-mic fingerprint/room signature. Pure function:
  classify(mics: &[MicStats], pairs: &[PairStats]) -> Topology
where Topology ∈ { PhaseArray(group), RoomMics(groups), Mixed{...} }.
Rules: high inter-mic correlation + small delay → phase array (phone
built-ins); low correlation / distinct room signatures → room mics.
Then layout(topology) -> PipelineTree (leaves = mics, nodes =
phase-cancel / mix / isolate / per-room-decode). Pure, testable with
synthetic MicStats. The DEVIATION TRIGGER: a distance metric between
the current tree's assumptions and live stats; exceed threshold →
rebuild. (Same shape as dead_air's adaptive threshold — derive from
the data, don't hardcode.)

### Stage 2 (pure-ish): resource scheduler
Pure function: schedule(ram_mb, n_cpus, n_active_mics, model_cost) ->
PipelinePlan { whisper_instances, assignment: mic→instance,
mode: PerMic | AsyncInsertion(queue) }. Known-quantity math: how many
whisper instances fit in RAM/CPU; if mics > instances, async-insert
(round-robin VAD-gated frames into a shared instance queue). Fully
testable with synthetic budgets. The HARD part (actually spawning/
sizing whisper) is whisper_pool integration — device-only.

### Stage 3 (the FSM): VAD-gated dynamic reshape
An FSM that, every 1500ms, reads per-mic VAD/noise, re-runs
classify→layout→schedule, and reshapes the pipeline if the plan
changed — with a 1500ms raw-stream buffer so in-flight audio isn't
lost during a reshape. The FSM transition logic + buffering policy is
pure + testable (feed it scripted per-tick mic stats, assert the
shape changes only on real deviations, never thrashes). The actual
audio-thread reshape is device-only.

### Stage 4 (advanced): Kalman speaker isolation + raw-stream mode
When the array geometry supports it (phase array with known/estimated
geometry), a Kalman filter fuses the streams to isolate speakers.
raw-stream mode: expose the raw streams → the ingest/layout pipeline.
This is DSP-heavy; the Kalman update equations are pure + testable
(state predict/update, no overshoot/divergence on synthetic tracks,
like the camera's damped-spring tests), but tuning needs real arrays.

### Screen-off / broadcast mode
A setting: default = mics follow screen-off power policy (uclamp +
possibly suspend capture); "broadcast/studio server" = keep mics hot
+ pipeline full-power while display off. Ties into EcoGuard: the
guard's `defer` predicate becomes `is_app_backgrounded() &&
!broadcast_mode`.

## What's pure/testable vs device-only (the honest split)
- TESTABLE NOW (no hardware): classify(), layout(), the deviation
  metric, schedule(), the FSM transition logic + anti-thrash, the
  Kalman update math.
- DEVICE-ONLY: real per-mic stats from PipeWire, device-change events,
  actually spawning N whisper instances, the audio-thread reshape,
  Kalman tuning on real arrays, power measurement.

## Open questions (need user / measurement)
1. How to GET per-mic + per-pair stats — does the capture stack expose
   individual mic channels, or only a mixed stream? (Determines if any
   of this is reachable.)
2. Phase-array geometry: estimated from correlation, or require a
   user "these are a phone pair / a 2-mic bar" hint?
3. whisper instance cost model: measured RAM/CPU per instance on the
   target (M2 / Qualcomm) — needed for the scheduler to be real.
4. Deviation-trigger threshold + the 1500ms cadence: start values, then
   tune on device.
5. Async-insertion ordering: strict VAD-priority, round-robin, or
   newest-first when the queue backs up?

## Reuse / alignment
- Fingerprinting (phonetic.rs / acoustic_index.rs) already vectorizes
  speakers — the room/speaker signature for classify() can build on it.
- VAD already exists in the voice path (the silence atomic / wake
  gate) — the FSM consumes it.
- The 1500ms cadence matches the existing app-backgrounded debounce —
  one heartbeat.
- EcoGuard (v0.1.516) extends cleanly to the broadcast-mode predicate.
