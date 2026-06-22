
Gamemode Trait System
Mode Definitions · Trait Specifications · Capability Extensions · Implementation Reference
Covers: SURVIVAL · ADVENTURE · CREATIVE · CRASH DUMMY  ·  Traits: BUILD · HEALTH · HUNGER · MOBTARGET · FLY · IMMUNE · NOTARGET · INVISTOPLAYERS · PASSTHROUGH · VISION

1.  Architecture Overview
The Gamemode Trait System decouples what a gamemode is called from what it actually does. Each gamemode is defined as a named set of active traits. Traits are atomic, independently-testable boolean flags that each govern one specific dimension of player capability. Every gameplay behaviour that differs between modes is the consequence of a trait being on or off — there are no hidden mode-specific code branches.

This architecture has three practical benefits: (1) new modes can be defined by listing trait combinations without writing mode-specific logic; (2) traits can be toggled individually at runtime for administrative or developmental purposes; (3) trait interaction rules are documented once per trait pair, not once per mode.

1.1  Trait List
Trait
Category
Effect summary
BUILD
World
Can place and break blocks
HEALTH
Survival
Has a mutable health pool; can be damaged and killed
HUNGER
Survival
Has a mutable hunger pool; depletes over time
MOBTARGET
AI
Hostile mobs passively detect and attack this player
FLY
Movement
Can toggle sustained flight without elytra
IMMUNE
Survival
Immune to all damage, knockback, and negative status effects
NOTARGET
AI
Mobs cannot acquire this player as a target, even when provoked
INVISTOPLAYERS
Visibility
Invisible to other players' clients; visible to server
PASSTHROUGH
Movement
Can move through solid blocks; no block collision
VISION
Perception
Full brightness, fog suppression, entity X-ray outlines

1.2  Mode–Trait Matrix
The four gamemodes are defined below. Each column shows which traits are active (●) or inactive (○). No other mode-specific logic exists — the mode is fully described by its column.

Trait
SURVIVAL
ADVENTURE
CREATIVE
CRASH DUMMY
BUILD
●
○
●
●
HEALTH
●
●
○
○
HUNGER
●
●
○
○
MOBTARGET
●
●
○
●
FLY
○
○
●
●
IMMUNE
○
○
●
●
NOTARGET
○
○
●
○
INVISTOPLAYERS
○
○
○
○
PASSTHROUGH
○
○
○
○
VISION
○
○
○
○

Note:  INVISTOPLAYERS, PASSTHROUGH, and VISION are not assigned to any base mode. They are administrative and developmental capability traits applied at runtime via command or permission node, on top of any mode.


2.  Gamemode Definitions
  SURVIVAL

2.1  Survival
The fully-consequential mode. Every player action has resource and survival implications. Resources must be earned, health must be maintained, and the world is actively hostile. This is the primary mode for all non-creative gameplay.

Active traits
Trait
Effect in Survival
BUILD
Block placement and breaking consume items from inventory and produce drops. Blocks cannot be placed if the inventory slot is empty.
HEALTH
20 HP maximum (configurable). All damage sources apply. Death triggers item drop, XP drop, and respawn with configured spawn mechanics. Health regenerates when hunger saturation is > 0.
HUNGER
20 hunger points, 20 saturation points. Depletes at 1/80th point per tick base rate. Sprint multiplier 4×. Jump 0.05× per jump. Mining 0.025× per block. At 0 hunger, sprint is disabled and health regen ceases. At ≤ 1 hunger on Normal difficulty, health begins draining to 10% (1 HP).
MOBTARGET
All hostile mobs include this player in passive target acquisition. Spawned mobs within aggro range immediately pathfind toward the player. Provoked neutral mobs (bees, iron golems, zombie pigmen) similarly target the player.

Inactive traits
FLY — gravity applies; no sustained flight without elytra or external effect.
IMMUNE — all damage and knockback resolve normally.
NOTARGET — mobs freely acquire and pursue this player.

Extended capabilities
  Survival-Specific Capability: RESOURCE ECONOMY
  · All tool durability depletion is active. Items break after reaching 0 durability.
  · Enchantments on tools apply at full effectiveness.
  · XP accumulates from mob kills, ore mining, smelting, breeding, and fishing.
  · XP is used for enchanting and anvil repair.
  · Block drops obey Fortune and Silk Touch enchantment rules.
  · Death XP drop: 7% of current XP capped at (level^2 + 7 × level), max 100 XP dropped.

  Survival-Specific Capability: ENVIRONMENTAL PRESSURE
  · Fire, lava, cactus, thorns, drowning, suffocation, void, wither, and poison all deal damage.
  · Ambient temperature from weather and biome modifies hunger depletion rate (cold biomes +10%).
  · Night cycle spawns hostile mobs at the surface — moon phase modulates spawn rates.
  · Shelter detection: players inside a structure receive reduced surface-mob spawn pressure.

  Survival-Specific Capability: NATURAL REGEN AND SATURATION
  · Saturation above 0: +0.5 HP per 80 ticks (4 seconds).
  · Saturation at 0, hunger ≥ 18: +0.5 HP per 80 ticks (normal regen).
  · Hunger < 18: regen disabled. Hunger ≤ 1: health drain active (difficulty-dependent).
  · Peaceful difficulty: health regenerates at full rate regardless of hunger.



  ADVENTURE

2.2  Adventure
The guided-experience mode. The player is a participant in a world, not its architect. Resources are scripted — provided by the map, not gathered. The world is still hostile and the player still has survival needs, but building is locked behind explicit world-designer permission. Intended for quest maps, story experiences, and curated content.

Active traits
Trait
Effect in Adventure
HEALTH
Full survival health model. Same HP pool, damage sources, death, and respawn mechanics as Survival. The threat of death is the mode's primary tension.
HUNGER
Full hunger model. Depletion rates identical to Survival. Food must be sourced from world-placed items or mob drops — no farming or crafting unless explicitly permitted by the map.
MOBTARGET
All hostile AI targeting is active. Mobs function identically to Survival. This is intentional: the map designer uses mob placement as an environmental design tool.

Inactive traits
BUILD — block interaction is blocked by default. Map designers grant BUILD selectively using item tags (CanPlaceOn / CanDestroy) on specific items, enabling targeted construction puzzles without opening general world modification.

Extended capabilities
  Adventure-Specific Capability: TAGGED ITEM PERMISSIONS
  · Items may carry CanPlaceOn:[block_type] and CanDestroy:[block_type] NBT tags.
  · A player in Adventure mode with a tagged item can interact with the specified block types only.
  · Untagged items cannot interact with any block. No block-selection outline appears for untagged items.
  · Tags are set by the map designer via command or loot table — never by the player.
  · This allows puzzle-construction sequences: 'use the lever key to open the door' is implemented
    as a lever with CanDestroy:obsidian, preventing its removal from the frame.

  Adventure-Specific Capability: SCRIPTED RESOURCE ACCESS
  · Item drops from mobs and chests are fully accessible.
  · Crafting, smelting, and enchanting are available only if the map designer places those blocks
    and assigns interaction permission via region flags.
  · This creates deliberate resource scarcity — the player has exactly what the map provides.

  Adventure-Specific Capability: CHECKPOINT SYSTEM (recommended extension)
  · On death, respawn at the most recently activated checkpoint rather than world spawn.
  · Checkpoint activation: player walks over a defined block or trigger zone.
  · Inventory is optionally preserved on death (configurable per checkpoint).
  · Checkpoints can grant/revoke traits — e.g., crossing into a ghost sequence grants NOTARGET
    and INVISTOPLAYERS temporarily, returning to MOBTARGET at the sequence exit trigger.

  Adventure-Specific Capability: COMBAT DIFFERENTIATION FROM SURVIVAL
  · Hunger depletion rate may be set to 0.8× survival rate — adventure pacing is slower.
  · Map-designer may apply a regional health modifier: certain zones deal double damage
    (dungeon interior) or half damage (training areas) via region property.
  · Adventure mode players are flagged for NPC dialogue trigger eligibility — NPCs ignore
    Creative and Crash Dummy players by default.



  CREATIVE

2.3  Creative
The uninhibited construction mode. The player is outside the survival system entirely — no health, no hunger, no mob threat, no gravity during flight. The world is a workspace. Intended for builders, designers, and world administrators working on world content.

Active traits
Trait
Effect in Creative
BUILD
Block placement is unlimited — no inventory consumption. Creative inventory provides all block types in infinite quantity. Block breaking is instant regardless of material, tool, or enchantment.
FLY
Flight is available at all times. Double-jump to toggle. Flight speed: 10.92 m/s horizontal by default, configurable. No fall damage accumulation while flying. Landing while flying does not trigger fall damage.
IMMUNE
All damage, knockback, and negative effects cancelled. Fire does not apply fire-tick. Void does not apply void damage. Explosions do not knock back. Creative players cannot be killed.
NOTARGET
No mob will acquire this player as a target. Mobs that the player walks through do not pathfind or attack. The player can work in a mob-occupied area without interruption.

Inactive traits
HEALTH, HUNGER — no survival UI. HP and food bars are hidden. Damage events do not apply.
MOBTARGET — no passive mob aggro. Combined with NOTARGET, the player is fully invisible to mob AI.

Extended capabilities
  Creative-Specific Capability: INSTANT BUILD TOOLS
  · BUILD in creative mode + middle-click on a placed block: clones that block to the active slot.
  · BUILD in creative mode + shift-middle-click: clones block including NBT data (chest contents,
    sign text, spawner entity type, etc.).
  · No block-break animation plays — blocks disappear on first hit.
  · Items broken by block-breaking do not drop — they cease to exist.

  Creative-Specific Capability: UNRESTRICTED ITEM ACCESS
  · Creative inventory contains every registered block and item type.
  · NBT-edited items can be carried and placed by creative players (survival players cannot
    pick these up from creative player drops without server-side permission).
  · Creative players can apply arbitrary enchantment levels via /enchant without limits.
  · Creative players can set entity attributes directly via item-NBT interaction.

  Creative-Specific Capability: WORLD ADMINISTRATION INTEGRATION
  · Creative mode players are automatically granted the build-admin permission node,
    allowing interaction with region-protected areas that standard players cannot modify.
  · WorldEdit and similar region-selection tools are available to Creative mode players.
  · Creative mode players can interact with command blocks directly (shift-click to open).
  · Region protection audit: Creative mode players see region boundary particles overlaid
    on the world when holding a region wand item.

  Creative-Specific Capability: SPAWN AND ENTITY TOOLS
  · Creative mode players can spawn entities by placing spawn eggs (available in creative
    inventory) regardless of regional mob-spawn restrictions.
  · Creative mode players can directly pick up entity data by shift-clicking an entity
    with an empty hand — produces a spawn egg for that entity with full NBT copy.
  · Creative mode players can teleport entities by holding their spawn egg and using
    the place-block action at a target location.



  CRASH DUMMY

2.4  Crash Dummy
The stress-test and systems-development mode. The Crash Dummy player is immune and can fly and build, but is fully visible to mob AI — mobs will target, pathfind to, and attack the player continuously. Damage is cancelled by IMMUNE, so attacks deal zero effective HP loss. This mode is used to test mob AI behaviour, aggro ranges, pathfinding through structures, combat mechanics, and entity interaction patterns without putting the tester at actual risk.

The name is intentional: a Crash Dummy is the entity that absorbs impacts so the real vehicle design can be evaluated. The Crash Dummy player absorbs mob attention and AI processing to observe how systems behave under live conditions.

Active traits
Trait
Effect in Crash Dummy
BUILD
Full creative-style build access — instant break, no inventory consumption. Used to set up test scenarios (spawn pads, observation platforms, trap structures) mid-test.
FLY
Flight available for positioning and observation. A Crash Dummy player can fly above a mob encounter to observe pathfinding, then descend to trigger aggro.
IMMUNE
All damage cancelled. Mob attacks register as events and are logged, but HP is never modified. Knockback is also cancelled, preventing the tester from being displaced during observation.
MOBTARGET
This is the defining trait of Crash Dummy. Mobs actively target and pursue the player as normal. The full AI pathfinding, aggro state machine, and attack resolution loop runs — attacks simply deal 0 damage on impact. Mobs do not know the player is immune.

Inactive traits
HEALTH, HUNGER — survival systems absent. Not relevant to a test context.
NOTARGET — explicitly absent. Mob targeting is the purpose of this mode. NOTARGET would defeat it.

Extended capabilities
  Crash Dummy-Specific Capability: ATTACK EVENT LOGGING
  · Every mob attack that resolves against a Crash Dummy player is logged to the debug channel:
    timestamp | attacker entity ID | attack type | nominal damage | actual damage (0) | knockback vector
  · Log output can be streamed to a file or a dedicated Discord webhook for offline analysis.
  · Aggregate logging: after a timed session, a summary report is generated:
    total attacks, attacks per mob type, average attack interval, pathfind success rate.

  Crash Dummy-Specific Capability: AI OBSERVATION OVERLAY
  · Crash Dummy players see a debug overlay (client-side rendering) showing:
    — Mob aggro state (passive/alert/combat) as a colour-coded label above each mob
    — Active pathfinding waypoints as floating particles along the mob's calculated route
    — Line-of-sight cone visualised as a particle fan from the mob's head
    — Target lock indicator: red line from mob to current target entity
  · Overlay is toggled with a configurable keybind, default [F7].

  Crash Dummy-Specific Capability: SCENARIO SCRIPTING TOOLS
  · Crash Dummy players can access a scenario palette: a hotbar-bound tool set for testing.
    Slot 1: Mob Spawner Wand — right-click to spawn configured mob at crosshair position.
    Slot 2: Scenario Timer — starts/stops a timed test window; logs start/stop timestamps.
    Slot 3: Entity Freeze — suspends all non-player entity AI ticks in a 16-block radius.
    Slot 4: Scenario Reset — clears all entities spawned since scenario start.
    Slot 5: Waypoint Marker — places a visible test-marker particle at crosshair position.
  · Scenario sessions can be named and saved as replay files for comparison.

  Crash Dummy-Specific Capability: AGGRO TRIPWIRE MONITORING
  · The Crash Dummy player can place invisible Aggro Tripwires — boundary markers that log
    whenever a mob AI pathfinder crosses them.
  · Use case: verify that a mob pursues the player through a corridor rather than taking
    a shorter direct path through walls (pathfinding correctness check).
  · Tripwires are visible only to Crash Dummy players; invisible to all other modes.




3.  Trait Specifications
Each trait is defined independently of any specific mode. Traits are the atomic units — modes are just named configurations of them. Each specification covers: the precise effect, the implementation hookpoint, and the interaction rules with other traits.

3.1  BUILD
Can place and break blocks in the world.
The player may interact with the world's block grid — placing blocks from inventory, breaking blocks to collect them, and using all block-interaction events (doors, levers, buttons, crafting tables, furnaces). Without BUILD, all block interaction attempts are silently ignored and the hand-swing animation does not produce any world state change.

  Implementation hookpoint
On each block-interaction event, check player.gamemode.hasTrait('build'). If false, cancel event before world mutation. Client-side: suppress block-selection outline if missing.

Interaction rules
Requires inventory system active (implied by presence of this trait).
Dragon Builder bending-based placement uses a separate BUILD_BEND trait variant that bypasses the item-in-hand requirement.
In regions with protection flags, BUILD is further filtered by region permission regardless of gamemode.



3.2  HEALTH
Has an active health pool that can be depleted by damage.
The player's health value is mutable — it can be reduced by all damage sources (fall damage, combat, environmental hazards, void, suffocation) and regenerated by natural regen, food saturation, or healing items. Without HEALTH, all incoming damage events are cancelled before HP is modified. The health bar UI is suppressed when this trait is absent.

  Implementation hookpoint
On DamageEvent, check player.gamemode.hasTrait('health'). If false, cancel damage. Health bar render: hide if missing.

Interaction rules
HEALTH absence does not imply immunity to knockback — knockback is a physics event, not a damage event. See IMMUNE.
HEALTH absence and IMMUNE coexistence: IMMUNE additionally cancels knockback and status effects. HEALTH alone cancels HP reduction only.
Death (HP = 0) triggers respawn flow only if HEALTH is active. Dead player without HEALTH cannot exist.



3.3  HUNGER
Has an active hunger pool that depletes over time and with exertion.
The player's hunger value is mutable — it decreases at rest and accelerates with sprint, jump, combat, and mining. At hunger = 0, health regeneration stops (if HEALTH is also present) and at near-zero hunger, walking speed is reduced and sprint is disabled. Hunger is restored by consuming food items. Without HUNGER, all hunger depletion events are cancelled and the food bar UI is hidden.

  Implementation hookpoint
On HungerTickEvent and FoodConsumeEvent, check trait. If missing, cancel depletion and skip saturation. UI: suppress food bar.

Interaction rules
HUNGER without HEALTH: hunger depletes but zero-hunger health damage does not apply (no HP to damage).
Hunger depletion rate may be multiplied by a gamemode scalar — survival standard rate = 1.0; adventure may use 0.8x.
Food items still trigger consume animation and sound even without HUNGER for consistency — they just have no effect.



3.4  MOBTARGET
Mobs and entities can detect, pathfind to, and attack this player.
The player is visible to hostile mob AI target selectors. Mobs will aggro, maintain line-of-sight tracking, and attack normally. Without MOBTARGET, the player is excluded from all AI target selection queries — equivalent to not existing in the mob's perception model. Mobs will not acknowledge the player's presence even if the player attacks them first (see NOTARGET for the contrast case).

  Implementation hookpoint
In MobTargetSelector, filter out players without this trait. Note: this is distinct from NOTARGET — MOBTARGET governs passive acquisition; NOTARGET governs provoked response.

Interaction rules
MOBTARGET + IMMUNE: mobs will still pathfind and swing but deal 0 effective damage. Mob attack animation fires.
MOBTARGET + NOTARGET: contradiction — MOBTARGET enables passive aggro; NOTARGET suppresses it. Resolution: NOTARGET takes precedence.
Player-owned tamed mobs ignore this flag — they use the owner relationship, not the target selector.



3.5  FLY
Can enter sustained flight mode without an elytra or other item.
The player may toggle flight by double-tapping jump. While flying: gravity does not apply, fall damage is not accumulated (regardless of HEALTH), and the player can move in all three axes at a configurable speed scalar. Without FLY, the double-jump toggle does nothing; the player is subject to normal gravity. FLY does not grant unlimited vertical reach — block interaction range is unchanged.

  Implementation hookpoint
On PlayerToggleFlightEvent, check trait. If missing, cancel. On PlayerMoveEvent while airborne and flying, skip gravity application.

Interaction rules
FLY is suspended inside certain region types (caves below depth threshold, enclosed spaces below ceiling threshold) if realism mode is enabled.
FLY + PASSTHROUGH: allows the player to fly through solid blocks. Collision is skipped only while both traits are simultaneously active.
Elytra flight is a separate code path and is not governed by FLY — it uses item-slot detection.



3.6  IMMUNE
Immune to all damage, knockback, and adverse status effects.
All incoming damage events, knockback vectors, and negative potion/status effect applications are cancelled before they modify player state. The player cannot die, cannot be moved by external forces (explosion, projectile impact, mob strike), and cannot receive Poison, Wither, Slowness, Blindness, or any negative effect. Positive effects (Speed, Jump Boost, Night Vision) are unaffected. IMMUNE is a superset of HEALTH cancellation — it additionally covers physics responses.

  Implementation hookpoint
On DamageEvent, PotionEffectEvent (negative), EntityKnockbackEvent: check trait, cancel if present. Positive PotionEffectEvent types are not filtered.

Interaction rules
IMMUNE does not prevent the player from taking fall damage from their own movement if HEALTH is also absent — no HP to reduce anyway.
IMMUNE suppresses fire-tick visual effect as well as damage for visual clarity.
IMMUNE players still trigger mob attack animations — the mob attempts, the damage resolves to zero.



3.7  NOTARGET
Mobs cannot acquire this player as a target, even when provoked.
The player is excluded from all mob target selectors, including provoked aggro. Even if the player strikes a mob, the mob will not retaliate by targeting the player — it may play a hurt animation but will not pathfind or counter-attack. NOTARGET is a stronger form of invisibility to AI than simply being invisible — it operates at the data layer, not the perception layer. Combined with MOBTARGET absence, the player is entirely transparent to mob AI.

  Implementation hookpoint
In all MobTargetSelector calls (passive, provoked, retaliation), filter players with NOTARGET. In MobHurtEvent where damager is NOTARGET player, skip target acquisition.

Interaction rules
NOTARGET does not suppress mob hurt animation or sound — the mob registers the hit, it simply doesn't retaliate.
NOTARGET + INVISTOPLAYERS: player is invisible to both mob AI and other players simultaneously.
NPC entities follow a separate target model and may ignore NOTARGET depending on their disposition setting.



3.8  INVISTOPLAYERS
Invisible to other players. Visible to the server and to themselves.
The player's entity render packet is not sent to other players' clients. From another player's perspective, the player does not appear on the world, does not appear on the minimap, does not show in the player list tab (configurable), and does not produce footstep particle effects. The invisible player can still interact with the world — block changes, item pickups, and sound effects from their actions may be detectable by observant players.

  Implementation hookpoint
On PlayerChunkLoadEvent and EntitySpawnPacketEvent, filter the invisible player from all other players' receivers. Sound events from invisible player's actions are not suppressed — only visual render packets.

Interaction rules
INVISTOPLAYERS does not affect mob AI — mobs use their own perception model. Combine with NOTARGET for full AI+player invisibility.
INVISTOPLAYERS players still appear in /list and admin-level commands. Server-side visibility is unaffected.
Armour stand entities left by an INVISTOPLAYERS player are visible — only the player entity packet is suppressed.
Block-break particles are client-rendered at the block position, not the player position — these remain visible to others.



3.9  PASSTHROUGH
Can move through solid blocks without collision.
The player's collision model is disabled against non-entity solid blocks. The player may walk, fly, or fall through any block regardless of its solidity. Fluid collision (water, lava) is still applied unless combined with appropriate immunity. Entities still collide with the player — PASSTHROUGH is block-collision only. The player does not receive suffocation damage from being inside blocks when HEALTH is active because damage is cancelled by the PASSTHROUGH trait's presence.

  Implementation hookpoint
On PlayerMoveEvent, skip block-collision resolution. On SuffocationDamageEvent, cancel if PASSTHROUGH is present. Entity-entity collision unchanged.

Interaction rules
PASSTHROUGH + FLY: the most unrestricted mobility combination — fly through any block in any direction.
PASSTHROUGH does not prevent block-interaction (door opening, button pressing) — those are interaction events, not collision events.
A PASSTHROUGH player inside a solid block sees the block's interior texture. Ensure block interior faces are rendered.
PASSTHROUGH is suspended inside force-field regions where server integrity requires boundary enforcement.



3.10  VISION
Enhanced perception — sees through fog, walls, and darkness.
The VISION trait bundles a set of enhanced perception capabilities: full-brightness rendering regardless of light level (equivalent to permanent Night Vision effect without the blue tint), suppression of all fog rendering (volumetric fog, vanilla distance fog, and system fog types from the weather integration), and entity outline rendering through solid blocks within a configurable range (X-ray outline at N blocks, default 32). The VISION trait is a tool trait intended for administrative and spectatorial use.

  Implementation hookpoint
Send Night Vision effect with ambient=true (no icon) on trait grant. Send fog suppression packet to client. Entity outline: server-side range query, send glow effect packet per entity in range.

Interaction rules
VISION entity outlines respect team colour — team members appear in team colour, others in white.
VISION does not affect other players' ability to see this player — it is a perception trait, not a visibility trait.
VISION fog suppression overrides all seven fog types from the integration plan, including Freezing Fog's rime visual — the rime surface shader is not suppressed, only the volumetric fog layer.
VISION + INVISTOPLAYERS: the invisible player can see everything but no one can see them.




4.  Trait Interaction Reference
The following table covers every meaningful two-trait combination and its resolved behaviour. Combinations not listed produce independent effects with no special interaction.

Trait A
Trait B
Interaction
HEALTH
IMMUNE
IMMUNE takes full precedence. Having HEALTH active is meaningless alongside IMMUNE — damage events are cancelled before HP is consulted. Having both is only useful for transition states where IMMUNE is temporarily removed.
HEALTH
HUNGER
Standard survival interaction: zero hunger prevents health regen and (on Normal+) causes slow health drain. Both must be present for this connection to function — HEALTH alone never drains from hunger.
MOBTARGET
NOTARGET
NOTARGET overrides MOBTARGET. If both are present (as in a scripted transition), NOTARGET wins. Mobs will not acquire this player regardless of MOBTARGET presence.
MOBTARGET
IMMUNE
Crash Dummy configuration. Mobs target and attack; damage resolves to zero. Knockback is cancelled by IMMUNE. The mob's full attack animation, sound, and event log run normally.
FLY
PASSTHROUGH
Maximum mobility. Player can fly in any direction and move through solid blocks. The combination is the closest analog to a spectator entity — but the player retains all other active traits (e.g. still visible, still targetable if MOBTARGET/NOTARGET allow).
FLY
HEALTH
Flight suppresses fall damage accumulation for as long as the flight toggle is active. If FLY is removed while the player is airborne, fall damage accumulation resumes from that height. Landing with FLY active never triggers fall damage.
NOTARGET
INVISTOPLAYERS
Combined full concealment. Invisible to player clients and absent from mob AI simultaneously. The player's world-interaction side-effects (sound, block changes, particle effects from broken blocks) remain detectable.
INVISTOPLAYERS
BUILD
An invisible player can still modify the world. Block placements and breaks are visible to other players as world-state changes — the block changes but no player is seen causing it. Sound is localised to the action position. This is the ghost-hands configuration.
INVISTOPLAYERS
VISION
Observer configuration. Cannot be seen by anyone; can see through fog, darkness, and walls. Appropriate for admin spectating and content review without disrupting the world state.
PASSTHROUGH
BUILD
PASSTHROUGH does not prevent block interaction. A player inside a solid block can still right-click adjacent blocks to open containers or trigger mechanisms. Block breaking from inside a block targets the block the player is occupying.
VISION
MOBTARGET
Survival observer edge case. A player in survival with added VISION trait can see through walls to watch mob approach — useful for content creators recording survival footage from a third perspective player.


5.  Runtime Trait Modification
Traits can be applied or revoked at runtime without a full mode switch. This enables scripted gameplay sequences that temporarily alter a player's capability profile.

5.1  Command Interface
  Command syntax
  /trait give   <player> <trait>         — Grant a single trait
  /trait revoke <player> <trait>         — Revoke a single trait
  /trait set    <player> <mode>          — Apply the full trait set for a named mode
  /trait list   <player>                 — Display the current active traits for a player
  /trait reset  <player>                 — Restore the player's assigned mode defaults
  /trait temp   <player> <trait> <ticks> — Grant a trait for N ticks, then auto-revoke


5.2  Scripted Transition Examples
  Example A — Ghost sequence in Adventure mode
  Trigger: player enters a trigger zone at a story beat
  Action:  /trait give {player} NOTARGET
           /trait give {player} INVISTOPLAYERS
           /trait give {player} PASSTHROUGH
  Result:  Player walks through walls, invisible to other players and mobs — ghost effect.
  Exit:    On leaving the sequence zone: /trait reset {player}  — restores Adventure defaults.

  Example B — Temporary immunity buff in Survival
  Trigger: player drinks a special potion item (custom item with NBT tag)
  Action:  /trait temp {player} IMMUNE 1200   (60 seconds = 1200 ticks)
  Result:  Player is immune for 60 seconds. IMMUNE auto-revokes at tick count.
  Note:    HEALTH remains active — the player has HP but cannot lose it during the window.

  Example C — Observer join Creative server
  Trigger: player joins with the observer permission node
  Action:  /trait set {player} CREATIVE
           /trait give {player} INVISTOPLAYERS
           /trait give {player} VISION
  Result:  Full creative capability with invisible observer overlay.

  Example D — Crash Dummy test warmup phase
  Trigger: test operator runs /scenario start
  Phase 1: /trait give {player} NOTARGET    — 10-second setup window, mobs ignore player
           [Tester positions themselves in test zone]
  Phase 2: /trait revoke {player} NOTARGET  — Test begins, mobs immediately acquire target
  Phase 3: /scenario end                    — /trait reset {player}, log exported



6.  Extended Capabilities
Beyond the base trait behaviour, each mode supports additional gameplay systems that layer on top of the trait foundation. These are not trait-governed — they are mode-aware systems that consult the player's current mode to determine their own behaviour.

6.1  Inventory System Variants
Mode
Inventory behaviour
SURVIVAL
36-slot player inventory. Item stacks deplete on use. Durability applies. Dropped on death (configurable to keep on death). Crafting grid in inventory screen (2×2). Full crafting via crafting table (3×3).
ADVENTURE
36-slot inventory. Items depleted normally. Death behavior matches survival. Item acquisition from world only (no bulk creative pickup). Crafting available only if map permits.
CREATIVE
Creative inventory tab (all blocks/items in infinite quantity) + standard 36-slot hotbar. Middle-click to clone blocks. Shift-middle-click to clone with NBT. Items do not deplete. Crafting available but irrelevant (all items available directly).
CRASH DUMMY
Creative-style inventory for build access. Additional fixed-slot Scenario Palette (slots 1–5 assigned to test tools, cannot be replaced during test session). Items do not deplete.

6.2  UI Elements by Mode
UI Element
Visible in mode(s)
Health bar
SURVIVAL, ADVENTURE — displayed when HEALTH is active. Hidden in CREATIVE, CRASH DUMMY.
Hunger bar
SURVIVAL, ADVENTURE — displayed when HUNGER is active. Hidden in CREATIVE, CRASH DUMMY.
Armour bar
SURVIVAL, ADVENTURE — shown when player has armour equipped and HEALTH is active.
XP bar
SURVIVAL — shown always. ADVENTURE — shown always. CREATIVE — hidden (no XP accumulation). CRASH DUMMY — hidden.
Crosshair block-select outline
All modes with BUILD active. Hidden when BUILD is absent (Adventure with no tagged item in hand).
Mob AI overlay (F7)
CRASH DUMMY only. Shows aggro state labels, pathfinding waypoints, LOS cone, target lock line.
Attack event log (sidebar)
CRASH DUMMY only. Scrolling log of incoming attack events with attacker, nominal damage, timestamp.
Mode indicator badge
All modes — a small persistent badge in the corner of the HUD shows the current mode name and active trait icons. Administrative trait additions (VISION, PASSTHROUGH, INVISTOPLAYERS) show as additional icons.
Region boundary particles
CREATIVE — when holding region wand item. CRASH DUMMY — when near aggro tripwire markers.

6.3  World Interaction Capabilities by Mode
Interaction
SURVIVAL
ADVENTURE
CREATIVE
CRASH DUMMY
Break blocks (own inventory)
YES (tool-speed)
Tagged items only
YES (instant)
YES (instant)
Place blocks
YES (consumes)
Tagged items only
YES (infinite)
YES (infinite)
Open containers
YES
YES
YES
YES
Craft items
YES
Map-permitted only
YES (irrelevant)
YES (irrelevant)
Use command blocks
NO
NO
YES (shift-click)
YES (shift-click)
Spawn entities
Via spawn eggs only
NO
YES (spawn eggs)
YES (scenario wand)
Pick up entity data
NO
NO
YES (shift-click)
YES (scenario tools)
WorldEdit operations
NO
NO
YES (with perm)
YES (scenario set)
Trigger NPC dialogue
YES
YES
NO (NPC filter)
NO (NPC filter)

6.4  Death and Respawn Behaviour
Mode
Death / Respawn behaviour
SURVIVAL
Death at 0 HP. Items dropped at death location (or kept, if keepInventory gamerule active). XP dropped as orbs. Respawn screen shown. Respawn at world spawn or bed/respawn anchor. Death counter incremented.
ADVENTURE
Death at 0 HP. Respawn at last activated checkpoint (if checkpoint system active) or world spawn. Inventory persistence configurable per checkpoint. Death message displayed to team/server.
CREATIVE
HEALTH is inactive — HP cannot reach 0. Void damage is cancelled. Death cannot occur. No respawn screen, no death event.
CRASH DUMMY
IMMUNE cancels all damage — HP cannot reach 0. Death cannot occur. If IMMUNE is temporarily revoked in a test scenario and the player somehow reaches 0 HP, they respawn instantly with full HP and IMMUNE is automatically reapplied.


7.  Appendix — Quick Reference
7.1  Mode Trait Sets (Machine-Readable)
  YAML format — gamemode trait definitions
  gamemodes:
    survival:
      traits: [BUILD, HEALTH, HUNGER, MOBTARGET]
    adventure:
      traits: [HEALTH, HUNGER, MOBTARGET]
    creative:
      traits: [BUILD, FLY, IMMUNE, NOTARGET]
    crash_dummy:
      traits: [BUILD, FLY, IMMUNE, MOBTARGET]

  # Administrative traits (not assigned to any base mode):
  # NOTARGET, INVISTOPLAYERS, PASSTHROUGH, VISION
  # Apply at runtime: /trait give <player> <trait>

7.2  Conflict Resolution Priority
Conflict
Resolution
MOBTARGET vs NOTARGET
NOTARGET wins — mobs do not target
HEALTH vs IMMUNE
IMMUNE wins — damage events cancelled before HP check
FLY active, player falls
Fall damage suppressed regardless of HEALTH state
PASSTHROUGH + suffocation
PASSTHROUGH cancels suffocation damage
VISION + any fog type
All volumetric fog suppressed; rime surface shader persists

End of Gamemode Trait System Specification
