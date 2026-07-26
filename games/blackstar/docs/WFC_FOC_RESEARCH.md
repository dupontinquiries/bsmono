# What fans loved about War for Cybertron / Fall of Cybertron
## Research pass (web) -> mapped against this game, with the gap list

Sourced from Metacritic user reviews, Steam discussions, and contemporary
reviews (GameWatcher, Push Square, PopMatters, Back2Gaming, MP1st, the FOC
wiki). The point: build the beloved mechanics we're missing, don't rebuild
what's here.

## What fans praised most (recurring themes)

1. **"Actual reasons to transform in combat."** The single most-repeated
   praise. FOC nailed transforming mid-fight as a tactical choice, not just
   travel. (Also the most-repeated CRITICISM when it failed -- PopMatters:
   "your brittle frame prevents you... the jet just becomes fast transport.")
2. **The weapon-hand / shoulder swap.** FOC's signature new feature, called
   out by name in multiple reviews ("switch which hand you hold your weapon in
   -- a great advantage when turning a corner"). Fans loved it; MP1st listed
   losing vehicle barrel-roll as a real regression.
3. **Escalation (Horde) with a buy economy.** Kills earn credits; spend on
   weapons/health/ammo/overshields, and BUY-GATES that open new map areas.
   Risk/reward, team roles (the Healer is vital), rising tension.
4. **Distinct class abilities the arena is built around** -- grapple, hover,
   sentry beam, cloak. Each class demands a different playstyle.
5. **Enemy variety = tactical tension.** "Flying Sniper bots add a lot of
   tension because they could be anywhere." Forces you to change tactics.
6. **The Teletraan / Pack-a-Punch weapon upgrade economy** (energon shards ->
   more clip, more damage, permanent perks like reduced ability cooldown).
7. **G1 fan-service + character-accurate feel**; Grimlock (dino) as a
   standout playable form.

## What THIS game already has (verified in source)

- Escalation waves + energon-credit buy economy + buy-GATES that expand the
  map (`g_stations`, STN_GATE/WALLBUY/PERK/REPAIR/BRIDGE/FORGE/TITAN) -- a
  full COD-Zombies-grade station framework. (theme #3, #6: COVERED, deep)
- Purchasable perks (Juggernaut/Overdrive/Rapid Coil) + energon weapon FORGE
  tiers (Pack-a-Punch). (theme #6: COVERED)
- 9 ability slots (hover/turret/shield/dash/cloak/overclock/resonance/pulse/
  blink) with class loadouts. (theme #4: COVERED)
- Meaningful vehicle-mode combat: beast forms fire signature attacks (spider
  web, dino FIRE BREATH, scorpion stinger+cannons), vehicle RAM scales with
  velocity. Grimlock-style dino is playable. (theme #1, #7: COVERED)
- Enemy variety: grunts, corrupted, hijack (airborne), apex, champions,
  combiners; ground-to-air flak vs a flying player. (theme #5: PARTIAL)

## The gaps (prioritized)

1. **[DONE, Update 222] Weapon-hand / shoulder swap.** Was entirely absent --
   the cannon was hardcoded to the right hand. This was FOC's most-praised
   signature feature. Added: Z toggles the weapon hand, the cannon swings
   across the body (smoothed lerp), muzzle flash follows. Peek/fire around
   cover from either shoulder.
2. **Flying enemies that SNIPE from elevation.** We have airborne hijackers
   and ground->air flak, but not the "sniper bots that could be anywhere"
   downward-pressure that fans singled out. A ranged air-sniper behavior
   (perch/hover at range, telegraphed shot) is the next authentic add.
3. **Transform as a combat DODGE.** FOC fans wanted the transform itself to
   matter defensively. A brief i-frame / speed burst on transform (already
   have xform_pop visuals) would reward transforming under fire.
4. **Per-weapon-hand cover peek bonus.** Tie the shoulder-swap to a small
   accuracy/exposure benefit when the gun is on the same side as the cover
   you're leaning past -- makes the swap tactical, not just cosmetic.
