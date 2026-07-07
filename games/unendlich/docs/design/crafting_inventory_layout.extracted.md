
Crafting & Inventory
UI Layout Reference Document
3×3 Crafting Grid · Station Indicator · 4×9 Inventory · Category Sidebar · Item Identification · Genome Tooltip System
All items in the reference image are identified and mapped to Unendlich item IDs. The visible recipe produces a Tidal Blade. Every inventory item in the lower grid is catalogued by type, Unendlich ID, category filter, and data bindings.

1.  Screen Overview
The crafting and inventory screen is divided into two horizontal zones separated by a subtle rule — upper crafting panel (approximately 45% of screen height) and lower inventory grid (55%). A vertical sidebar of eight category filter buttons runs along the left edge of the entire screen. The visual aesthetic throughout: deep space void with blue energy-vein patterns in the background, dark cell borders with blue corner accent dots at each cell's four corners, ornate dark-metal frame with spiral motifs at corners.

Zone
Content and dimensions
Left sidebar
8 icon buttons, vertically stacked. Each is a square tile with a category glyph in grey-white. The active filter glows blue with a selection indicator (▶) on the left edge. Approximately 80px wide.
Upper — crafting panel
Contains the 3×3 ingredient grid (left), a double-chevron arrow (»), the result slot (large, ornate gold frame), and a secondary station-indicator slot (smaller, blue frame, below result). Background: the void pattern is more active here — animated concentric swirl visible in the upper right area behind the result slot.
Lower — inventory grid
4 rows × 9 columns = 36 item slots. Each slot is a square dark cell with blue corner accents. Inventory items are rendered as stylised 2D icons in a slightly-elevated pixel-art style — more detailed than standard game item icons, each with subtle ambient occlusion and rim-light at the icon edges.



  📂  Left Sidebar — Category Filter System

2.  Left Sidebar — Category Filters
Eight category filter buttons. The first (diamond/gem icon) is active in the reference image — indicated by the ▶ selection triangle on the left edge of the button and a bright blue background fill vs the unselected grey. Clicking a filter category repopulates the inventory grid to show only items of that type.

Slot
Icon (reference image)
Unendlich category — what it filters
1
◆ Diamond / gem shape
GEMS & CRYSTALS: resonance_crystal, amethyst_shard, marine_crystal_shard, marine_glow_crystal, volcanic_quartz, chi_stone variants, resonance_pearl, resonance_shard, echo_shard. Any item whose icon is a crystal or gem cluster.
2
🌿 Leaf / herb
FLORA & REAGENTS: kelp_dried, wild_herb, sugar_cane_item, cave_mycelium_fragment, weeds_block, elemental leaf reagents, coral_branch, bamboo_bundle, dune_grass samples.
3
✦ Asterisk / star burst
BENDING INSTRUMENTS: all focus stone types (seismic, tidal, thermal, current_reader, arc_conduit, mineral_heart, scatter_lens), catalysts (chi_stone_uncut/tuned, volcanic_crystal_rod), genome_reader, resonance_font items.
4
🔨 Hammer
EQUIPMENT & ARMOUR: all helm, chest, arm, leg, hand, and feet armour pieces. Belt and accessory slot items. Does not include weapons (those are category 7).
5
❄ Snowflake
ICE & WATER MATERIALS: thin_ice, icicle_block, frost_block, cave_glider_membrane, marine_crystal_shard raw form, serpentinite variants, brine_crystal_deposit. Cold-element crafting components.
6
🍃 Leaf (simpler)
RAW MATERIALS & COMPONENTS: iron_ingot, gold_ingot, copper_ingot, ancient_alloy_ingot, bone, chain_links, feather, leather_item, string_item, flint, shell fragments, raw ore chunks.
7
⊙ Target / crosshair
WEAPONS: tidal_blade, seismic_mattock, arc_lance, ancient_alloy_blade, chi_quarterstaff, windcutter, any weapon item. Dragon artefacts used as weapons also appear here.
8
👝 Bag / sack
CONSUMABLES & DRAUGHTS: all food items (bread, cooked meats, vegetables), all elemental draughts (seismic_tea, tidal_broth, mountain_wind_essence, storm_charge_vial), ancient_elixir, resonance_pearl (consumable mode).


  ⚗  Crafting Panel — 3×3 Grid, Result, and Station Indicator

3.  Crafting Panel
The crafting panel occupies the upper zone. The 3×3 ingredient grid sits left-of-centre. A double-chevron arrow (»» in amber) sits between the grid and the result slot. The result slot is rendered in an ornate gold frame — significantly larger than inventory cells, implying visual importance. Below the result is a smaller blue-framed station indicator slot.

3.1  Visual Cell Anatomy
  Cell construction — crafting grid vs result vs station slots
  CRAFTING GRID CELLS (9 ingredient slots):
    Size: approximately 80×80px at reference resolution.
    Border: dark metal frame with blue corner accent dots (one at each corner).
    Background: near-black void fill with subtle blue energy veins visible.
    Item icon: centred, ambient-lit, slight elevation shadow under icon.
    Stack count: not visible in reference image — implied as single items here.

  RESULT SLOT (output item):
    Size: approximately 140×140px — 1.75× the ingredient cell size.
    Border: gold-tone ornate frame with bevelled edges. More decorative than grid cells.
    Background: slightly lighter — the result is presented as already manifested.
    Effect: completed result has a faint glow ring around it (element-matched).
    The result in this recipe: Tidal Blade (blue glow ring = Water affinity).

  STATION INDICATOR SLOT (below result):
    Size: approximately 80×80px — same as ingredient cells.
    Border: blue-lit frame (distinct from both gold result frame and plain grid cells).
    Content: shows which forge or crafting station is required to produce this recipe.
    If station is unavailable: the slot pulses amber. If available: steady blue.
    In the reference image: contains a compass-logbook icon = Tidal Anvil required.

  DOUBLE-CHEVRON ARROW (»»):
    Between grid right edge and result left edge.
    Amber/gold colour matching the result frame.
    Animated: pulses slowly when a valid recipe is detected.
    Static when no valid recipe is present.

3.2  The Visible Recipe — Full Ingredient Analysis
The reference image shows all 9 crafting slots filled. This is the Tidal Blade recipe with cross-element component complexity — it requires gems from three disciplines (Water, Brine/Information, Fire), a draught catalyst, a bending instrument, a biological material, and two artefact components.

Slot
Item (reference image description)
Unendlich item ID
Recipe role
[1,1] top left
Blue single crystal, pointed, teal-luminous
marine_crystal_shard
Primary material. Water element substrate of the blade — this crystal lattice becomes the blade edge.
[1,2] top centre
Purple crystal cluster, multi-faceted, amethyst hue
amethyst_shard
Resonance depth contributor. Amethyst raises resonance_depth during forging. Brine/Information trace affinity.
[1,3] top right
Orange-red gem cluster, fire-toned, angular
volcanic_quartz
Cross-element trace material. Embeds Fire trace affinity — the Tidal Blade heat tolerance originates here. Also a lens element for light refraction at the blade edge.
[2,1] mid left
Brown cylindrical vial/bottle with dark stopper
tidal_broth
Infusion catalyst. The tidal broth is the quench liquid for the marine_crystal_shard. Activates Tidal Temper infusion automatically at compatible stations.
[2,2] mid centre
Dark spiked star mace — multi-point black star with handle
chi_stone_tuned (Earth)
Resonance amplifier. Earth chi stone provides seismic grounding — prevents the water-crystal lattice from fracturing during the quench. Counterintuitive but essential.
[2,3] mid right
Large blue-white multi-point crystal cluster, bright luminous
marine_glow_crystal
Emission component. Gives the blade its bioluminescent edge glow (emissive 0.08 at blade tip in darkness). Phosphorescent properties are permanent once forged.
[3,1] bottom left
Dark hooded cloak shape — slumped fabric, deep shadow
cave_glider_membrane
Handle wrap material. Thin-tissue flexibility gives grip and dampens arc discharge vibration from Storm-element technique interactions. Raw variant preserves translucency.
[3,2] bottom centre
Blue swirling vortex orb — spiral galaxy in a sphere
resonance_pearl
Spatial anchor component. Embedding in the crossguard gives the blade spatial memory — the wielder can use it as an Information Bending anchor. The pearl's independent function is consumed but its resonance enters the genome.
[3,3] bottom right
Curved ivory-coloured claw/tusk — crescent shape, organic
dragon_claw
Blade curve guide. Metal is forged around the claw then the claw is removed. Leaves a trace of the source dragon's elemental genome as a sway mark in the finished blade.

3.3  Result and Station Indicator
Slot
Contents and function
RESULT — large gold frame
Tidal Blade: curved blade with gold crossguard, silver-blue edge. Rendered at 1.75× ingredient size. Blue glow ring (Water affinity aura). Hovering over the result shows the full tooltip from the item lore document — name, type, rarity (Rare), slot (Primary weapon), mechanical effects, lore quote, and attribution. The genome preview (Information bender or Genome Reader required to unlock) shows below the standard tooltip.
STATION INDICATOR — blue frame
Compass-and-logbook icon = Tidal Anvil. This slot communicates which forge is needed. If the player does not have access to the Tidal Anvil (not yet discovered or The Tidal Anvil quest chain Stage 2 incomplete), the slot pulses amber and shows a hint text: 'Requires Tidal Anvil — found in submerged ruins.' If access is confirmed, the slot glows steady blue.

Sway preview note:  The crafting panel also shows the projected sway marks for the output item based on current environmental conditions. If crafting during H > 0.65 (recommended for Tidal Blade), a Water sway preview icon appears above the result slot. If a tamed Water dragon is within 40 blocks, a dragon sway preview icon appears alongside it. These are projections — they do not consume anything. They confirm the recipe will produce the desired genome.



  📦  Inventory Grid — 4 Rows × 9 Columns, 36 Slots

4.  Inventory Grid — All Items Identified
The lower inventory grid has four rows of nine slots each — 36 total. The reference image shows 36 items, every slot occupied. Items are organised by the player's current category filter (gems/crystals active in the reference image, but the inventory is shown with all categories visible for reference). Each row is catalogued below.

4.1  Row 1 — Gems and Minerals (positions 1–9)
Pos
Icon
Item (image description)
Category
Unendlich ID and notes
R1-1
◆
Blue single crystal, small pointed
marine_crystal_shard — smallest variant, single-point. Crafting component for Tidal instruments and Water armour sets.
Gems
R1-2
◆
Purple multi-crystal cluster, large
amethyst_shard — bioluminescent purple. Resonance Font crafting, Information/Brine instruments. emit 0.05.
Gems
R1-3
◆
Orange-red angular gem cluster
volcanic_quartz — fire-orange. Fire technique instruments, lens elements. Glows at R > 0.60.
Gems
R1-4
◆
Green crystal, tall single point
chi_stone_uncut — Earth attunement. Pale green from iron-mineral content. Crafts into chi_stone_tuned with element-specific ore.
Gems
R1-5
◆
Large blue-white multi-point crystal cluster
marine_glow_crystal — largest size variant. High bioluminescent emission. Required for Scatter Lens and Resonance Pearl crafting.
Gems
R1-6
🐚
Dark spiral nautilus shell — ammonite
marine_shell — raw form of marine_crystal_shard. Must be processed at Tidal Anvil to yield shards. Rare drop from coastal encounters.
Materials
R1-7
🧪
Brown cylindrical vial, dark stopper
tidal_broth — Water elemental draught. H threshold +0.10 for 3 minutes. Also used as infusion catalyst in Tidal Blade recipe.
Consumable
R1-8
⬛
Dark grey irregular chunk, rough surface
raw_copper_block — unprocessed copper ore. Smelts to copper_ingot × 2 at any forge. Required for Water and Air armour sets.
Materials
R1-9
🌿
Green moss/lichen clump, fluffy texture
cave_mycelium_fragment — bioluminescent cave fungal sample. Crafting reagent for Resonance Font processes. Emits faint glow (L 2 in darkness).
Flora

4.2  Row 2 — Instruments, Armour, and Artefacts (positions 10–18)
Pos
Icon
Item (image description)
Category
Unendlich ID and notes
R2-1
💎
Blue crystalline ice-like ore chunk, irregular
raw_iron_block — iron-rich ore chunk with characteristic blue-grey crystalline surface. The blue tone indicates high-grade ore with crystal lattice formation.
Materials
R2-2
🔮
Gold pendant with large green jewel, loop ring
tidal_focus_stone — the Water discipline focus stone. Gold setting, green marine gem. H threshold +0.12. The pendant version (worn at chest as alternate to off-hand slot).
Instruments
R2-3
⚙
Blue metallic circular device, clockwork face
genome_reader — the Information Bending instrument craftable at Resonance Font. Reads item genomes (element_affinity, resonance_depth, etc.) without requiring active Information bending from the player.
Instruments
R2-4
🛡
Dark plate chest armour, heavy segmented
seismic_plate — Earth set chestpiece. Deepslate-grey armour with iron-seam visible. +0.07 S threshold, −12% Earth chi cost.
Equipment
R2-5
🥊
Dark armoured gauntlet, fist shape
fault_vambraces — Earth set arm armour. Heavy plate construction. +0.04 S threshold, Seismic Anchor range +4 blocks.
Equipment
R2-6
👢
Dark armoured boot, right foot
flow_greaves — Water/Tidal set leg armour. Dark leather over plate. +0.03 H threshold, speed in water +15%.
Equipment
R2-7
🔗
Brown leather belt with large square buckle
leather_belt — accessory slot item. Carries item pouches. Currently unenchanted — no genome modification. +2 inventory quick-access slots.
Equipment
R2-8
🦷
Grey curved claw/fang, pointed tip
dragon_claw — drop from dragon territory encounter or gifted at bond level 50+. Used in Tidal Blade recipe as blade-curve guide. Carries source dragon's elemental genome as sway.
Materials
R2-9
🔮
Purple glowing orb with internal light source
resonance_shard_orb — a resonance_shard that has absorbed a sustained Choir frequency and manifested as a sphere. Epic tier. Crafting component for Ancient-tier recipes at Ancient Shrine Forge.
Artefacts

4.3  Row 3 — Artefacts, Instruments, and Reagents (positions 19–27)
Pos
Icon
Item (image description)
Category
Unendlich ID and notes
R3-1
🌀
Blue swirling galaxy vortex sphere
resonance_pearl — Information Bending artefact. Stores one spatial resonance point. Also used as recipe ingredient (Tidal Blade slot [3,2]) — using as ingredient consumes it.
Artefacts
R3-2
💎
Clear/white multi-point crystal cluster, bright
resonance_crystal (passive) — unactivated. Reads as L 0.08 emissive. Becomes resonance_crystal_active after 100 Information Bending technique passes through it.
Gems
R3-3
⚙
Blue metallic orb with spherical housing, smaller
scatter_lens — Information Bending focus stone. L threshold +0.12. The smaller blue orb (distinct from genome_reader in R2-3 which has a clockwork face; this has a smooth lens surface).
Instruments
R3-4
🧥
Dark cloak/mantle, slumped fabric texture
scatter_cloak — Information set torso piece. Translucent-edge cape with prismatic fade. +0.06 L threshold, −15% Information chi cost.
Equipment
R3-5
📚
Dark leather-bound book, thick pages
ancient_tablet_fragment — a codex-form tablet from pre-civilisation shrine. Readable by Information benders (Temporal Echo Read reveals one line of the shrine builders' language). Crafting component for Information instrument recipes.
Artefacts
R3-6
⭐
Spiky black star shape, multi-point irregular
conductive_ore — raw Storm/electrical bending material. The spiky irregular star shape reflects the branching crystal growth pattern of natural conductive ore deposits. Required for Arc Conduit and Storm armour.
Materials
R3-7
🍂
Red-orange leaf cluster, branching structure
elemental_leaf_reagent — a fire-attuned dried botanical collected from high-R volcanic forest zones. Crafting component for Thermal draughts and Fire-sway infusion catalysts.
Flora
R3-8
🧪
Blue potion with star/sparkle particles inside
storm_charge_vial — Storm elemental draught. All Storm chi costs −20% for 2 minutes. Warning: static discharge side effect. Crafted from conductive_ore + water.
Consumable
R3-9
🎋
Bundle of thin sticks/reeds, tied
bamboo_bundle — bamboo_block_side material. Required for Air armour set (Windveil Mask craft) and chi_quarterstaff base component.
Materials

4.4  Row 4 — Biological Materials, Consumables, and Components (positions 28–36)
Pos
Icon
Item (image description)
Category
Unendlich ID and notes
R4-1
🪶
Orange-brown feather, single, quill visible
feather — standard crafting component. × 4 required for Windveil Mask (Air set). × 6 for Atmosphere Wrap. × 3 for Current Gloves. Common drop from aerial fauna.
Materials
R4-2
🔮
Gold amber orb/sphere, warm internal glow
amber_resonance_gem — a solidified resonance_pearl variant formed in high-R thermal conditions. Warm amber tone (Fire trace affinity). Crafting component for Fire-affinity Information instruments.
Gems
R4-3
🐚
Cream-white nautilus shell, different orientation
marine_shell (alt orientation) — second shell in inventory, suggesting the player has collected multiple. Or: a different species of shell. Both process into marine_crystal_shard at Tidal Anvil.
Materials
R4-4
🗡
Small curved dagger, plain crossguard
tidal_blade (damaged) — a lower-quality or pre-crafting-bench version of the Tidal Blade. May have been found before the player had Tidal Anvil access. Lower resonance_depth, fewer infusion slots. The current recipe (in the crafting grid above) may be an upgrade path.
Weapons
R4-5
🧪
Small blue-teal round potion, bulbous shape
tidal_broth (small) — a smaller dose variant. Restores H threshold +0.06 for 90 seconds (half duration of the standard vial). Useful for short encounters.
Consumable
R4-6
🌿
Large flat green herb leaves, fresh
wild_herb — surface flora. General crafting reagent. Used in: elemental draught recipes as the 'flavour' component (the herb provides the base liquid's character without affecting property modifiers).
Flora
R4-7
🪸
Pink-red coral branch, forked tips
coral_branch — Brine/Water crafting material from coastal reef formations. Used in Brine_vest and Suspension_Coat crafting. High H-affinity — adds Water sway mark when included in any recipe.
Flora
R4-8
🦴
White-grey bone, long, clean
bone — base crafting material. Used in chi_stone processing (the bone ash provides a binding agent in the chi stone tuning process). Also a Food byproduct from cooked_beef preparation.
Materials
R4-9
⛓
Silver-grey chain links, 3 links visible
chain_links — metal component. Used in: tidal_focus_stone pendant setting, genome_reader housing construction, leather_belt buckle reinforcement. Crafted from iron_ingot × 2.
Materials



  💬  Item Hover — Tooltip, Genome Overlay, and Sway Preview

5.  Item Hover — Tooltip and Genome Display
Hovering any cell in either the crafting grid or inventory produces a tooltip at the cursor position. The tooltip layout follows the standard format from the item lore document, with additional layers available based on the player's Information Bending capabilities.

5.1  Standard Tooltip — All Players
  Standard tooltip — visible to all players on hover
  ┌─────────────────────────────────────────────────────────────┐
  │  ITEM NAME (element accent colour)                          │
  │  Type · Rarity · Slot                                       │
  │  ─────────────────────────────────────────────────────────  │
  │  Mechanical effect line 1                                   │
  │  Mechanical effect line 2                                   │
  │  ─────────────────────────────────────────────────────────  │
  │  'Lore text in italics — the item's voice'                 │
  │    — Attribution, source                                    │
  │  ─────────────────────────────────────────────────────────  │
  │  Found: acquisition source hint                             │
  └─────────────────────────────────────────────────────────────┘

  Tooltip background: frosted glass (backdrop blur 16px).
  Width: 280px fixed. Height: variable.
  Appears after 0.4-second hover delay (avoids tooltip spam on quick movement).
  Disappears 0.2 seconds after cursor leaves the cell.

5.2  Genome Overlay — Information Benders and Genome Reader
A second overlay panel appears below the standard tooltip when: (a) the player has an active Information bending discipline with the Resonance Depth node unlocked, OR (b) the player has a Genome Reader device in their inventory. This panel shows the item's full genetic profile:

  Genome overlay — conditional display
  ┌─────────────────────────────────────────────────────────────┐
  │  GENOME  [◈ Information Bending required / Reader active]   │
  │  element_affinity:  Water (primary) + Storm trace           │
  │  resonance_depth:   0.74  ████████░░ (74%)                 │
  │  material_purity:   0.68  ██████░░░░ (68%)                 │
  │  craft_signature:   Tidal Anvil (submerged, H=0.72)        │
  │  infusion_capacity: 2 remaining  ██░░░░ (of 3 total)       │
  │  sway_marks:  [Water ●] [Storm ●] [Dragon:Water ○]         │
  └─────────────────────────────────────────────────────────────┘

  Bar colours per property:
    resonance_depth: blue (Information palette)
    material_purity: grey-white (neutral — cannot be element-tinted)
    infusion_capacity: amber (resource count — low = amber warning)
    sway_marks: element-coloured circles (filled = active, empty = absorbed)

  If the reader is active and the item has History (Temporal Echo or Memory Bind
  enchantment): a third 'History' section shows the 5 most recent locations where
  this item was used for a bending technique.

5.3  Recipe Ingredient Highlighting
When a recipe is present in the crafting grid, hovering any ingredient in the inventory highlights all matching items in the inventory grid (a blue ring appears around cells containing the same item type). Hovering the result slot highlights all nine ingredient cells simultaneously. Hovering any ingredient cell also dims cells that are not part of the current recipe — reducing visual noise.

If the player is missing an ingredient (the crafting grid slot is empty but the recipe requires it): hovering the empty slot shows 'Missing: [item_id]' in the slot, and the inventory grid highlights any items that could fill that slot.
If an ingredient is available in the inventory: a small blue arrow appears at the bottom of the inventory cell pointing toward the crafting grid (directional visual hint — 'this goes there').
Quantity indicators: if a recipe requires × 2 of an item and the player has × 3, the inventory cell shows '3 / 2 ✓' — sufficient quantity in green. If insufficient: '1 / 2 ✗' in amber.


  🎨  UI Aesthetic Specification — Dark Void with Blue Energy

6.  UI Aesthetic Specification
The crafting and inventory UI uses the darkest palette of all game screens — deeper than the discipline character panels. The design language: void and electricity. The player's items exist in deep space, suspended in dark cells, lit by blue energy that traces the interconnections between them.

6.1  Background and Energy Veins
Background fill: near-black (8, 12, 20) RGB — very slightly blue-tinted, not neutral black. This separates visually from the cell borders (which are a slightly warmer dark metal) without being a visible colour.
Energy veins: thin blue-teal lines (0.5–1px) trace organic patterns through the background, most visible at the outer edges and around the cell borders. These are animated — they pulse slowly at 0.4 Hz, brightest at the cell corner accents. They represent the resonance field connecting items.
Concentric swirl: in the upper-right area of the crafting panel, a faint concentric circular pattern is visible — as if the crafting process creates a local resonance disturbance. This is more visible when a valid recipe is detected and animates inward toward the result slot.
Outer frame: dark metal decorative border with ornate spiral motifs at corners (clockwise-rotating scroll pattern). The frame has subtle relief — embossed rather than flat. The spirals are the same spiral glyphs seen on Ancient Shrine stonework.

6.2  Cell Visual Language
Grid cells: 80×80px dark fill with four blue dots at corners (the corner accent system). The corner dots brighten when the cell is hovered (transition 80ms). When occupied: the item icon sits in the cell with a subtle drop shadow and rim-light on the icon's edges.
Cell hover: the cell's corner dots intensify to full blue. A faint blue halo appears under the icon. The border between cells slightly illuminates.
Cell selection (item dragged from): the source cell's background brightens to a very dark blue (10, 20, 40) and the corner dots pulse at 1.5 Hz while the drag is in progress.
Result slot: gold-toned frame. When a valid recipe is detected: the frame illuminates with a warm amber pulse (1.0 Hz) and the result icon appears. When no valid recipe: frame is dark and shows a faint '?' placeholder.
Station indicator slot: blue-tinted frame (distinct from gold result frame). When station is available: steady blue. When unavailable: amber pulse + the station icon dims.

6.3  Item Icon Rendering
All item icons are rendered in a consistent stylised 2D style — more detailed than traditional pixel art (the reference image shows significant shading, material differentiation, and ambient lighting on each icon) but maintaining a clear silhouette at small sizes.
Gem/crystal icons: internal refraction highlights. A bright point-light appears at the crystal tip or facet peak. Element-appropriate glow corona around luminous items.
Equipment icons: the armour pieces use three-quarter isometric views. Dark metal with highlight at the curved surfaces. Internal detail (rivets, seams) visible at the icon's native resolution.
Consumable icons: bottles show liquid level and colour. The liquid colour matches the elemental draught type: brown (Earth/Seismic), blue-teal (Water/Tidal), amber (Fire/Thermal), near-clear with stars (Storm).
Artefact icons: the resonance_pearl and resonance_crystal_active icons have animated frames — the swirling vortex in the resonance_pearl rotates at 0.2 Hz. This is the only motion in the inventory grid.



End of Crafting & Inventory Layout Reference Document. All items identified from the reference image are mapped to canonical Unendlich item IDs from the item lore and crafting systems documents. The recipe visible in the crafting grid produces a Tidal Blade at the Tidal Anvil station. Genome overlay system, ingredient highlighting, and tooltip format are canonical for all inventory interactions.
