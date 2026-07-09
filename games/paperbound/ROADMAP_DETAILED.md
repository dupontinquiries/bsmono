# Paperbound Expansion - Detailed Implementation Roadmap

## Executive Summary
This roadmap outlines the complete path from current state (design + skeleton) to shipping a full multiplayer game with voice, SFX, spells, and 2v2 support. Total estimated effort: **80-120 hours** over 4-6 weeks.

---

## Phase 1: Card & Sigil Integration (Tier 1) — 12-16 hours

### Objective
Get all 30 cards deployable in game.py with full sigil mechanics working.

### Tasks

#### 1.1 Implement 8 New Sigil Mechanics in game.py
**File**: `game.py`, **Function**: `Game.update()` (unit combat section)

**Sigils to add**:
1. **swiftdive** (Raven Crows)
   - Flocking behavior: units move together
   - Extra damage when attacking same target simultaneously
   - Implementation: Check nearby allies attacking same foe

2. **inksplash** (Ink Golem)
   - On hit: leave hazardous ink puddle at target location
   - Puddle: damages units standing on it for 5 HP/sec
   - Persists for 3 seconds
   - Implementation: Add puddle objects to game state on hit

3. **knock** (Tidal Wave)
   - Large knockback: pushes entire lane of enemies away
   - Range: affects all foes in a line perpendicular to attacker
   - Distance: 80-120 pixels
   - Implementation: Find all units in knock zone, apply push

4. **charm** (Piper)
   - Building aura: nearby enemies forget targets, stand idle
   - Range: 150 pixels
   - Effect: enemies have 0 movement speed while charmed
   - Implementation: Override target selection for nearby foes

5. **steal** (Void Critter)
   - On hit: take 2 mana from enemy team pool
   - Sound effect + voice line "Stolen!"
   - Implementation: `enemy.ai_ink -= 2`

6. **freeze** (Frost Mage)
   - On hit: stun target for 1.5 seconds
   - Cold visual effect (particle)
   - Implementation: `target.stunned = 1.5`

7. **forge** (Iron Smith)
   - Building aura: nearby allies +20% armor, +30% damage
   - Range: 140 pixels
   - Implementation: Apply multiplier to allies' dmg/hp regen

8. **stealth** (Shadow Cat)
   - Hidden until first attack
   - Gains 50% extra damage on first hit
   - Implementation: `self.stealthed = True` flag

**Effort**: 2 hours (read existing sigil code, copy pattern, adapt)

#### 1.2 Update CARDS dictionary with new stat balancing
**File**: `game.py`, **Lines**: 52-82

Verify costs align with balance:
- Raven Crows: 3 cost (3 tiny units) ✓
- Ink Golem: 6 cost (1600 HP, slow) ✓
- Tidal Wave: 7 cost (2000 HP, massive) ✓
- Piper: 4 cost (building, no damage) ✓
- Void Critter: 5 cost (stealth, mana drain) ✓
- Frost Mage: 4 cost (ranged, freeze) ✓
- Iron Smith: 5 cost (building, buff) ✓
- Shadow Cat: 3 cost (assassin, stealth) ✓

**Effort**: 0.5 hours

#### 1.3 Add new card visuals to origami_animal()
**File**: `game.py`, **Function**: `origami_animal()`

For each new card, add a simple folded paper visual:
- Raven: wing folding pattern
- Ink Golem: blob/square shape
- Tidal Wave: wave ridges
- Piper: humanoid with instrument
- Void Critter: angular, dark
- Frost Mage: pointy robes
- Iron Smith: blocky/geometric
- Shadow Cat: lean cat shape

**Effort**: 3 hours (each takes ~20 mins procedural drawing)

#### 1.4 Test all 30 cards in gameplay
**File**: `game.py`

**Test procedure**:
1. Deploy all 30 cards manually via deck builder
2. Verify no crashes
3. Check each sigil triggers (use voice lines for feedback)
4. Balance adjustments if any feel OP/weak

**Effort**: 2 hours (play testing, balance tweaks)

#### 1.5 Update menu and deck builder to show all 30
**File**: `game.py`, **Functions**: `draw_deckbuilder()`, `draw_card()`

- Unlock all 30 cards in deck builder debug menu
- Show sigil descriptions for new cards
- Verify cost display works

**Effort**: 1 hour

---

## Phase 2: Spell System Integration (Tier 2) — 14-18 hours

### Objective
Spells are castable in-game, tied to team mana, with proper effects.

### Tasks

#### 2.1 Add Spell UI to bottom of screen
**File**: `game.py`, **Function**: `render()`

Add spell bar showing:
- 5 spell icons (Fire, Heal, Petrify, Surge, Meteor)
- Cost for each (3, 2, 4, 2, 5)
- Current team mana counter
- Cooldown timers

**Effort**: 2 hours

#### 2.2 Implement spell casting logic
**File**: `game.py`, **Function**: `Game.cast_spell(spell_name, target_pos)`

For each spell:
1. **Fire Blast** (3 mana, 150 range)
   - AoE 60px radius
   - Deal 200 damage to all units in radius
   - Red particle burst

2. **Heal Wave** (2 mana, 200 range)
   - Restore 300 HP to all nearby allies in 200px radius
   - Green particle wave
   - Cooldown: 4s

3. **Petrify** (4 mana, 120 range)
   - Stun 80px radius, 1.5s duration
   - Gray overlay particle
   - Cooldown: 6s

4. **Martial Surge** (2 mana, 150 range)
   - Nearby allies (150px) get 1.5x damage for 3s
   - Gold glow particle
   - Cooldown: 3.5s

5. **Meteor** (5 mana, 200 range)
   - Single target 350 damage
   - Huge impact particle, screen shake
   - Cooldown: 8s

**Effort**: 6 hours (each spell ~70 mins: mechanics + visuals + feedback)

#### 2.3 Wire spell input to board clicks
**File**: `game.py`, **Function**: `main()` event loop

- Right-click on spell icon to select it
- Left-click on board to target and cast
- Show range preview while spell selected
- Consume mana on successful cast

**Effort**: 2 hours

#### 2.4 Add spell sound effects
**File**: `game_mp.py` / `game.py`, **Function**: `SFX.play()`

- Spell cast: 1200 Hz tone (0.15s)
- Fire impact: 900 Hz tone (0.1s)
- Heal: 1400 Hz tone (0.2s)
- Stun: 600 Hz tone (0.3s)
- Victory chime: 1600 Hz tone (0.5s)

**Effort**: 1 hour

#### 2.5 Add spell voice lines
**File**: `game_mp.py` / `game.py`, **Function**: `VOICE.say()`

- Cast triggers: "Fire blast!", "Healing wave!", "Petrify!", "Martial surge!", "Meteor strike!"
- On hit: "Direct hit!", "Healed!", "Stunned!"
- On cooldown: "Spell cooling down"

**Effort**: 1 hour

#### 2.6 Balance & test
**File**: `game.py`

- Play matches, cast spells frequently
- Verify costs feel right
- Check cooldowns prevent spam
- Ensure visual feedback is clear

**Effort**: 2 hours

---

## Phase 3: Voice & SFX Full Integration (Tier 3) — 12-16 hours

### Objective
Voice lines and SFX trigger contextually throughout gameplay.

### Tasks

#### 3.1 Implement context-triggered voice lines
**File**: `game.py`, add to `Game` class

**Triggers**:
- Deploy: "Sending in a {card_name}!"
- Attack: "Attack!" or "Strike!" (random)
- Kill: "Unit slain!" or "{unit_name} defeated!"
- Boss taunt: "The {boss_name} laughs at you!"
- Victory: "Victory is ours!" or "The page is won!"
- Defeat: "We have fallen..." or "The ink runs dry..."
- Spell cast: "{spell_name}!"
- Low HP tower: "Our tower is falling!"

**Effort**: 4 hours (write ~30 voice lines, add triggers)

#### 3.2 Wire SFX to game events
**File**: `game.py`, **Function**: `Game.update()` and `Game._on_death()`

**Events**:
- Deploy: `SFX.play("deploy", freq=600)`
- Attack: `SFX.play("attack", freq=800)`
- Death: `SFX.play("death", freq=200)`
- Spell cast: `SFX.play("spell_cast", freq=1200)`
- Tower hit: `SFX.play("tower_hit", freq=750)`
- Victory: `SFX.play("victory", freq=1000)`
- Defeat: `SFX.play("defeat", freq=300)`

**Effort**: 2 hours

#### 3.3 Add voice line probability & variations
**File**: `game_mp.py`

Store voice line pools with probabilities:
```python
VOICE_LINES = {
    "deploy": ["Sending in a {}!", "Go {}!", "Attack with {}!"],
    "kill": ["Slain!", "Got it!", "One down!", "Defeated!"],
    ...
}
```

**Effort**: 2 hours

#### 3.4 Record audio fallback (optional)
**File**: Create `/paperbound/sounds/` directory

If TTS unavailable, load pre-recorded WAV files instead. For now, procedural is fine.

**Effort**: Optional (4 hours if done)

#### 3.5 Audio settings menu
**File**: `game.py`, **Function**: `draw_settings()`

- Master volume slider
- Voice volume slider
- SFX volume slider
- Music volume slider
- Mute all toggle

**Effort**: 2 hours

#### 3.6 Test all audio flows
**File**: `game.py`

- Play full match, verify all voice lines trigger
- Check no audio clipping or stuttering
- Verify mute works

**Effort**: 2 hours

---

## Phase 4: 2v2 Mode Implementation (Tier 4) — 16-20 hours

### Objective
Full 2v2 gameplay with shared team resources.

### Tasks

#### 4.1 Dual-lane board layout
**File**: `game.py`, refactor board rendering

- Split book into two lanes (left & right)
- Each player controls one lane
- River runs down center
- Bridges per lane

**Layout**:
```
Player 1 (left)    |    Player 2 (right)
================River================
Enemy 1 (left)     |    Enemy 2 (right)
```

**Effort**: 4 hours

#### 4.2 Implement shared team elixir
**File**: `game.py`, **Class**: `Game`

- `self.team0_mana = 10.0` (shared by player 1 + 2)
- `self.team1_mana = 10.0` (shared by enemies)
- Both players draw from same pool
- Mana regenerates for team pool, not per-player

**Effort**: 2 hours

#### 4.3 Add player-specific towers
**File**: `game.py`, **Class**: `Tower`

Each player needs:
- 1 king tower (shared with partner, lose if destroyed)
- 2 princess towers (personal, lose HP if partner loses theirs)

**Effort**: 3 hours

#### 4.4 Spectator mode for waiting player
**File**: `game.py`, **Function**: `render()`

If 4 players total and 1 waiting:
- Waiting player sees full board
- Can watch match but not deploy
- Pings on kills/victories

**Effort**: 3 hours

#### 4.5 2v2-specific arenas
**File**: `game.py`, create 2 new arena entries

Add to `ARENAS`:
- **"2v2_study"**: Wider board, two parallel rivers
- **"2v2_volcano"**: Lava creeps from both sides

**Effort**: 3 hours

#### 4.6 Balance adjustments
**File**: `game.py`

- Tower HP adjustments (kings stay 2600, princesses start 1500)
- Mana regeneration rate (maybe 0.5 per 2.8s now, shared)
- Spell cost adjustments (might need tweaking for team play)

**Effort**: 2 hours

---

## Phase 5: LAN Networking & Matchmaking (Tier 5) — 18-24 hours

### Objective
Full multiplayer sync, lobby, and matchmaking.

### Tasks

#### 5.1 Complete network state sync
**File**: `game_mp.py`, **Class**: `GameServer`, `GameClient`

**Messages**:
- `JOIN`: Player joins, server assigns team & lane
- `STATE`: Full game state (all units, towers, mana)
- `ACTION`: Player deploys card, casts spell
- `SYNC`: 50ms heartbeat to catch up lagging clients
- `VICTORY`: Someone won, announce results

**Effort**: 6 hours

#### 5.2 Client-side prediction + rollback
**File**: `game.py` + `game_mp.py`

- Client predicts own unit movements
- Server sends authoritative state every 100ms
- Rollback if prediction was wrong
- Smooth interpolation between updates

**Effort**: 5 hours

#### 5.3 Lobby system
**File**: `game_mp.py`, new scene

- Host creates lobby (pick arena, game mode 1v1/2v2)
- Up to 4 players can join
- Host can boot players
- Ready button to start

**Effort**: 4 hours

#### 5.4 Chat & emotes
**File**: `game.py`, **UI**: Chat box

- Quick messages: "Good game!", "Nice play!", "GG"
- Emotes: :), :P, :/
- Character limit: 50 chars
- Visible in corner of screen

**Effort**: 3 hours

#### 5.5 Leaderboard (local)
**File**: `game_mp.py`, new scene

- Track wins/losses per player
- Save to `~/.paperbound/leaderboard.json`
- Show top 10 players on main menu

**Effort**: 3 hours

#### 5.6 Reconnection logic
**File**: `game_mp.py`, **Class**: `GameClient`

- Client auto-reconnects if connection drops
- Resync state on reconnect
- Announce "Player reconnected" in chat

**Effort**: 2 hours

---

## Phase 6: Polish & Optimization (Tier 5 continued) — 10-14 hours

### Objective
Make the game feel smooth, responsive, and professional.

### Tasks

#### 6.1 Screen shake on impacts
**File**: `game.py`, **Function**: `render()`

- Heavy hit (>100 dmg): shake 2-3px for 0.2s
- Meteor spell: shake 5px for 0.4s
- Tower destruction: shake 8px for 0.6s

**Effort**: 1 hour

#### 6.2 Unit animations
**File**: `game.py`, **Function**: `origami_animal()`

- Idle bob: unit floats up/down slowly
- Attack telegraph: unit leans back, swings forward
- Death flip: unit rotates 180° and fades
- Stun overlay: grayed out with ice particles

**Effort**: 3 hours

#### 6.3 Particle effects v2
**File**: `game.py`, **Class**: `Particle`

- Spell impacts: burst of 20 particles
- Death: spiral upward while fading
- Heal: green healing aura
- Stun: blue crystalline particles
- Knockback: particle trail showing direction

**Effort**: 3 hours

#### 6.4 Performance profiling
**File**: `game.py`, **Function**: `main()`

- Measure FPS, unit count, particle count
- Optimize render calls (batch where possible)
- Cache frequently-created objects
- Profile on 200+ unit matches

**Effort**: 2 hours

#### 6.5 Settings persistence
**File**: `game_mp.py`, new file `~/.paperbound/settings.json`

- Save audio volumes
- Save graphics settings
- Save keybindings
- Load on startup

**Effort**: 2 hours

#### 6.6 Tutorial/onboarding
**File**: `game.py`, new scene

- Interactive tutorial on first run
- Teach deck building
- Teach spell casting
- Teach 2v2 teamwork (if applicable)

**Effort**: 3 hours

---

## Timeline Estimate

| Phase | Name | Hours | Weeks | Difficulty |
|-------|------|-------|-------|------------|
| 1 | Cards & Sigils | 12-16 | 0.5 | Medium |
| 2 | Spells | 14-18 | 1 | Medium |
| 3 | Voice & SFX | 12-16 | 0.5 | Low |
| 4 | 2v2 Mode | 16-20 | 1 | Hard |
| 5 | Networking | 18-24 | 1.5 | Hard |
| 6 | Polish | 10-14 | 0.5 | Medium |
| **TOTAL** | | **82-108** | **5-6** | - |

---

## Success Criteria

### By end of Phase 1 (Week 1)
- [ ] All 30 cards deploy
- [ ] All 8 new sigils work
- [ ] No crashes on full roster
- [ ] Deck builder shows all 30

### By end of Phase 2 (Week 2)
- [ ] 5 spells castable
- [ ] Spell effects visible
- [ ] Spell cooldowns work
- [ ] Balance feels fair

### By end of Phase 3 (Week 2.5)
- [ ] Voice lines trigger
- [ ] SFX on all major events
- [ ] Audio settings work
- [ ] No audio clipping

### By end of Phase 4 (Week 3.5)
- [ ] 2v2 board displays correctly
- [ ] Shared mana works
- [ ] Towers behave correctly
- [ ] Spectator mode works

### By end of Phase 5 (Week 5)
- [ ] Server/client sync
- [ ] Lobby system works
- [ ] Chat functional
- [ ] Leaderboard shows

### By end of Phase 6 (Week 5.5)
- [ ] Game feels smooth
- [ ] 60 FPS in most situations
- [ ] Settings saved
- [ ] Tutorial works

### Final Validation
- [ ] 1v1 matches playable start-to-finish
- [ ] 2v2 matches playable start-to-finish
- [ ] LAN multiplayer works end-to-end
- [ ] No major bugs or crashes
- [ ] Documentation complete

---

## Dependencies & Blockers

**No blockers** - all groundwork complete!

**Nice-to-haves**:
- Recorded voice lines (instead of TTS)
- Background music per arena
- Card cosmetics/skins
- Custom map editor
- Mobile version

---

## Developer Handoff Notes

### For Phase 1 Developer
- Start with sigil implementations (read pounce/armored/chill as examples)
- Each sigil is 20-30 lines of code
- Copy the pattern, adapt to mechanics
- Test by deploying cards and checking behavior

### For Phase 2 Developer
- Study how projectiles work in game.py
- Spells are similar but affect areas instead of single units
- Cooldowns already implemented; just reuse that pattern
- Sound effects are just `SFX.play("name")` calls

### For Phase 3 Developer
- Voice is non-blocking threading; won't lag gameplay
- Add triggers at strategic points (on_deploy, on_kill, on_victory)
- Create a voice line pool for variety
- Test by playing matches and listening

### For Phase 4 Developer
- This is the big refactor
- Board layout needs careful planning
- Make sure pathfinding works for two lanes
- Test extensively with 4 players (local co-op)

### For Phase 5 Developer
- UDP is low-latency but unreliable (packets can be lost)
- Implement resend logic for critical messages
- Server is authority; clients predict and rollback
- Test on various network conditions (simulated lag)

### For Phase 6 Developer
- Polish makes 80% of the difference
- Animations and particles sell the game
- Profile early, optimize late
- User feedback is gold

---

## Git Commit Strategy

```bash
git init paperbound-mp
git add game.py game_mp.py *.md

# Phase 1
git commit -m "feat(cards): add 8 new cards with sigil mechanics"

# Phase 2
git commit -m "feat(spells): implement 5-spell casting system"

# Phase 3
git commit -m "feat(audio): voice lines and SFX integration"

# Phase 4
git commit -m "feat(2v2): dual-lane 4-player board and team mana"

# Phase 5
git commit -m "feat(network): UDP server/client with state sync"

# Phase 6
git commit -m "chore(polish): animations, effects, settings persistence"

git tag v1.0.0-release
```

---

## Success! 🎉

By following this roadmap, you'll have:
- ✅ All 30 cards with unique gameplay
- ✅ 5 powerful spells
- ✅ Immersive voice & sound
- ✅ Full 4-player 2v2 support
- ✅ LAN multiplayer (invite friends)
- ✅ Professional polish

**Total project**: ~3500 lines of code, ~600 lines of docs, 5-6 weeks of work.

**Next developer**: Welcome aboard! Start with Phase 1, follow the tasks, and reference this roadmap for guidance. You've got this! 🚀

