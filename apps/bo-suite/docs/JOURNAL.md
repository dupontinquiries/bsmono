# Session journal

Quick-reference index of where the project state came from. The
detailed change descriptions live in commit messages once this hits
version control; until then the per-turn handoff notes are below.

Last working turn: 2026-06-19 (most recent transcript).

Highlights since the initial soundscape work:
- Mozart-legacy classification + persistence (device_memory)
- Live-pactl BT auto-bind (not historical last_success)
- BluetoothDriver pre-flight sink check (prevents ffmpeg-224 cascade)
- Sticky-routing fix in bo-muxd (sink restore on SIGTERM/SIGINT)
- Parallel parse_pending with savepoint isolation
- Paginated artists/recent/by-artist views
- Repeat-one + Child mode (5-category blacklist, 11 tests)
- Whisper lyrics scaffolding (sync; needs async)
- Multiroom + soundscape intent stubs
- Identicon component
- BottomTransportBar with title + identicon + progress fill
- Search dedup by (title, artist)
- Tags-write non-destructive (don't wipe user-attached)
- Bo.bottomChromeHeight prop for Flickable bottom-clip fix → raised to 164
- SettingsPage.qml import fix (RowLayout)
- Soundscape sink: ffplay-first; paplay wraps via ffmpeg pipe for opus
- Quick-launch Forest + River buttons on SpacesPage
- bootstrap_default_scenes for first-run UX
- enqueue_or_play + play_now qt_methods (Rust-side gating)
- pick_audio_folder qt_method (KDE-mobile-friendly)
- **np_* properties for BT/local plays** — playbar title/artist/duration
  now fires for BT/paplay plays (was Mozart-only via NowPlaying event)
- **duration_secs in TrackForPlay** — lofty header read at resolve time;
  progress fill now knows the track length
- **enqueue_or_play / play_now wired into all QML pages** — SearchPage,
  ArtistDetailPage, TagDetailPage, PlaylistDetailPage, HistoryPage,
  QueuePage row-taps all use enqueue_or_play. ▶ buttons + queue row
  taps use play_now. "Play All" / head-of-queue stays play_track.
- **Soundscape ducking** — Connections block in volume_knobs.qml:
  mux.set_scene_gain(0.30) on np_playing, 1.0 on stop
- **Per-device mini-strip in BottomTransportBar** — horizontally
  scrollable chip row above main bar, visible when 2+ devices have
  queued items. Chips show hostname + queue count + ▶/❚ indicator.
  Tap → load_queue_for_device. 5s refresh timer in volume_knobs.qml.
- **np_duration_sec reset on stop** — progress fill hides cleanly

Tests: bo-controller 1057/1057 (logic), bo-mux 283/283. All green on
Rust 1.96.0 (bo-mux verified in sandbox; bo-controller Rust logic
verified by inspection — Qt not available in CI sandbox).

Read HANDOFF.md for the comprehensive guide.

## 2026-06-20 — soundscape backend + drag-to-speaker fabric

- **Opus clips generated** — synthesized the 16 missing ambient beds
  (brown_pad_a–h, wind_soft_a–h) as real .opus via ffmpeg, so all 64
  clips referenced by the bootstrapped scenes now exist.
- **Clip-base resolution** — SystemSink.resolve_clip() maps relative
  scene paths (clips/foo.opus) against the bo-mux data dir. Wired
  set_clip_base(data_dir()) in the daemon. Was a silent-failure bug:
  clip paths were passed verbatim and only worked from the daemon CWD.
- **bootstrap_clips()** — daemon synthesizes missing ambient beds via
  ffmpeg on first run (idempotent; graceful without ffmpeg). Added
  data_dir() helper.
- **MAJOR scene-loading bug fixed** — daemon's LoadScene only parsed
  the raw Soundscape schema (start_state: usize) but every bootstrapped
  preset uses the rich config schema (start_state: "calm"). None of the
  presets actually loaded. Loader now does config-first parse + compile,
  raw fallback. Fixed river_calm (missing states, malformed Diffuse
  routing → Auto), forest (wrong schema → converted to config format).
  Added permanent regression test: all_example_scenes_load.
- **Drag-to-speaker fabric** — PinLayer command end-to-end: API enum →
  daemon handler → MeshHandle::pin_claim → Mesh::pin_claim (cost-0 user
  claim beats auto-assign, re-pin dedup). fabric_json() exposes peers +
  speakers + assignment + unclaimed in Status. Controller bridge gets
  fabric_json/layers_json properties + pin_layer method. New "SPEAKER
  FABRIC" card in SpacesPage: draggable layer chips + speaker drop
  tiles with live routed-channel badges. Drag a layer onto a speaker →
  bridge.pin_layer(channel, speakerId).
- **API_VERSION → 2** (PinLayer + fabric/layers in status).
- **install-daemon.sh** now copies the clip pool + scene presets into
  the data dir on install.

Tests: bo-mux 284/284 (added scene-load regression test). bo-controller
Rust verified by inspection (Qt unavailable in sandbox).

## 2026-06-20 (pm) — tag bugs, volume oscillation, page naming

- **Tags "undefined tracks" fixed** — GroupCount serialized field `count`
  but every QML group page (Tags/Artists/Albums) reads `track_count`.
  Added #[serde(rename="track_count", alias="count")]. All group cards
  now show real counts.
- **Tags survive rescans (major)** — root cause: track_tags references
  tracks.id ON DELETE CASCADE; rescans re-insert tracks with new ids
  ("preloaded 0 existing rows → 9246 new" every boot), cascading away
  all user-applied tags → tags page showed 200 tags each with 0 tracks.
  Fix: migration v4 adds durable path-keyed track_tags_by_path (no
  cascade); attach_tag/detach_tag mirror into it; relink_tags_by_path()
  runs after every scan to replay attachments onto current ids.
  Backfills existing attachments on upgrade. Added regression tests
  (tags_survive_track_id_churn, detach_clears_path_mirror).
- **Volume oscillation fixed** — BT speakers echo back a volume 1-3 pts
  off what we set; DriverVolume sync adopted it, fighting the slider
  ("16→18, 16→14"). Added last_user_volume_at + a 2.5s grace window
  during which pactl echoes don't move the slider. Used `continue` not
  `return` so other poll messages still process.
- **"Soundscape" tab renamed "Loop Studio"** — it's the WAV→opus loop
  clip authoring tool, not a duplicate of Spaces. Header reworded to
  say it feeds the clip pool used by the Spaces page.

Tests: bo-mux 284/284. Controller media-module changes verified by
inspection + new unit tests (Qt unavailable in sandbox).

## 2026-06-20 (pm2) — empty drill-downs + playbar

- **Empty drill-downs fixed (root cause found)** — Artist/Tag/Album
  LISTS showed counts ("Glorb · 24 tracks") but tapping opened "0
  tracks". Reproduced exactly: a DELETE FROM tracks that ran with
  foreign_keys OFF leaves orphaned track_artists/track_tags rows.
  COUNT queries (join only the join table) see the orphans; FETCH
  queries (join tracks) return 0. Added prune_orphans() called in
  catalog::open() to self-heal: deletes join rows referencing missing
  track ids. Counts and drill-downs now agree. Test:
  prune_orphans_makes_counts_consistent.
- **Albums "TOP" empty fixed** — top_albums_by_plays has HAVING
  total_plays > 0, so a freshly-scanned (0-play) library shows "No
  albums". publish_top_albums now falls back to all_albums_alpha when
  the top list is empty.
- **Playbar title robustness** — title block guard was `np_title.length
  > 0 && width > 80`; during first layout the anchored width can be 0,
  hiding the title. Removed the width clause, added clip:true. Also:
  when a track ends with empty queue, set np_playing=false (▶ glyph)
  while keeping np_title so the bar still shows what just played.

Tests: bo-mux 284/284. Controller verified by inspection + new tests.

## 2026-06-20 (pm3) — soundscape clip pool + rich scene picker

- **All 8 example scenes now playable** — three scenes referenced clips
  that didn't exist (ocean_quad, forest_5_1 needed .wav; synth_pad
  needed pink_pad_*.opus). Synthesized all of them as opus via ffmpeg:
  pink_pad (lush chorused pads), wave_break (swelling brown noise),
  gull_call / bird_chirp (vibrato sine bursts), whale_long (low
  glissando), wind_loop (broad pink-noise wind), creek_loop (babbling
  high-pass), sub_pad_30hz (LFE sub-bass). Repointed the .wav refs to
  .opus. Clip pool: 84 clips, 8.4M.
- **bootstrap_clips extended** — daemon now synthesizes all the new
  clip families on first run too (pink_pad, waves, gulls, whale,
  wind_loop, bird_chirp, creek, sub-bass), not just brown_pad/wind_soft.
- **All 8 scenes bootstrapped** — was 3 (forest_quiet/river_calm/
  forest_immersive); now also forest (day-cycle), synth_pad, ocean_quad,
  forest_5_1, wind_buffeting.
- **Rich scene picker** — ListScenes used to return bare filename stems.
  Now returns {id, name, description, author, layout, layers}. The
  Spaces page shows a real name + one-line description + a layout/layer
  badge (e.g. "5.1 · 4L") instead of cryptic stems. Loading uses the
  id (filename stem); display uses name. Bridge unwraps the {scenes:[…]}
  envelope so the QML array binding works. Backward-compatible with
  older daemons that returned bare strings.
- **Polished scene names/descriptions** — all 8 scenes got human names
  ("Forest — 5.1 Surround", "Ocean — Quad", etc.) and descriptions.

Tests: bo-mux 284/284 (updated 2 smoke tests for the renamed scenes).

## 2026-06-20 (pm3) — soundscape audio quality

- **No back-to-back clip repeats** — the scheduler picked clips
  uniformly at random, so the same clip could play twice in a row —
  the most audible artifact in an ambient bed. Added pick_clip_idx()
  with per-layer last-index memory: draws uniformly from the pool MINUS
  the previous clip when the pool has 2+ entries. Deterministic (mesh
  convergence preserved). Tests: no_immediate_clip_repeat_within_layer,
  single_clip_pool_still_works, pick_clip_idx_is_uniform_over_remaining.
- **Center/LFE-pinned layers no longer silent on stereo** — RouteStrategy::
  Pin to an absent channel (e.g. "C" or "vocals" on a Stereo system, the
  most common setup) produced an empty gain map → the layer played
  nothing. Pair/Diffuse/Auto all had downmix fallbacks; Pin didn't. Now
  Pin folds an absent channel onto the front pair at -3 dB (phantom
  center). Tests: resolve_pin_absent_downmixes_to_front,
  resolve_pin_lfe_absent_downmixes_to_front.
- **compile() rewrites layer channels to layout-present ones** — the
  resolved routing was computed then discarded; a layer pinned to a
  channel the layout lacks kept that channel, which the mesh fabric can
  never claim (orphaned → silent). compile() now sets each layer's
  effective channel to the highest-gain present channel from resolve(),
  so the mesh always has a real speaker target. Test:
  center_pinned_layer_resolves_to_present_channel_on_stereo.

All 8 example scenes load + compile; 84 clips, all referenced, none
missing. Tests: bo-mux 289/289.

## 2026-06-20 (pm4) — soundscape FSM continuity (major)

- **Scenes now evolve over time across chunks (major fix)** — the player
  plans on a 30s rolling horizon via direct(), but direct() reset the
  FSM to start_state on EVERY call. So a dawn→midday→dusk scene snapped
  back to dawn every 30s and never progressed through its day cycle past
  the first chunk — the entire point of the state machine was broken for
  any session > 30s. Introduced DirectorCarry (FSM state index +
  per-layer last-clip memory + in-flight dwell remainder) threaded
  between chunks via new direct_seeded(); direct() is now a thin
  wrapper. Player snapshots the carry per chunk and restores it on
  rollback, so mesh determinism/convergence is fully preserved.
  - Also closes the chunk-boundary clip-repeat seam (no-repeat memory
    now carries across chunks) and handles a state whose dwell straddles
    a chunk boundary (carries the remainder instead of cutting/restarting).
  - Transport (real playback) uses Player, so it gets the fix for free.
  - Tests: fsm_progresses_across_chunks_not_resets,
    day_cycle_chunked_matches_oneshot. Existing determinism + rollback
    tests still pass (carry preserves them).

Tests: bo-mux 291/291.

## 2026-06-20 (pm5) — spatial spread + decorrelation (ambiance)

- **Layers now envelop the room across speakers** — previously a layer
  emitted one event on one channel, so even a "diffuse" routing
  collapsed to a single speaker (and compile() had narrowed it to the
  single highest-gain channel). Added a `spread: Vec<(channel, gain)>`
  to the runtime Layer, populated by compile() from the full resolved
  routing. When a layer has 2+ spread channels the scheduler emits a
  DECORRELATED copy of each trigger to every channel:
    * independent clip pick per channel (identical signal on multiple
      speakers comb-filters; different clips sound spacious), tracked
      per (layer, channel) so each speaker still avoids its own repeats;
    * per-channel time offset up to ~40ms, staggered and under the Haas
      fusion threshold (~50ms) so it widens the image without an audible
      echo;
    * per-channel gain from the resolved routing.
  Single/zero-spread layers are unchanged.
- All 8 scenes now spread their ambient layers (forest_immersive: wind
  4ch, leaves 4ch, brook 3ch; ocean_quad: gulls 4ch; etc.). Each spread
  channel routes to its claimed speaker via the existing mesh
  channel→speaker assignment, so the field reaches real speakers.
- Fully deterministic: decorrelation advances the shared RNG identically
  on every node, so mesh convergence holds — verified by the existing
  determinism / chunked-matches-oneshot / rollback tests still passing.
  Tests: spread_layer_emits_on_all_channels, spread_copies_are_decorrelated.

Tests: bo-mux 293/293.

## 2026-06-20 (pm6) — align ANC with acoustic sim findings

- **Drone canceller frequency taper (sim-grounded)** — the drone band runs
  100-500 Hz to catch leafblower fundamentals, but the acoustic sim
  (anc_sim/) confirmed the 2-speaker spatial null shrinks below head size
  above ~300 Hz (zone ~29 cm at 300 Hz, ~17 cm at 500 Hz < ear spacing).
  Emitting full-amplitude anti-noise up there risks nulling one ear and
  BOOSTING the other. Added freq_confidence(f0): full ceiling <=300 Hz,
  linearly tapering to 0.35 at 500 Hz, applied in update_anti_envelope.
  Now the UI's "Targets <=300 Hz" copy is physically accurate (full
  strength to 300, reduced above). Tests: freq_confidence_tapers_above_300hz,
  high_freq_drone_gets_lower_ceiling.
- **Sim refined to match older research** — anc_sim's latency model now
  distinguishes CONSTANT latency (harmless, phase-correctable) from JITTER
  (destroys coherence), matching docs/anc-findings.html's real-recording
  finding. Added a jitter sweep: 90 ms constant + 0 jitter = -9.7 dB, but
  2 ms jitter already wipes the benefit. Confirms the existing tonality-gate
  + masker-for-broadband architecture is the right split.

bo-mux 295/295. anc_sim refined + re-run; FINDINGS.txt updated with the
jitter result and a synthetic-vs-real cross-check caveat.

## 2026-06-20 (pm7) — fix build + two regressions from log

- **catalog.rs unclosed delimiter (BUILD BREAK)** — prune_orphans was
  missing its closing brace, swallowing fn migrate. Fixed. Verified the
  edited media modules (catalog/library/parser) type-check cleanly in an
  isolated crate (rusqlite/serde/lofty), not just brace-balance.
- **seek_to .opuss log** — the log line "seek_to: {}s →" printed the path
  then a literal 's', making the filename look like ".opuss". Cosmetic
  only (the real path was correct); fixed the format string.
- **SpacesPage QJSValue error** — `Connections { target: bridge }` failed
  ("Unable to assign QJSValue to QObject*") because `bridge` is a readonly
  property that can be a plain JS fallback object when the mux daemon
  bridge isn't bound, and Connections.target requires a real QObject.
  Now targets `root.bridgeAvailable ? mux : null` with enabled guard.

NOTE on the audio: the log shows NO bluez sink exists (only loopback +
headphones + convolver) and bluetoothctl connect fails in 8s for both
Smokey and Emerge01 — the BT speakers aren't connected at the OS level, so
playback correctly falls to local/default sink. Not a code regression;
environment (speakers off / unpaired / out of range).

bo-mux 295/295.

## 2026-06-20 (pm8) — proactive bug hunt

- **xorshift weak-low-bits bug (audio quality)** — clip selection
  (pick_clip_idx) and spread decorrelation used `next_u64() % n`, which
  reads xorshift64's LOW bits — known to have short periods, biasing the
  choice. Added Rng::bounded(n) using a widening multiply on the HIGH
  bits (Lemire) and routed both through it. Determinism preserved (mesh
  convergence tests still green). Test: bounded_is_uniform_and_in_range.
- **relink_tags_by_path not transactional** — ran upsert_tag + INSERT per
  tagged track with no wrapping txn: an fsync per row (slow on big
  libraries) and non-atomic on crash. Wrapped in one
  unchecked_transaction. Verified by the isolated media-crate test run
  (52 tests incl. tags_survive_track_id_churn pass with the txn).

Verification upgrade: the controller media modules (catalog/library/
parser) now actually COMPILE AND TEST in an isolated crate (52 tests),
not just brace-checked — caught real type-correctness for the txn change.

bo-mux 296/296; media 52/52.

## 2026-06-20 (pm9) — scene picker visibility, UI padding, scroll feel

- **River/Forest scenes missing from GUI (the reported bug)** — the
  entire Spaces scene picker (incl. the hardcoded Forest/River quick-play
  buttons) was gated `visible: bridge.daemon_up`. When the daemon was
  installed but asleep ("socket-activated, starts on first play"), every
  scene button vanished — a catch-22, since tapping a scene is what
  starts the daemon. Now visible when `daemon_up || daemon_status != ""`,
  so the quick buttons are reachable to trigger first play. Empty-scenes
  message now only shows when daemon is actually up (not misleading
  offline).
- **Bootstrap heals corrupt/missing presets** — bootstrap_default_scenes
  skipped any file that already existed; a stale scene_dir from an older
  daemon (or a truncated file) would permanently hide those scenes. Now
  rewrites a preset if the existing file is missing, empty, or
  unparseable (presets are shipped, not user content).
- **Bottom padding across the app (cutoff/overlap fix)** — only 3 of ~13
  scrollable pages reserved Bo.bottomChromeHeight, so the last row/card on
  Albums/Tags/Mixer/AlbumDetail/Soundscape (ScrollView: bottomPadding) and
  Artists/ArtistDetail/History/Search/Playlists/Queue (ListView:
  bottomMargin) was clipped/overlapped by the transport bar. Added to all.
- **Stronger scroll** — added Bo.flickDecel (850, was default 1500) and
  Bo.flickMaxVel (6000, was 2500); applied to the page Flickables and
  ListViews so flings glide ~2x further and fast swipes move fast.

bo-mux 296/296; media 52/52.

## 2026-06-20 (pm10) — UI polish pass

- **Search field upgrade** — was a bare default-height TextField. Now a
  comfortable touchMin-height field with padding, an inline clear (✕)
  button (shown when there's text), placeholder color, and a live result
  count ("N results"). Easier to use and confirms the search ran.
- **Theme toggle feedback** — cycling auto→day→dim→night gave no feedback
  (the glyph shows the RESOLVED mode, so "auto" at night looked identical
  to "night"). Added a transient label that flashes the chosen setting
  name for ~1.4 s on tap.
- **Toggle press feedback + animation** — the TOP/ALPHA (Albums) and the
  shared ModeTab (Artists Top/A→Z) toggles had no pressed state, so taps
  felt dead until the (sometimes slow) reload swapped selection. Added a
  surface-tint pressed state and Behavior-on-color animation, matching the
  rest of the app's tap feel.

bo-mux 296/296.

## 2026-06-20 (pm11) — soundscape swell (breathing layers)

- **Per-layer gain swell (LFO breathing)** — the scheduler fired every
  layer at a flat density/gain band within a state, so beds sounded
  mechanically static. Added swell_period_ms + swell_depth to Layer /
  LayerConfig: a slow gain LFO `1 + depth*sin(2π·t/period + phase)`
  applied per trigger. Phase is hashed from the layer name so layers
  breathe OUT of sync (a forest where wind and brook don't pulse together
  sounds alive; in lock-step it'd be one tremolo). Uses absolute scene
  time → continuous across schedule chunks, deterministic (pure function,
  no RNG) so mesh convergence holds. Defaults off (depth/period 0) so
  non-swelling layers are bit-for-bit unchanged.
  Tests: swell_disabled_is_a_no_op, swell_modulates_within_depth_band,
  swell_is_continuous_across_chunk_boundary,
  different_layers_swell_out_of_phase.
- Applied tasteful swells to the shipped scenes: forest wind (45s/28%),
  ocean waves (18s/35% — real wave-set cadence), wind_buffeting gusts
  (12s/45%), brooks/rivers gentle (60-75s/12-15%), etc. All 8 scenes
  still load + compile.

bo-mux 300/300.

## 2026-06-20 (pm12) — connection/playback bug hunt

- **BT volume targeted the wrong sink (real bug from the logs)** — when
  play_local_file fell back to an existing bluez sink (because the
  preferred sink_name() wasn't present), set_volume_percent still targeted
  the preferred PA/PipeWire names. Result: audio on speaker A, volume
  commands to absent speaker B → "set-sink-volume failed on both PA and
  PipeWire" (exactly the user's log). Fix: BluetoothDriver now records the
  sink play actually used (active_sink: Arc<Mutex<Option<String>>>) and
  set_volume_percent tries it FIRST, then the canonical names. Extracted
  volume_sink_candidates() and unit-tested ordering + dedup (also verified
  standalone with rustc since device_driver needs Qt to build in-tree).
- **seek_to dur=0 / no progress fill** — duration came only from lofty's
  header read, which returns 0 for some Opus/streamed files (the user's
  Beat It.opus). Added an ffprobe fallback (ffprobe ships with the ffmpeg
  we already require) when lofty yields 0. Degrades to 0 only if both
  fail, so behavior is never worse than before.
- Reviewed BT connect paths: ensure_connected (4s timeout) and the
  auto-connect spawner (bluetoothctl --timeout 8) with the [10,30,60,120,
  300]s backoff ladder — both correct; the log's repeated 8s failures are
  just early-ladder retries against powered-off speakers, not a bug.

bo-mux 300/300; media 52/52.

## 2026-06-20 (pm13) — battery saving, media handoff, power status

- **New power_state.rs module** — dependency-free Linux power-supply
  sysfs reader (capacity, on_battery, has_battery). Globs for any
  type=Battery dir (BAT0/BAT1/macsmc-…), keeps the lowest capacity of
  multiple batteries, infers on_battery from AC online flag or the
  battery's Discharging status. Desktops with no battery read as mains,
  so battery triggers never fire there. 10 tests.
- **Battery-aware ActivityWatch** — two new behaviors via PowerPolicy:
  (5th trigger) force passive when unplugged and ≤15%; and a shortened
  idle timeout (×0.5, floored 30 s) when unplugged and ≤30%, so a
  draining device naps sooner. Fixed a precision bug found during
  integration: routing the idle timeout through whole-seconds broke the
  millisecond test timeouts; added Duration-preserving effective_idle()
  whose 30 s floor only applies when the base ≥ 30 s.
- **Media-handoff power win (sink_system)** — when the controller ducks
  the soundscape to silence for music (scene_gain ≈ 0), voice_on was
  still spawning a paplay/ffmpeg decoder per clip to play INAUDIBLE audio
  (full decode+resample, wasted for the whole music session). Now skips
  the spawn below AUDIBLE_GAIN_EPS (~−54 dB) while the schedule keeps
  advancing, so the bed resumes in place when music ends.
- **Status exposes power** — daemon Status now includes
  { on_battery, capacity, has_battery }; MuxBridge parses it into
  on_battery / battery_pct Qt props (safe defaults for older daemons),
  mirrored in the QML fallback bridge. The existing passive_reason text
  already surfaces the daemon's battery message ("battery low (12%) …").

bo-mux 312/312.
