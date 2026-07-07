
Touch & Controller Input
Reference v2
Handheld Mode · Touchpad Gestures · Bending Grammar · Inventory · Voice · Controller Mode (v2) · Volume Buttons · LAN IMU Routing

Version history
  v1 → v2: Added §6 Controller Mode — phone-as-controller over LAN, volume button mapping, gesture strip token UI, IMU sensor routing via iio-sensor-proxy / libiio, splitscreen integration.

v1 sections unchanged. All section numbering preserved. New material in §6 and §7.

1.  Input Mode Overview
The game supports two input modes determined by how the device is being used:

Mode
Device state
Input surface
Handheld
Phone rendering the game (solo play or online)
Full-screen touch: left thumb movement zone, right thumb swipe gesture zone, touch-anywhere voice trigger, tap HUD elements. Volume buttons: media actions.
Controller (v2)
Phone connected as LAN controller to render box
Controller UI: virtual thumbsticks, gesture token strip, ability buttons, volume buttons as game actions, microphone as Choir input, IMU as camera assist and gesture state. Phone screen shows only the controller interface.

Mode detection: the phone automatically enters controller mode when it successfully completes the pairing handshake with an Unendlich render box on the LAN (see Phone Controller document). The game UI transitions from the full-screen render to the controller layout. No manual mode switch required.



  📱  Screen Zones — Handheld Mode (v1 — unchanged)

2.  Handheld Screen Zones
In handheld mode the phone renders the game. The touch surface is divided into functional zones that overlap the game viewport. These zones are always active — no UI chrome is drawn over them. The player touches the appropriate screen region to interact.

2.1  Zone Layout
  Handheld mode — screen zone map
  ┌─────────────────────────────────────────────────────────────────────┐
  │  STATUS BAR (top 5%): Chi bar, element icon, health, stamina         │
  ├─────────────────────────────────────────────────────────────────────┤
  │                                                                       │
  │                    GAME VIEWPORT (85% of screen)                      │
  │                    Touch-anywhere: voice trigger hold                 │
  │                                                                       │
  │   LEFT THUMB              │              RIGHT THUMB                 │
  │   (lower-left 35%)        │              (lower-right 35%)           │
  │   Single touch: movement  │              Swipe: gesture trace        │
  │   Floating anchor stick   │              Tap: technique modifier     │
  │                           │                                           │
  ├───────────────────────────┴───────────────────────────────────────────┤
  │  ABILITY BAR (bottom 10%): 5 ability slots, touchable                 │
  └─────────────────────────────────────────────────────────────────────┘

2.2  Left Thumb Zone — Movement
Floating anchor: stick origin placed at first touch contact. Does not need to be in a fixed position.
Dead zone: 8% of zone radius. Contacts inside dead zone produce no movement.
The movement vector is fed to the character controller. When actively bending (mid-technique), the left thumb redirects the bending output direction rather than the character's walk direction.
Double-tap: crouch / stance change (context-sensitive).

2.3  Right Thumb Zone — Gesture Trace
In handheld mode, the right thumb zone accepts gesture path traces — the player physically draws the gesture shape on the screen surface. The gesture recogniser compares the drawn path against the registered vocabulary.

Single swipe: directional technique modifier (direction of swipe = technique direction).
Two-finger hold: summons the gesture grammar radial menu (select from OPEN/CLOSE/HOLD/CHARGE/RELEASE/SWEEP by radial position, same tokens as the controller's dedicated strip).
Circular trace: activates the super technique if chi is sufficient.
Pinch: zoom in / out on the map when the map overlay is active.

2.4  Voice Trigger — Touch-Anywhere
In handheld mode, any three-finger contact anywhere on the game viewport activates the voice trigger. The player speaks a Choir word or ability alias while three fingers are held. The Choir recognition system processes the audio from the device's built-in microphone (no network streaming in solo handheld mode — local processing only).

Three-finger hold: voice active (microphone listening, blue ring visible on all three contact points).
Three-finger release: voice commit (Choir processes the last spoken word against the vocabulary).
Voice trigger timeout: 4 seconds of silence after activation auto-commits with no word detected.


  ✨  Bending Gestures — Trace Vocabulary (v1 — unchanged)

3.  Bending Gesture Traces — Handheld Mode
The right thumb zone in handheld mode recognises gesture traces. These are classified against the registered gesture vocabulary from the Gaze Rays & Voice Echo document. The trace classifier runs locally on the device.

3.1  Earth Voice Shapes — Touch Trace Equivalents
Voice shape name
Touch trace
Bending technique triggered
RISE
Vertical upward swipe (fast)
Pillar rise. Speed of swipe scales pillar height.
WALL
Horizontal swipe (either direction)
Stone wall — orientation matches swipe direction.
ARCH
Curved upward swipe, arc path
Formation arch — requires natural arch formation context.
HOLD
Hold fingertip still (> 0.4s)
Sustained seismic anchor at targeted point.
SHATTER
Rapid spread-outward from centre
Seismic pulse — radius scales with spread speed.
PULL
Swipe toward body (inward)
Stone drag — pulls target terrain toward the player.

3.2  Water Voice Shapes — Touch Trace Equivalents
Voice shape name
Touch trace
Bending technique triggered
SURGE
Fast outward horizontal swipe
Tidal Surge — wave direction matches swipe.
FLOOD
Spreading expand gesture (all fingers)
Flood technique — water spreads to cover area.
VEIL
Slow upward swipe
Water Veil — protective mist field erects upward.
FREEZE
Downward swipe + hold
Glacial form — freezes water at targeted position.
BRIDGE
Long horizontal swipe (full zone width)
Water Bridge — horizontal span of water.
ANCHOR
Tap + hold at formation context location
Tidal Anchor — formation context required.


  📦  Inventory & Menu Gestures (v1 — unchanged)

4.  Inventory and Menu Touch Gestures
4.1  Inventory Access
Two-finger swipe DOWN from status bar: opens inventory grid. Same as the INVENTORY tab in the character panel.
Two-finger swipe UP: closes inventory and returns to play.
In inventory: single-finger drag to move items. Two-finger pinch on item: inspect (opens tooltip with genome overlay if Information bending active).

4.2  Map and Mini-Map
Two-finger pinch on mini-map (status bar): expands mini-map to full screen. Pinch again to collapse.
On expanded map: drag to pan, pinch to zoom. Tap to place a waypoint. Tap formation name label to show the Formation Dictionary entry.

4.3  Character Panel Navigation
Three-finger swipe LEFT: opens character panel (tabs: Discipline, Skills, Bindings, Equipment, Stance, Voice).
Three-finger swipe RIGHT: closes character panel.
In character panel: single-finger swipe LEFT/RIGHT to navigate tabs. Tap node: select ability. Long-press node: open ability detail.


  🎤  Voice Integration (v1 — unchanged)

5.  Voice Integration — Handheld Mode
5.1  Handheld Voice Architecture
In handheld mode, all voice recognition runs locally on the device. The Choir recognition pipeline processes audio from the built-in microphone without network involvement. This differs from controller mode where voice is streamed to the render box.

Recognition latency target: < 60ms from word completion to technique trigger (local processing, no network hop).
Ambient noise handling: the device's built-in noise suppression (VOICE_COMMUNICATION audio source on Android) is used. In high-noise environments (e.g., near a cave waterfall POI where the game's audio output includes broadband noise through the speaker), push-to-talk (three-finger hold) is recommended over always-on mode.
Voice and gesture are independent: either can trigger a technique. If the player builds a gesture sequence AND speaks the matching trigger word simultaneously, the word fires first (word recognition completes faster than sequence commit).



  NEW IN v2
  🎮  Controller Mode — Phone as LAN Controller (v2)

6.  Controller Mode — v2 Addition
Controller mode activates when the phone successfully pairs with a render box on the LAN. The phone's game render stops. The controller interface replaces it. All input is forwarded to the render box over the network. The render box presents the game on its display in splitscreen if multiple phones are connected.

Connection:  The phone app discovers the render box via mDNS service '_unendlich._tcp.local'. No IP entry required. Tap the server name in the discovery list to pair. Full technical specification in the Phone Controller document.

6.1  How the Screen Layout Changes
Every v1 zone concept has a controller mode equivalent. The visual presentation changes; the conceptual function is preserved:

v1 handheld zone
v2 controller equivalent
What changed
Left thumb zone — movement float stick
Left thumb zone — movement float stick
Identical behaviour. Floating anchor, same dead zone spec. No change.
Right thumb zone — gesture trace
Right thumb zone — camera look stick
In controller mode the right zone is a look/camera stick (no gesture tracing). Gesture is handled by the dedicated gesture strip.
Gesture trace (drawn path)
Gesture token strip (tap buttons)
Gesture grammar tokens (OPEN/CLOSE/HOLD/CHARGE/RELEASE/SWEEP) are now discrete tap buttons on a horizontal strip. No path tracing. Faster and more reliable when the phone is not the game display.
Three-finger hold — voice trigger
Voice Hold button + always-on stream
Voice streams continuously to the render box. A dedicated Voice Hold button mutes/unmutes. Three-finger anywhere still works as a fallback but the dedicated button is preferred.
Ability bar (bottom 10%)
Ability buttons (right action zone)
5 ability slot buttons in the right action zone. Larger tap targets than the handheld HUD bar. Cooldown arc display on each button.
Volume buttons — media volume
Volume buttons — game actions
In controller mode, volume buttons are intercepted before OS audio handling. Vol UP = evasive dash. Vol DOWN = interact. Combined = pause. (See §7.)
HUD (top 5%)
HUD strip — minimal info only
Chi bar, element icon, mini-map, ability cooldown pips, player slot colour. Does not include full game HUD elements — those are on the render box display.

6.2  Controller Mode Gesture Strip
The gesture strip replaces the path-trace gesture system. Six token buttons in a horizontal row allow the player to build a gesture grammar sequence by tapping:

  Gesture strip — token buttons and sequencing rules
  TOKENS (left to right): [OPEN] [CLOSE] [HOLD] [CHARGE] [RELEASE] [SWEEP]

  BUILDING A SEQUENCE:
    Tap tokens in order. Each tap adds the token to the sequence.
    A readout strip below the token buttons shows the current sequence.
    Maximum sequence length: 4 tokens.

  COMMIT:
    Hold the last-tapped token button for > 0.3s → sequence fires.
    OR speak the voice trigger word for the desired technique.
    OR tap the ability slot button for a pre-bound sequence.

  CLEAR:
    Two-finger swipe upward across the strip → sequence clears.
    Shake the phone (accel magnitude > 25 m/s²) → sequence clears.

  DIRECTIONAL MODIFIER (SWEEP token):
    When SWEEP is tapped, the direction is taken from the current left stick position.
    Left stick at rest: SWEEP defaults to forward (character facing direction).

  SUPER TECHNIQUE:
    A large button, centre of the strip, always visible.
    Single tap: fires super if chi pool is full.
    Long press (0.5s): shows cooldown status and estimated recharge time.

  FORMATION CONTEXT TOKENS (unlocked via skill tree):
    Formation-context techniques appear as additional token buttons that slide
    into view from the right side of the strip when the player is within a
    recognised formation context (e.g., near a hoodoo site: RISE (hoodoo) appears).

6.3  Gyroscope-Assisted Camera Look
When connected in controller mode, the phone's IMU data is routed to the render box via iio-sensor-proxy / libiio. The gyroscope quaternion delta is applied to the camera as a fine-adjustment layer on top of the right thumbstick input:

Right thumbstick: coarse camera rotation (large sweeping turns, standard gamepad behaviour).
Phone tilt (gyroscope): fine camera adjustment (aiming precision, slow scan). Sensitivity: adjustable in settings. Default: gyro contributes 25% of total camera delta; stick contributes 75%.
The phone does not need to be held in a specific orientation — calibration is performed at session start (or manually via HUD strip centre long-press) and the resting orientation is the zero-point.
Gyro assist can be disabled in settings for players who prefer stick-only camera control.

6.4  ROOTED Gesture State via IMU
When the phone is held approximately level (accelerometer z-axis close to −g, x and y axes < 0.5 m/s²), the ROOTED gesture state is automatically flagged. This provides the same 'grounded contact with earth' requirement that some Earth bending techniques need, without requiring the player to explicitly tap a ROOTED token:

Phone held level (tabletop position): ROOTED state active. Earth techniques with GROUNDED prerequisite: automatically met.
Phone tilted beyond 25° from level: ROOTED state clears. The player must tap the ROOTED token if the technique requires it.
This creates a natural physical correlation: a player who holds their phone flat (as if touching a surface) naturally satisfies the GROUNDED condition.


  🔊  Volume Buttons — Controller Mode Mapping (v2)

7.  Volume Buttons — Controller Mode
Volume button intercept requires the app to hold audio focus (Android: AudioManager; iOS: MPRemoteCommandCenter). In controller mode, the buttons are always intercepted — the phone plays no game audio (audio is on the render box), so no media volume adjustment conflict occurs.

Button + gesture
Game function
Detail
Vol UP — single tap
Evasive dash
Direction from left stick at tap moment. Air sub-discipline: automatic Evasive Gust (4-block). All others: momentum dodge.
Vol UP — double tap (< 400ms)
Sprint toggle
Holds sprint state. Second double-tap deactivates. Useful when thumbs are busy with gesture building.
Vol UP — hold (≥ 1000ms)
Guard / block
Earth: Stone Wall forward. Water: Tidal Veil. Air: Wind Wall. Storm: ground arc. Maintains while held.
Vol DOWN — single tap
Interact
Context-sensitive: pick up, open, activate forge, read inscription (Information Bending Temporal Echo on focused object).
Vol DOWN — double tap
Crouch toggle
Crouch: speed −50%, detection range −40%. Earth benders prone: passive Seismic Read active.
Vol DOWN — hold (≥ 1000ms)
Seismic Read pulse
Active while held. Earth benders: full range + entity detection. Others: terrain-only, limited range.
Vol UP + Vol DOWN simultaneously
Pause / menu
Opens in-game menu on the phone screen. Other players continue playing uninterrupted.


  📐  IMU Sensor Input — Quick Reference (v2)

8.  IMU Sensor Input Reference
IMU data routes from the phone to the render box via libiio network backend (iiod protocol). Full technical specification in the Phone Controller document. This section summarises the gameplay input mappings for reference.

Sensor
Input mapping
Gameplay use
Rotation quaternion (100 Hz)
Camera look delta
Gyro-assisted fine camera control. 25% camera contribution by default.
Accelerometer — level hold
ROOTED state flag
Phone held level → ROOTED prerequisite auto-satisfied for Earth techniques.
Accelerometer — spike (> 25 m/s²)
Shake event
Single shake: gesture sequence clear. Double shake: technique interrupt.
Gyroscope Z-axis rapid rotation
PIVOT token
Rapid Z rotation (> 4 rad/s) fires PIVOT gesture token. Air/Storm stance transitions.
Tilt forward (pitch > 30°, sustained)
Dragon approach command
Phone tilted forward: tamed dragon receives approach command (if in range).



End of Touch & Controller Input Reference v2. Sections 1–5 are unchanged from v1. Sections 6–8 are new additions in v2: controller mode (§6), volume button controller mapping (§7), and IMU sensor quick reference (§8). Full architecture specification for the phone-as-controller system is in the companion Phone Controller document.
