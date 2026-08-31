Legendary Weapons & Bestiary — Addendum
Winged Blades · Twin Axes · Chain-Daggers · Golem-kin · Beast Archetypes
Written to extend `docs/design/item_armor_lore_tooltips.extracted.md` in the same voice and format. Ties directly to the live `sim::legendary_weapon` system: every weapon below names its real `WeaponClass`, element (from the `element_idx` table — Wood/Water/Air/Dust/Fire/Storm), and the `ProcKind` ability that element rolls into for that class. Lore text is placeholder in the same sense as the source document — mechanical framing is meant to be canonical, prose is not final voice.

Sourced from a batch of reference models handed over for shape study: winged-blade weapons, several axe forms, a chain-and-blade weapon type with no existing in-game analogue, and a set of golem/beast reference models for roster expansion. Each section below is written to justify why that shape exists in the world, not just what it does.


  🗡  The Winged Blades — Flying Sword & Wings of Order

These two are written as a pair — one weapon, one counterpart — because a blade that flies on its own raises an obvious question: whose permission did it ask. The Flying Sword answers "none." The Wings of Order exist because someone built an answer to that.

  Flying Sword (unclaimed)
  Sword — Wand-class (Blade shape, Wand archetype) · Legendary · Primary weapon
  Element: Storm (element_idx 5 → LightningArc proc)
  Does not require a held grip to attack — thrown, it does not return until it chooses to; recall is a request, not a command
  LightningArc proc: on throw-release, arcs to the two nearest hostiles within reach before returning to guard position
  Passive: cannot be disarmed. It can only be set down.

  "It flew before anyone taught it to. We assume someone built it that way on purpose, but no record says who, and the sword has never said either. It answers to being thrown. It does not answer to being kept."
    — Recovered fragment, unmarked forge-ruin, Storm passage
  Found: Found: Storm-touched ruin sites, extremely rare. Cannot be crafted — no known forge record survives.

  Wings of Order
  Artefact — Legendary · Back slot / off-hand
  Element: Air (element_idx 2 → GaleCut proc, expressed as a defensive cutting field rather than an offensive swing)
  Grants controlled flight while chi reserves are above 30% — the wings fold the instant that threshold is crossed, mid-air, without warning
  GaleCut proc (passive, on nearby hostile spellcast): a short cutting gust interrupts the cast if the wearer is within range
  Design intent: "Order" here means *procedure*, not virtue — the wings enforce their own rule (the 30% cutoff) regardless of what the wearer wants in the moment

  "Every set carries the same rule etched into the spine where the feathers meet: 'Flight is a privilege the wearer's own body extends. It is not the wings' to extend past a body's willingness to keep flying.' Nobody has translated why the wings phrase it that way. Everyone who has run their reserves to zero mid-flight understands it immediately, on the way down, once."
    — Teaching inscription, Windward Dojo, weapon rack (annotated by three different hands)
  Found: Crafted: dragon_feather (Air, Elder) + resonance_shard + ancient_alloy_scrap. Recipe fragment found only at high-altitude ruin sites.


  🪓  Axe Forms — Three Weights, Three Elements

Reference shapes covered a wide range — a light fantasy hatchet up through a heavy two-hand plate-splitter. Rather than reskin one axe three times, each weight got its own element and its own reason to exist.

  Kindling-Cleft
  Axe · Rare · Primary weapon
  Element: Fire (element_idx 4 → FlameWave proc)
  Light, fast for an axe — closer to the Dagger archetype's swing time than a standard Axe
  FlameWave proc: every third hit ignites a shallow ground-line in the swing's arc
  Best against: wood-framed structures and Wood-element creatures, which is either a design coincidence or a very old grudge nobody wrote down

  "It was forged to split cordwood before it was ever swung at anything that could swing back. The temperament didn't change when the job did. It still thinks it's cutting firewood. Whatever's in front of it just has the misfortune of being flammable."
    — Account of a woodcutter-turned-fighter, name lost, weapon-shed graffiti
  Found: Crafted: volcanic_crystal_rod + iron ingot × 4.

  Emerald Warbill
  Greataxe · Rare · Primary weapon
  Element: Wood (element_idx 0 → FrostNova proc, reflavoured in-world as a sudden root-lattice burst rather than frost — same mechanical shape, different dressing)
  Heavy, slow, highest base damage of the three
  Proc (root-lattice burst): briefly roots non-flying hostiles in a small radius on a solid hit
  Found on golem-adjacent sites — the current working theory is that Emerald Golems don't wield weapons so much as occasionally *shed* one

  "We did not find a forge. We found a golem-shaped depression in moss that had clearly been standing in one place for a very long time, and this, resting against a nearby stone as if set down rather than dropped. We are choosing not to speculate further in this report."
    — Field observation, unnamed surveyor, Emerald Golem territory
  Found: Found: Emerald Golem territory sites. Cannot be reliably crafted — component sourcing unclear.

  Stormline Hatchet
  Axe · Uncommon · Primary weapon
  Element: Storm (element_idx 5 → LightningArc proc)
  Fastest of the three axes; the trade-off for an axe running a Dagger-tier swing speed is a narrower proc window
  LightningArc proc: shorter arc range than the Flying Sword's, but no throw/return cost — always live
  Entry-tier legendary; the intended "first real weapon" once someone can reliably reach a Storm-touched ore vein

  "Not every weapon needs an origin story. Some of them just need to hit twice before the other one hits once. This one does that. Ask it for more and it will disappoint you exactly as much as any hatchet would."
    — Teaching note, Stormstrike Dojo, entry-rack label
  Found: Crafted: conductive_ore × 2 + iron ingot × 3.


  ⛓  Chain-Daggers — a Weapon Shape With No Slot Yet

Reference material included a knife-on-a-chain design with no equivalent in the live `WeaponClass` enum (`Dagger, Blade, Greatsword, Axe, Greataxe, Spear, Staff, Wand`). Writing this up as a Dagger variant rather than proposing a ninth class: the numbers below are real Dagger-archetype numbers (fast, low reach, high crit), and the chain is *reach flavor*, not a new reach stat — mechanically it behaves like a Dagger with a longer, narrower hit window. If a real "Chainblade" class is wanted later, that's a `sim::legendary_weapon.rs` change, not a lore one, and should be scoped separately.

  Tideward Chainknife
  Dagger (chain-flavored) · Rare · Primary weapon
  Element: Water (element_idx 1 → WaterJet proc)
  Same archetype numbers as any forged Dagger — fast swing, high crit chance, short base reach
  Flavor note: the "reach" a chain suggests is spent on *retrieval*, not the hit itself — thrown, it returns to hand on the WaterJet proc rather than needing to be walked back for
  Practically: plays identically to a Water Dagger. The chain is why it looks the way it does, not why it plays the way it does — worth being honest about that in the tooltip rather than promising a mechanic the numbers don't back up

  "People ask why it's on a chain if the reach doesn't change. It changes what happens after you miss. A dropped dagger is gone until you walk back for it. A dropped chainknife is a mistake you get to take back immediately, which is its own kind of edge."
    — Practitioner account, Lienne of the Brine Shore, weapon notes (same source as the Tidal Blade entry — evidently a specialist)
  Found: Crafted: marine_crystal_shard × 2 + iron ingot × 2 + tidal_focus_stone.


  🐺  Bestiary Addendum — Roster Candidates

Reference set included golem variants (stone, emerald, moss, ice), a sabretooth beast, and a stag-headed humanoid ("The Human Deer" — resolved below as a hostile horror archetype, not a companion candidate). Written up as design candidates with one distinct hook each — none of this is committed to `CreatureKind` or the spawn/genome systems; it's a starting brief for whoever scopes that work next.

  Golem-kin (four material variants)
  The four golem references (stone, emerald, moss, ice) read as one creature family with a material-driven ability each, not four separate designs:
  — Stone Golem: baseline. High mass, slow, the "default" golem behavior already implied by existing mass-scaled stagger mechanics.
  — Emerald Golem: per the Emerald Warbill entry above, occasionally carries/sheds a real weapon drop — the only golem variant with a legendary-tier loot table.
  — Moss Golem: passive regeneration while standing on grass/dirt blocks; the intended "avoid melee, wait it out, or bring fire" variant.
  — Ice Golem: on-death effect — a short-lived ice-patch hazard in its death radius, punishing melee that doesn't retreat immediately after the kill.
  All four should probably share one base rig with a material-swapped shader/skin rather than four separate models, matching how the reference set treats them (visibly the same silhouette, different surface).

  Sabretooth (beast archetype)
  A fast, low-health pack hunter — the intended answer to "what hunts the player instead of waiting to be hunted." Ability idea: a pounce/knockdown on approach rather than a sustained combo, rewarding players who create distance immediately after being hit rather than trading blows.

  The Human Deer — resolved: it is the Wendigo the Lauscher entry already promised
  This was left as an open direction in the first draft of this addendum ("tameable creature vs. NPC — undecided"). Resolved: it's neither. It's hostile, untameable, and it's the payoff for a line that's already sitting in the codebase — `sim::lauscher.rs`'s own doc comment draws the contrast before this creature existed to fill it: *"Where a Wendigo is hunger and a Rake is the shape at the foot of the bed, the Lauscher is the sense of being heard."* The Lauscher got built. The Wendigo didn't, yet. The stag-antlered reference model is the shape for it.

  Design pattern borrowed directly from `lauscher.rs`, deliberately mirrored so the two entries read as siblings — same structure (pure deterministic decision core, an environmental gate, a Mode state machine, integer/fixed-point, rolls back with the sim), different hook. Lauscher hears; this hunts by cold.

  Gate — climate, not lair-imprint: real hook into `temperature.rs`'s scalar °C field and `world/season.rs`, not a new fictional system. Emerges only when local surface temperature crosses a starvation threshold (deep-winter cold, not merely "night" — a Lauscher-dark cave stays lair-worthy year-round; this thing's whole premise is that the world itself has to be starving too) AND it's night. Warm season, or a heated/lit interior, and there's structurally nothing to gate it awake — no lair to return to, no imprint to carry into summer. It doesn't wait underground for the cold; it isn't *there* until the cold makes it be.

  Hunt — hunger, not hearing: rather than chasing loudness (Lauscher's `heard_loudness_world` hook), it tracks whichever nearby creature (player or fauna) has the lowest recent caloric/warmth state — hungriest and coldest reads as loudest to it, inverted from Lauscher's actual-sound model. Mechanically: hooks the existing hunger/vitality and `temperature.rs` local-warmth values a player already has (campfire proximity, worn insulation), not a new stat. A well-fed player standing in firelight is closer to invisible to it than a cold, starving one standing in full torchlight — the inversion is the point; it doesn't care what it can see, only what's going without.

  Untameable — structurally, not just a flag: no bond-bar. Where the dragon system explicitly recognises tamed-dragon return (Dragon Heartstone: *"the coupling depth doubles"*), this creature has no reciprocal state to build — approaching it, feeding it, or reducing its threat never produces a Trust/Bond delta. It's not `bond_tame`-adjacent content that happens to be dialed hostile; it doesn't participate in that system at all. The design reason: everything else that's namedropped in `lauscher.rs` as a contrast (Wendigo, Rake) is there specifically to be things a bond system says no to, on purpose, so the tame/bond mechanic means something when it says yes to a dragon instead.

  Mode (mirrors Lauscher's four-state shape, different meanings):
  — `Dormant` (Lauscher's `InLair`, but there is no lair — this is "does not currently exist as an active entity," not "is somewhere hiding")
  — `Prowling` — cold has crossed the gate, night has fallen, it is real now and moving, but hasn't fixed a target
  — `Stalking` — has a caloric-deficit target fixed; closes distance slowly, matches pace rather than sprinting, the horror-pacing choice (Lauscher's `Hunting` closes on a *location*; this closes on a *body*, and it is patient about it)
  — `Starved-Off` — dawn, or the target's warmth/fullness recovers past threshold (fire, shelter, food) before it closes: it doesn't give up violently, it just stops being sustained by the world's conditions and the gate closes on it mid-stride

  No drops, no loot table entry, no crafting recipe reference — deliberately absent from this document's usual `Found:` line. It isn't a resource. Tentative flavor line for whenever this gets a real tooltip/encounter text pass:

  "It does not stalk you because it is hungry. It stalks you because you are. It has been checking for a long time."
    — Recovered fragment, unattributed, found nailed inside an abandoned winter shelter

  Scoping note for whoever picks this up: `sim::lauscher.rs` is the correct reference implementation to clone the *shape* of (module doc-comment header, `LairProfile`-equivalent-but-climate struct, `Mode` enum, pure decision-core function signature taking world state and returning an action) — do not reuse its actual darkness/humidity thresholds, they're Lauscher-specific. The caloric/warmth-deficit "loudness" needs its own threshold constants, tuned separately, likely living in a new `sim::wendigo.rs` (or similarly named) module rather than extending `lauscher.rs` itself, matching how Lauscher's own header treats Wendigo as a sibling concept, not a subtype.


End of addendum. Weapon mechanical framing (WeaponClass, element_idx, ProcKind) is meant to be implementable as-is against the live `sim::legendary_weapon` system. The Human Deer / Wendigo entry is scoped in enough detail to implement directly against `temperature.rs` and `world/season.rs` — see its scoping note for the recommended module boundary. The three remaining golem variants and the sabretooth are lighter design candidates, not committed content.
