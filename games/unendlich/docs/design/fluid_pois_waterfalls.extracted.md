
Fluid POIs
Waterfalls, Lavafalls, Veil Gates & Hybrid Systems
World Generation · Environmental Properties · Bending Interaction · Acoustic Profiles · Crafting Sway · Mob Ecology · Dragon Visibility · Formation Dictionary
A waterfall is not a visual feature. It is a singularity in the flow field where velocity spikes, phase changes occur, erosion becomes vertical and catastrophic, and air and water interpenetrate. Every type generates a distinct environmental property signature, acoustic field, bending context, and mob behavioural environment.

1.  Waterfalls as Multi-System Events
In a procedural world, a waterfall is a state transition between elevation systems. The river system document defines rivers as flow fields on a landscape. Waterfalls are the singularities in those fields — points where:

Velocity spikes from steady laminar flow to free-fall chaotic jet (5–60 m/s depending on type)
Phase changes occur: continuous sheet → droplets → aerosolised mist — the water transitions through three physical states along a single vertical distance
Erosion mode switches from lateral (river meandering) to vertical catastrophic (plunge erosion)
Air and water become a single mixed medium — the mist plume is neither fully gas nor fully liquid
Acoustic energy is concentrated: RT60 values of 3–30 seconds, the natural reverb equivalent of a cathedral

Each of these properties connects to an existing Unendlich system. Mist connects to the fog system and dragon visibility. RT60 connects to the Choir/voice echo and Lauscher acoustic ecology. Phase transitions connect to environmental properties H, W, R. Erosion history connects to Information Bending Temporal Echo read depth. The plunge pool is a deep cave biome entry point.

1.1  Environmental Property Profiles — Summary
POI Type
H
R
W
P
S
L
B
Alpine Plunge
+0.35
−0.20
+0.18
−0.05
+0.08
−0.05
0
Tiered / Tufa
+0.30
−0.10
+0.08
0
+0.04
+0.06
+0.12
Curtain
+0.40
−0.15
+0.22
−0.08
0
+0.08
0
Veil Gate
+0.48
−0.18
+0.05
+0.10
0
−0.15
0
Cave Waterfall
+0.42
0
0
+0.20
+0.10
−0.25
+0.08
Frozen Waterfall
+0.15
−0.40
+0.05
+0.05
0
+0.12
+0.05
Thermal Waterfall
+0.38
+0.30
+0.12
+0.08
+0.06
−0.08
+0.20
Lavafall
+0.05
+0.70
+0.15
0
+0.12
−0.10
+0.30
Mega Waterfall
+0.55
−0.20
+0.35
−0.15
+0.05
+0.05
0
Hybrid Lava-Water
+0.25
+0.50
+0.28
+0.15
+0.15
−0.20
+0.25

Property modifiers listed are the peak values within 30 blocks of the waterfall centroid, decaying with inverse-square falloff to zero at 120 blocks. B column: base material mineral richness (Brine affinity marker — affects which crafting sway marks are possible).



  🎵  Acoustic Integration — RT60, Choir, and Lauscher Ecology

2.  Acoustic System Integration
Waterfalls are the most acoustically extreme environments in the world. Every waterfall type has a characteristic reverberation time (RT60) — the time for a sound to decay by 60 dB from its peak. This directly integrates with three existing systems:

Choir / voice echo: high RT60 means voice shapes linger. A bender performing Rise (Earth shape) at a cave waterfall with RT60 = 20s will hear their own Choir note decay for 20 seconds, interfering with or amplifying subsequent shapes depending on timing.
Lauscher acoustic ecology: Lauschers near high-RT60 sites accumulate extremely degraded audio in their 6-slot database — the waterfall's broadband noise fills every slot with corrupted signal within 48 in-game hours of proximity. A Lauscher that lived near a cave waterfall for a week broadcasts only white noise and waterfall echo.
Formation sway for Information Bending: at high-RT60 sites, Information Bending techniques have their temporal echo range automatically extended. The acoustic environment is already storing echo — the bender reads from that stored field.

POI Type
RT60 range
Bending and acoustic consequence
Alpine Plunge
3–10 s
Choir shapes: echo with 2–4 distinct repeats before fading. Water bending shapes (Surge, Flood) receive natural reverb amplification — their Choir trigger word echoes back and can re-trigger the technique outline. Lauscher: curious but disoriented.
Tiered / Tufa
4–12 s
Stacked echo delays match tier spacing. Earth and Water shapes layer naturally — a bender learning to use the tier echo as a timing cue gains a passive chi cost reduction. Lauschers near tufa systems have richer audio databases.
Curtain
6–15 s
Broadband roar with low-frequency dominance. Choir shapes at curtain sites have their bass partials (1–4) amplified by the waterfall's low-frequency field. Storm bending arc shapes are masked in pitch but spatially directed.
Veil Gate
6–18 s
Dual-layer: exterior roar + interior echo cavity. Inside the veil: low frequencies amplified, high frequencies collapse. Information Bending Temporal Echo range extended +3 seconds. The cave 'breathes' with 0.3 Hz pressure oscillations (piston effect).
Cave Waterfall
10–30 s
Most extreme natural echo in the world. Sound becomes spatially disorienting. Lauschers near cave waterfalls are the most audio-corrupted creatures in the game. Information benders gain Temporal Echo range +6 seconds. Voice shapes become structurally entrained to the cave's resonant frequencies.
Frozen Waterfall
2–6 s
Damped. High frequencies collapse. Sound is brittle and sharp — short reverb tail with fast high-frequency cutoff. Choir shapes at frozen sites feel percussive. Earth bending shapes (especially Strike forms) are very clean.
Thermal Waterfall
3–8 s
Low-pass filtered by steam density. Sound is muffled and heavy. Steam absorbs frequencies above 800 Hz. Choir shapes at thermal sites lose their upper partials — the partial series 8–16 are absorbed. Information bending shapes sound unusual.
Lavafall
2–8 s
Masked by thermal wind noise and gas venting. Low bubbling rumble + occasional cracking pops. Choir shapes are audible but compete with the lavafall's ambient sound. Earth bending Seismic pulse is particularly readable through the lava noise.
Mega Waterfall
10–30 s
Continuous low-frequency roar bed — audible kilometres away. Local weather effects dampen high-frequency Choir shapes. The mega waterfall produces its own standing wave pattern that can interfere with or amplify bending Choir shapes at 1–4 Hz wavelength resonances.



  💧  Alpine Plunge Waterfall    ★ ★★★★★★★☆☆
  Glacial hanging valley jet — bright white ribbons, cyan core, constant mist halo
  WORLD GENERATION REQUIREMENTS
    Biome: mountainous, glacial adjacency, y > 120.
    Rock type: granite or gneiss (hardRock flag). Soft rock biomes produce tiered falls instead.
    Elevation drop: ≥ 24 blocks within 8 horizontal blocks.
    River source: tributary river with source at y > 150 discharging over main valley edge.
    Rarity: 1 per 400×400 area in valid biome. High spawn rate in glacial biomes near y=160+.

  BENDING INTERACTION
    Water: massive amplification. H ≥ 0.65 at centroid. Flood technique: free (H source available).
             Surge: reach +6 blocks from ambient water supply. Tidal broth effect doubled at plunge pool.
    Air: W ≥ 0.50 in mist plume zone. Storm Ride recharge possible from updraft column above falls.
             Pressure Lance: +4 blocks reach from water-air interaction at mist boundary.
    Earth: Seismic Read reveals 3000+ years of plunge pool erosion history as a Temporal Echo ghost.
             The plunge pool stone is one of the geologically 'loudest' places for Information Bending.
    Storm: Mist ionisation creates passive arc conductor path through the mist column.
    Fire: Suppressed. R ≤ 0.30 at centroid from cooling mist. Thermal Phase techniques unavailable.
    Information: Scatter field naturally active within mist range (40 blocks). Dragon visibility −35%.

  CRAFTING SWAY (at forge placed within 20 blocks)
    Water sway: guaranteed (H ≥ 0.65 ambient).
    Air sway: moderate (W ≥ 0.50 in mist plume, threshold met).
    Earth sway: available at plunge pool base (S ≥ 0.50 from constant impact vibration).
    Fire sway: unavailable.
    Formation sway: Hanging Valley sway (plunge pool formation) — unlocks Altitude Breath infusion
                   at forge near an Alpine fall without requiring the Storm Tower Anvil.

  MOB ECOLOGY
    Alpine cave gliders roost in the mist halo (W > 0.50, H > 0.60).
    Pack predators approach the plunge pool for water — their bounding region tracking is degraded
    by the ambient noise (constant S pulse from impact degrades Seismic Read baseline).
    Aerial raptors use the thermal column above the mist as a soaring assist.
    Lauschers near alpine falls acquire a distinctive pattern: audio databases fill with waterfall hiss.
    Mob FSM: ALERT radius reduced 20% from ambient noise masking. Hunting convergence slower.
    Dragon visibility to prey in mist plume: fog-shape rule applies — thin mist at steep angle
    barely clips the sightline. Medium mist (ρ=0.15, height=8): T=0.18 from hover. Real cover.

  FORMATION DICTIONARY ENTRIES
    plunge pool — the deep basin at the base of a vertical fall (canonical geological term)
    hanging valley — the elevated tributary valley above the main glacial trough
    glacial lip — the cliff edge where the hanging valley drops free
    mist halo — the persistent aerosol zone within 20 blocks of any major fall
    splash apron — the fan of wet rock at the plunge pool perimeter

  VISUAL / SHADER SPECIFICATION
    Shader: vertical velocity field driving foam at high intensity (foam: high, spray: extreme).
    Particles: high-velocity sphere at crest, breakup into droplet field at mid-fall,
               mist particle system at plunge pool (same volumetric particle renderer as fog system).
    Mist: water sway fog layer generated: H-type mist, height 8 blocks, ρ = 0.15–0.35.
    Rainbow: if sun angle > 30° and mist density > 0.10: dispersion prismatic arc rendered
              using RGB channel offset technique (same as Information void shader).
    Plunge pool: Worley-noise surface distortion driven by impact vector. Caustic ceiling below
                 water surface. Cave glider membrane translucency technique for foam layer.
    LUT: cool blue push, high contrast at white fall vs dark rock.

  PLAYER HAZARDS
    Entry into waterfall stream: horizontal force applied (3 m/s away from fall centreline).
    Plunge pool depth: 6–12 blocks depending on fall height. Swimming required.
    Fall into plunge pool from height: damage = (fall_height_above_pool − 8) × 0.5 HP per block.
    Mist exposure at R < 0.15: hypothermia onset after 60 seconds. Movement speed −15%.


  🌿  Tiered / Tufa Waterfall    ★ ★★★★★★★☆☆
  Staircase of glowing water steps — turquoise pools, fog layering between levels
  WORLD GENERATION REQUIREMENTS
    Biome: karst, limestone, or travertine-heavy terrain.
    Rock type: alternating hard/soft layers in the heightmap (deterministic from rock_layer_seed).
    Elevation drop: 16–48 blocks distributed across 3–7 distinct tier lips.
    Water: moderate-flow river with mineral content (B ≥ 0.15 in source biome).
    Rarity: 1 per 280×280 area in karst biome. Common near cave system entrances.

  BENDING INTERACTION
    Water: H ≥ 0.60 at all tiers. Inter-tier pools are stable Water bending platforms —
             techniques fired from a pool surface benefit from a stable source bonus: chi cost −8%.
    Earth: mineral deposition (B = +0.12) is accessible via Seismic Read — the travertine
             layers store a geological memory of every mineral pulse from the source spring.
             Ancient tufa systems (age flag ≥ 500 years in world-gen) have 4× echo depth.
    Brine: the mineral suspension in the water column enables Brine techniques without
             the normal H + mineral_content prerequisites. The water IS the suspension.
    Information: the stacked echo delays create a natural timing reference for Temporal Echo.
                   Each tier's echo delay corresponds to a distinct time-slice in the acoustic past.

  CRAFTING SWAY
    Water sway: guaranteed.
    Earth sway: moderate (B > 0.12 from mineral deposition — Earth has mineral affinity).
    Brine sway: available if mineral content B > 0.18 at forge site.
    Formation sway (Tufa Terrace): items crafted at a forge placed on a travertine shelf gain
                    the Mineral Suspension infusion automatically at no slot cost.

  MOB ECOLOGY
    Biofilm development on tier faces supports micro-fauna colonies.
    Cave mycelium analogues colonise the perpetually-wet tier faces — faint bioluminescence
    (L +0.06 at night on wet rock faces, same shader as cave_mycelium_crimson texture).
    Small aquatic creatures rest in inter-tier pools during the day, hiding from aerial raptors.
    Lauschers find tiered falls acoustically rich — 'stacked echoes' produce distinct per-tier
    audio signatures that fill a Lauscher's database with complex multi-delay material.

  FORMATION DICTIONARY ENTRIES
    tufa terrace — calcium carbonate step formation built by mineral-laden water
    travertine shelf — similar deposition shelf, harder and more crystalline than tufa
    bio-terrace — a terrace where biofilm has colonised the face (appears teal-green)
    inter-tier pool — the stable pool between two cascade lips
    lip collapse — the geological event when a tier's leading edge breaks and steps forward

  VISUAL / SHADER SPECIFICATION
    Per-tier: separate flow shader instance with Subsurface scattering in shallow inter-tier pools.
    Wet rock: specular stacking — each tier face has a wet multiplier on roughness (×0.40).
    Biofilm: teal-green overlay texture on tier faces when biological age flag > 10 years.
    Mineral deposits: visible crystal growth on tier lip edges (Worley-based crystal node shader).
    Mist: fog pockets accumulate in terrace concavities — not a uniform mist field but a
           spatially-trapped fog that sits in inter-tier pools. ρ = 0.20–0.40 in the pocket,
           dissipating rapidly above pool rim.


  🌈  Curtain Waterfall    ★ ★★★★★☆☆☆☆
  Entire cliff becomes liquid glass — sheet flow, wall of sound, constant rainbow
  WORLD GENERATION REQUIREMENTS
    Biome: basalt plateau, escarpment cliff, post-glacial valley. Uniform resistant rock required.
    Rock type: basalt or dark_serpentinite (uniform_hard_rock flag).
    Elevation drop: ≥ 16 blocks, cliff width: ≥ 24 blocks (creates sheet flow geometry).
    River: high-discharge river reaching the cliff edge — volume requirement is highest of all types.
    Rarity: 1 per 600×600 area. Rare globally but always dramatic when generated.

  BENDING INTERACTION
    Water: the continuous sheet is a massive Flood technique output — channelling a reverse-Flood
             (drawing water back) to create a passage through the curtain costs 60 chi.
             Moving through a curtain fall counts as a Water bending stance technique.
    Air: the massive aerosol plume creates W ≥ 0.60 in a 60-block radius. Storm Ride is trivial
           here. Curtain falls are preferred training sites for Air benders.
    Information: rainbow formation creates a persistent prismatic scatter field — same optical
                   physics as the Scatter Lens instrument. Information Bending in a rainbow zone:
                   light path reading extends to 30 blocks (normally 15).
    Storm: arc discharge through the sheet is spectacular and costs −15% chi (highly conductive path).

  CRAFTING SWAY
    Water sway: very strong (highest H value of surface falls at +0.40).
    Air sway: strong (W ≥ 0.60 in plume).
    Information sway: available if rainbow is active (sun angle satisfied).
    Formation sway (Sheet Edge): items crafted at the cliff lip (at the fall's origin, not base)
                    gain Bridge enchantment capacity automatically.

  MOB ECOLOGY
    The curtain itself is a barrier — mobs on the cliff top and base are separated.
    Aerial creatures use the updraft column (W > 0.60) for sustained soaring.
    The rainbow zone is a visual anchor point for Information-attuned creatures.
    Dragon approach through a curtain fall: the sheet provides geometric occlusion (100% block
    for prey directly behind the curtain at steep angles). A dragon must approach from the side.

  FORMATION DICTIONARY ENTRIES
    veil fall — a curtain waterfall spanning the full cliff width
    arc pool — the plunge basin where a rainbow perpetually forms
    sheet edge — the cliff lip from which sheet flow develops
    curtain face — the vertical surface of the flowing sheet
    spray forest — the unusual vegetation sustained by constant mist at a curtain base

  VISUAL / SHADER SPECIFICATION
    Sheet: high transparency with strong Fresnel edge glow. The sheet thins as it falls —
             the top 4 blocks are near-opaque, the middle are translucent, the bottom 2 blocks
             before impact are nearly invisible (pure mist). Dual-sided rendering required.
    Rainbow: dispersion layer — RGB channel separation ±4px, rendered as a separate additive pass
             when sun_angle > 30° and ρ > 0.10. Full prismatic arc, not just a halo.
    Mist wall: volumetric mist at ρ = 0.30–0.55, height 20+ blocks. This is the fog-shape
                principle in reverse — mist from a curtain fall is TALL, providing maximum cover
                from aerial predators. A dragon hovering above sees T = exp(−0.40 × 20) = 0.0003.



  🌀  Veil Gate (Hidden Entrance Waterfall)    ★ ★★★★★★★★★
  Bright curtain — dark hollow behind — breathing cavity — mist that masks everything
  WORLD GENERATION REQUIREMENTS
    Derived from: Curtain Waterfall with additional undercutting condition.
    A Veil Gate generates when a Curtain Waterfall's erosion cycle produces a recession cavity
    of ≥ 8 blocks depth behind the sheet. This requires: cliff age flag ≥ 200 years AND
    horizontal soft_rock_layer adjacent to hard cap rock.
    Generation: hard_cap_rock → undercutting → plunge pool deepening → cliff recession → hollow.
    Rarity: 1 per 3 Curtain Waterfalls generated. About 1 per 1800×1800 area.
    Interior space: 8–20 blocks wide, 4–12 blocks tall, 6–16 blocks deep. Always dry.

  INTERIOR ENVIRONMENT
    Temperature: 6–18°C (mild — the waterfall moderates extremes).
    Humidity: H = 0.85–0.95 (saturation). The interior is perpetually damp.
    Airflow: pulsing at 0.3 Hz — the cave 'breathes' from pressure oscillations at impact.
               This piston effect means the interior alternates between slight positive and negative
               pressure relative to exterior. Audible as a 0.3 Hz rhythm in the interior air.
    Visibility: exterior to interior — 0% (mist + spray + curtain opacity blocks all view).
                 Interior to exterior — 100% (looking out through the mist is like looking through glass).
                 The asymmetry is what makes veil gates feel magical.

  BENDING INTERACTION
    Information: the interior is the best natural Information Bending environment in the world.
                   The 0.3 Hz breath rhythm, the dual-layer acoustics, and the mist-scatter field
                   combine to extend Temporal Echo range to +5 seconds. The interior has been
                   'breathing' for 200+ years — each breath is a stored acoustic event.
                   A bender performing Temporal Echo inside a Veil Gate hears the waterfall's
                   entire erosion history in acoustic fragments.
    Water: the curtain itself is manipulable from the interior — a bender can reverse-flow the
             curtain (creating a passage) for 40 chi. This is how Veil Gate exits are accessible
             from inside without drowning.
    Earth: the recessed cavity is a geological formation — Seismic Read in the interior shows
             the erosion history as a 3D spatial echo of the original cliff face.
    Air: the 0.3 Hz breath rhythm is an Air bender's environment — they can amplify the rhythm
           to produce a sustained pressure differential that drives the piston effect further,
           creating an acoustic wave usable as a Vacuum Snare technique at triple range.

  CRAFTING SWAY
    A forge placed inside a Veil Gate interior receives ALL of: Water sway, Information sway,
    and a unique Veil Gate sway not available at any other location:
    Veil Gate sway: items gain 'asymmetric visibility' — equipped by the player, dragon fog
                     sensitivity scalar is multiplied by 1.8 (better than the Void Cloak enchantment).
                     The item reflects the Veil Gate's own visual asymmetry.
    Crafting at a Veil Gate forge: highest Information Bending genome output of any location
    except the Resonance Font (which ties). Best location for Scatter Mantle set crafting.

  QUEST AND WORLD SIGNIFICANCE
    The Veil Gate is the world's most frequent hidden location entry point. Veil Gates contain:
      · Ancient inscriptions: 40% chance (the shelter's historical use).
      · Rare item: 60% chance (the location is a natural cache site for any creature that found it).
      · Lauscher den: 25% chance — Lauschers strongly prefer Veil Gates as roosting sites.
        A Lauscher in a Veil Gate has the richest audio database in the world (dual acoustic field).
    The Lauscher's Gift quest chain Stage 2 directs the player to follow a Lauscher that roosts
    in a Veil Gate — the degradation loop phrase ('The font is where the stone speaks light')
    was originally recorded in a Veil Gate and degraded outward from that location.

  FORMATION DICTIONARY ENTRIES
    veil gate — the hidden cavity behind a curtain waterfall (canonical Unendlich term)
    curtain cave — a recessed pocket space behind a veil fall (geological term)
    breathing hollow — the piston-effect ventilated cavity characterised by rhythmic airflow
    hollow reach — the deepest accessible point of the veil gate interior
    mirror exit — the interior surface of the curtain, through which the exterior is visible

  VISUAL / SHADER SPECIFICATION
    Dual-space rendering: exterior volumetric waterfall sheet + interior wet rock GI bounce.
    Interior wet rock: roughness × 0.25 (maximum wetness). PBR reflections at near-mirror level.
    Interior fog: dense mist trapped inside (ρ = 0.40–0.60 near the curtain wall, ρ = 0.05
                   at the back wall). The gradient is dramatic — the front is fog, the back is clear.
    Curtain translucency from interior: the sheet appears almost entirely transparent from inside —
                   the 'liquid glass' effect. From exterior: completely opaque white spray.
    Piston breath visible: particle wisps move toward and away from the curtain at 0.3 Hz.
    Ancient inscription rendering: if an inscription exists, it is carved stone (v2 POM depth).

  PLAYER EXPERIENCE
    The Veil Gate is the canonical 'discovery moment' in the world — the moment a player
    walks through the curtain and finds the interior. The experience is scripted to be cinematic:
      1. Approaching exterior: vision gradually obscures with mist. Audio: roar increases.
      2. Entering the curtain: 0.5-second 'through water' visual distortion (chromatic aberration,
         waterfall sheet fills screen, then clears to the interior space).
      3. Inside: sudden quiet. The roar is outside. The interior has only the 0.3 Hz breath.
         RT60 = 12 s. Dripping. The curtain is now a window — the exterior visible through it.
      4. If Lauscher is present: it is already here, curiosity accumulator at maximum.
         It has been waiting. It does not flee.


  🕳  Cave Waterfall (Underground Cascade)    ★ ★★★★★★★☆☆
  Black stone amphitheater — floating water curtain — light shafts from collapse above
  WORLD GENERATION REQUIREMENTS
    Location: within an existing cave system, at least 20 blocks below surface.
    Formation: underground river encounters a vertical shaft collapse (karst fault drop).
    Vertical drop: 8–30 blocks within the cave void. Chamber diameter: 15–40 blocks.
    Lighting: low (L < 0.15) except where god-ray shafts enter through collapse openings.
    Generation: triggers when a generated river system's underground path drops more than 8 blocks
                 within a single cave chamber. Frequency: 1 per major cave system.

  BENDING INTERACTION
    Earth: strongest Seismic Read location in the game (S > 0.70 from constant impact vibration
             + cave resonance amplification). Seismic pulse range: +8 blocks. The cave chamber
             IS the seismic amplifier — every technique pulse is heard by every creature inside.
    Water: H > 0.80 throughout chamber. All Water techniques available without H threshold.
             Tidal Anchor (formation context) available at the underground karst context.
    Information: RT60 = 10–30 s. Temporal Echo extended +6 s. The cave's multi-path echo
                   creates recursive Temporal Echo — echoes of echoes. Benders report hearing
                   events from multiple timelines simultaneously. This is accurate.
    Brine: mineral-rich water (B = +0.08). Heavy Brine technique density +20% from mineral load.

  CRAFTING SWAY
    Earth sway: very strong (S > 0.70).
    Water sway: strong (H > 0.80).
    Information sway: strong (RT60 > 10 s).
    Formation sway (Shaft Fall): items crafted at a cave waterfall forge gain 'acoustic memory'
                    — when held by an Information bender, the item produces an audible RT60 tone
                    at the frequency of the cave chamber. This tone is usable as a Choir trigger.

  MOB ECOLOGY AND DRAGON VISIBILITY
    Cave fauna density is highest near cave waterfalls — the perpetual water source and high H
    make these the most biologically rich underground locations.
    Cave bioluminescent mycelium: guaranteed near cave waterfall plunge pools (L −0.25 ambient
    creates the mycelium's preferred dark zone, but the water provides the growth medium).
    Lauschers in cave waterfall chambers: the most degraded audio databases in the game.
    After 24 hours of proximity, their database is 100% corrupted by cave waterfall noise.
    Dragon visibility: dragons do not enter cave systems. For aerial predators, cave waterfall
    prey is acoustically detectable (dragons with Earth attunement can seismic-read through
    the cave ceiling if S > 0.70 at surface — the cave waterfall's impact vibration reaches up).

  FORMATION DICTIONARY ENTRIES
    shaft fall — a vertical underground waterfall drop through a cave void
    amphitheater pool — the splashback chamber at the base of an underground fall
    stalactite curtain — hanging calcite formations around the fall's path
    black throat — the narrow vertical chimney form above the fall entry point
    collapse window — a surface opening in the cave ceiling that admits light (god-ray source)

  VISUAL / SHADER SPECIFICATION
    God-rays: if a collapse opening exists, the waterfall-generated mist acts as the god-ray
                scattering medium. The shaft of light illuminates the mist column dramatically.
                Same god-ray shader as the habitat background renderer — 80-sample radial blur.
    Low-light GI: all surfaces in the chamber are wet. Roughness × 0.30 on cave stone.
    Subsurface water glow: the water in the plunge pool is depth-rendered with teal-green
                             subsurface scatter from the bioluminescent mycelium colonies.
    Mist: trapped in the vertical shaft — ρ = 0.35–0.65, concentrated in the shaft,
            ρ = 0.05–0.10 in the main chamber. The shaft is opaque mist; the chamber is clear.


  ❄  Frozen Waterfall    ★ ★★★★★★☆☆☆
  Glass-blue walls with internal flow — the river continues inside the ice
  WORLD GENERATION REQUIREMENTS
    Biome: arctic, high-altitude (y > 150) in temperate biomes during winter seasonal cycle.
    R < 0.15 sustained for ≥ 14 in-game days triggers partial freeze. R < 0.05 for 30 days: full freeze.
    Glacier-fed sources produce frozen falls more readily than rainfall-fed sources.
    Seasonal: thaw begins when R > 0.20 for 7 days. The fall cracks, flows, re-freezes, cracks.
    The seasonal cycle is a gameplay event — thaw reveals items cached in or under the ice.

  BENDING INTERACTION
    Water (Glacial sub-discipline): ice shapes available without the normal R < 0.15 prerequisite
             — the frozen fall IS the ice. Glacial techniques in the fall's radius: chi cost −15%.
    Earth: the ice has geological properties — Seismic Read through ice reveals the original
             rock face geometry beneath the freeze layer (the ice 'remembers' the surface).
    Information: bubble inclusions in ice layers are acoustic fossils — each bubble trapped a
                   moment of sound. Temporal Echo at a frozen fall reads the bubbles' acoustic
                   record. RT60 = 2–6 s but the bubble-echo extends the reach by the bubble's age.
    Air: very dry cold air (low vapour pressure, minimal mist). W = +0.05 only (brittle air).
    Fire: suppressed. All Fire techniques unavailable unless R > 0.30 (impossible here).
           Fire bending near a frozen fall: the ice reflects the energy. Self-damage risk.

  CRAFTING SWAY
    Water sway (Glacial): strong (the ice IS water in solid state).
    Air sway: faint (dry cold air, W < 0.10).
    Earth sway: faint (ice has geological resonance from the rock beneath).
    Unique: Frozen Waterfall forge (seasonal) — items crafted here during full freeze get
             the 'ice memory' property: they warm up in high-R environments and the warmth
             persists for 90 seconds after leaving (same as thermal vent sway but reversed).

  FORMATION DICTIONARY ENTRIES
    frost curtain — a partially frozen waterfall face, with active flow visible through ice
    ice pillar — a frozen column accumulated from drip and re-freeze cycles
    hollow icicle — ice that forms around a still-flowing liquid core
    bubble layer — a stratum of trapped air bubbles in the ice (acoustic fossil record)
    collapse season — the period of freeze-thaw when old ice breaks and falls

  VISUAL / SHADER SPECIFICATION
    Ice material: subsurface scattering at 8px radius, blue spectral absorption tint.
                  Bubble inclusions: render as small spheres with high internal reflectance.
    Internal flow: the liquid core inside the ice is rendered as a translucent blue channel
                   visible through the ice shell (thin membrane technique — same as wing membrane).
    Surface crack propagation: when R is near the thaw threshold, normal map animation drives
                               crack network growth outward from the ice surface.
    Sharp specular: the ice face has near-mirror specular with hard highlights.


  ♨  Thermal / Volcanic Waterfall    ★ ★★★★★☆☆☆☆
  Orange-white mineral cascades — constant boiling mist — living heat aesthetic
  WORLD GENERATION REQUIREMENTS
    Biome: geothermal zone, volcanic periphery, or deep cave thermal vent exposure.
    Requires: geothermal_spring flag AND mineral_content B > 0.20.
    Temperature: water exits at 30–90°C. R = +0.30 at source, decaying to +0.10 at 30 blocks.
    Chemical: pH extreme possible (acidic OR alkaline depending on mineral profile).
    Rarity: 1 per 500×500 in geothermal biomes. Rare globally.

  BENDING INTERACTION
    Fire (Thermal Phase sub-discipline): unique interaction. The thermal water column is a
             Thermal Phase medium — Fire bending into the steam produces superheated steam jets.
             Thermal Phase chi cost: −20% at thermal waterfall sites.
    Water: H = +0.38 but the water is hazardous (temperature damage if not heat-resistant).
             Brine techniques amplified (B = +0.20) — the mineral suspension is native.
    Earth: mineral deposition creates seismic-readable geological 'memories' — a bender can
             Seismic Read the deposition history to date the spring's geological age.
    Information: steam absorbs upper partials (8–16) of the Choir frequency. Information
                   techniques sound different here — flatter, bass-heavy. The steam IS a filter.

  CRAFTING SWAY
    Fire sway: strong (R = +0.30 at source).
    Water sway: strong (H = +0.38).
    Earth sway: strong (B = +0.20 mineral content).
    Brine sway: guaranteed near source (H + B combination satisfies Brine threshold).
    Unique compound: Thermal + Water sway at the same forge → Flame Drink infusion
                     available at any temperature of quench (normally requires volcanic brine only).

  FORMATION DICTIONARY ENTRIES
    mineral cascade — a colour-stained waterfall fed by mineral-rich geothermal spring
    sulfur terrace — thermal step deposits coloured yellow-orange by sulfur compounds
    steam vent fall — where groundwater becomes steam at the exit point
    hot pool — a thermally-heated plunge basin (temperature damage zone)
    color band — a horizontal stripe on mineral cascade faces from changing mineral composition

  VISUAL / SHADER SPECIFICATION
    Mineral colouration: albedo varies by mineral type. Iron: orange-red. Sulfur: yellow.
                          Calcium carbonate: white. The waterfall face is a rainbow of deposition.
    Heat haze: full-screen distortion field (same as volcanic rift in Forge Breaker void shader).
    Steam: volumetric steam at ρ = 0.25–0.50. High spatial variance — steam billows unevenly.
    Emissive mineral surfaces: high-mineral faces have emissive glow (0.04–0.10) at R > 0.50.
    LUT: warm amber-orange. High emissive saturation. Steam desaturates midground.



  🌋  Lavafall (Molten Waterfall)    ★ ★★★★★☆☆☆☆
  Orange-white molten core — dark crust tearing open — constant heat shimmer
  WORLD GENERATION REQUIREMENTS
    Biome: volcanic rift zone, caldera rim, lava tube rupture site.
    Rock type: volcanic_basalt + active_magma_chamber flag required.
    Formation mechanisms:
      (a) Overflow cascade — lava lake spills over rim edge
      (b) Tube collapse — lava tube roof breaks, creating vertical drop
      (c) Cliff-fed flow — magma reaches fault edge
    Flow rate: 0.1–5 m/s (viscoplastic). Significantly slower than any water type.
    Rarity: 1 per 600×600 in volcanic biomes. Never in non-volcanic biomes.

  PHYSICS DIFFERENCES FROM WATER
    Lava does NOT produce droplets — it produces stretching filaments and tearing sheets.
    Cooling: fast at surface. The outer 1–2 blocks of falling lava form basalt crust mid-air.
             The crust tears as it falls, exposing glowing interior. New crust forms. Repeat.
    Viscosity: the flow is laminar, not turbulent. Sheet coherence maintained much longer than water.
    Gas venting: SO₂ and CO₂ emission from the flow surface. Oxygen displacement at base.
    Erosion: thermal fracturing, not mechanical. The fall does not 'drill' a plunge pool —
               it melts channels and builds new basalt formations by sequential cooling.

  BENDING INTERACTION
    Fire: massive amplification. R = +0.70 at centroid. All Fire techniques available.
             Forge Breaker super technique (Volcanic Hammer): recharge rate doubled near lavafalls
             (the ambient R pulse matches the recharge trigger condition continuously).
             Dragon-breath compatible: a tamed Fire dragon near a lavafall is effectively at
             Volcanic Forge equivalent sway — the lava IS the forge heat.
    Earth: the lavafall is an Earth bender's future terrain — it is building the world.
             Seismic Read near a lavafall shows the geological future (the basalt that will form)
             alongside the geological past. A rare dual-temporal read.
    Water: DANGEROUS. Water bending into a lavafall produces explosive steam vaporisation.
             Contact zone: 4-block area where water meets lava → instant steam explosion (8 HP),
             creates a 3-block steam cavern if sustained (see Hybrid section).
    Other: Air, Storm, Information techniques all suppressed (R > 0.70 burns Choir frequencies).

  CRAFTING SWAY
    Fire sway: dominant and very strong.
    Earth sway: moderate (the cooling basalt has geological resonance).
    Dragon sway: if tamed Fire dragon present — triple Fire sway marks in one session.
    Lavafall forge placement: the forge is placed on a cooled basalt ledge near (not in) the flow.
                              Direct contact with lava destroys any standard forge block.
                              The Volcanic Forge is the only station that can be placed adjacent
                              to an active lavafall without damage.

  PLAYER HAZARDS
    Proximity: lava_proximity_damage begins at 3 blocks from the flow. 2 HP/second.
    Contact: instant 8 HP. Fire resistance (heat_resistance Foundation node) reduces to 0 HP.
    Gas zone: within 6 blocks of lavafall base, O₂ < 0.80. Stamina drain begins.
               Storm/Information bending exacerbates — chi draw from oxygen-depleted air is harder.
    Obsidian floor: where lava meets water, instant obsidian formation. Creates new traversal terrain.
    Steam explosion: contact of player-cast Water techniques with lava → 8 HP per cast. Warning.

  FORMATION DICTIONARY ENTRIES
    lava lip — the cliff edge where molten rock overflows and begins to fall
    tube overspill — the point where a lava tube's roof breaks and the flow drops
    basalt pillar — a cooled lavafall column (same geological formation as the Domain of Stone super)
    crust tear — the surface fracture in cooling lava that exposes the glowing interior
    molten apron — the flat lava deposit at the base of a lavafall before it cools

  VISUAL / SHADER SPECIFICATION
    Lava body: emissive core (1.0) with surface crust normal map (crack propagation shader).
                The crust is dark basalt roughness 0.95. The crack reveals orange emissive beneath.
                Same technique as volcanic_crust texture, at 10× scale.
    Filament tearing: vertex shader displacement on the falling flow — thin stretching geometry
                       appears between the main flow and the wall as the lava pulls away.
    Heat distortion: full-screen distortion field at R > 0.70. Very strong near the flow.
    Gas plume: volumetric steam with yellow-tinted Mie scattering (sulfur colouration).
    LUT: Fire void LUT (red +10, blue −8) at maximum intensity. No vignette — the heat fills
         the entire frame as a luminous presence.


  🌊  Mega Waterfall (Planetary Scale)    ★ ★★★★★★☆☆☆
  The valley becomes moving atmosphere — the waterfall behaves like a vertical ocean
  WORLD GENERATION REQUIREMENTS
    Formation: continental plate drop, rift zone, post-glacial drainage collapse.
    Elevation drop: ≥ 80 blocks. Valley width: ≥ 120 blocks.
    Flow: requires a mega-river (volume ≥ 10× standard river). One per continent region.
    Rarity: 1 per world region (roughly 2000×2000 area). The singular landmark of its region.
    Effect: mega waterfalls are biome-modifying — the mist and weather effect extends 800 blocks.

  BENDING INTERACTION
    Every bending discipline is impacted. Environmental properties at centroid are the highest
    of any surface waterfall: H = +0.55, W = +0.35, L = +0.05 (mist-light scattering).
    All Water and Air techniques: available without threshold at any point within 400 blocks.
    Cloud seeding: the mega waterfall generates a permanent local weather system — rain, fog,
                   and occasional lightning. Storm benders receive auto-recharge within 600 blocks.
    Information: the scale produces standing acoustic waves at km range. Temporal Echo near
                   a mega waterfall can read geological events 10,000+ years old.
    Formation sway: unique 'Thunder Valley' sway available only at mega waterfall sites.
                     Items gain a weather_reading property — they detect approaching weather
                     events 60 seconds before they arrive.

  WORLD SIGNIFICANCE
    Mega waterfalls are civilisation-adjacent — every culture in the world builds near them.
    The Ancient Shrine builders placed shrines within 500 blocks of mega waterfalls — the
    acoustic environment and environmental property richness made them ideal resonance sites.
    Ryujin Triad relay nodes cluster near mega waterfalls (the constant ionised mist makes
    arc routing extremely efficient — a natural conductor field).
    Dragons: Water and Storm dragons roost within 1000 blocks of mega waterfalls.
              The permanent weather system provides thermal soaring support year-round.
              Fire dragons avoid the region — R < 0.30 throughout the weather zone.

  FORMATION DICTIONARY ENTRIES
    thunder valley — the canyon below a mega waterfall
    spray forest — unusual vegetation sustained by continuous mist from a mega fall
    cloud ledge — the cliff level where atmospheric mist forms permanently
    standing wave — the acoustic interference pattern audible 1 km from a mega fall
    weather engine — informal term for a mega waterfall's atmospheric generation system

  VISUAL / SHADER SPECIFICATION
    Full volumetric environment water rendering — the mist IS weather, not just particles.
    The fog system's W-type atmospheric fog applies at full scale: height 40+ blocks, ρ = 0.35.
    Rainbow: permanent, large-arc. The dispersion layer at mega scale is visible from 500 blocks.
    Cloud formation: a render layer above the fog shows genuine cloud-like volumes — the same
                     volumetric particle renderer as the fog system but at 100× scale.
    Atmospheric scattering: the directional light is scattered by the mist column, producing
                             a god-ray system at landscape scale (2km range).



  ⚗  Hybrid Lava-Water Systems — Phase Transition Zones

12.  Hybrid Lava-Water Systems
Hybrid systems occur where water and lava systems intersect — underground rivers descend into geothermal zones, karst networks intersect volcanic plumbing, or rift systems create stacked fluid layers. These are the most visually extreme environments and the most dangerous to traverse without preparation.

12.1  The Stacked World Model
  Vertical layer structure of a hybrid system
  TOP LAYER — Surface hydrology:
    Surface waterfalls (any type) feed an underground river system.
    Environmental properties: standard for surface waterfall type.

  MIDDLE LAYER — Underground void network:
    Cave waterfalls, underground rivers, karst chambers.
    Temperature gradient begins: surface 10–20°C → cave 10–30°C.
    Oxygen: 19–21% (normal).

  TRANSITION LAYER — Steam zone (100–400°C):
    Where underground water approaches lava.
    Explosive vaporisation events — periodic, not continuous.
    Steam caverns: created by sustained water-lava contact.
    Oxygen: depleted (17–19%) — mild stamina drain begins.
    Environmental properties: H = 0.60, R = +0.50, W = +0.25 (steam flow).

  BOTTOM LAYER — Geothermal / magmatic:
    Lavafall chambers, magma chamber adjacency.
    Temperature: 700–1200°C.
    Oxygen: 15–17% — severe stamina drain. Sprint impossible.
    Environmental: R = +0.70, B = +0.25 (mineral outgas).


12.2  Water-Lava Contact Events
When a water source (player-cast or naturally flowing) contacts lava, the following occurs in sequence:

Contact instant: explosive steam burst — 3-block radius, 8 HP, P pulse (+0.25 in 6 blocks). Duration: 0.3 seconds.
Steam cavern formation: sustained contact (> 2 seconds) carves a 3–8 block steam cavern in the surrounding rock. Obsidian forms on all water-side surfaces of the contact zone.
Obsidian gallery: if the lava was flowing through a tunnel, the entire tunnel section is glassified — smooth obsidian walls. A glassified tunnel is the geological record of a water-lava contact event.
Gas venting: CO₂ and SO₂ emission from the steam zone. These are detectable by Earth benders as a distinct Seismic Read signature (the gas expansion has a P component that the Seismic Read registers as anomalous ground movement).

12.3  Bending in Hybrid Systems
Discipline
Hybrid system interaction
Earth
Access to both geological extremes simultaneously — a Seismic Read in a hybrid system reads the geological past (water erosion above), present (steam zone phase boundary), and near-future (cooling basalt below). The deepest geological echo available in any environment.
Water
Every technique usable above the transition layer. Below the transition: Water techniques interact with lava (steam explosion). Used deliberately as an exploration tool — a bender can create new obsidian platforms by Surge-ing water into lava to cool specific areas.
Fire
Dominant below the transition. Cannot use above (R < 0.30 in steam zones — the steam actively suppresses R). Fire benders are the only ones who can move freely in the lava zone without heat resistance gear.
Air
The steam zone generates constant W > 0.30 from thermal convection. Air bending is very effective in the steam layer — the steam is technically atmospheric air and responds to pressure manipulation. Storm Ride in a steam vent: double duration (hot convective column).
Storm
Steam is an excellent arc conductor (ionised and wet). Stormstrike in the steam zone arcs at maximum range. The arc branching hits every conductive surface (obsidian is excellent conductor). Caution: the arc also hits the lava through the steam, returning energy to the caster.
Information
The phase transition boundary is the most information-dense environment in the world — every phase change (water → steam → condensate → lava crust) is an acoustic event. Temporal Echo at the phase boundary reads geological events at extreme depth. The steam absorbs upper partials (see Thermal section) but the lower partials travel very far in the dense medium.

12.4  Hybrid System Formation Dictionary
obsidian gallery — a glassified tunnel formed by water-lava contact, with smooth obsidian walls
steam cavern — an explosive vaporisation cavity at the water-lava boundary
phase boundary — the spatial line where water and lava meet and neither fully dominates
thermal shelf — a cooled lava ledge above the active flow, stable enough for footing
sulfur bloom — a yellow mineral deposit around steam vents at the transition layer
obsidian mirror — a flat obsidian surface formed by still-water contact with lava (near-perfect reflector, Information bending: reflects light paths usable for long-range reading)



  🌍  Fluid POI World Generation Algorithm

13.  World Generation Logic
Fluid POIs are generated as a downstream pass from the river system. Every river is evaluated for POI eligibility once the terrain heightmap is settled. The generation proceeds in priority order — higher-priority types claim their conditions first; remaining conditions are evaluated for lower-priority types.

13.1  POI Evaluation Order
Priority
Type
Condition (evaluated from river flow data)
1
Lavafall
active_magma_chamber flag within 30 blocks AND elevation_drop ≥ 8 blocks. Claims the site exclusively.
2
Thermal Waterfall
geothermal_spring flag within 20 blocks AND B ≥ 0.20 AND elevation_drop ≥ 8.
3
Frozen Waterfall
R < 0.15 sustained AND elevation_drop ≥ 8. Seasonal — dormant in warm seasons.
4
Mega Waterfall
elevation_drop ≥ 80 AND river_volume ≥ mega_threshold AND valley_width ≥ 120.
5
Curtain Waterfall
elevation_drop ≥ 16 AND cliff_width ≥ 24 AND uniform_hard_rock flag. May upgrade to Veil Gate.
5a
Veil Gate (upgrade)
Curtain site with cliff_age_flag ≥ 200 AND soft_rock_layer adjacent to hard cap. 1-in-3 chance.
6
Alpine Plunge
y ≥ 120 AND hard_rock flag AND elevation_drop ≥ 24 AND tributary_river flag.
7
Tiered / Tufa
alternating_rock_layers flag AND elevation_drop 16–48 AND B ≥ 0.15 in source.
8
Cave Waterfall
underground_river flag AND elevation_drop ≥ 8 within cave void. Requires existing cave system.
9
Hybrid System
Generated when cave_waterfall AND lavafall OR thermal are within 60 blocks of each other.

13.2  Post-Generation Setup
Environmental property field: generated as an inverse-square falloff from the POI centroid. Values from Section 1.1 at centroid, zero at 120 blocks.
Formation Dictionary: all formation names associated with the generated type are automatically added to the world's FormationLexicon as undiscovered entries. Players who visit and identify them unlock the entry.
Fog layer: if H contribution ≥ +0.25, a fog layer is spawned using the fog system's POI-local fog type matching the waterfall's character (ground mist for alpine, volumetric for mega, mineral steam for thermal).
Acoustic field: RT60 value set in the acoustic system for the location. Lauscher database corruption timers start immediately on any Lauscher within range.
Mob spawner adjustment: water-adjacent mob spawn tables enabled. Cave waterfall: underground fauna spawners. Lavafall: Fire-element fauna only.
Sway registration: the forge placement system registers this POI as a sway source. Any forge placed within range receives the appropriate sway marks during crafting.
Quest hook: if the world's quest chains are active and a Veil Gate or Cave Waterfall is generated, the Lauscher's Gift quest chain registers this location as a valid Stage 2 destination.


14.  Master Reference Table
POI Type
RT60
Peak H
Peak R
Rarity
Dragon cover
Alpine Plunge
3–10 s
0.65
0.30
Common
Medium
Tiered / Tufa
4–12 s
0.60
0.35
Common
Low
Curtain
6–15 s
0.70
0.30
Rare
High (tall mist)
Veil Gate
6–18 s
0.90
0.28
Very Rare
Very High
Cave Waterfall
10–30 s
0.82
0.35
1/cave system
N/A (underground)
Frozen
2–6 s
0.35
0.05
Seasonal
Low (minimal mist)
Thermal
3–8 s
0.68
0.60
Rare
Medium (steam)
Lavafall
2–8 s
0.25
0.85
Rare
Very Low
Mega
10–30 s
0.90
0.25
1/region
Extreme (weather)
Hybrid
Layered
0.55
0.80
Very Rare
Variable


End of Fluid POIs document. Connects to: River Systems (source waterfall locations), Fog System (mist layer generation per POI), Gaze Rays & Voice Echo (RT60 acoustic fields), Animal Voice & Lauscher (database corruption near high-RT60 sites), Formation Recognition Dictionary (all formation names above are dictionary entries), Mob Brain Object Permanence (ambient noise degrading Seismic Read baseline), Crafting Infusion & Genetics (sway at POI-adjacent forges), Dragon Fog Visibility (mist shape vs density at each type).
