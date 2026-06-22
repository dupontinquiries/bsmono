
CHARACTER PANEL
Bending Disciplines · Skill Constellation · Gear Inventory · Resonance Rating
Reference document — screen layout, component descriptions, and data bindings for the character panel as illustrated in the UI reference image. Water Bending (Oceanbinder / Tidal sub-discipline) is the active discipline shown.

1.  Screen Overview — What the Image Shows
The character panel is a full-screen overlay. The player character — a Water bender mounted on a Water dragon (blue-crystalline leviathan, ocean environment, full moon above) — occupies the centre-left of the frame as the primary visual anchor. The dragon and rider are rendered as the elemental void background for the Water / Tidal sub-discipline: the ocean surges behind them, the dragon's body provides the environmental light source (bioluminescent blue eye, scale reflection), and the full moon in the upper background is the lunar maximum state that produces faint Information sway during nighttime crafting.

Three distinct UI zones are visible in the reference image, each at a different depth plane in the frosted glass rendering system:

Zone
Content and screen position
Header bar
Top-left: player name 'Glibtiger', season/rank status line, and a small progress bar. Top-right: navigation tabs (CLAN | COLLECTIONS | JOURNEY | CHARACTER | INVENTORY | Settings). Active tab 'CHARACTER' is brighter / underlined.
Skill constellation
Left 35% of screen, vertically centred. A floating arrangement of seven crystalline-glyph icons in the Water Bending blue palette — each icon is a skill node in the Oceanbinder sub-discipline skill tree. A large spiral-vortex icon (centre-left) is the active super technique. Four smaller flanking nodes visible: flame-droplet (top), crossed-instruments (mid-left, slightly occluded), crystalline-arrow (mid-right), insect/airy glyph (lower-left). The bottom of the constellation has an 'OCEANBINDER — WARLOCK SUBCLASS' label.
Gear column
Right 18% of screen, vertical. POWER 1796 in large gold at the top, then a shield icon with value 8 (recovery/resilience equivalent), then six equipment slots with numeric values and equipment thumbnails: 41, 22, 106, 123, 34, 45. Bottom-right corner: 'E Gear Stats ⊠ Dismiss' prompt.
Ability bar
Bottom-centre strip: five square icons in a horizontal row, each representing an active ability slot. All five icons are in the Water/crystalline aesthetic — snowflake, branching-current, trident/arrow, circle/shield, orbital target. These are the equipped technique bindings.



  👤  Header Bar — Identity, Season, and Navigation

2.  Header Bar
The header bar runs the full screen width at 60px height. It is a dark translucent band (opacity 75%, dark navy fill) — not the frosted glass system used for panels, but a dedicated identity band that persists across all panel tabs.

2.1  Left Side — Player Identity
  Left header content — as shown in reference image
  [CLAN SYMBOL]  Glibtiger
  // SEASON 209  RANK 164  ♦ 1796  ⚔ 18,051
  [XP PROGRESS BAR — approximately 15% full, teal-blue colour]

  TRANSLATION TO UNENDLICH SYSTEM:
  Player Name:  Glibtiger
  Season:       Season 209 (world time index — maps to seasonal bending events)
  Rank:         164 (Resonance Rank — overall bending mastery across all disciplines)
  ♦ 1796:       Resonance Rating (power equivalent — aggregate of equipped gear genome scores)
  ⚔ 18,051:     Total chi expenditure lifetime (measures bending experience depth)
  Progress bar: Current rank progress toward Rank 165

  Clan symbol (top-left, diamond/wing glyph): maps to the player's dojo affiliation.
  In Unendlich: this is the dojo brand mark from the Bending Fighting Culture system.
  The glyph changes when the player's primary Dojo changes.

2.2  Right Side — Navigation Tabs
Tab label
Unendlich equivalent and content
CLAN
Dojo affiliation panel. Shows the player's current Dojo, stance renown within the Dojo, Global Dojo Board entries, and active fusion partners. Dojo brand mark is the same glyph shown in the header.
COLLECTIONS
FormationLexicon — all formation terms the player has registered, their renown tier, and the geological dictionary entries they map to. Also: all encounter mob types added to the mob viewer.
JOURNEY
Quest chain status panel. Shows all seven quest chains with current stage, NPC relationships, and world consequence log ('Sable's stance became Legendary. Someone noticed.'). This is the player's narrative record.
CHARACTER
Active tab — shown in the reference image. The full skill constellation, discipline display, gear column, and ability bar. The elemental void background is determined by the active discipline (here: Water / Oceanbinder / Tidal).
INVENTORY
Item grid view — all carried items with genome read available (Information benders see additional genome column). Crafting materials, consumables, artefacts, dragon-sourced items.


  🌊  Elemental Void — Water / Tidal Background as Rendered

3.  Elemental Void — Water Discipline (Oceanbinder)
The background of the character panel is the elemental void shader for the Water discipline, with the Tidal sub-discipline modifier. In the reference image this is rendered as a full cinematic scene rather than an abstract shader — a design that places the player's mounted Water dragon in the environmental foreground of the menu itself. This is the 'character exists inside their element' principle from the UI design specification realised at maximum visual intensity.

  What is visible in the reference image — scene elements
  WATER DRAGON (foreground subject):
    Crystalline blue-black body. Bioluminescent eye — single bright blue source (L equivalent 14).
    Serpentine form, open jaw showing crystalline teeth. Riding surface: the dorsal ridge.
    The dragon's scales catch the moonlight — this is the thin-film iridescence from the Water
    attunement scale shader specification (Water scale material: near-mirror smoothness, belly sheen).
    The rider controls with reins — the dragon rider bond is visible in the UI as a relationship.

  PLAYER CHARACTER (rider):
    Female bender, dark armour with Water/Tidal Mantle aesthetic (flowing, mid-weight fabric,
    long hem still oscillating from recent movement — cloth simulation visible).
    Holds a curved weapon — consistent with Tidal Blade from the item lore document.
    Hair animated by the W field the Water dragon generates (dragon presence sway: Air +W effect).

  ENVIRONMENT:
    Ocean surface — wave surge visible at frame bottom (Water void shader: wave-cut geometry).
    Full moon, upper centre — lunar maximum state (L = nighttime maximum from moonlight).
    Overcast cloud ring around moon — high-H, moderate-P atmospheric condition.
    Crystal ice formations at lower right — Glacial Water sub-discipline influence visible in
    the environment even though active sub-discipline is Tidal (trace affinity, not primary).
    The scene is the Tidal sub-discipline environment: deep ocean pressure, lunar tide, surge.

  DOF APPLICATION (v2 rendering):
    Dragon and rider are the focal plane — sharp.
    Moon and clouds: 28px bokeh. Crystal formations: 18px. Water surface: 12px.
    Skill constellation icons (left): virtual depth 0.8 — slightly soft against the sharp rider.




  ⭐  Skill Constellation — Oceanbinder / Tidal Sub-Discipline Nodes

4.  Skill Constellation — Bending Discipline Tree
The skill constellation occupies the left third of the screen, rendered as a floating array of crystalline glyph icons against the void background. Each icon is a 3D glass sphere node at a virtual depth between the background (far, soft) and the rider (near, sharp). In the reference image, seven nodes are clearly visible at various positions. The constellation layout follows the Water discipline's river-delta metaphor — a branching tributary structure where central nodes split into lateral branches.

The OCEANBINDER / Tidal sub-discipline is the active selection. The nodes shown are Water and Tidal techniques — blue-white crystalline aesthetic throughout. One node at the top is distinctly warmer (flame-droplet icon) — this is a trace Fire affinity node unlocked through a cross-discipline technique, or it represents an environmental synergy node (Water + high-R conditions).

4.1  Identified Nodes — Left Screen Constellation
Reading the reference image node positions and icon shapes, left to right and top to bottom:

  ❄  Glacial Crest  [Environmental Synergy]  ● UNLOCKED
    Water techniques in sub-zero R conditions: ice formation radius +4 blocks. Passive — activates automatically when R < 0.15.
  🔥  Thermal Current  [Cross-discipline Trace]  ● UNLOCKED
    When H > 0.65 AND R > 0.55 simultaneously: water technique chi cost −12%. Hot spring and volcanic coast zones. Trace Fire affinity node — available to Tidal sub-discipline only.
  ⚔  Tidal Strike  [Active Technique]  ● UNLOCKED
    Crystalline-edged melee Water technique. Contact applies soaked status. Stacks with Tidal Blade weapon bonus. Currently occluded — hover to reveal full node.
  🌀  Abyssal Vortex  [Super Technique — Active]  ● UNLOCKED
    THE CENTRAL LARGE NODE. The spiral-vortex icon visible in the reference image. Full-screen effect: summons a sustained water vortex centred on the player. Duration: 8 seconds. All nearby creatures receive Water surge knockback every 2s. Chi cost: 120. Cooldown: 90 seconds. This is the player's super technique — the most expensive node in the tree.
  🏹  Surge Lance  [Active Technique]  ● UNLOCKED
    Directional high-velocity water projectile. Long range, single target, piercing (passes through up to 3 entities). Forms from ambient H — requires H > 0.30 minimum. Fast cast, short cooldown.
  🦋  Current Drift  [Passive]  ○ locked
    Passive movement: the player leaves a water-surface trail in high-H environments. Creatures tracking the player by scent: tracking distance −20% (water masks scent signature). Bioluminescent creatures: trail visible for 30 seconds.
  ⭕  Tidal Anchor  [Formation Context]  ○ locked
    Unlocked at sea arch and coastal formation types only. Creates a stable water bridge formation at coastal geography. Range matches Tidal sub-discipline bonus. Formation sway embedded: Bridge enchantment capacity automatically available.
4.2  Sub-Discipline Label — OCEANBINDER
The lower-left of the constellation shows 'OCEANBINDER — WARLOCK SUBCLASS' in the reference image. Translation to the Unendlich system:

Reference image label
Unendlich equivalent
OCEANBINDER
Sub-discipline name: Tidal (Water primary, Tidal sub-discipline active). The Oceanbinder name becomes the player's Dojo-acknowledged title at Known renown. It appears in the stance system as 'Oceanbinder' if the player registers it — otherwise it defaults to the canonical 'Tidal Practitioner.'
WARLOCK SUBCLASS
Discipline tier: Tidal sub-discipline is Water primary's depth-specialisation. The 'Warlock' term maps to the resonance depth classification — a bender whose resonance_depth in the Water discipline exceeds 0.70 qualifies for Tidal access. The player's resonance_depth, shown implicitly by the number of unlocked nodes, is approximately 0.75–0.80 (five of seven nodes unlocked).


  ⚡  Ability Bar — Five Active Technique Slots

5.  Ability Bar — Active Binding Row
The ability bar runs horizontally at the screen bottom below the constellation label. Five square icon slots, each representing one active technique binding. These are the techniques currently equipped in the player's Bindings panel — the ones mapped to gesture grammar sequences and voice words. In the reference image all five are in the Water/crystalline aesthetic, indicating a full Water-attuned loadout.

5.1  The Five Slot Assignments — Translated
Slot
Icon description (reference image)
Unendlich technique binding
1
Crystalline snowflake — radial, symmetrical
Glacial Form (passive super state). Voice bind: 'freeze.' Gesture: GROUND + HOLD + SUSTAIN. Environmental: available at R < 0.20 + H > 0.40. The player's defensive super — not a vortex, a crystalline shell.
2
Branching current — three-fork split
Tidal Surge (active). Voice bind: 'surge.' Gesture: OPEN + SWEEP + PUSH. Direction-locked. The primary offensive Water technique — horizontal wave knockback. This is the technique the Tidal Anvil's crafting bonus amplifies.
3
Trident / arrow composite — forward-pointing
Surge Lance (active). Voice bind: 'lance.' Gesture: CHARGE + RELEASE. Single directional projectile. The fast-cast option in the Water loadout — lower chi, longer range than Surge.
4
Circular orbital — ring with inner dot
Tidal Anchor (formation context). Voice bind: 'anchor.' Available only near coastal formation types. Creates the water bridge formation. The 'utility' slot — not combat, navigation and traversal.
5
Orbital target — cross-hair ring
Abyssal Vortex (super technique). Voice bind: 'vortex.' Gesture: OPEN + CHARGE + SUSTAIN + SWEEP (full sequence). This is the expensive slot — 120 chi, 90s cooldown. The spiral-vortex large node in the constellation.



  🛡  Gear Column — Power, Resilience, and Equipment Slots

6.  Gear Column — Right Side Inventory Strip
The gear column occupies the right 18% of the screen — a vertical strip of information. In the reference image it contains six distinct sections reading top to bottom: the Resonance Rating display, a resilience/recovery score, and six equipment slot rows each showing a numeric value and a thumbnail of the equipped piece.

6.1  Resonance Rating — POWER 1796
  Resonance Rating display — as shown in reference image
  POWER
  ♦ 1796

  TRANSLATION TO UNENDLICH SYSTEM:
  The Resonance Rating (♦ 1796) is the aggregate genome score of all equipped items.
  Calculated as:
    resonance_score = SUM over all equipped items of:
      (base_item_level × material_purity × (1 + resonance_depth))
      + sway_mark_bonus (each sway mark adds 12 to the score)
      + enchantment_bonus (each active enchantment adds 8–24 depending on slot cost)

  At 1796, this player has:
    Most equipment at Rare tier, some at Epic.
    At least 2–3 items with strong sway marks (3+ sway marks).
    Active enchantments on most pieces.
    resonance_depth average across items: approximately 0.68.

  The gold diamond (♦) is the Resonance Rating icon — the same symbol used throughout
  the header bar. It distinguishes the bending power rating from the chi rating (⚔).

6.2  Resilience — Shield Icon, Value 8
Below the Resonance Rating, a shield icon with value 8. In the reference image this appears as a pentagon shield badge in gold with '8' inside. Translation:

Resilience (score 8 of 10) maps to the aggregate physical damage resistance from all equipped armour pieces — a secondary stat derived from armour material type and weight class, not from the bending genome.
At resilience 8: player takes 18% less physical damage. Each resilience tier above 5 reduces damage by 5%. Achievable with full Rare-tier armour sets — consistent with the Resonance Rating of 1796.
The shield icon is distinct from elemental resistance — elemental resistance is displayed in the property bar below the equipment slots (not visible in this view but accessible via the 'E Gear Stats' prompt at bottom-right).

6.3  Equipment Slot Rows — Six Slots with Values
Six rows, each with a numeric value on the left, a small icon, and an equipment thumbnail on the right. Reading top to bottom from the reference image:

Slot
Value
Icon
Equipment and translation
Head
41
↑
A dark helmet — visually consistent with the Tidal Hood (water bender helms). Value 41 is the slot's contribution to Resonance Rating. Higher than neighbouring slots, suggesting an Epic-tier piece with strong resonance_depth.
Torso
22
⊙
Armour vest — Brine Vest aesthetic (dark, flowing). Value 22 is lower than head — this slot may have lower material_purity or fewer sway marks. Circular icon suggests a core / resonance centre body piece.
Arms
106
◎
Gauntlet/bracer — noticeably higher value (106). This is the highest value slot visible, suggesting a legendary or ancient-tier piece. May have the Dragon Tongue enchantment (fits the dragon rider relationship visible in the background). Armour thumbnail visible — heavy crystalline gauntlet.
Legs
123
⊗
Boot/greave — value 123 is second highest. Possibly the Flow Greaves (full Tidal Mantle set). The ⊗ icon suggests a mobility-focused slot. The 4-piece Tidal Mantle set bonus (Brine suspension field density +20%) would be active at this loadout.
Focus
34
◈
Focus stone / catalyst slot — the bending instrument slot. Value 34 is moderate. The Tidal Focus Stone (H threshold +0.12) would contribute this score. The diamond-frame icon (◈) indicates a focus-type slot rather than an armour slot.
Weapon
45
◎
Weapon slot — the Tidal Blade would contribute approximately this value at Rare tier. The circular repeat icon suggests dual-slot (primary + off-hand combined view). May also show as the Arc Conduit in off-hand position.

Reading the gear pattern:  The two highest values (106, 123) are in arm and leg slots — unusual. Suggests a player who found extraordinary arm and leg pieces (possibly ancient-tier) but has not yet upgraded the torso. This is a normal mid-progression loadout pattern: two exceptional outlier pieces surrounded by solid but not exceptional other slots.

6.4  Gear Stats Prompt — Bottom Right
The reference image shows 'E Gear Stats ⊠ Dismiss' at the bottom-right corner. This is a contextual control prompt — pressing E (or equivalent) opens the full property bar for the currently hovered equipment slot. In Unendlich:

Gear Stats opens: the full genome readout for the hovered piece (element_affinity, resonance_depth, material_purity, craft_signature, sway_marks, infusion_capacity), the property modification bars (S, H, R, W, P, L, B), and any active enchantments.
Gear Stats is restricted in what it shows by default. The genome section requires either: Information bending active (Temporal Echo Read on the item from the character sheet), or a Genome Reader device in inventory. Without either, only the property bars and enchantments are visible.
Dismiss (⊠) closes the character panel and returns to play. The keybind is shown because the character panel can be opened at any time, including while in motion or near enemies — dismissing quickly matters.


  📊  Full Data Model — All Values Visible in the Reference Image

7.  Full Data Model — Reference Image Decoded
Complete table of every labeled value and UI element visible in the reference image, translated into Unendlich system terminology:

UI element (reference image)
Value shown
Unendlich translation
Player name
Glibtiger
Player identifier. Dojo affiliation glyph shown to left.
Season
Season 209
World time index. Each season brings seasonal bending events and discipline availability changes.
Rank
164
Resonance Rank. Aggregate mastery across all bending disciplines, not Water-specific.
♦ (diamond) value
1796
Resonance Rating. Genome aggregate score of all equipped items.
⚔ (sword) value
18,051
Lifetime chi expenditure. A measure of bending experience depth.
XP bar
~15% full
Progress toward Rank 165.
Active tab
CHARACTER
Character panel active. Other tabs: CLAN (dojo), COLLECTIONS (formations), JOURNEY (quests), INVENTORY (items).
Sub-discipline name
OCEANBINDER
Active sub-discipline: Tidal (Water primary). Player-registered name at Known renown tier.
Sub-discipline tier label
WARLOCK SUBCLASS
Resonance depth classification: Water resonance_depth ≥ 0.70 unlocks Tidal access.
Skill nodes visible
7 nodes (5 active)
Tidal sub-discipline constellation: 5 of 7 nodes unlocked. 2 locked nodes visible (dimmer).
Large central node
Spiral vortex icon
Abyssal Vortex — super technique. 120 chi, 90s cooldown. Highest chi-cost node in the tree.
Ability bar slots
5 icons
Active bindings: Glacial Form, Tidal Surge, Surge Lance, Tidal Anchor, Abyssal Vortex.
POWER display
♦ 1796
Resonance Rating repeated at gear column top for quick reference.
Shield badge
8
Resilience score (8/10). Physical damage resistance −18%.
Head slot value
41
Tidal Hood or equivalent (Rare+ tier). Contributes 41 to Resonance Rating.
Torso slot value
22
Brine Vest or equivalent (Uncommon–Rare). Lowest-value slot — upgrade priority.
Arm slot value
106
Exceptional arm piece — likely Epic or Legendary. Dragon Tongue enchantment probable.
Leg slot value
123
Highest individual slot. Ancient-tier greave or full-set-bonus piece.
Focus slot value
34
Tidal Focus Stone (H threshold +0.12). Rare tier.
Weapon slot value
45
Tidal Blade primary or equivalent. Rare tier with Tidal Flow enchantment.
Bottom prompt
E Gear Stats / ⊠ Dismiss
E: opens full genome readout for hovered slot (requires Information bending or Genome Reader). ⊠: returns to play.


8.  Implementation Notes — What This Screen Requires
The reference image establishes a visual target for the CHARACTER panel. The following are the implementation requirements inferred from the image that must be met to match the visual quality shown:

Requirement
Technical specification
Dragon rider mount in void
The player's tamed dragon appears in the elemental void background when bond depth ≥ 40. At bond depth < 40: the standard Water void shader (abyssal pressure, caustic ceiling). At bond depth 40+: the rider and dragon composite scene. The dragon pose is determined by the rider's active discipline: Tidal → serpentine ocean pose.
Crystal ice trace elements
The Glacial trace affinity (visible as ice crystal formations at lower right of the reference) appears when the player has any Glacial-sub or Ice node unlocked in their Water tree. It is a background layer composited at 30% opacity over the standard Tidal void.
Lunar maximum background
Full moon state in the character panel background matches the current in-world time. If the player opens the character panel at lunar maximum (in-game), the moon is full. The faint Information sway this produces is noted in the crafting document — the character panel also uses the calendar to drive the void shader state.
Constellation node 3D depth
From the v2 rendering specification: nodes at virtual depth 0.8 (slightly behind the focal plane of the rider). The reference image shows this correctly — the nodes are soft-edged compared to the sharp rider. DOF blur on nodes: 4–6px.
Gear slot thumbnail rendering
Thumbnails in the gear column are live renders of the equipped piece at 64×64, rendered with the item's actual material shaders at reduced resolution. The arm slot piece (value 106) shows a dark crystalline texture consistent with the Dragon Tongue enchantment's visual signature.
Resilience badge gold colour
The shield badge at value 8 is gold — indicating this stat is at a threshold (8/10 = high tier). At 6/10 the badge is white-silver. At 4/10 or below, copper. The colour communicates tier without requiring the player to know the number ranges.


End of Character Panel Reference Document. This document describes the UI reference image (Water Bending / Oceanbinder / Tidal, Resonance Rating 1796, character name Glibtiger) as a player skill, bending discipline, and gear inventory panel for Unendlich. All element translations, node identifications, and gear interpretations are based on the Unendlich system documentation.
