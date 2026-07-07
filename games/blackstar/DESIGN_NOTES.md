# Design notes: lessons from the genre (research synthesis)

Researched a decade of reviews + player feedback on the closest comparable
third-person transforming-robot shooters, to inform this game. All paraphrased.
Our game is an original arena brawler; this is design signal, not content.

## Protect (what players consistently loved)
- **Transformation as a TACTICAL tool, not eye-candy.** The most-praised moments
  were flipping form mid-fight (dump ordnance -> transform -> reposition -> finish
  -> escape). Our spark-resonance stealth loop + specialist alt-mode weapons push
  exactly this.
- **Vehicle-mode traversal highs** (cruising roadways, dive-bombing from flight)
  were called the best moments. Keep flight/traversal feeling great.
- **Deep PART-SWAP customization** (heads/chests/wheels + metallic-sheen sliders)
  was the sequel's headline win. Our genome + paint/finish/alt-mode already
  exceeds what shipped -- lean into it (randomize/copy-genome, part swaps).
- **Class identity with real tradeoffs** (fragile-deadly scout vs tanky leader).
  Our pclass matches; keep the extremes distinct.

## Avoid (what players consistently criticized)
- **DARK TONE IS A STRENGTH, NOT A FLAW -- PRESERVE IT.** The grim, desaturated
  war palette was load-bearing: the War/Fall era held its audience with it, and
  the later entries that chased brighter/vibrant looks saw the fanbase thin. The
  "brown metal" reviews were really about lack of READABLE VARIATION WITHIN the
  dark, not the dark itself -- a different problem. Do NOT brighten toward vibrance.
  The target is variation via material, silhouette, light TEMPERATURE, fog and
  accent -- not brightness. And it must be OUR darkness (from the film scripts):
  not standard Cybertron grey, but a distinct cold/corrupted mood that bends toward
  COSMIC SCALE as the story moves into the latter arc.
- **Ammo scarcity + slow reloads breaking flow** -- hugely disliked. WE SIDESTEP
  this entirely with the barrel-HEAT model (manage temperature, never scrounge).
  Do NOT add ammo pickups/reloads.
- **A pause on transform** made escaping awkward. Keep our morph readable but
  never a dead-time trap; the tactical value depends on it being snappy.
- **AI that didn't take cover.** Our cover-seeking AI closes this gap.
- **Bots blending into the setting** WITHIN the dark. Not solved by brightening --
  solved by a FAINT cold silhouette rim so edges separate while the tone stays grim.

## Acted on this pass
- **Correction logged:** dark tone is a deliberate, working style -- preserve it.
- Silhouette rim RETUNED to a faint cold void-sheen (not a bright neon outline),
  so bots separate from the murk WITHOUT fighting the grim palette.
- **Cosmic-scale atmosphere:** outer_rim + shield_edge now get colder, denser
  void-fog so distance feels vast + pressing -- a distinct dark register bending
  toward the film-script cosmic mood, never brighter.
- Confirmed heat-not-ammo is the right call; documented to protect it.
