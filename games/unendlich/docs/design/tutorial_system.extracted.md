
Tutorial System
Practice World, Wizard Mentors & Choir Harmonisation
Tutorial Menu · Practice World · Seven Element Wizards · In-World Display Screens · Bending Technique Steps · Choir Harmonisation · Voice Matching · Controller Onboarding
The tutorial is not a series of pop-up boxes in the main world. It is a contained practice world with real environmental properties, real bending mechanics, named NPC mentors drawn from the main quest lore, and physical display screens that teach technique, voice shape, and Choir harmonisation. Everything earned in the practice world — the first partial coupling, the first formation name, the first stance — carries forward into the main world.

1.  Design Philosophy — The Tutorial as First Memory
The practice world creates the player's first memory of the game. When they stand in the main world for the first time and see a chi stone deposit glowing in a cave wall, they already know what it is — Miren showed them. When they hear a Lauscher stalking them, they remember that moment in the tutorial when the curiosity accumulator reached its maximum and the creature stepped into the clearing. The tutorial is not instruction. It is experience that the main world will make sense of.

Three principles govern all tutorial design:

Every tutorial beat has a sensory anchor — something the player sees, hears, or feels that is distinct enough to remember. The first harmonisation should be memorable enough to be described to a friend. The moment Miren's stone sings is a moment, not a menu.
No tutorial beat is gated on perfection. A player who fails to harmonise on the first attempt is still shown the next beat. Failure produces helpful feedback; it does not block progress. The Choir system takes time — the tutorial gives a taste, the main world teaches mastery.
The tutorial NPCs are the same people the player will meet in the main world through the quest chains. Meeting them in the tutorial first makes those quests warmer. The player has history with Miren before Miren gives them the Seismic Song quest.



  📋  Tutorial Menu — Element Selection and Progress

2.  Tutorial Menu
The tutorial menu is the player's entry point on first launch and remains accessible from the main menu throughout the game. It presents all seven elements as visual tiles, tracks completion per element, and provides a Quick Start path for players who want immediate guidance.

2.1  First Launch Flow
  First launch sequence — before the main world is created
  SCREEN 1 — WELCOME:
    'This world is built from seven forces.'
    'You will learn to speak to them.'
    'Begin with one. Return for the others.'
    [Start Tutorial]  [Skip to Main World]

  If [Skip to Main World]: main world generated normally.
  If [Start Tutorial]: practice world loaded. Tutorial menu appears.

  SCREEN 2 — ELEMENT SELECTION (Tutorial Menu):
    Seven element tiles arranged in a ring.
    Each tile shows the element's live void shader (miniature, from UI spec).
    Below each tile: the wizard's name and a one-line description.
    Centre: 'Choose your first element. You can return here for any of the others.'

  SCREEN 3 — ELEMENT DETAIL (tap a tile):
    Full-screen expansion of the tile's void shader.
    Wizard name + portrait description.
    What you will learn: 3 bullet points.
    Estimated time: 8–12 minutes per element.
    [Begin] [Back]

  After completing one element: return to Screen 2, that tile marked complete (✓).
  Any order allowed. No element is gated on another.

2.2  Tutorial Menu Layout
The tutorial menu is a dedicated screen accessible from the main menu under JOURNEY → Practice World. It shows all seven element tiles plus the Chorus Guide tile (the voice/Choir specialist available after any one element is completed):

Tile
Wizard
'What you will learn' preview text
Earth
Miren the Lapidary
How stone listens. How to read the ground's memory. How to speak the word that makes rock rise.
Water
Lienne of the Brine Shore
How water moves before you touch it. How the surge finds the shortest path. How the brine remembers the shore.
Fire
Daro the Forgemaster
How heat thinks. How to direct the exhale. How to work with fire rather than produce it.
Air
Verath, High Passage Senior
How air knows you are there. How pressure listens. How the current teaches faster than any instruction.
Storm
Sable (provisional)
How the arc finds its path. How to stand where the lightning wants to go. How to be the circuit, not the flame.
Brine
Carrier Designation 7 (field record)
How the mineral remembers. How suspension holds without holding. How Carrier 7 learned to move heavy things quietly.
Information
The Scholar (name withheld)
How light decides what to tell you. How sound stores what it witnessed. How to read what the room has been keeping.
Choir (unlocked)
The Chorus Guide
Available after one element completed. How your voice and the world share a frequency. How to harmonise with something larger than yourself.

Sable note:  Sable is noted as 'provisional' because her renown was stripped by the Ryujin Triad. She was asked to teach as part of her restoration process. In the tutorial, she says this without being asked — it is the first thing she says. This plants the Stormwright quest hook before the player has ever heard of the quest.



  🌍  Practice World — Environment Design

3.  The Practice World
The practice world is a contained world instance, approximately 400×400 blocks, generated with the tutorial biome template. It is physically real — environmental properties are active, bending mechanics work identically to the main world, and creatures behave as they do in the main world. The practice world is not a simulation. It is a place.

3.1  Zone Layout
  Practice world zone map — top-down view
  ┌─────────────────────────────────────────────────────────────────┐
  │  AIR ZONE                     │  STORM ZONE                    │
  │  Open sky plateau, y=140+     │  Storm Tower replica, exposed  │
  │  W > 0.45 ambient             │  P < 0.40, arc-conductive ore  │
  ├───────────────────────────────┴────────────────────────────────┤
  │                    CENTRAL HUB                                  │
  │             Crystal-lit gathering space                         │
  │           Seven display screens, one per element               │
  │             All wizard NPCs present and addressable            │
  │          Chorus Guide's platform in the exact centre           │
  ├───────────────────────────────┬────────────────────────────────┤
  │  EARTH ZONE                   │  WATER / BRINE ZONE            │
  │  Deep cave access, chi stone  │  Coastal shelf, tidal pool     │
  │  S > 0.55 ambient             │  H > 0.65 ambient, B > 0.12    │
  ├───────────────────────────────┤                                │
  │  FIRE ZONE                    ├────────────────────────────────┤
  │  Volcanic ledge, lava below   │  INFORMATION ZONE              │
  │  R > 0.60 ambient             │  Crystal cave, L > 0.75 (bio)  │
  └─────────────────────────────────────────────────────────────────┘

  Each zone's environmental properties are precisely set:
  Zone boundaries transition over 20 blocks (gradual property shift).
  The central hub is property-neutral (all properties at 0.30 — mid-range).
  Moving into a zone: the player's property meter shifts noticeably.

3.2  Practice World Special Properties
No durability loss. Items do not degrade in the practice world. The player can experiment without resource cost.
Infinite chi. The chi pool regenerates at 10× the normal rate. The player will never run out during a tutorial beat.
Technique safety. Fall damage is reduced 80%. Environmental hazards (lava proximity, steam zones) produce 20% of their normal damage. The player can get close to dangerous things.
One practice dragon. A juvenile Water dragon is present in the practice world, already at bond depth 10. It will not attack. It follows the player at a respectful distance. The Choir harmonisation tutorial uses it — the first partial coupling is performed with this dragon as the target.
A Lauscher. One Lauscher inhabits the central hub and the Earth zone. Its curiosity accumulator starts at zero. If the player is patient — if they stand still for three minutes in the Earth zone — the Lauscher approaches. This is not a tutorial beat. It is a bonus discovery for attentive players.



  🧙  Wizard NPCs — Lore-Accurate Element Mentors

4.  Wizard NPCs — The Seven Mentors
Each element has one named mentor NPC stationed in their zone and at their display screen in the central hub. They speak on approach, respond to player actions, and have distinct dialogue for every tutorial beat. Their names, personalities, and biographies are consistent with their appearances in the main world quest chains.

4.1  Miren the Lapidary — Earth
  Character profile — Miren
  APPEARANCE: A compact woman in her fifties with stone-dust in the seams of her
  hands. Her tools are always within reach. She stands still comfortably — the
  stillness of someone who has been listening for forty years.

  ZONE: Earth Zone — she stands at the base of a chi stone deposit, one hand
  resting against the stone. Not touching it meaningfully. Just resting.

  PERSONALITY: Practical, warm, measures words carefully. Does not explain more
  than necessary. Believes the student learns best by doing.

  OPENING LINE:

  Miren:  "I've been listening to this stone since before you were born. I'll teach you to hear what it's been saying."
  [Player approaches for the first time]

  Miren:  "Pick it up. The uncut chi stone. It won't bite."
  [After explaining chi stone, waiting for player to collect from deposit]

  Miren:  "Now hold still. Stop thinking. The stone is already talking. You're just not quiet enough yet."
  [Before Seismic Read introduction]

  Miren:  "There. You felt that. That's the song. Everything I've been telling you about — that feeling — that's it."
  [After first successful Seismic Read]

4.2  Lienne of the Brine Shore — Water / Brine
  Character profile — Lienne
  APPEARANCE: Tall, weathered, coastline-practical clothing. She moves the way
  water moves — no wasted motion, everything purposeful and slightly ahead of where
  you expect it to go.

  ZONE: Water/Brine Zone — standing at the tidal pool edge, bare feet in shallow
  water. She is never entirely on dry land during the tutorial.

  PERSONALITY: Direct, generous, genuinely interested in what the student notices.
  She asks more questions than she answers. Talks about water as if water has opinions.

  Lienne:  "The water already knows what it wants to do. Your job is not to make it do something. Your job is to agree with it at the right moment."
  [Opening line]

  Lienne:  "What did it feel like when the surge moved? Don't tell me what you did. Tell me what it felt like."
  [After first Tidal Surge]

  Lienne:  "The Brine is just water that's been thinking for longer. Give it time."
  [Brine sub-discipline introduction]

4.3  Daro the Forgemaster — Fire
  Character profile — Daro
  APPEARANCE: Heavy, scarred, armour with visible heat-cracking along the chest.
  The cracks glow faintly even when Daro is not actively bending. He has been
  bending Fire since he was seven years old.

  ZONE: Fire Zone — standing on the volcanic ledge, looking down at the lava.
  Comfortable. Familiar. The way you look at a fire in a fireplace.

  PERSONALITY: Generous with knowledge, impatient with hesitation. He has no
  patience for students who are afraid of fire. 'If you're afraid of it, you
  don't understand it yet. Understanding will fix the fear.'

  Daro:  "You're not going to hurt yourself. This isn't a lesson in caution. It's a lesson in direction. There's a difference."
  [Opening line]

  Daro:  "Don't push. Don't force. Exhale and let the heat go where it was already going. You're redirecting it, not creating it."
  [Before first exhale technique]

  Daro:  "Better. You see how it moved? It wanted to go that way. You just agreed with it."
  [After first successful Fire technique — echoing Lienne's language, intentionally]

4.4  Verath, High Passage Senior — Air
  Character profile — Verath
  APPEARANCE: Light, minimal. An instructor from the High Passage Windward Dojo.
  Appears to be in their thirties but moves like someone who has been climbing
  difficult things since childhood. Often appears to be slightly too still.

  ZONE: Air Zone — standing at the very edge of the plateau. Comfortable there.
  Not standing near the edge. At it. Different.

  PERSONALITY: Economical. Uses the minimum words. Watches the student more than
  they speak. When they do speak, the words are chosen carefully.

  Verath:  "The air already knows you're here. It noticed before you arrived."
  [Opening line — only line before first technique]

  Verath:  "Again. Slower this time."
  [After first attempt at Air Spout, regardless of success]

  Verath:  "You'll understand why I said it that way after the third session."
  [After completing the Air tutorial — cryptic, but true]

4.5  Sable (Provisional) — Storm
  Character profile — Sable
  APPEARANCE: Storm Dojo equipment with one modification: the Ryujin Triad
  designation patch has been removed. The place where it was is visible.
  She has not covered it.

  ZONE: Storm Zone — at the top of the practice Storm Tower, maintaining it.
  She is already doing something when the player arrives. She was here before them.

  PERSONALITY: She does not explain herself. She assumes the player will eventually
  understand and treats the tutorial accordingly. She teaches well. She knows she
  teaches well. That is one of the things that was taken from her.

  Sable:  "They asked me to teach. I said yes. I don't know what that makes me yet. Let's start with the basics."
  [Opening line — references her stripped renown unprompted]

  Sable:  "You're not trying to hit it. You're trying to complete the circuit. There's a path that the arc wants to take. Your job is to make it available."
  [Before first Stormstrike]

  Sable:  "Good. That's actually good. Don't look surprised — you did the right thing. Own it."
  [After first successful arc technique]

4.6  Carrier Designation 7 (Field Record) — Brine
Brine discipline does not have a living mentor in the tutorial. Instead, a recovered field record from Carrier Designation 7 is displayed on the Brine zone's display screen. The player reads the record, which teaches the Brine discipline through Designation 7's personal account of learning it. The record is voiced — the Carrier's voice is heard through the display screen's Choir resonance speakers.

  Designation 7 (recording):  "Day 14 of the northern circuit. First time I got the suspension field to hold without a bender present. I had the chi stone tuned wrong — too much Earth in it, not enough H. But when I corrected it... I felt it settle. Like something that had been uncertain made a decision."
  [Field Record, Northern Circuit, Day 14]

  Designation 7 (recording):  "The Brine doesn't need you to be present. It just needs you to have set it up correctly. Most bending needs the bender nearby. This doesn't. That's the thing that took me longest to understand about it."
  [Field Record, Northern Circuit, Day 22]

4.7  The Scholar — Information
  Character profile — The Scholar
  APPEARANCE: Middle-aged, precise, carries a Genome Reader and a personal
  FormationLexicon. Their name is withheld in all tutorial records — they prefer
  to be known by the work rather than the person.

  ZONE: Information Zone — in the crystal cave, examining bioluminescent mycelium.
  They do not look up when the player enters. They know the player is there.
  They have been aware since before the player entered.

  PERSONALITY: Precise, sometimes difficult to follow, never condescending.
  They find information itself genuinely interesting, not as a means to an end.
  Everything is interesting to them. They communicate this.

  The Scholar:  "The cave has been recording everything that happened in here for forty years. We're about to listen to some of it."
  [Opening line — Temporal Echo introduction]

  The Scholar:  "You heard something. I know you heard something. What was it?"
  [After first Temporal Echo Read — genuine curiosity]

  The Scholar:  "That's what it always is. The ordinary things. The extraordinary ones don't announce themselves."
  [After player describes what they heard in the Echo]



  📚  Per-Element Tutorial Flow — Seven Beat Sequences

5.  Per-Element Tutorial Flow
Each element's tutorial has seven beats. The structure is identical across all elements — the content is element-specific. This allows returning players (doing their second or third element) to anticipate the structure while experiencing entirely different content.

Beat
Name
What happens
1
Arrival
Player enters the zone. The environment changes — property meter on the display screen shifts visibly. The wizard speaks the opening line.
2
Element sense
Wizard introduces the element's primary property (S for Earth, H for Water, etc.). The display screen shows the property meter live. Player is asked to 'feel' the environment — walk around, notice the property shift.
3
First voice shape
Wizard demonstrates the element's simplest voice shape (Rise, Surge, etc.). The display screen shows the technique's gesture grammar. Player attempts to speak the word. Choir recognition provides feedback.
4
First technique
Player executes the first technique using gesture + voice together. The display screen shows the skill tree node lighting up. Wizard responds to success or provides correction on failure.
5
Choir note
Wizard demonstrates the element's Choir base frequency. Display screen shows the harmonic profile visualiser. Player attempts to match the frequency. Feedback: pitch visualiser, wizard response.
6
First harmonisation
Player sustains the Choir match for 3 seconds. The technique fires at maximum intensity with the full void shader for 2 seconds. The first partial coupling is established. This is the tutorial's climax.
7
What comes next
Wizard introduces the element's sub-disciplines briefly. Shows the rest of the skill tree (locked, but visible). Hints at the main world quest associated with them. The tutorial ends; the player returns to the hub.

5.1  Earth Tutorial — Miren's Seven Beats
  🪨  Step 1: Arrival
  Player: Enters Earth Zone from hub. Ambient sound shifts — lower, more resonant. Stone walls close in slightly (the zone feels underground).
  Feedback: Property meter on display screen: S shifts from 0.30 to 0.55. A small '+' pulse on the S bar. Miren speaks from near the chi stone deposit.

  🌍  Step 2: Earth sense
  Player: Walk to the chi stone deposit. Place one hand against it (approach + hold interact).
  Feedback: A seismic ripple animation plays through the stone surface at the touch point. The display screen briefly shows the S value spike to 0.70. Miren: 'The stone just heard you.'

  💬  Step 3: First voice shape — RISE
  Player: The display screen shows the RISE gesture diagram. Say the word 'rise' aloud — or tap the RISE token on the gesture strip (controller mode).
  Feedback: If the word is recognised: a small stone pillar rises 1 block near the player's feet. If not recognised: Miren demonstrates the word with her own voice, displayed on screen as a waveform. Player tries again. No limit on attempts.

  ⚡  Step 4: First technique — Pillar Rise
  Player: Build the gesture sequence (ROOTED + LIFT + HOLD) or say 'rise' with the ROOTED state active (phone level, or feet planted for 2 seconds).
  Feedback: A full-height chi stone pillar rises. The pillar has the characteristic amber glow. The display screen shows the skill tree node (Pillar Rise) lighting. Miren: 'Good. That's the song. You heard it.'

  🎵  Step 5: Choir note — Earth frequency
  Player: Miren produces an Earthstone — a small chi stone tuned to Earth. She holds it and hums the Earth base frequency (220 Hz, deep amber tone). The display screen shows the harmonic profile: bars 1–4 in amber.
  Feedback: The waveform visualiser on the display screen shows Miren's 220 Hz fundamental clearly. The bars pulse gently. The player sees what they need to match.

  ✨  Step 6: First harmonisation
  Player: Player attempts to hum, sing, or vocalise a tone matching 220 Hz. The pitch detector shows the player's frequency on the waveform visualiser. When within ±15 cents for 3 seconds: the partial 1 coupling triggers.
  Feedback: HARMONISATION MOMENT: the Stone Warden void shader fills the screen for 2 seconds — dark cave with amber fault lines, mineral dust. The Pillar Rise the player was about to activate fires at triple height. Miren: 'The stone just heard both of us. That's different from just you.' The player's harmonic profile now shows 5% coupling at bands 1–4.

  🔭  Step 7: What comes next
  Player: The display screen shows the full Earth skill tree — all nodes visible but locked. Miren points to three nodes (Seismic Read, Domain of Stone, Ancient Stone sub-discipline).
  Feedback: Miren: 'The full technique is in there. The quest that teaches it is in the world. You'll find it when you find someone selling chi stones.' The player returns to hub. Earth tile in tutorial menu marked complete (✓).

5.2  Information Tutorial — The Scholar's Seven Beats
  🔮  Step 1: Arrival
  Player: Enters Information Zone. Bioluminescent mycelium glows on cave walls. The cave has RT60 = 8 seconds — the player hears their own footsteps decay long after each step.
  Feedback: Display screen: L property shifts from 0.30 to 0.78. The Scholar does not look up: 'You're recording this, whether you know it or not. The cave is.'

  👂  Step 2: Information sense
  Player: Stand still in the cave for 10 seconds. The game's ambient sound mix shifts — the player begins to hear layered echoes of previous sounds in the cave.
  Feedback: The display screen shows the Temporal Echo range indicator expanding. The Scholar: 'That sound you just heard — that was someone else's footstep. About three minutes ago. The cave kept it.'

  💬  Step 3: First voice shape — ECHO
  Player: The display screen shows the Scatter Field gesture. Say 'echo' or perform the CLOSE + HOLD token sequence.
  Feedback: The Scatter Field activates — a subtle distortion field visible around the player. The Scholar: 'Everything in your range just became slightly less sure of where it is. Including you.'

  ⚡  Step 4: First technique — Temporal Echo Read
  Player: Focus on the mycelium colony on the cave wall (look at it for 2 seconds) then say 'echo' or tap OPEN + HOLD + RELEASE.
  Feedback: The Temporal Echo Read plays — the player hears 30 seconds of the cave's recent acoustic history. The Scholar has been here for a long time; the echo includes their own voice, speaking. The Scholar watches the player's face.

  🎵  Step 5: Choir note — Information frequency
  Player: The Scholar holds a Scatter Lens and produces the Information base frequency (740 Hz, a mid-range teal tone). Display screen shows the harmonic profile: bars 1–4 in teal.
  Feedback: The waveform is complex — Information Choir uses more upper partial variation than Earth. The player sees bars 1–4 active but also faint glimmers at 5–7. The Scholar: 'The upper partials are yours. I'm just giving you the root.'

  ✨  Step 6: First harmonisation
  Player: Player matches the 740 Hz fundamental. When sustained 3 seconds: the Information partial coupling triggers.
  Feedback: HARMONISATION MOMENT: the Echo Reader void shader fills the screen — interference wavefront pattern, prismatic chromatic scatter, pinpoint stars. The player hears an expanded Temporal Echo that extends back 6 seconds further than the baseline — they hear something from much earlier in the tutorial. The Scholar: 'You just changed what you could hear. That's what harmonising with this particular element does.'

  🔭  Step 7: What comes next
  Player: The Scholar shows the full Information skill tree. Points to Temporal Cascade (the super) and the Acoustic Flood sub-bender path.
  Feedback: The Scholar: 'The Lauscher in this world has been recording since the practice world was created. If you can earn its trust — and you know how to do that now — its database is worth reading.' The player returns to hub.




  🖥  In-World Display Screens — Physical Tutorial Panels

6.  In-World Display Screens
Display screens are physical objects in the practice world — emissive stone tablets in Earth and Brine zones, light-crystal projections in Information and Air zones, volcanic obsidian panels in Fire and Storm zones, and a bioluminescent membrane panel in Water zones. Each element's aesthetic matches its zone material.

6.1  Display Screen Types per Zone
Zone
Screen material
Visual character
Earth
Chi stone slab
Dark deepslate surface with amber chi stone inlay. Emissive amber in the display areas. The screen feels geological — like reading a cave wall that has learned to show images.
Water / Brine
Crystalline membrane
A thin water-crystal panel suspended vertically in a stone frame. The display area shows through the semi-translucent crystal. Bioluminescent teal at the frame edges.
Fire
Obsidian panel
Polished obsidian surface. Display content appears as heat-emissive orange-white imagery on the dark mirror surface. The screen is hot to approach.
Air
Light-crystal projection
No physical panel — the display is a projected light field from a crystal array. Text and images float in the air. Interacted with by reaching into the light.
Storm
Conductive ore matrix
A grid of conductive ore segments that carry arc discharge to display content. Active areas glow blue-white with flowing arc. Very specific to the Storm Tower aesthetic.
Information
Mycelium resonance panel
A flat mycelium colony on smooth stone. The bioluminescent network activates to display content — characters form from living light. The screen breathes.
Central hub
Resonance Font crystal
A central Resonance Font crystal from which all seven elements' display content can be projected. The hub's seven screens surround this crystal, each in its zone's material.

6.2  Screen Content Zones — Per Screen
Every display screen shows four content zones simultaneously, arranged in quadrants:

  Display screen quadrant layout
  ┌───────────────────────────────┬───────────────────────────────┐
  │  TOP-LEFT: PROPERTY METER     │  TOP-RIGHT: SKILL TREE         │
  │  Live environmental readings  │  Current tutorial lesson node  │
  │  Seven bars (H,R,W,P,S,L,B)  │  highlighted in element colour │
  │  Current zone properties lit  │  Adjacent nodes visible (dim)  │
  │  Player's position in field   │  Unlocked on tutorial complete │
  ├───────────────────────────────┼───────────────────────────────┤
  │  BOTTOM-LEFT: GESTURE GUIDE   │  BOTTOM-RIGHT: CHOIR DISPLAY  │
  │  Current technique's grammar  │  16-bar harmonic profile       │
  │  Tokens shown in sequence     │  Wizard's note shown in amber  │
  │  Player's current input shown │  Player's frequency in white   │
  │  Animated: next token pulses  │  Match indicator: green ring   │
  └───────────────────────────────┴───────────────────────────────┘

  Each quadrant updates in real time. The skill tree quadrant advances
  automatically when the player completes a beat.
  The Choir display is only active during beats 5 and 6 (Choir note + harmonisation).
  During other beats it shows the element's idle waveform (gentle, non-instructive).

6.3  Choir Display — Harmonic Waveform Visualiser
The Choir display is the tutorial's most technically detailed screen. It shows the 16-bar harmonic profile visualiser from the mob/dragon viewer Voice panel specification, adapted for instructional use:

Wizard's demonstration: when the wizard produces a Choir note, all 16 bars animate from their idle positions to show the note's partial series. The bars light in the element's accent colour. The fundamental (bar 1) is the most prominent.
Player input: the player's microphone input is processed in real time. A white bar (the player's frequency) appears at the detected fundamental frequency. The player sees the white bar and the amber bar side by side.
Match indicator: when the player's fundamental is within ±15 cents of the wizard's fundamental, a green ring appears around bar 1 of the player's display. Within ±5 cents: the ring becomes a steady green. Sustained for 3 seconds: harmonisation triggers.
Partial series progression: during the Introduction tutorial for the Choir system, only bars 1–4 are highlighted as the target. Bars 5–16 are visible but dimmed with a label 'these are yours alone — they develop through play.'
Failure feedback: if the player's frequency drifts significantly away, the white bar moves and the match indicator disappears. No sound effect — the disappearance is enough. Miren/Lienne/etc. says nothing until 5 failed attempts, then offers a specific suggestion.



  🎵  Choir Harmonisation Tutorial — Full Specification

7.  Choir Harmonisation Tutorial
Choir harmonisation is the skill of matching the world's elemental frequencies with your own voice. It is not singing. It is tuning — producing a sustained tone close enough to the element's base frequency that the partial series begin to interact. The tutorial teaches this in five stages, spread across the element tutorials. The dedicated Choir tutorial (with the Chorus Guide in the hub centre) is available after any one element is completed and provides deeper instruction.

7.1  Technical Implementation — Pitch Detection
  Pitch detection pipeline for the Choir harmonisation system
  INPUT: microphone audio stream (48kHz mono, 20ms Opus frames from phone OR
         local device microphone in handheld mode).

  STEP 1 — Autocorrelation-based F0 detection:
    Window: 2048 samples (~43ms at 48kHz).
    Autocorrelation computed for lags 27–544 (corresponding to 88–1760 Hz range).
    Peak in autocorrelation identifies the fundamental period.
    Confidence: correlation coefficient at peak — below 0.80 = 'no voice detected'.

  STEP 2 — Partial series extraction:
    FFT of the same window identifies energy at F0, 2×F0, 3×F0 ... 16×F0.
    Relative amplitudes map to the 16-bar visualiser display.

  STEP 3 — Match comparison:
    Target frequency: wizard's demonstrated F0 (element-specific, see §7.2).
    Cents deviation: 1200 × log2(player_F0 / target_F0).
    ±15 cents: match indicator appears (green ring, not sustained).
    ±5 cents: strong match (green ring steady).
    Sustained 3s at ±15 cents: harmonisation triggers.

  STEP 4 — Partial coupling update:
    On harmonisation trigger: partial coupling at bands 1–4 increases by 5%.
    Stored in user profile. Carries to main world.
    Subsequent harmonisations: coupling continues to increase.
    Maximum coupling without main-world practice: 15% (tutorial cap).

7.2  Element Base Frequencies
Element
F0 (Hz)
Musical approx.
Character and why this frequency
Earth
110 Hz
A2
Deep, below speech range. Requires chest resonance — you feel it in the sternum, not the throat. The geological bass frequency. Most players hum this more easily than sing it.
Water
293 Hz
D4 (middle D)
Comfortable speech-adjacent. Warm, liquid quality. The closest to a speaking voice. Most accessible for new players. Lienne's demonstrations are gentle.
Fire
440 Hz
A4 (concert A)
The universal tuning reference. Clean, immediate, recognisable. Daro says: 'It's the most honest frequency. Nobody argues about it.'
Air
528 Hz
C5
Bright, clear, above the comfortable speech zone. Requires an open throat. The frequency that resonates in the upper chest and soft palate. Verath demonstrates it effortlessly.
Storm
174 Hz
F3
Low-mid. The frequency of electrical hum. The player must drop below their speech register slightly. Sable's demonstration crackles through the conductive ore display.
Brine
396 Hz
G4
Dense, mineral-weight feel. Designation 7's recording demonstrates it with a slight reverb from the corridor it was recorded in. The player matches the recording, not a live voice.
Information
741 Hz
F#5
Highest of all elements. Requires a falsetto or head voice for many players. The Scholar demonstrates it with a focused precision that makes it seem simple. It isn't.

7.3  The Chorus Guide — Dedicated Harmonisation Tutorial
The Chorus Guide is available in the central hub after any element tutorial is completed. They are the only tutorial NPC who is not drawn from the main world's quest characters — they exist only in the practice world, as a teaching construct.

  The Chorus Guide — character and approach
  APPEARANCE: The Chorus Guide appears differently to different players.
  Their visual appearance is generated from the player's first harmonised element:
    Earth first: the Guide appears as an older person with stone-dust hands (Miren-adjacent).
    Water first: the Guide has the coastal weathering of Lienne's type.
    Storm first: the Guide has the operational stillness of Sable.
  This is not explained. It is simply true. The player may notice eventually.

  ZONE: The exact centre of the central hub. A Resonance Font crystal projects
  the Guide's image. They are semi-translucent — more projection than presence.

  VOICE: The Guide's voice contains audible harmonics at all seven element frequencies
  simultaneously, very faintly. When the Guide speaks, the Choir display shows
  all seven bars active at low intensity.

7.4  Chorus Guide Tutorial — Five Stages
  👂  Step 1: Listening before speaking
  Player: The Guide asks the player to close their eyes (a suggestion, not a mechanic) and listen to the hub's ambient sound for 30 seconds. The practice world plays a composite of all seven element sounds at low volume.
  Feedback: The Choir display shows all seven frequency bands slowly pulsing in sequence. The Guide: 'Seven things. All of them have been speaking since before we arrived. We learned to speak back.'

  🎵  Step 2: Your voice in the spectrum
  Player: The Guide asks the player to speak normally — say anything. The pitch detector captures the player's speaking F0 and displays it on the Choir visualiser.
  Feedback: The Guide: 'That's where you are. That's your voice's natural centre. The elements you're closest to are the ones nearest to that frequency. Begin there — the distance is shorter.'

  🎯  Step 3: Single element match
  Player: The Guide demonstrates the player's first harmonised element's frequency again. The player matches it.
  Feedback: At successful match (±15 cents, 3 seconds): the Guide demonstrates what harmonisation does to a technique — an Earth pillar rises, much taller than the player's own Rise. The Guide: 'You don't need to understand why it works. You need to notice that it does.'

  🔄  Step 4: Partial series introduction
  Player: The Guide shows bars 5–7 of their own harmonic profile — the personal 'character' partials. These are different from the Guide's element partials because they are the Guide's.
  Feedback: 'Those middle bars — 5 through 7 — those are what makes your voice yours within the element. They develop as you bend. You can't learn them here. But you can hear them beginning.' The player's bars 5–7 show the faintest possible glow — 0.01% — acknowledging potential.

  🐉  Step 5: The practice dragon
  Player: The Guide leads the player to the practice dragon's location in the hub. The dragon is juvenile Water. The Guide demonstrates the owner voice mesh introduction — the same harmonic introduction from the Dragon Rider quest chain Stage 2.
  Feedback: Player vocalises their Water Choir frequency toward the dragon. If successful: the first partial coupling with the dragon is established (bands 13–16, 3%). The dragon responds — vocalises back at the player's frequency, slightly adjusted to its own pitch. The Guide: 'It knows you now. Or the beginning of you. That's what the coupling means — beginnings.'



  🎮  Controller Onboarding — Adaptive Zones in Tutorial Context

8.  Controller Mode Onboarding in Tutorial
When a player uses the phone controller in tutorial mode, the adaptive gesture zones (from the Phone Controller document v2) begin their introduction phase. The tutorial is the best context for this — the player is already learning, and the zone introduction boxes align with the tutorial's beat-by-beat instruction style.

8.1  Tutorial-Specific Zone Introduction Sequence
Rather than showing all zone introduction boxes simultaneously (overwhelming), the tutorial introduces each zone at the beat where it's first needed:

Tutorial beat
Zone introduced
Introduction text shown in the zone box
Beat 1 — Arrival
Left thumbstick
'MOVE — drag here to walk.' The introductory box appears as the player enters the zone. It is large and labelled clearly. It adapts from this first touch.
Beat 3 — First voice shape
Gesture token strip
'GESTURES — tap these to bend.' The strip introduction box expands with all six tokens labelled. The first token for this element's technique pulses to draw attention.
Beat 4 — First technique
Ability button 1
'ABILITY 1 — tap to fire.' The ability button introduction box appears for the technique being taught. The button is at 2× size initially.
Beat 5 — Choir note
Voice hold button
'VOICE — hold and speak.' The Voice Hold button introduction appears with a microphone glyph and a pulsing animation. This is the player's first explicit introduction to the voice trigger.
Beat 6 — Harmonisation
Right thumbstick
'LOOK — drag to turn.' The camera zone is introduced last because in the Choir harmonisation moment, the player needs to be facing the display screen — a natural camera interaction moment.

Carry-forward:  By the end of the Earth tutorial (7 beats), all major zones have been introduced and have collected 7+ touches each. The learning phase begins during the first tutorial, and by the time the player enters the main world they have partially-adapted zones. They enter the main world already calibrated to their grip.


9.  What Carries Forward from the Practice World
The practice world is not a throwaway experience. Everything earned there is real:

Earned in practice world
Carried to main world
Choir partial coupling (per element)
The coupling depth (up to 15% at bands 1–4) is stored in the user profile and immediately active in the main world. The player enters the main world with the beginning of a Choir identity.
Practice dragon partial coupling (bands 13–16, 3%)
The coupling with the practice dragon is noted in the player's Dragon Rider history. In the main world, encountering a juvenile Water dragon of the same genome produces a slightly faster bond formation — the dragon recognises a familiar harmonic.
First voice shape (vocabulary)
The canonical voice word ('rise', 'surge', etc.) is registered in the Choir corpus with a baseline confidence from the tutorial attempts. The player's voice already has history with the system.
Controller zone calibration
All zone calibration data carries. The player does not see introduction boxes again for zones already calibrated. They enter the main world with zones in the Learning or Settling phase.
Named NPCs encountered
When the player first meets Miren in the main world (Seismic Song quest chain), the quest dialogue has an additional line: 'I've seen you before. The practice world. You're further along than most.' This is a small acknowledgement of the tutorial relationship.
Lauscher patience record
If the player earned the Lauscher approach in the practice world (standing still 3 minutes in the Earth zone), they carry a notation that they know how to wait. The first Lauscher encounter in the main world begins with the Lauscher slightly closer than it would be for a new player — already one curiosity accumulator beat in.


End of Tutorial System document. Connects to: Resonance Bending v2.0 (all bending techniques taught in tutorial), Gaze Rays & Voice Echo (Choir system, voice shape vocabulary, Temporal Echo), Dragon Voice genetics (partial coupling system used in Chorus Guide Stage 5), Phone Controller v2 (adaptive gesture zones, controller onboarding sequence), Quest Chains in Crafting & Genetics (Seismic Song, Lauscher's Gift, Dragon Rider — all referenced by tutorial NPCs). The Chorus Guide character exists only in the tutorial; the seven element wizards appear in both the practice world and the main world quest chains.
