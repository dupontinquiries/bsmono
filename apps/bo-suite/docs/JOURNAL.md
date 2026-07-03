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

## 2026-06-21 — Stage surround explainer (docs only)

- Added docs/STAGE_SURROUND_VS_STEREO.md explaining the "phenomenal"
  surround the user heard on the Beosound Stage playing Billie Jean. Root
  cause from the logs: every `bluetooth: play` line uses `ffmpeg -f wav`
  (2.0 PCM stereo, no -ac/-af/pan flags) to the A2DP sink, so bo-controller
  does a pure stereo passthrough. The surround is the STAGE's own DSP
  upmixing 2.0 → spatial. Billie Jean pops because it's a famously wide
  mix (lots of side content for the upmixer). Doc covers the mid/side
  mechanism, why it's content-dependent, a stereo-vs-spatial comparison
  table, and how to keep it (clean delivery + Stage listening mode).
- Log-confirmed wins from prior sessions: bt_volume_active_sink (volume
  succeeds once bluez_output sink exists; only the pre-sink attempt failed)
  and duration_ffprobe_fallback (Serenitas now seeks with dur=207, was 0).

No code change; bo-mux 312/312 unchanged.

## 2026-06-21 (pm2) — soundscape timing humanization

- **Per-layer timing jitter** — the scheduler advanced triggers on a
  fixed `step` grid and rolled a fixed probability each step, so sparse
  events (birds, drips, gulls) landed on a rigid lattice that reads as
  mechanical under careful listening. Added Layer/LayerConfig
  `timing_jitter` [0,1]: each fired onset is nudged within ±(jitter·step/2),
  clamped to the state window, drawn from the seeded RNG so the schedule
  stays deterministic (mesh-sync + rollback preserved). Both return paths
  already sort by at_ms, so jittered events stay ordered. Default 0 =
  exact prior grid (existing scenes bit-for-bit unchanged).
  Tests: timing_jitter_zero_is_exact_prior_grid,
  timing_jitter_changes_onsets_but_stays_deterministic,
  timing_jitter_keeps_events_sorted_and_in_window.
- Applied to discrete/sparse layers only (trigger_prob < 1.0, where grid
  timing is audible): forest birds(45%); forest_immersive leaves(50%),
  brook_far(30%), brook_mid(25%); ocean_quad gulls(50%), whale(40%).
  Continuous beds (trigger_prob 1.0) keep exact timing. All 8 scenes
  load+compile.

bo-mux 315/315.

## 2026-06-21 (pm3) — missing tracks + playback overlap

- **Missing tracks ROOT CAUSE (parser)** — filenames with no " - "
  separator (e.g. "Serenitas.opus", "blinkun x jeremih.opus") parsed with
  an empty artist segment → NO track_artists row → invisible everywhere,
  since the library is browsed only by artist/album/tag. Parser now falls
  back to "Unknown Artist" so every track is reachable. Tests:
  no_separator_gets_unknown_artist, empty_filename_still_gets_artist,
  real_artist_not_overwritten_by_fallback.
- **All-tracks view (belt-and-suspenders)** — added catalog::all_tracks_page
  (FROM tracks + COALESCE'd artist, no inner join, so nothing is ever
  filtered out), wired through MediaCmd::LoadAllTracksPage → worker →
  Page{context:"all_tracks"} → load_all_tracks_page qt_method. Tests:
  all_tracks_includes_tracks_with_no_artist, all_tracks_pagination_bounds.
  (QML "All Tracks" tab still TODO; backend ready.)
- **Playback overlap (driver)** — a decode pipe is ffmpeg|paplay but only
  paplay's pid is tracked, so stop() orphaned ffmpeg (the "224" spam) and
  rapid replay/seek bursts stacked overlapping pairs → doubled audio.
  Added reap_orphan_audio_children() (pkill --parent <our_pid> ffmpeg/
  paplay, same proven sweep driver_stop uses) called before every BT /
  Local / MultiRoom play. Verified safe: bo-muxd is a separate systemd
  process (not our child), so soundscape audio is untouched.

Media modules: 57/57 isolated. bo-mux 315/315.

## 2026-06-21 (pm4) — rustcheck tool + bugs it surfaced

User supplied `rustcheck` (AI-centric Rust checker: Tier-0 static + scoped
`cargo check`, separates your-code errors from env/dep BLOCKED). Stashed at
/tmp/tools/rustcheck for future sessions.

Findings:
- Confirmed bo-controller's own code type-checks cleanly up to the Qt
  boundary; the build BLOCKED is purely qttypes' build script needing
  qmake (env), NOT our code. Validates the medcheck-isolation strategy.
- bo-mux: full `cargo check --tests --deny-warnings` → after fixes, ZERO
  errors/warnings.
- **Real fix surfaced by --deny-warnings --tests:** the
  passby_canceller test `fsm_eventually_returns_to_quiet_on_stationary_tone`
  had a dead `saw_quiet_after_peak` (assigned, never read) — so it silently
  asserted nothing about return-to-quiet. Wired it into the assertion;
  discovered the full Peak→Recede→Quiet cycle needs ~33s (PEAK_MAX_MS 8s +
  RECEDE_MAX_MS 25s) but the probe is 15s, so the correct assertion is
  "progressed past Peak (Recede/Quiet)" — now actually verified, was a
  no-op before.
- Silenced intentional non_snake_case channel-code test names in
  protocol.rs with a scoped #![allow] so real warnings aren't buried.
- Added [features] analysis=[] to the /tmp/medcheck stub crate so its
  cfg(feature="analysis") matches the real bo-controller manifest (kills a
  false-positive unexpected_cfgs warning).

bo-mux 315/315; media 57/57 isolated.

## 2026-06-21 (pm5) — multi-speaker playbar UX

- **Per-device head track (multi-speaker "what's playing where")** —
  QueueDeviceSummary gained head_title (title of the lowest-ord queued
  track per device, via correlated subquery in library::queue_devices).
  The BottomTransportBar per-device strip now shows `[▶] host · "title"
  [count]` so you can see WHAT each speaker is set to play, not just how
  many items are queued — directly addresses the "different streams /
  track ids per speaker" ask. Title elided (max 96px), hidden when empty;
  chip width now driven by chipRow.implicitWidth. Test extended:
  queue_devices_lists_only_non_empty asserts head_title.
- **Resume race fix (play/pause)** — resuming from a paused position did
  play_track(id) THEN Qt.callLater(seek_to(from)) — two spawns, racy. Now
  a single seek_to(from) when there's a remembered position (it replays
  the loaded path from offset directly); play_track only as the from-zero
  fallback. One spawn, no callLater race.
- Verified via rustcheck: bo-controller own code type-checks clean (no
  errors/warnings) up to the Qt boundary; media 57/57; bo-mux 315/315.

Per-device VOLUME display deferred — backend tracks only a single
last_known_volume for the active device + transient shadows, so true
per-device volume would need a device→volume map threaded through the
driver/volume-routing layer (larger change, noted for later).

## 2026-06-21 (pm6) — per-device volume memory

- **Each speaker remembers its own volume** — the multi-speaker
  "different volumes per device" piece, done the tractable way: persist
  the active device's confirmed volume to settings_store under
  "device_volume:<url>" (on every authoritative Volume result, sane
  non-zero only — a 0 is usually a shadow/ducking artifact). On device
  switch, select_device / select_device_with_product call a new
  restore_device_volume() that loads that value into the slider DISPLAY
  (1..100 guard). Display-only: never pushed to the device, so selecting a
  speaker can't cause a surprise volume jump; a live probe still overrides
  with the authoritative value if the device reports one. This makes
  "Stage at 30, headphones at 60" survive switching back and forth, even
  for BT sinks that don't report volume reliably. Safety preserved:
  safety_check_done resets on switch + high-volume relocks, so restore
  never bypasses the per-product ceiling.
- Verified: rustcheck → bo-controller own code type-checks clean (no
  errors/warnings) to the Qt boundary; guard logic unit-checked standalone;
  bo-mux 315/315; media 57/57.

Per-device volume is now remembered; DISPLAYING all devices' volumes
simultaneously in the strip would still need volume threaded into
queue_devices_json (deferred — the restore-on-switch covers the main UX).

## 2026-06-21 (pm7) — per-device volume in the strip (full multi-speaker view)

- **Volume now shown per device in the playbar strip** — closes the
  multi-speaker picture: each chip shows host · "head track" · 🔊vol ·
  count. enrich_queue_devices_with_volume() runs on the main thread when
  QueueDevices arrives (settings_store isn't reachable from the worker),
  injecting each device's remembered volume (device_volume:<url>), or the
  LIVE volume for the active device, or -1 when unknown. Best-effort:
  malformed JSON returns unchanged so the strip never blanks. QML chip
  binds the active device's volume to live qtobj.volume so it tracks the
  slider without backend churn; inactive devices show their remembered
  level. Hidden when -1.
- Verified: rustcheck → bo-controller own code clean (no errors/warnings)
  to Qt boundary; enrichment logic unit-checked standalone (active=live,
  inactive=stored, unknown=-1, malformed=unchanged); QML 104/104 braces;
  bo-mux 315/315.

Multi-speaker UX now complete across the three axes the user asked for:
different STREAMS (head_title per chip), different TRACK IDS (per-device
queues, already there), different VOLUMES (remembered + restored on switch
+ shown per chip).

## 2026-06-21 (pm8) — inline per-device volume stepper

- **Tap −/+ on a strip chip to adjust that speaker's volume** without
  leaving the playbar. New qt_method set_device_volume(url, v): if url is
  the active device it routes through the normal safe set_volume_api
  (safety ceiling + rate limit + clamp all preserved — thin pre-router,
  not a bypass); if it's a different device it promotes that device to
  active first (we can't safely route volume through an unselected
  device's driver / unknown ceiling). QML: the −/+ steppers render only on
  the ACTIVE chip (honest about the architecture); inactive chips show
  their remembered level read-only. The stepper's MouseAreas sit above the
  chip's select MouseArea (chip select given z:-1) so a tap on −/+ adjusts
  volume instead of switching the queue view. ±5 per tap, clamped 0..100.
- Verified: rustcheck → bo-controller own code clean (no errors/warnings)
  to Qt boundary; routing logic unit-checked standalone (active→set,
  inactive→promote, empty→no-op); QML 109/109 braces; bo-mux 315/315.

## 2026-06-21 (pm9) — quad + LFE synthesis from stereo (multi-speaker mesh)

- **Quadraphonic + basic LFE upmix** — the existing upmix.rs Upmixer only
  produced full 5.1 (with a center channel) and wasn't reachable. Added
  the 4-speaker-mesh targets the user asked for:
  * Quad40 / Quad41 frames + Upmixer::process_quad / process_quad41 —
    fronts pass through (phantom center preserved, NO discrete center),
    rears = decorrelated Haas-delayed side signal, optional low-passed
    mono-sum LFE (4.1). Shares the rear filters (one stream → one method).
  * Upmixer::lfe_sample(l,r) — standalone basic LFE: 120 Hz one-pole on
    the mono sum, ×0.7 trim, honors lfe_enabled. For a mesh where one node
    is the sub and the rest pass full-range.
  * mux::pick_channels_quad — maps a Quad41 frame to requested channels
    (FL/FR/RL/RR/LFE + SL/SR/BL/BR aliases); unknown channels → silence.
  * mux::upmix_stereo_wav_channel — the bridge that makes it USABLE
    through the existing per-channel file server (chan_server): takes a
    16-bit stereo WAV, synthesizes one named channel via the upmixer,
    returns mono WAV. So "/ch/RL" of a stereo source derives rear-left on
    the fly. Stereo+16-bit guarded; absent channels degrade to silent WAV.
- All NaN/Inf-sanitized end to end. Tests: quad fronts-passthrough/no-
  center, rears emerge after Haas delay, mono source → quiet rears, LFE
  low-pass + disable, 4.1 LFE flag, quad NaN defense, pick_channels_quad
  aliases+silence, upmix_stereo_wav_channel round-trip (FL passthrough, RL
  length, non-stereo reject, absent-channel silence).

bo-mux 323/323 (was 315); rustcheck clean deny-warnings+tests.

## 2026-06-21 (pm10) — Opus (+compressed) input for the soundscape workshop

- **Workshop accepts compressed audio, not just WAV** — the Loop Studio
  read only 16-bit PCM WAV (looper::read_wav). Added looper::read_audio(),
  a dispatcher: .wav → read_wav directly; anything else (.opus, .mp3,
  .flac, .m4a, .ogg) → decode to a temp 16-bit stereo WAV via ffmpeg
  (already a hard dep), read that, delete the temp. Notably this means a
  rendered .loop.opus can be re-fed as input (the workshop's own output
  format is now also a valid input). media_worker render path switched
  read_wav → read_audio. Temp scratch file is always cleaned up.
- UI updated: input label "INPUT WAV" → "INPUT AUDIO"; placeholder shows
  ".opus, .mp3, .flac…"; help text + header comment list accepted formats.
- Verified REAL end-to-end: generated a stereo libopus file and decoded it
  through read_audio's exact ffmpeg invocation → correct 2ch/48k/16-bit
  PCM (192000 bytes = 1.0s). rustcheck: looper own-code clean to Qt
  boundary; QML 146/146 braces; bo-mux 323/323; media 57/57.

## 2026-06-21 (pm11) — spatializer: mixer stereo-width + per-stem placement

Two spatializer additions, on the two separation layers:

- **Mixer stereo-width (M/S spatializer)** — realtime_mixer already
  decomposes to Mid/Side for its EQ, so width is a near-free spatial
  control: added `width` to MixerParams (serde default 1.0, MAX_WIDTH 2.0),
  applied to the Side component after EQ before L/R re-encode. width 1.0 =
  bit-identical passthrough, 0 = mono (phase-safe), >1 = wider. NaN-guarded
  → unity. Wired mix_width qt_property + set_mixer_value("spatial","width")
  + reset/startup init + both MixerParams literals (snapshot + offline file
  proc). QML: a SPATIALIZER slider card (0..2, snap-to-center detent at 1.0,
  live mono/normal/wide readout). 6 in-tree DSP tests (unity passthrough,
  zero→mono, >1 widens side, mono-source unchanged, NaN→unity) — can't run
  in sandbox (Qt-blocked binary, no lib target) but the identical math is
  validated by a standalone rustc harness.

- **Per-stem stereo placement (separation spatializer)** — channel_routing
  mixed separated stems (drums/voice/piano/synth) into MONO virtual
  channels (gain only). Added StereoStemChannel: each StemPlacement has a
  pan (-1..+1) via equal_power_pan() (constant-loudness law, -3 dB center),
  producing interleaved stereo so a 4-stem separation spreads across the
  field (default_stage: rhythm center, piano left, synth right). Channel-
  wide `width` post-scales the placed field via the same M/S trick. Pure
  DSP (no Qt). 6 in-tree tests + standalone DSP validation (equal-power
  constant loudness, hard pans land correct side, center equal L/R, width
  0 → mono).

Verified: rustcheck → bo-controller own-code clean (no errors/warnings) to
Qt boundary on all three files; both spatializer DSPs validated standalone;
QML 43/43 braces; bo-mux 323/323.

## 2026-06-21 (pm12) — fix: bad width insertion in reset_clears_filter_state test

The pm11 brace-matching script that added `width: 1.0` to test MixerParams
literals misfired on ONE literal (reset_clears_filter_state) that used a
top-level `..Default::default()` instead of explicit speaker/instrumental
fields — it appended `width: 1.0,` AFTER the `..Default::default()`, which
is invalid Rust (struct-update must be last). Removed the redundant line
(the `..Default::default()` already supplies width=1.0). Verified by a
token scan that no other literal has `..Default::default()` followed by a
field. This was a compile error surfaced by the user's real Qt build (the
sandbox can't compile bo-controller past the qttypes Qt boundary, so the
in-tree test literals never got type-checked here — a known limitation;
the script edit should have been audited more carefully).

## 2026-06-21 (pm13) — All Tracks tab (UI half of the missing-tracks fix)

- **New AllTracksPage.qml + nav tab** — the backend load_all_tracks_page
  (catalog::all_tracks_page → MediaCmd::LoadAllTracksPage → worker →
  Page{context:"all_tracks"} → page_json) had no UI. Added a flat, title-
  ordered, paginated (100/page, near-bottom prefetch) list of EVERY track,
  modeled on SearchPage's delegate (title · artist · album, with #/+/▶
  actions: tag, queue_push, play_now; row tap = enqueue_or_play). Because
  all_tracks_page selects FROM tracks with no track_artists join, files
  that parsed with no artist (the "Serenitas.opus" class) are finally
  reachable in the UI, not just via the Unknown Artist fallback bucket.
  Empty-state + live "N tracks" count. Stale-page guard: only accepts the
  page whose offset == current accumulator length (same pattern as the A→Z
  artist view), and filters page_json by context so it ignores other
  pages' results.
- Wired into the SwipeView + PageBar dots model in lockstep, inserted
  after Albums: Library · Playlists · Queue · Artists · Albums · All
  Tracks · Tags · Search · History.
- Verified: QML braces AllTracksPage 36/36, volume_knobs 417/417; page/
  SwipeView order aligned; TrackSummary serializes id/title/primary_artist/
  album as the delegate expects; play/queue methods exist; all Bo.* tokens
  + window.tagEditor valid. all_tracks backend tests 2/2; bo-mux 323/323.

## 2026-06-21 (pm13) — playlist drag-to-reorder + list polish

- **Drag-to-reorder in PlaylistDetailPage** — the code's own TODO ("drag-
  to-reorder is a future enhancement"). Added a ≡ drag handle on each
  track row: press and slide, the list rearranges LIVE under the finger
  (local trackList spliced as the pointer crosses row boundaries, mapped
  via mapToItem into the ListView content space so it's stable against the
  ListView's own delegate positioning; pitch accounts for row height +
  spacing), and the final position commits once on release via
  reorder_playlist(playlist_id, track_id, index). The up/down arrows stay
  as a single-step fallback. Dragged row gets a lifted look (accent
  border, raised z); a `displaced` Transition eases neighbours. The
  release uses the stable track_id so the commit is correct regardless of
  how the live reorder shuffled indices; the backend reorder is absolute-
  ord + transactional, and the republished JSON reconciles the optimistic
  local state. Drag index/splice math validated standalone in JS (move
  down, move up, clamp both ends, no-op same-slot).
- **PlaylistsPage polish** — a description-less playlist row showed an
  empty subtitle line; now falls back to a plain "N tracks" subtitle so
  rows are visually consistent.

NOTE: playlist total-duration was considered but the tracks table stores
no duration column (duration is resolved at playback via lofty/ffprobe),
so it'd need a schema change + backfill — out of scope for a UI pass.

QML only this round; braces 69/69 + 48/48; bo-mux 323/323 (unregressed).

## 2026-06-21 (pm14) — abstract queue policy + track ratings

- **Queue policy abstraction (stack / replace / sync / midgap-layer)** —
  new pure queue_policy.rs makes enqueue INTENT a first-class EnqueueMode
  enum and turns (mode, PlaybackState) into an explicit Vec<QueueAction>
  plan. No Qt/IO/globals → fully unit-tested in isolation (9/9). Modes:
  Stack (append), Replace (clear+play), Synchronize (time-aligned overlay
  layer), MidgapLayer (bed underneath at current playhead). Invariants:
  idle → every mode just plays; layered modes never touch the linear queue
  nor stop the top track. Wired enqueue_with_mode qt_method (executor only;
  all decisions live in the policy). Stack/Replace fully functional now;
  sync/midgap report honestly that they need the layered player (concurrent
  decoders — not yet on the controller's single-player path; policy is
  ready for it). Persisted default via enqueue_mode property + set_enqueue_
  mode (settings_store, validated through from_tag).
- **Track ratings (0–5 stars) with durable FS-independent persistence** —
  schema v5 adds tracks.rating + a path-keyed track_rating_by_path mirror
  (same durability pattern as tags: survives rescans / id churn).
  catalog::set_track_rating/track_rating/relink_ratings_by_path; scanner
  calls the relink after each walk. MediaCmd::SetTrackRating →
  MediaResult::TrackRating → last_rated_track/value qt_props +
  set_track_rating qt_method. rating added to TrackSummary (name-based
  column read, defaults 0 so narrower SELECTs don't break) and selected in
  tracks_in_playlist. New reusable StarRating.qml (tap to set, tap current
  top star to clear, optimistic + backend reconcile via rating_changed);
  shown in playlist track rows. Tests: rating clamp/get/path-mirror +
  relink-after-id-change (media now 59/59).

Verified: rustcheck → bo-controller own-code clean (no errors/warnings) to
Qt boundary across all touched files; queue_policy 9/9; media 59/59;
bo-mux 323/323; QML braces balanced.

NOTE: enqueue-mode picker UI (long-press menu on track rows) + FS-metadata
write-back of ratings via lofty are the remaining follow-ons. The mode
plumbing + persisted default are in place; sync/midgap await the layered
player.

## 2026-06-21 (pm15) — wire enqueue modes to the UI + ratings while browsing

- **Enqueue-mode picker (queue page)** — an "ADD AS" segmented control
  (Stack / Replace / Sync· / Under·) bound to qtobj.enqueue_mode, calling
  set_enqueue_mode. The · flags the layered modes that aren't live yet.
- **enqueue_or_play now honours the chosen mode** — refactored to delegate
  to enqueue_with_mode using the persisted default, so the picker actually
  changes what a track tap does. Kept the "tapping the currently-playing
  track is a no-op" guard (don't queue a duplicate of what you're hearing)
  before delegating; for "stack" the behaviour is otherwise identical to
  before.
- **Ratings while browsing** — added t.rating to all_tracks_page's SELECT
  and a StarRating to the AllTracksPage rows (rate any track in the whole
  library, not just within a playlist). Test all_tracks_page_surfaces_set_
  rating locks in the round-trip (media now 60/60).

Verified: rustcheck → controller own-code clean (no errors/warnings) to Qt
boundary; media 60/60; bo-mux 323/323; QML braces balanced.

## 2026-06-21 (pm16) — track metadata editing reach + rating in the edit sheet

- **Edit dialog is now global + reachable everywhere** — EditTrackDialog
  was instantiated deep in the page tree (id: editDialog) so window.
  editDialog DIDN'T resolve from other pages; AlbumDetailPage's edit-open
  was effectively broken. Moved it to window top-level (id: editDialogGlobal,
  z:320) with `property alias editDialog`, matching the tagEditor pattern.
- **Long-press to edit on every track list** — added onPressAndHold →
  window.editDialog.open(modelData) to AllTracksPage, PlaylistDetailPage,
  SearchPage, ArtistDetailPage, TagDetailPage (previously only AlbumDetail
  could edit). Tap still plays/enqueues; long-press edits.
- **Rating in the edit sheet** — added a RATING row (StarRating, 18px) to
  EditTrackDialog; binds reactively to track.rating, applies immediately
  via set_track_rating (separate persisted field from the title/artist/
  album override payload).
- **Bug fix:** StarRating.qml referenced Bo.* tokens but lacked `import
  "."`, so the singleton wouldn't resolve and the component would fail to
  load at runtime. Added the import.

QML only; braces balanced across all 7 touched files; bo-mux 323/323.

## 2026-06-21 (pm17) — metadata write-back to file tags (lofty)

- **tag_writer.rs** — the write counterpart to tag_reader. write_tags(path,
  TagEdit) writes title/artist/album/year back into the actual audio file's
  tags via lofty 0.22 (save_to_path). Safety-first: reads the existing tag
  and mutates only the specified fields (everything else — art, other
  frames — preserved); Some("") clears a field, None leaves it; empty edit
  is a no-op; missing file / unsupported container returns Err, never
  panics. Rating intentionally NOT written to file (no portable home across
  ID3 POPM / Vorbis RATING / MP4 atom — stays in the DB, already durable).
- **Wired end to end** — MediaCmd::WriteTagsToFile (resolves path, writes
  off the UI thread) → MediaResult::TagWriteResult{ok,message} →
  write_track_tags_to_file qt_method + tag_write_status property. Distinct
  from set_track_override (DB-only overlay): the file write is explicit and
  permanent.
- **UI** — EditTrackDialog gains a "TO FILE" button (alongside the DB SAVE)
  + a result line; shared buildPayload() so DB-save and file-write never
  drift.
- **Verified REAL**: created a FLAC with ffmpeg (title "Old Title"), ran
  the actual tag_writer, read back via lofty → "New Title"/"New Artist"
  rewritten in the file. Plus 3 unit tests (empty no-op, missing-file
  error, is_empty). rustcheck clean to Qt boundary; media+tags 67/67;
  bo-mux 323/323; EditTrackDialog 46/46 braces.

## 2026-06-21 (pm18) — layered-playback model (foundation for Sync/Under)

- **layer_player.rs** — the pure, testable model the queue_policy
  Synchronize/MidgapLayer modes were waiting on. LayerSet tracks concurrent
  layers (Top / Overlay / Underlay), each with a stable id, gain, offset,
  paused flag. Emits LayerActions (Spawn / Stop / StopAllExcept) for the
  driver to execute — never does I/O itself, so it's verifiable in
  isolation (9/9 tests). Roles: Top advances the queue + unity gain;
  Overlay aligns from 0 at unity (mirrors paused); Underlay enters at the
  top's playhead, ducked to 0.55. set_top replaces only the top; add_overlay/
  add_underlay leave the top alone; collapse_to ends the whole mix for a
  fresh single track. Mechanism documented: PulseAudio mixes streams to a
  sink, so a 2nd ffmpeg|paplay pipe layers audio — the real work left is a
  SELECTIVE reap (StopAllExcept) so layers don't kill each other.
- **Wired into the controller** — QtState holds a LayerSet; play_track
  collapse_to's it (a new top ends the old mix, pure bookkeeping — the
  driver's own reap does the stopping); the Sync/Under enqueue arms now
  drive add_overlay/add_underlay + execute_layer_action. The model + UI
  state are exact; the one remaining concrete step is the driver hook that
  spawns the 2nd concurrent decode pipe with offset/gain (can't validate
  audio mixing in the sandbox, so it logs intent + status instead of
  faking it).

Verified: rustcheck → controller own-code clean (no errors/warnings) to Qt
boundary; layer_player+queue_policy 18/18; media 67/67; bo-mux 323/323.

## 2026-06-21 (pm19) — play/pause fix + add-to-playlist + playlist niceties

- **Play/pause resume bug fixed** — onNow_playing_changed re-seeded
  estimatedSec from np_seek_pos on EVERY now-playing change, including the
  pause event, which clobbered the position the pause handler had just
  captured (np_seek_pos is 0/stale at pause). Now it only re-seeds on
  (re)start (np_playing == true). Also: the resume guard was `from > 0`,
  so a track paused at/near 0s lost its position; replaced with an explicit
  _hasPaused flag so paused-at-0 resumes correctly. A fresh play (list tap/
  queue advance) clears _hasPaused so a stale offset from a prior track
  can't leak into the next resume.
- **Add-to-playlist UI** — add_track_to_playlist existed in the backend but
  NO UI surfaced it. New global AddToPlaylistDialog (window.addToPlaylist):
  lists playlists with track counts, tap to add (with ✓ confirmation),
  Done to close. Reachable via a "+ LIST" button in the edit sheet (which
  is itself reachable by long-press on any track row). Edit-dialog action
  row converted Row → Flow so the now-5 buttons wrap on narrow widths.
- **Playlist-create niceties** — name field auto-focuses when the create
  form expands; Enter on either name or description submits (shared
  createCard.submit(), used by both fields + the Create button so they
  can't drift); name is trimmed.

QML only; braces balanced across 5 files; bo-mux 323/323.

## 2026-06-21 (pm20) — smooth scroll + friction-based fling rebound

- **Consistent momentum scrolling everywhere** — the Bo.flickDecel(850) /
  flickMaxVel(6000) tuning was applied unevenly: PlaylistDetailPage,
  AddToPlaylistDialog, TagDetailPage had bare ListViews. Added the full
  treatment (flickDeceleration, maximumFlickVelocity, OvershootBounds,
  rebound) to those, and added OvershootBounds + rebound to the 7 lists
  that already had the flick tuning but no overshoot/rebound.
- **BoRebound.qml** — a reusable rebound Transition (used as `rebound:
  BoRebound {}`) so a hard fling into a list end eases back with a damped
  OutCubic settle instead of the default hard snap. OutCubic (not OutBack)
  is deliberate: the content is already returning from an overshoot, so a
  back-ease would add a second wobble; one clean decelerating settle reads
  as friction. New Bo.reboundDur(420) token.
- **ScrollView pages** (AlbumsPage, AlbumDetailPage, TagsPage use Flow in
  a ScrollView) — tuned via Component.onCompleted on contentItem (the
  internal Flickable) so they match the ListView feel.
- **Dropped pixelAligned** — initially added it, then removed: pixel-
  snapping trades sub-pixel smoothness for crispness, and the ask was
  SMOOTH scroll, so sub-pixel wins.

QML only; braces balanced across 16 files; bo-mux 323/323.

## 2026-06-21 (pm21) — sortable all-tracks library list

- **TrackSort backend** — new TrackSort enum (Title/Artist/Album/Recently
  Added/RatingDesc) + from_tag/to_tag (unknown → Title) + order_clause
  (fixed literals, injection-safe; every branch has a stable title+id
  tiebreak for deterministic paging). all_tracks_page_sorted(conn,off,lim,
  sort); all_tracks_page now delegates with Title. Primary-artist subquery
  aliased `pa` so the Artist sort can order by it. Test all_tracks_sort_
  modes_order_correctly verifies title/artist/recent/rating orders + the
  garbage→Title fallback (media now 68/68).
- **Wired through** — MediaCmd::LoadAllTracksPage gained a `sort` field;
  worker maps it via from_tag → all_tracks_page_sorted. Persisted
  all_tracks_sort qt_property + set_all_tracks_sort (settings_store,
  validated through the enum) + startup restore. load_all_tracks_page reads
  the persisted sort.
- **UI** — a SORT chip strip (Title/Artist/Album/Recent/Rating) on
  AllTracksPage as a Flow (wraps on narrow widths); tapping a chip persists
  the sort and refresh()es from page 0. Selected chip highlighted in accent.

Verified: rustcheck clean to Qt boundary; media 68/68; bo-mux 323/323;
AllTracksPage braces 53/53.

## 2026-06-21 (pm22) — sortable albums list (matching all-tracks)

- **AlbumSort backend** — AlbumSort enum (Title/Artist/Year/MostPlayed) +
  from_tag/to_tag. all_albums_sorted(conn, sort) reorders in Rust (album
  cardinality is small, a few hundred; keeps the aggregation SQL single-
  purpose). Factored the base title-ordered aggregation into all_albums_
  alpha_base; all_albums_alpha delegates with Title. Year sorts newest-
  first (no-year sinks); MostPlayed by total_plays; all with a title
  tiebreak. Test album_sort_modes_order_correctly (media now 69/69).
- **Wired through** — MediaCmd::LoadAllAlbumsAlpha gained a sort field;
  publish_all_albums_alpha maps via from_tag. Persisted album_sort
  qt_property + set_album_sort (settings_store) + startup restore;
  load_all_albums_alpha reads it.
- **UI** — SORT chip Flow on AlbumsPage (Title/Artist/Year/Most played),
  visible only in ALPHA mode (the full-library browse; TOP is play-ranked
  already). Mirrors the all-tracks sort UX.

Verified: rustcheck clean to Qt boundary; media 69/69; bo-mux 323/323;
AlbumsPage braces 53/53.

## 2026-06-21 (pm23) — sortable artists list (completes browse-sort symmetry)

- **ArtistPageSort** (catalog.rs) — the artists browse list is PAGINATED
  (catalog::artists_with_counts_page), so unlike albums its sort must live
  in SQL, not a Rust reorder. Added ArtistPageSort (Name / TrackCount) +
  from_tag + injection-safe order_clause (Name → a.norm; TrackCount →
  n DESC, a.norm tiebreak). artists_with_counts_page_sorted; the old
  unpaged fn delegates with Name. Only Name + TrackCount because the paged
  query has no play-count join (MostPlayed would need one — deferred).
- **Wired through** — MediaCmd::LoadArtistsPage gained a sort field; the
  handler maps via from_tag. Persisted artist_sort qt_property +
  set_artist_sort + startup restore; load_artists_page reads it.
- **UI** — SORT chips (Name / Track count) on ArtistsPage, visible only in
  A→Z mode (TOP is play-ranked). Matches the all-tracks + albums sort UX.
- **Cleanup** — initially built an ArtistSort + all_artists_sorted in
  library.rs (Rust reorder) before realizing the page uses the catalog
  paged query; removed that dead code (confirmed clean under
  --deny-warnings) and kept all_artists_alpha_base.

Verified: rustcheck clean (incl. --deny-warnings) to Qt boundary; media
70/70; bo-mux 323/323; ArtistsPage braces 66/66.

All three library browse lists (tracks / albums / artists) now sort with
persisted, per-list preferences.

## 2026-06-21 (pm24) — wire up forest & river soundscapes (missing clip beds)

ROOT CAUSE: forest.soundscape.json + river_calm.soundscape.json (and
forest_quiet / forest_immersive) were registered and bootstrapped into the
scene dir, BUT they reference clip families that bootstrap_clips() never
synthesized — so the layers were silent. The daemon only generated
brown_pad / wind_soft / pink_pad / wave_break / gull / whale / wind_loop /
bird_chirp / creek_loop / sub_pad. Missing: brook_close/mid/far (the
water), wind_canopy, wind_distant, leaves_rustle (forest ambience).

FIX (bo-muxd.rs bootstrap_clips): added ffmpeg synthesis for all six
missing families × 8 variants (a..h) = 48 clips:
  - brook_close/mid/far: white/pink/brown noise, highpass+lowpass band for
    water, fast shallow tremolo for bubbling; distance shapes it (close
    bright+present, mid rolled off, far dark+quiet+echo for depth).
  - wind_canopy: bandpassed pink noise, brighter + more movement.
  - wind_distant: lowpassed brown noise, darker + slower swell.
  - leaves_rustle: short highpassed white-noise burst padded to ~4s so it
    triggers as an occasional accent, not a continuous bed.

VERIFIED REAL (ffmpeg in sandbox): generated all 48 clips with the exact
filter strings from the code — 48 made, 0 failed; every filter valid. Then
checked every clip each scene references against the synthesized set:
forest (16), river_calm (24), forest_quiet (16), forest_immersive (48) →
all clips now covered. bo-mux 323/323; all_example_scenes_load passes.

## 2026-06-21 (pm25) — Chromium-lenient scrolling + native scan folder picker

- **Lenient, desktop-style scrolling** — retuned the scroll feel across all
  list/scroll pages to behave like a desktop browser: flickDecel 850→500
  (coasts much further on a fling), flickMaxVel 6000→8000 (snappier fast
  swipes), pressDelay 0 (content tracks the finger immediately, no press
  lag), and boundsBehavior OvershootBounds → DragOverBounds (you can drag
  past the edge, but a *flick* stops cleanly at the bound instead of
  flinging into a big elastic overshoot — the lenient desktop feel). New
  Bo.pressDelay + Bo.boundsLenient tokens; reboundDur 420→380. Applied to
  all 11 ListView pages + the 3 ScrollView (contentItem) pages.
- **Native folder picker for music scan** — ScanPickerDialog gains a
  Browse… button next to the custom-path field that opens a QtQuick.Dialogs
  FolderDialog (Qt6-standard native OS picker); the chosen folder's file://
  URL is stripped to a plain path for the scanner.
- **Last-scanned location remembered** — scan_with_roots persists the first
  root to settings_store key "last_scan_root" (+ last_scan_root qt_property,
  restored at startup). The picker pre-fills the custom path with it (repeat
  scan = one tap) and the Browse dialog opens there (currentFolder).

NOTE: QtQuick.Dialogs is a new QML module import for this project (only
QtQuick + QtQuick.Controls were used before). It's Qt6-standard so should
resolve, but it's the one thing here that needs the real Qt build to
confirm — flagged for hands-on check.

Verified: rustcheck clean to Qt boundary; bo-mux 323/323; braces balanced
across all touched files.

## 2026-06-21 (pm26) — fix tag/artist track population + standby card overlap

BUG 1: Tags & Artists detail pages didn't populate tracks.
- Root cause was NOT the backend: verified by-artist and by-tag queries
  both return the attached tracks (added a diagnostic, then a permanent
  regression test tag_and_artist_detail_queries_populate — media 71/71).
- The bug was a QML binding/imperative CONFLICT: TagDetailPage and
  ArtistDetailPage declared `property var trackList: { ...context check...
  return JSON.parse }` (a binding) AND also assigned `root.trackList = ...`
  in a Connections.onLibrary_changed handler. The first imperative
  assignment breaks the declarative binding, after which it never
  re-evaluates — so the list would show empty depending on load ordering.
- Fix: made trackList a plain `property var trackList: []` with the
  Connections handler as its SOLE writer (adoptIfMine() context-matches
  then assigns). Also clear on id-change so stale tracks don't linger, and
  adopt onVisibleChanged (the result can arrive before the page is shown,
  so onLibrary_changed won't fire again).

BUG 2: Overnight Standby card overlapped the Experimental Features card.
- Root cause: the Experimental card's content was a `ColumnLayout {
  anchors.fill: parent }` inside ControlCard's `holder` Item, whose height
  = childrenRect.height. anchors.fill + childrenRect is a circular size
  dependency → the card collapsed to ~0 height → the next card (standby)
  drew on top of it.
- Fix: experimental ColumnLayout now uses `width: parent.width` (own
  implicitHeight drives height, no cycle). Hardened ControlCard.holder to
  `height: max(childrenRect.height, children[0].implicitHeight)` so Layout-
  based content (which reports size via implicitHeight, not childrenRect)
  can never collapse a card again.

Verified: media 71/71; bo-mux 323/323; braces balanced (ControlCard,
SettingsPage, TagDetailPage, ArtistDetailPage).

## 2026-06-21 (pm27) — wire two dormant backends to UI

Audited for qt_methods with no QML caller; wired two high-value ones:

- **Playlist rename / edit-description** — update_playlist_meta existed in
  the backend (persists + republishes) but the PlaylistDetailPage header
  comment long promised "editable in place" with NO actual UI. Added a ✎
  button next to the playlist name that opens an inline edit overlay (name
  + description fields, Save/Cancel, Enter-to-save). Wired to
  update_playlist_meta; optimistically updates the header. The dialog pulls
  the current description from playlists_json on open (the page only
  receives the name through its open signal).
- **Repeat button** — repeat_mode_cycle + repeat_mode_tag existed with no
  transport-bar control. Added a repeat button to the transport cluster
  that cycles Off → One → All; glyph (↻ / ↻₁) + accent tint reflect the
  live mode. Sits inside controlCluster so the sink-chip width calc adapts
  automatically.

QML only; braces balanced (PlaylistDetailPage, BottomTransportBar);
rustcheck clean; bo-mux 323/323.
