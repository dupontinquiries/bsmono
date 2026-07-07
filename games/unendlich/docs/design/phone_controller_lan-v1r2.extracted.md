
Phone Controller
LAN-Connected Device as Gesture Interface — v2
Network Architecture · DHT Join (v2) · Adaptive Gesture Zones (v2) · Touchscreen Zones · Volume Buttons · Voice Microphone · iio-sensor-proxy / libiio · Local Splitscreen · Virtual Controller Driver
  v1 → v2:  Added §2b DHT Join Method (Kademlia session discovery, cross-subnet, survives IP changes). Added §3b Adaptive Gesture Zones (introductory bounding boxes that learn touch positions via EMA and become sticky after 20 touches per zone).
The phone does not render the game. The render box renders everything. The phone is a pure input surface: a touchscreen controller, an iio sensor stream, a voice microphone, and two hardware buttons. All input arrives at the render box over LAN and enters the game's input system as a standard virtual controller.

1.  System Overview
The phone-as-controller system allows any LAN-connected phone to act as a game controller for a session running on a dedicated render box (Linux desktop or laptop). The phone screen shows only the controller interface — the game viewport, audio, and all visual output are on the render box's display. Multiple phones can connect simultaneously for local splitscreen.

Component
Role
Render box
Runs the game. Receives all input streams from connected phones. Renders one viewport per phone in splitscreen. Handles all Choir voice recognition and sensor fusion.
Phone (Android / Linux)
Pure input device. Shows the controller UI. Streams: touch events, IMU sensor data via iiod protocol, Opus-encoded voice PCM, hardware button events. Never renders game content.
LAN
Local network (Wi-Fi or USB tethering). Discovery via mDNS. WebSocket transport. ~12 kB/s per phone. Works on any 2.4 GHz Wi-Fi or better; USB tethering preferred for competitive latency.
unendlich-bridge daemon
Service running on the render box. Handles phone discovery, pairing, and stream demultiplexing. Creates one virtual controller device (uinput) per phone. Routes IMU data to iio-sensor-proxy. Routes voice PCM to the Choir recognition pipeline.



  📡  Network Architecture — Discovery, Pairing, and Streaming

2.  Network Architecture
2.1  Service Discovery — mDNS
The render box advertises itself via mDNS (Avahi on Linux) as a DNS-SD service. The phone app scans for this service and presents discovered servers in a pairing list. No manual IP entry required on the same LAN segment.

  Avahi mDNS service definition — /etc/avahi/services/unendlich.service
# Render box: Avahi service definition
# /etc/avahi/services/unendlich.service

<service-group>
  <name replace-wildcards='yes'>Unendlich Game — %h</name>
  <service>
    <type>_unendlich._tcp</type>
    <port>47832</port>
    <txt-record>version=2</txt-record>
    <txt-record>players=2/4</txt-record>  <!-- current/max slots -->
    <txt-record>session=active</txt-record>
  </service>
</service-group>

The phone app uses the platform's mDNS resolver (Android: NsdManager; iOS: NSNetServiceBrowser) to discover this service. On discovery, the app displays the server hostname, current player count, and session status. The user taps to initiate pairing.

2.2  Pairing Handshake
  WebSocket upgrade sequence on port 47832
  CLIENT → SERVER:  HTTP GET /pair
    Headers:
      Upgrade: websocket
      X-Device-Name: [phone model]
      X-Device-Capabilities: touch,imu,voice,buttons
      X-IMU-Rate: 100        (Hz — server may negotiate down)
      X-Voice-Sample-Rate: 48000

  SERVER → CLIENT:  101 Switching Protocols
    Headers:
      X-Player-Slot: 2       (1–4: assigned player slot)
      X-IMU-Rate-Accepted: 100
      X-Voice-Codec: opus
      X-Session-Token: [hex32]  (used for reconnect on network drop)

  After upgrade: bidirectional binary WebSocket frames.
  Server sends heartbeat (0x00) every 500ms.
  Client must respond within 1000ms or connection is closed (player disconnected).
  On reconnect with valid session token: player slot is held for 30 seconds.

2.3  Multiplexed Stream Protocol
All data travels in binary WebSocket frames. Each frame has a 6-byte header identifying the stream type and timestamp, followed by a variable-length payload. A single frame can contain data from only one stream type.

  Binary frame protocol — all streams multiplexed over one WebSocket
// Frame header — 6 bytes
struct FrameHeader {
    u8  stream_type;    // 0x01=touch 0x02=imu 0x03=voice 0x04=button 0x05=heartbeat
    u8  flags;          // 0x01=final_fragment 0x02=reliable
    u32 timestamp_ms;   // monotonic ms since session start (little-endian)
}

// IMU payload — 24 bytes per frame (one quaternion + linear acceleration)
struct ImuPayload {
    f32 qw, qx, qy, qz;     // orientation quaternion (phone local frame)
    f32 ax, ay;              // linear acceleration X,Y (m/s² in phone frame)
    // az omitted — vertical axis not used for gameplay
}

// Touch payload — variable, 8 bytes per active contact
struct TouchContact {
    u8  id;               // 0–9 finger ID
    u8  action;           // 0=down 1=move 2=up
    u16 x;                // normalised 0–65535 across screen width
    u16 y;                // normalised 0–65535 across screen height
    u16 pressure;         // 0–65535 (scaled from platform touch pressure)
}

// Button payload — 2 bytes
struct ButtonPayload {
    u8  button_id;    // 0x01=vol_up 0x02=vol_down 0x03=power_short (reserved)
    u8  action;       // 0=pressed 1=released
}

// Voice payload — Opus packets, 20ms frames, 48kHz mono
// Variable length. Silence suppression: no frame sent when dBFS < −45.

2.4  Transport Options and Latency
Transport
Typical RTT
Notes and use case
USB tethering (rndis / NCM)
0.5–2ms
Best latency. Phone appears as a USB network interface. Render box routes traffic via the USB interface IP. Recommended for competitive play. Charges phone simultaneously.
Wi-Fi 5 GHz (same AP)
2–8ms
Good for casual play. Subject to interference and AP scheduling. Use 5 GHz band to avoid 2.4 GHz congestion from other devices.
Wi-Fi 2.4 GHz
5–25ms
Acceptable for non-competitive. High variance. Voice recognition latency may be perceptible.
Wi-Fi Direct / P2P
1–5ms
Phone-to-box without an AP. Android supports Wi-Fi Direct. Eliminates AP scheduling jitter. Requires bridge configuration on the render box.


  🔗  DHT Join — Decentralised Session Discovery

2b.  DHT Join Method
mDNS (§2.1) requires the phone and render box to be on the same broadcast domain — a second router, VPN tunnel, or VLAN breaks it. The DHT join method supplements mDNS with a lightweight Kademlia distributed hash table so phones discover render boxes by stable node ID rather than IP address. DHT discovery survives IP changes, subnet boundaries, and multiple concurrent sessions on the same network.

2b.1  Node Identity — Stable 160-Bit IDs
Every participant (render box and phone) generates a stable 160-bit node ID once and persists it across sessions. The ID is derived from device-stable data and is not tied to IP address:

  Node ID generation — render box (Rust) and phone (Kotlin)
// Render box: stable node ID from machine-id + game app identifier
fn render_box_node_id() -> [u8; 20] {
    let machine_id = fs::read_to_string("/etc/machine-id").unwrap();
    let input = format!("unendlich-renderbox:{}", machine_id.trim());
    sha1::Sha1::from(input).digest().bytes()
}

// Phone: stable ID from Android Settings.Secure.ANDROID_ID
// or iOS: UIDevice.identifierForVendor UUID → SHA1
fun phoneNodeId(): ByteArray {
    val androidId = Settings.Secure.getString(contentResolver, Settings.Secure.ANDROID_ID)
    return sha1("unendlich-phone:$androidId")
}

// Session announcement key: what render boxes publish, phones look up
// KEY = SHA1("unendlich-session:" + session_id)
// session_id: a 6-char alphanumeric code shown on the render box display
// Value stored at this key: render box's current IP:port + player_slots
fn session_key(session_id: &str) -> [u8; 20] {
    sha1::Sha1::from(format!("unendlich-session:{}", session_id)).digest().bytes()
}

2b.2  Bootstrap and Discovery Sequence
The phone tries four discovery methods in sequence, short-circuiting at the first success. Each subsequent method has higher latency but wider reach:

  Discovery sequence — ordered by speed
  STEP 1 — mDNS (same broadcast domain, < 200ms):
    Query _unendlich._tcp.local. If any response arrives within 200ms, use it.
    Works: same subnet. Fails: VLAN, second router, VPN.

  STEP 2 — Cached addresses (LAN cache, < 50ms):
    Try the last 4 IP:port pairs from successful previous sessions.
    Send a probe WebSocket ping to each; first to respond wins.
    Works: render box IP is stable. Fails: DHCP reassigned the IP.

  STEP 3 — DHT lookup (cross-subnet, 500ms–5s):
    1. Phone contacts bootstrap nodes (see §2b.3).
    2. Phone sends FIND_VALUE for session_key(session_id) to closest known node.
    3. Iterative Kademlia lookup converges on the render box's node.
    4. Render box's current IP:port is the stored value.
    5. Direct WebSocket connection proceeds as normal.
    Works: different subnet, IP changed, multiple sessions. Fails: no bootstrap reachable.

  STEP 4 — QR code / manual entry:
    Render box displays a QR code encoding: ip:port + session_id + node_id.
    Phone scans it. Direct connection bypasses all discovery.
    Works: always. Requires: phone camera + user action.

  SHORT-CIRCUIT: as soon as one step succeeds, remaining steps are cancelled.
  The phone UI shows which discovery method succeeded (small icon in pairing dialog).

2b.3  Bootstrap Nodes
Primary bootstrap: any currently-connected phone or render box on the same network acts as a bootstrap peer (all participants run a minimal DHT node). A phone that successfully joined a session can bootstrap subsequent phones.
LAN bootstrap cache: when any discovery succeeds, the peer's node ID and address are cached. On next session, the cache entries are tried as bootstrap nodes before attempting the full discovery sequence.
Optional relay: the game can be configured with a known relay node address (e.g., a small server on a home server, Pi, or cloud VPS). The relay stores DHT announcements from render boxes and serves lookups. It does not relay gameplay traffic — only discovery metadata. Configuration: `~/.config/unendlich/relay.conf → relay_host = my-server.example.com:4731`.
Session ID display: the render box always shows the 6-char session ID on screen (element-coloured at top-left of the game display). The phone app has a session ID input field as the fallback — the user types the code from the TV screen, and the app looks up the DHT value for that key.

2b.4  Render Box DHT Announcement
  Rust — render box DHT session announcement via libp2p Kademlia
// Render box: announce session to DHT, keep announcement fresh
use libp2p::kad::{Kademlia, KademliaConfig, Record, Key};

async fn announce_session(session_id: &str, listen_port: u16) {
    let key   = Key::new(&session_key(session_id));
    let value = serde_json::to_vec(&SessionAnnouncement {
        ip:            local_lan_ip(),         // current LAN IP
        port:          listen_port,             // WebSocket port (47832)
        node_id:       render_box_node_id(),
        player_slots:  [false; 4],              // availability
        session_name:  hostname(),
    }).unwrap();

    // PUT to DHT — record TTL 120 seconds, re-published every 60s
    let record = Record { key, value, publisher: None, expires: ttl(120) };
    kademlia.put_record(record, Quorum::One).await.unwrap();

    // Re-announce on IP change (DHCP renewal etc.)
    network_change_listener().for_each(|_| announce_session(session_id, listen_port));
}

// k-bucket parameters for a small LAN DHT (4 players, < 20 devices)
let config = KademliaConfig::default()
    .set_replication_factor(NonZeroUsize::new(3).unwrap())  // k=3 for small nets
    .set_record_ttl(Duration::from_secs(120));




  📱  Phone UI — Controller Screen Layout

3.  Phone Controller Screen Layout
In controller mode the phone screen is divided into functional zones. The game is not rendered on the phone. The phone displays the controller interface only — a dark minimal layout that does not distract from the game visible on the render box display.

3.1  Zone Layout — Portrait or Landscape
  Portrait layout (phone held vertically)
  ┌───────────────────────────────────────────────────┐
  │  PLAYER HUD STRIP (top 8%)                        │
  │  Chi bar · Element icon · Mini-map · Player slot  │
  ├────────────────────────┬──────────────────────────┤
  │ LEFT THUMB (30%)       │ RIGHT THUMB (30%)        │
  │                        │                          │
  │  [Virtual thumbstick]  │  [Camera / look stick]   │
  │  Movement: WASD        │  Camera yaw + pitch      │
  │                        │                          │
  │                        │  Gyro assist: ON by def. │
  │  (floating zone —      │  (phone tilt = fine aim) │
  │   anchor where finger  │                          │
  │   first touches)       │                          │
  ├────────────────────────┴──────────────────────────┤
  │  BENDING GESTURE STRIP (centre 15%)               │
  │  [OPEN][CLOSE][HOLD][CHARGE][RELEASE][SWEEP]      │
  │  6 token buttons — tap to add to gesture sequence │
  │  Hold last token: commit sequence                 │
  │  Two-finger swipe up on strip: clear sequence     │
  ├────────────────────────┬──────────────────────────┤
  │ LEFT ACTIONS (20%)     │ RIGHT ACTIONS (20%)      │
  │                        │                          │
  │  [Ability 1]           │  [Ability 3] [Ability 4] │
  │  [Ability 2]           │  [Ability 5]             │
  │  [Super]               │  [Voice hold]            │
  ├────────────────────────┴──────────────────────────┤
  │  BOTTOM STRIP (5%) — context actions              │
  │  [Inventory] [Map] [Menu] [Interact]              │
  └───────────────────────────────────────────────────┘

  Vol UP button: Evasive dash (always accessible regardless of screen touch)
  Vol DOWN button: Use / interact with held item

  Landscape layout (phone held horizontally — recommended)
  ┌────────────────────────────────────────────────────────────────────────┐
  │ PLAYER HUD (top 6%): Chi bar · Element · Mini-map · Ability cooldowns  │
  ├────────────────┬────────────────────────────┬──────────────────────────┤
  │ LEFT THUMB     │   GESTURE STRIP (centre)   │ RIGHT THUMB              │
  │ (25% width)    │   (50% width, 40% height)  │ (25% width)              │
  │                │                            │                          │
  │ [Movement]     │ [OPEN][CLOSE][HOLD]        │ [Look/Camera]            │
  │ floating stick │ [CHARGE][RELEASE][SWEEP]   │ floating stick           │
  │                │                            │ + gyro assist tilt       │
  │                │   Super: large centre btn  │                          │
  │                │                            │                          │
  ├────────────────┤   [Ability 1–5 row below]  ├──────────────────────────┤
  │ L2: Sprint/    │                            │ R2: Bending technique    │
  │ dodge (hold)   │                            │ context modifier         │
  │ L3: Crouch     │ [Inventory] [Menu] [Voice] │ R3: Lock-on / Seismic   │
  └────────────────┴────────────────────────────┴──────────────────────────┘

  Landscape is preferred. The gesture strip is wider, making token sequences easier.
  L2/R2 are the lower edges of the phone — held with index fingers when horizontal.

3.2  HUD Strip — What the Phone Shows Instead of the Game
The phone HUD strip is a minimal overlay at the top of the controller screen. It shows the information most immediately relevant to the player's decisions — not the full game HUD (which is on the render box display) but the subset the player needs to look at without looking up at the screen:

Chi bar: current chi fill (0–100%). Fills are visible as a horizontal bar in the element's accent colour. Depleted chi pulses the bar amber.
Element icon: primary discipline icon (the same glyph from the character panel). Changes when sub-discipline active.
Mini-map: a 48×48px radar showing the player's immediate 60-block radius. Entities shown as dots. No terrain detail — just relative positions.
Ability cooldowns: five small circular pips, one per ability slot, showing remaining cooldown fraction. Visually consistent with the full ability bar on the render box.
Player slot indicator: the corner shows the player's assigned colour (P1 blue, P2 amber, P3 green, P4 red). Used to identify one's own splitscreen viewport on the big screen.


  📐  Adaptive Gesture Zones — Introductory Boxes That Learn and Stick

3b.  Adaptive Gesture Zones
Every gesture zone on the controller screen starts large and shrinks to the user's actual touch patterns over time. A first-time player sees clearly labelled bounding boxes showing exactly where to place each thumb. As they play, the zones track where touches actually land, shift their centre to match the user's natural grip, and compact their radius to the 95th-percentile envelope of real contacts. After 20 touches per zone, adaptation slows dramatically — the zone is sticky: it holds its learned position and resists drift unless a clearly new consistent placement emerges.

3b.1  Zone Lifecycle — Four Phases
Phase
Touch count
Zone behaviour and visual state
Introduction
0 touches
Large dashed bounding box, element-coloured, corner handles animated with gentle pulse. Label centred in the zone: 'MOVE', 'LOOK', 'GESTURES', 'ABILITIES'. Helper arrow or glyph matching the action. Zone covers 55–65% of its designated screen region — deliberately generous.
Learning
1–9 touches
Box is solid outline but fading (opacity decreases from 100% to 40%). Zone centroid visibly shifts toward accumulated touch positions. Bounding box physically shrinks each touch. Small text fades in briefly: 'calibrating 4/10'. After each touch, the box slides and contracts with a 200ms eased animation.
Settling
10–20 touches
Box is a ghost outline (opacity 15%). Still contracting slowly. Label gone. The zone feels like it's disappearing — which is correct. User's natural grip is now the control surface. Adaptation rate alpha still at 0.08: still responsive to new data.
Sticky
20+ touches
Box invisible during normal play. Adaptation rate drops to alpha = 0.02: very slow drift allowed, strong resistance to change. Zone holds learned position through session. Stored in user profile and restored next session. Out-of-zone touch: brief ghost flash of the box boundary (100ms, not punitive).

3b.2  Adaptation Mathematics
The calibration model uses an exponential moving average (EMA) for the zone centroid and a running percentile estimate for the bounding radius. Both update on every touch contact:

  Rust — ZoneCalibration: EMA centroid, P95 radius, alpha decay, guide opacity
// Per-zone calibration state — persisted in user profile
pub struct ZoneCalibration {
    pub zone_id:       ZoneId,
    pub centroid:      Vec2,       // EMA of touch positions (starts at zone hint centre)
    pub radius_p95:    f32,        // running 95th-percentile distance from centroid
    pub touch_count:   u32,        // cumulative touches recorded
    pub alpha:         f32,        // current EMA weight (decays over time)
    pub sticky:        bool,       // alpha has dropped below sticky threshold
    pub initial_size:  Vec2,       // original introductory box dimensions
    pub distances:     VecDeque<f32>, // ring buffer of last 40 centroid distances
}

impl ZoneCalibration {
    pub fn record_touch(&mut self, pos: Vec2) {
        self.touch_count += 1;

        // ── Alpha decay schedule ──────────────────────────────────────
        // Fast learning at start (alpha=0.15), slows at 20 touches,
        // bottoms out at 0.02 (sticky — very slow drift allowed).
        self.alpha = if self.touch_count < 20 {
            0.15 - (self.touch_count as f32 / 20.0) * 0.13  // 0.15 → 0.02
        } else {
            0.02  // sticky floor
        };
        if self.touch_count == 20 { self.sticky = true; }

        // ── Centroid EMA update ───────────────────────────────────────
        self.centroid = self.centroid.lerp(pos, self.alpha);

        // ── Radius percentile update ──────────────────────────────────
        let dist = pos.distance(self.centroid);
        self.distances.push_back(dist);
        if self.distances.len() > 40 { self.distances.pop_front(); }
        // P95: sort the ring buffer, take index (len * 0.95).ceil()
        let mut sorted = self.distances.iter().copied().collect::<Vec<_>>();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p95_idx = ((sorted.len() as f32 * 0.95).ceil() as usize).min(sorted.len()-1);
        // Ensure minimum zone radius (never smaller than 48px — thumb minimum)
        self.radius_p95 = sorted[p95_idx].max(48.0);
    }

    /// Current display box: centred on calibrated centroid, sized to P95 radius.
    /// At touch_count=0: returns initial_size hint. Transitions smoothly.
    pub fn display_rect(&self) -> Rect {
        let lerp_t   = (self.touch_count as f32 / 20.0).min(1.0);
        let hint_rad = self.initial_size.min_element() * 0.5;
        let radius   = lerp(hint_rad, self.radius_p95, lerp_t);
        Rect::from_centre(self.centroid, Vec2::splat(radius * 2.0))
    }

    /// Opacity of the guide box: 1.0 at 0 touches → 0.0 at 20+ touches.
    pub fn guide_opacity(&self) -> f32 {
        (1.0 - self.touch_count as f32 / 20.0).clamp(0.0, 1.0).powf(0.7)
    }
}

3b.3  Per-Zone Introductory Box Specification
Each zone type has a specific introductory box configuration: initial size, label text, helper glyph, and the colour that matches the currently selected element's accent:

Zone
Initial size
Label
Introductory behaviour
Left thumbstick
38% × 40% screen
MOVE
Shows a circular virtual stick diagram inside the box (centre ring + outer circle). Animated: the centre ring pulses outward gently to suggest movement. Corner handles on the bounding box have small directional arrows. Label fades at touch_count=5.
Right thumbstick / camera
30% × 40% screen
LOOK
Shows a stylised camera/eye glyph. If gyro assist is active: a small phone-tilt icon overlaid with '+ tilt'. Corner handles have circular rotation arrows.
Gesture token strip
Full width × 18%
GESTURES
Each of the 6 token buttons has its label displayed at maximum contrast during introduction. A small → arrow shows sequence direction. The strip height shrinks to the P95 of actual touch y-positions (adapts to the user's natural strip-tap height).
Ability buttons 1–5
20% × 20% each
ABILITY 1–5
Buttons shown at double their final size initially. Number labels prominent. Shrink to P95 envelope of per-button touch positions. Each button calibrates independently — the user may naturally tap some buttons precisely (small adapted box) and others loosely (larger).
Super button
Central 22%
SUPER
Larger than ability buttons even in final state. Adapts centroid but minimum radius is 60px (never smaller — super is a high-stakes tap, needs generous target). Element-accent glow ring that pulses when chi is full.
HUD strip
Full width × 8%
(no label)
HUD strip does not adapt — it is fixed at top 8%. No calibration needed: the player taps specific icons, not a freeform zone.

3b.4  Visual Rendering — Box Animation
  Zone guide box rendering specification
  INTRODUCTION PHASE (opacity 1.0):
    Border:      dashed, 2px, element accent colour, dash length 8px, gap 4px.
    Corner icons: small square handles at all four corners, 8×8px, filled accent.
    Animated:    corners pulse at 0.8 Hz (scale 1.0 → 1.2 → 1.0). Draws eye.
    Background:  element accent at 8% opacity (very faint fill — space feels owned).
    Label:       centred, 18pt, white, bold. Element accent shadow offset 1px.
    Helper glyph:below label, 32px icon, 60% white opacity.

  LEARNING PHASE (opacity 1.0 → 0.15 as touch_count goes 0 → 20):
    Border:      solid, 1.5px, lerps from accent to white over this range.
    Resize:      box animates to new display_rect() over 200ms (ease-out quad).
    Shift:       centroid movement animates over 150ms (spring, damping 0.6).
    Counter:     'calibrating N/10' flashes for 400ms on each touch, then fades.
                  Disappears at touch_count=10.
    Corners:     pulse stops at touch_count=5. Corners stay as static handles.

  SETTLING PHASE (opacity 0.40 → 0.15):
    Border:      1px solid white, low opacity. Just visible.
    No animation on the box itself — movement is subtle, user barely notices.

  STICKY PHASE (opacity 0.0, invisible during normal play):
    Out-of-zone touch:  box flashes to opacity 0.30 for 100ms then back to 0.
                         Element accent colour. Not a penalty — just information.
    Long-press outside: (3 seconds in empty screen space) → recalibration dialog.

  RECALIBRATION DIALOG:
    'Reset all zones to default?' [Reset all] [Reset this zone] [Cancel]
    Resets touch_count to 0, centroid to hint centre, radius to initial.
    User profile: calibration data saved per-profile, restored on next session.

3b.5  User Profile Persistence
Zone calibration is saved to the user profile file on the render box and synchronised to the phone at session start. Each phone-player combination has its own calibration record:

  JSON — user profile zone calibration data format
// Zone calibration saved in user profile — JSON, one file per player
// ~/.config/unendlich/profiles/player_{slot}_zones.json
{
  "profile_version": 2,
  "device_id": "a3f9...",   // phone's stable node ID — ties cal to specific device
  "zones": {
    "left_stick": {
      "centroid": [0.28, 0.65],    // normalised 0..1 screen coords
      "radius_p95": 0.12,           // normalised
      "touch_count": 847,
      "sticky": true
    },
    "right_stick": {
      "centroid": [0.72, 0.65],
      "radius_p95": 0.09,
      "touch_count": 612,
      "sticky": true
    },
    "gesture_strip": {
      "centroid_y": 0.52,          // strip adapts in y-axis only (height)
      "height_p95": 0.08,
      "touch_count": 2140,
      "sticky": true
    },
    "ability_1": { "centroid": [0.81, 0.72], "radius_p95": 0.06, "touch_count": 344, "sticky": true },
    "ability_2": { "centroid": [0.88, 0.78], "radius_p95": 0.05, "touch_count": 218, "sticky": true },
    "super":     { "centroid": [0.50, 0.50], "radius_p95": 0.11, "touch_count": 89,  "sticky": true }
  }
}
// On new device: calibration starts from scratch (touch_count=0 for that device_id).
// On same device, new session: calibration loaded and continues from previous state.



  👆  Touch Zones — Gesture Grammar on Glass

4.  Touch Zone Specification
4.1  Virtual Thumbstick — Floating Anchor
Both thumbstick zones use a floating anchor — the stick centre is placed at the first touch contact point, not at a fixed screen position. This accommodates different hand sizes and grip styles without requiring calibration.

Dead zone: 8% of zone width from anchor centre. Contacts within the dead zone produce no movement input.
Outer edge: 90% of zone width from anchor. Contact at or beyond the outer edge produces maximum input (1.0) in that direction. Linear interpolation between dead zone and outer edge.
Thumbstick release: when the finger lifts, the stick smoothly returns to centre over 80ms (spring return animation). Sudden contact return after release: no snap — the new contact becomes the new anchor.
Left stick: outputs movement vector (x, y) to the movement system. When the character is bending mid-technique, the left stick redirects the bending output rather than character movement.
Right stick: outputs camera delta (dx, dy). Combined with gyroscope tilt when gyro assist is enabled (see Section 6 IMU). The gyro provides fine adjustment; the stick provides coarse rotation.

4.2  Bending Gesture Token Strip
The gesture token strip replaces the gesture swipe system used in the handheld (on-screen) mode. Instead of physically tracing gesture paths, the player taps token buttons to build a gesture grammar sequence — faster and more reliable on a small phone screen that is not the game viewport.

Token button
Maps to
Bending use (from Resonance Bending spec)
OPEN
OPEN gesture
Water: Surge start. Earth: release-upward. Storm: arc spread. Air: pressure expansion.
CLOSE
CLOSE gesture
Earth: compress/form. Water: pool. Information: Scatter Field concentration.
HOLD
HOLD gesture
Any sustained technique: continue applying chi to maintain the output. Release to terminate.
CHARGE
CHARGE gesture
Storm: accumulate before release. Fire: build thermal phase. Earth: compress before launch.
RELEASE
RELEASE gesture
Any charged technique: fire the accumulated output. Always follows CHARGE.
SWEEP
SWEEP gesture
Directional modifier: the sweep direction is determined by left stick direction at time of token tap.

Building a sequence: tap tokens in order. The sequence is shown as a small readout strip at the bottom of the gesture strip zone. When the last token is held (finger down for > 0.3s), the sequence is committed and the technique fires. Two-finger swipe upward on the strip: clears the current sequence. Maximum sequence length: 4 tokens.

Sequence commit also fires on: voice trigger word (the voice activates the last-built sequence), or the voice alias of a specific ability (directly fires without building manually).
Super technique: the large central button in the gesture strip. Single tap: fires the super if the chi pool is full. Long press (0.5s): shows super cooldown status and estimated recharge time.

4.3  Ability Slot Buttons
Five ability slot buttons corresponding to the five bindings in the character panel. Each button shows the element icon and a cooldown arc. Tapping fires the ability's bound gesture + voice combination instantly (no manual token building required for pre-bound techniques).

Visual feedback: on tap, the button brightens for 100ms then shows the cooldown arc draining to zero. If chi is insufficient: the button pulses amber with the current chi fraction shown.
Voice hold button: a dedicated button that activates the microphone for a voice trigger. Holding it sends the mic stream to the Choir recognition pipeline. Releasing after the word is detected triggers the matched ability. This is separate from the always-on voice stream (see Section 5).


  🔊  Volume Buttons — Hardware Input Mapping

5.  Volume Button Mapping
The phone's physical volume buttons are accessible regardless of touch zone state — they provide reliable hardware input that does not require screen contact. This makes them ideal for actions that must be available in any context, including when both thumbs are occupied.

Button + action
Game function
Bending context
Vol UP — single press
Evasive dash
Direction: left stick direction at press time. Air sub-discipline: Evasive Gust (automatic 4-block dash). All others: momentum dodge in movement direction.
Vol UP — double press
Sprint toggle
Holds sprint without requiring touch contact. Useful when both thumbs are occupied with gesture building and camera control.
Vol UP — hold (1s)
Block / guard
Earth: Stone Wall raised in front. Water: Tidal Veil. Air: Wind Wall. Storm: arc ground. Hold until release to maintain.
Vol DOWN — single press
Interact / use
Context-sensitive: open inventory item, pick up, activate forge, read inscription (Information Bending Temporal Echo Read on focused object).
Vol DOWN — double press
Crouch / go prone
Toggle. In prone: movement speed −50%, visual profile reduced (creature detection range −40%). Earth benders in prone: passive Seismic Read active.
Vol DOWN — hold (1s)
Seismic Read pulse
Active for all archetypes (limited range for non-Earth). Earth benders: full range + entity detection. Others: terrain-only read. Duration: while held.
Vol UP + Vol DOWN simultaneously
Pause / menu
Opens the in-game menu on the phone screen. The render box display shows the pause overlay on this player's viewport. Other players continue uninterrupted.

Implementation note:  Volume buttons must be intercepted before the OS audio system handles them. On Android: AudioManager.requestAudioFocus() + MediaSession key events. On iOS: MPRemoteCommandCenter. The phone app must be in foreground and have audio focus for volume buttons to route to the game rather than adjusting media volume.



  🎤  Voice Microphone — Choir System over LAN

6.  Voice Microphone — Choir Integration
The phone microphone streams voice audio to the render box where the Choir recognition pipeline processes it. The phone performs no recognition locally — it is a conduit. This design keeps all Choir vocabulary, alias matching, and voice corpus processing on the render box where computational resources are available, and ensures the Choir system behaves identically whether the player is using a local desktop microphone or a phone over LAN.

6.1  Audio Capture and Encoding
  Kotlin — Android voice capture and Opus encoding pipeline
// Android: phone-side voice capture pipeline
val audioRecord = AudioRecord(
    MediaRecorder.AudioSource.VOICE_COMMUNICATION,  // noise suppression enabled
    sampleRate = 48_000,
    channelConfig = AudioFormat.CHANNEL_IN_MONO,
    audioFormat = AudioFormat.ENCODING_PCM_16BIT,
    bufferSize = 4096   // ~43ms at 48kHz — keeps latency manageable
)

// Opus encoder: 8 kbit/s VOIP mode (low latency, good quality for voice)
val encoder = OpusEncoder(
    sampleRate = 48_000,
    channels = 1,
    application = Opus.APPLICATION_VOIP   // VOIP mode: lower algorithmic delay
)
encoder.complexity = 5    // mid-complexity: good quality, reasonable CPU
encoder.frameSize = 960   // 20ms frames at 48kHz

// Silence suppression: skip encoding frames where RMS < SILENCE_THRESHOLD
const val SILENCE_THRESHOLD_DBFS = -45.0f
// At -45 dBFS, the player is not speaking — no frame sent, saves bandwidth

// Noise suppression: Android VOICE_COMMUNICATION source includes platform NS.
// Additional WebRTC noise suppression applied if available (API 29+).

6.2  Render Box Choir Routing
The unendlich-bridge daemon on the render box receives Opus voice frames and routes them to the Choir recognition system as a virtual microphone device. The Choir system does not distinguish between local and network audio sources.

  Rust — render box voice routing: Opus decode → ALSA loopback → Choir pipeline
// Render box: unendlich-bridge voice routing
// Opus frames arrive in WebSocket voice stream packets

fn route_voice_frame(player_slot: u8, opus_bytes: &[u8]) {
    // Decode Opus → raw PCM
    let pcm = opus_decoder[player_slot].decode(opus_bytes)?;

    // Write to per-player virtual ALSA loopback device
    // The game's Choir pipeline reads from this device as a normal microphone.
    alsa_loopback[player_slot].write_pcm(&pcm)?;

    // Also forward to the speech activity detector:
    // Updates the phone's voice indicator LED and tells the phone UI to
    // show the 'listening' ring around the voice hold button.
    speech_activity[player_slot].process(&pcm);
}

// ALSA loopback setup (requires snd-aloop kernel module):
// sudo modprobe snd-aloop
// Creates /dev/snd/pcmCxDy for each player slot.
// The game's Choir pipeline is configured with per-player device paths:
// player_1_mic = 'hw:Loopback,0,0'
// player_2_mic = 'hw:Loopback,0,1'
// player_3_mic = 'hw:Loopback,0,2'
// player_4_mic = 'hw:Loopback,0,3'

6.3  Voice Trigger Modes
Mode
Behaviour
Always-on stream
The microphone streams continuously. The Choir recognition pipeline is always listening. Voice trigger words fire immediately. Silence suppression prevents bandwidth waste during non-speech. Default mode.
Push-to-talk (Voice Hold button)
The player holds the Voice Hold button to activate the microphone. The mic is muted otherwise. Preferred in noisy environments or for players who do not want always-on. The phone shows a clear visual indicator (red ring around the button) when the mic is hot.
Gesture-activated
The microphone activates automatically when the player begins building a gesture sequence (first token tap) and deactivates 2 seconds after sequence commit or clear. Voice and gesture are expected to coincide — this mode assumes the player speaks the ability name while building the sequence.


  📐  iio-sensor-proxy / libiio — Sensor Routing over LAN

7.  IMU Sensor Routing — iio-sensor-proxy and libiio
The phone's accelerometer, gyroscope, and magnetometer are routed to the render box using the libiio network backend. libiio is the standard Industrial I/O library for Linux and already supports remote sensor access over TCP via the iiod daemon protocol. The phone runs an iiod-compatible service; the render box connects via libiio's network context.

7.1  libiio Network Architecture
  Standard libiio remote sensor routing
  PHONE (sensor source):
    Runs: unendlich-phone-iiod  (iiod-compatible service exposing Android sensors)
    Port: 30431 (standard iiod port)
    Exposes as IIO devices:
      /dev/iio:device0  — accelerometer  (channels: accel_x, accel_y, accel_z)
      /dev/iio:device1  — gyroscope      (channels: anglvel_x, anglvel_y, anglvel_z)
      /dev/iio:device2  — rotation       (channels: rot_quat_x, rot_quat_y, rot_quat_z, rot_quat_w)
    Sampling rate: 100 Hz for gyro/rot, 50 Hz for accel

  RENDER BOX (sensor consumer):
    libiio network context: iio_create_network_context(phone_ip)
    iio-sensor-proxy reads from this context via D-Bus
    The game reads sensors from iio-sensor-proxy as if they were local hardware

  DATA PATH:
    Android SensorManager → unendlich-phone-iiod → TCP/iiod protocol →
    libiio network context → iio-sensor-proxy D-Bus → game engine sensor API

7.2  Phone-side iiod Service (unendlich-phone-iiod)
Android does not natively include iiod. The unendlich-phone-iiod is a lightweight Android service that implements the iiod wire protocol, reading from Android's SensorManager and presenting the data as if it were native IIO hardware:

  Kotlin — unendlich-phone-iiod Android service (iiod wire protocol emulation)
// unendlich-phone-iiod: minimal iiod protocol implementation for Android
// Exposes Android sensor data as iiod-compatible XML device descriptors

class IiodService : Service() {
    private val sensorManager by lazy { getSystemService(SensorManager::class.java) }
    private val serverSocket = ServerSocket(30431)   // standard iiod port

    // Device descriptor sent on client connect (iiod XML format)
    private val deviceXml = """
        <context name='unendlich-phone' description='Android Phone Sensors'>
          <device id='iio:device0' name='accelerometer'>
            <channel id='accel_x' type='le:s32/32>>0' index='0'/>
            <channel id='accel_y' type='le:s32/32>>0' index='1'/>
            <channel id='accel_z' type='le:s32/32>>0' index='2'/>
            <attr name='sampling_frequency' value='50'/>
          </device>
          <device id='iio:device1' name='gyroscope'>
            <channel id='anglvel_x' type='le:s32/32>>0' index='0'/>
            <channel id='anglvel_y' type='le:s32/32>>0' index='1'/>
            <channel id='anglvel_z' type='le:s32/32>>0' index='2'/>
            <attr name='sampling_frequency' value='100'/>
          </device>
          <device id='iio:device2' name='rotation_vector'>
            <channel id='rot_w' type='le:s32/32>>0' index='0'/>
            <channel id='rot_x' type='le:s32/32>>0' index='1'/>
            <channel id='rot_y' type='le:s32/32>>0' index='2'/>
            <channel id='rot_z' type='le:s32/32>>0' index='3'/>
            <attr name='sampling_frequency' value='100'/>
          </device>
        </context>
    """

    // Sensor events are pushed to the connected libiio client via iiod PUSH format
    fun onSensorChanged(event: SensorEvent) {
        // Convert Android sensor float[] to iiod binary sample format
        // Scale: Android accel in m/s²; iiod expects raw integer; scale factor in XML
        pushIiodSample(event)
    }
}

7.3  Render Box iio-sensor-proxy Configuration
  iio-sensor-proxy.conf — per-player network backend configuration
# /etc/iio-sensor-proxy.conf — configure per-player phone sensor backends

[player1]
backend = network
host    = 192.168.1.101    # phone IP (assigned by DHCP; bridge daemon updates this)
port    = 30431
devices = accelerometer,gyroscope,rotation_vector

[player2]
backend = network
host    = 192.168.1.102
port    = 30431
devices = accelerometer,gyroscope,rotation_vector

# iio-sensor-proxy exposes all device sensors via D-Bus:
# net.hadess.SensorProxy  →  /net/hadess/SensorProxy
# AccelerometerOrientation property: updated at 50Hz from phone accel
# The game reads orientation changes via D-Bus signal subscription:

# gdbus monitor -y -d net.hadess.SensorProxy  (debug: watch orientation events)

7.4  Sensor-to-Game Mapping
Sensor data
Game input mapping
Rotation quaternion
Camera look assist: the quaternion delta between frames is converted to camera yaw/pitch delta, scaled by gyro_sensitivity setting. Added to the right thumbstick input for fine camera control. Net effect: right stick for coarse look, phone tilt for precise aim.
Accelerometer X/Y
ROOTED gesture state: when the phone is held approximately level (accel_z close to −9.8 m/s², accel_x and _y < 0.5 m/s²), the ROOTED gesture token is automatically added to any gesture sequence built in this orientation. Earth bending techniques requiring ground contact: auto-satisfied.
Accelerometer magnitude spike
Shake detection: magnitude > 25 m/s² for > 50ms = shake event. Shake clears the current gesture sequence (emergency cancel). Second shake within 200ms: interrupt current technique (same as touch two-finger swipe up on gesture strip).
Gyroscope Z-axis (yaw)
Stance rotation: rapid Z-axis rotation > 4 rad/s triggers a stance-rotation event — in the bending fighting culture system, this corresponds to the PIVOT gesture token. Useful for Air and Storm bending stance transitions that require a rapid turn.
Tilt (pitch forward > 30°)
Dragon command gesture: when the player's tamed dragon is in range and the phone is tilted forward significantly, the dragon receives an 'approach' command. This is the physical gesture of pointing/gesturing at the ground — matching the visual command a rider would give.

Calibration:  iio-sensor-proxy orientation is calibrated to the phone's resting position when the session starts. Tapping the HUD strip's centre for 1.5 seconds performs a re-calibration — useful when the player's grip shifts. The calibration is per-session and per-player.



  🖥  Local Splitscreen — Render Box Configuration

8.  Local Splitscreen on the Render Box
Each connected phone creates one player viewport on the render box display. The render box renders all viewports simultaneously. The phone screen is never used for game rendering — it is always the controller surface only. This design eliminates the latency and bandwidth problems of streaming game video to the phone.

8.1  Viewport Layout by Player Count
  Viewport layouts — render box display
  1 PLAYER: full screen (normal solo play)

  2 PLAYERS: horizontal split
  ┌──────────────────────┬──────────────────────┐
  │  Player 1 (P1 blue)  │  Player 2 (P2 amber) │
  │  50% width           │  50% width           │
  └──────────────────────┴──────────────────────┘

  3 PLAYERS: T-split (P1 top, P2 bottom-left, P3 bottom-right)
  ┌──────────────────────────────────────────────┐
  │  Player 1                   (50% height)     │
  ├──────────────────────┬───────────────────────┤
  │  Player 2 (25% h)   │  Player 3 (25% h)     │
  └──────────────────────┴───────────────────────┘

  4 PLAYERS: quad split
  ┌──────────────────────┬──────────────────────┐
  │  Player 1 (P1 blue)  │  Player 2 (P2 amber) │
  │  50% width/height    │  50% width/height    │
  ├──────────────────────┼──────────────────────┤
  │  Player 3 (P3 green) │  Player 4 (P4 red)   │
  └──────────────────────┴──────────────────────┘

  Each viewport renders a fully independent game camera.
  The world simulation is shared — all players inhabit the same world state.
  Splitscreen boundary: 2px dark rule with the player's accent colour.

8.2  Audio Routing in Splitscreen
Audio is mixed for the shared display — not per-player. The master audio mix represents the world as experienced from the approximate centre of all player positions. Individual player microphone audio (Choir voice) is routed only to that player's recognition pipeline, not to the shared mix.

Spatial audio: positioned relative to the average of all player positions. At 2-player split, this is the midpoint between the two characters.
Per-player audio: not possible without per-player headphones. The shared audio is the primary experience.
Headphone passthrough: if the render box has a headphone output, the phone's own speaker can receive a per-player audio mix (the bridge daemon mixes a player-specific stream and streams it back to the phone via the WebSocket voice stream in reverse). This enables headphone audio for individual players. 8kB/s per player back-stream.

8.3  Splitscreen Performance Budget
Players
Resolution per viewport
Performance target
1
Full display res
1080p@60fps or 4K@30fps. No splitscreen overhead.
2
50% × full display res
2× 960×1080@60fps (horizontal split at 1080p). GPU budget approximately 1.6× solo (two draws, shared geometry culling).
3
Variable
P1: 1920×540. P2+P3: 960×540 each. GPU budget approximately 2.2× solo.
4
25% × full display res
4× 960×540@60fps. GPU budget approximately 2.8× solo. Requires a mid-range GPU for smooth operation at 60fps.


  🎮  Virtual Controller Driver — uinput Integration

9.  Virtual Controller Driver — uinput
The unendlich-bridge daemon creates one uinput virtual controller device per connected phone. The game's input system reads these as standard Linux input devices — it does not know or care whether input came from a physical gamepad or a phone over LAN.

9.1  uinput Device Creation
  Rust — uinput virtual controller creation in unendlich-bridge daemon
// unendlich-bridge: create one uinput device per phone connection
use input_linux::{UInputHandle, EventKind, AbsoluteAxis, Key, RelativeAxis};

fn create_virtual_controller(player_slot: u8) -> UInputHandle<File> {
    let uinput = UInputHandle::new(File::open("/dev/uinput").unwrap());

    // Set device properties
    uinput.set_evbit(EventKind::Absolute).unwrap();   // thumbsticks
    uinput.set_evbit(EventKind::Key).unwrap();         // buttons

    // Thumbstick axes (EV_ABS)
    for axis in [AbsoluteAxis::X, AbsoluteAxis::Y,   // left stick
                  AbsoluteAxis::RX, AbsoluteAxis::RY, // right stick
                  AbsoluteAxis::Z,  AbsoluteAxis::RZ] { // triggers
        uinput.set_absbit(axis).unwrap();
        uinput.set_absinfo(axis, -32768, 32767, 16, 128).unwrap();
    }

    // Buttons (EV_KEY) — mapped to phone touch zone actions
    for key in [Key::South,  // Ability 1 / interact
                Key::East,   // Ability 2 / cancel
                Key::North,  // Ability 3 / super
                Key::West,   // Ability 4
                Key::TL,     // L2 — sprint/dodge (left edge)
                Key::TR,     // R2 — technique modifier (right edge)
                Key::TL2,    // Vol DOWN
                Key::TR2,    // Vol UP
                Key::Select, // Inventory
                Key::Start,  // Menu/pause
                Key::Mode] { // Voice hold
        uinput.set_keybit(key).unwrap();
    }

    // Device name identifies player slot for the input system
    let name = format!("Unendlich Phone Controller P{}", player_slot);
    uinput.create(&name, 0x1234, 0x5678, 0).unwrap();
    uinput
}

9.2  Input Routing Table
Phone input
uinput event
Game function
Left thumbstick X/Y
ABS_X / ABS_Y
Character movement (horizontal plane). Applied after dead zone and sensitivity curve.
Right thumbstick X/Y
ABS_RX / ABS_RY
Camera yaw and pitch delta. Combined with gyroscope delta at 1:3 ratio (gyro fine, stick coarse).
Gesture token: OPEN
KEY_SOUTH down/up
Fires the OPEN token into the gesture sequence buffer. The game's gesture recogniser reads the token stream.
Gesture token: CHARGE
ABS_Z (left trigger)
Analogue: the trigger axis position encodes the CHARGE level. Full trigger = full charge accumulation.
Ability slots 1–5
KEY_SOUTH/E/N/W + layer
Ability slot buttons mapped to face buttons with layer modifier (L2 held = ability layer).
Vol UP single press
KEY_TR2 tap
Evasive dash. Direction from current ABS_X/Y state at the moment of the key event.
Vol DOWN single press
KEY_TL2 tap
Interact / use item.
Vol UP + Vol DOWN
KEY_START
Pause / menu.
Gyroscope quaternion
ABS_RX/RY delta
Camera look assist. Delta quaternion converted to euler angle change, scaled, added to right stick.
Shake event
KEY_MODE tap
Sequence clear / technique interrupt.
Voice stream (active)
KEY_MODE hold
Voice hold state active. The Choir pipeline is listening and will trigger abilities on word detection.


  ⚡  Performance Specifications and Latency Budget

10.  Performance Specifications
Metric
Target
Notes
Touch input latency (USB tethering)
< 8ms
Frame capture → WebSocket send → bridge receive → uinput event. USB reduces TCP round-trip to < 2ms.
Touch input latency (Wi-Fi 5GHz)
< 20ms
AP scheduling adds 2–10ms variable delay. Total budget including game input processing.
Voice recognition latency
< 80ms
Opus frame (20ms) + network (< 10ms) + Choir recognition (< 50ms). Same as local mic pipeline.
IMU stream rate
100 Hz gyro, 50 Hz accel
iiod protocol supports push at these rates. Gyro at 100 Hz produces smooth camera assist.
IMU latency (sensor to game)
< 25ms
Android sensor HAL (< 5ms) + iiod push + network + iio-sensor-proxy D-Bus signal.
Voice bandwidth per phone
8 kB/s
Opus at 8 kbit/s VOIP mode. 4 players = 32 kB/s total voice traffic. Trivial on any LAN.
Total bandwidth per phone
~14 kB/s
Touch (event-driven ~0.5 kB/s) + IMU (1.6 kB/s) + voice (8 kB/s) + overhead. Well within 2.4GHz Wi-Fi capacity.
Max players (splitscreen)
4
Limited by viewport rendering budget. 4-player split at 1080p requires mid-range GPU.
Connection recovery (dropout)
30 second hold
Session token held for 30 seconds. Player's character pauses on disconnect. Reconnect resumes seamlessly.



End of Phone Controller document. Dependencies: Touch Input Reference v2 (for handheld-to-controller mode transition spec), Gaze Rays & Voice Echo (Choir recognition pipeline that receives phone microphone audio), Resonance Bending v2.0 (gesture grammar tokens that the phone strip buttons emit).
