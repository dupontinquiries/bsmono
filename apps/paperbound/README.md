# Paperbound

An origami lane-battler: fold beasts onto an open book, cross the ink river by
its ribbon-bookmark bridges, and topple the enemy's paper pagodas — across six
themed arenas, against rule-breaking bosses.

## Run it (Linux/macOS/Windows, Python 3.10-3.14)
```bash
pip uninstall -y pygame     # only if you installed vanilla pygame before
pip install pygame-ce       # the community fork - this is what the game targets
python game.py
```
Render promo screenshots instead of playing: `python game.py --shot`

## Flow
Menu -> (Deck Builder / Choose Arena / Codex-Debug) -> Battle. The whole shell is
the book itself; switching sections turns the page.

## Controls
- Menu: click the book's buttons.
- Deck Builder: click unlocked cards to add/remove (8-card deck). Locked cards
  show as torn-out pages until unlocked.
- Battle: click a card to select, click your half of the book to deploy. Ink
  regenerates 1 per 2.8s (cap 10). Click after a result to return to menu.

## Cards (22, every one with a sigil)
Pounce, Armored, Feather (fly), Chill (slow aura), Frenzy, Drinker (swarm heal),
Slam, Taunt (tree), Toxic (DoT), Sharp (retaliate), Relentless (survive one
lethal hit), Towerbane (tower-only), Sap (heal aura), Manyfold (splits on death),
Doublestrike, Rush (charges in the open), Dormant (sleeps until struck, then
rages), Beach (river-drop, dries out), Grasp (long river reach), Thorns
(retaliation aura), Plague (area swarm). Cards cycle through your 8-card deck.

## Arenas (map pool, each themed)
- The Binder's Study - warm candlelight, ink river. Home of the Angler boss.
- The Weather Page - cold blue, driving rain, lightning flashes + bolts.
- The Ashen Leaf (Volcano finale) - red light, ember rain, lava creeping in from
  the far edge, burning back-row units.
- The Fault Lines (Earthquake) - fissures open at shifting spots, swallow ground
  units, then reseal and relocate.
- The Spilled Wells - a central ink-tub POI; hold it to gain bonus ink.
- The Cold Margin - pale frost palette, side-angled light, falling snow.

Each arena carries its own paper texture, palette, surroundings, light color and
angle, and field size (special maps are larger).

## Bosses
- The Angler (Study): every ~10s hooks your nearest riverside unit, reels it
  across the water on a visible line, and flips it to fight for the enemy.

## Debug
Codex / Debug page: toggle Unlock All to test every card, plus a sigil codex.

## Under the hood
Fixed-timestep sim (20 tps) rendered at 60 fps, letterboxed to any window size.
Procedural art only: extruded 3D-prism units/towers with directional candle-lit
shading and cast shadows, worn/torn/stained page textures baked per arena,
Inscryption-style boss hands that physically place enemy cards. Bridges are a
hard barrier - ground units are funnelled to a ribbon and can never walk on open
water; only flyers and river-hoppers cross freely.

## Next
Deck persistence to disk, more bosses (Trapper, the colossal origami dragon),
audio (paper-crease SFX, ink plops, page-turn stingers, per-arena ambient beds),
overtime / double-ink rules.
