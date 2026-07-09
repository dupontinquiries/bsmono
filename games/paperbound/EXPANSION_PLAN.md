# Paperbound Expansion: Multiplayer, Voice, SFX, Content

## Phase 1: Refactored Codebase (1-2 days estimated)
- NetworkGame class wrapping Game for LAN sync
- Spell system (distinct from units)
- 6 new cards (crow swarms, ink golems, tidal waves, etc.)
- 3 new arenas (sunken library, ice cavern, corrupted forest)
- Performance: object pooling, spatial grid for unit queries
- Dedicated server mode for LAN

## Phase 2: Audio & Voice (parallel)
- pyttsx3 for AI voice lines (works offline)
- SFX library: deploy, attack, death, spell cast, tower hit, victory
- Ambient tracks per arena (royalty-free or generated)
- Voice line triggers: boss taunts, unit flavor, victory chants
- Volume mixer with mute toggles

## Phase 3: Graphics Polish
- Particle system v2: spell auras, impact rings, screen shake
- Weather enhancements: lightning bolts on each strike, ash drifts
- Unit animations: attack telegraphs, death animations, idle bob
- Map-specific visual themes (more grime in dark arenas, ice on frost)
- Post-processing: vignette with arena color tint, bloom on spells

## Phase 4: 2v2 Mode
- Four-player board: two lanes per team (diagonal layout)
- Shared king tower, separate princess towers per player
- Team elixir (shared pool) vs. personal spend limits
- Spectator mode for waiting players
- 2v2-specific arenas (larger, asymmetric)

## Phase 5: Network Multiplayer
- UDP-based message protocol (state sync every 50ms)
- Rollback / prediction for smooth movement
- Lobby system (broadcast + port selection)
- Chat / emote wheel
- Ranked leaderboard (stored locally or on optional server)

## Tech Stack Additions
- `pyttsx3` — offline TTS for voice lines
- `numpy` — spatial hashing for performance
- `socket` + custom protocol — LAN sync
- `threading` — dedicated server mode

## Timeline
If built sequentially: ~200-300 lines per card/arena, ~400 for spell system,
~600 for network layer, ~500 for voice/sfx, ~800 for 2v2 layout.
**Estimated final codebase: 4,500–5,500 lines.**

Current cut: focus on **Phase 1 + Phase 2 + partial Phase 3** to get playable
multiplayer with voice and sfx in one go (working title: game_mp.py).
