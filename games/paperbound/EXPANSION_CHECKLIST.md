# Expansion Features Checklist

## ✅ Completed This Session

### Content
- [x] 8 new cards (Raven Crows, Ink Golem, Tidal Wave, Piper, Void Critter, Frost Mage, Iron Smith, Shadow Cat)
- [x] 3 new arenas (Sunken Library, Ice Cavern, Corrupted Forest)
- [x] 5 spells (Fire Blast, Heal Wave, Petrify, Martial Surge, Meteor Strike)
- [x] Spell system architecture
- [x] Card sigil descriptions for new cards

### Audio & Voice
- [x] Voice system (pyttsx3 integration)
- [x] SFX player (procedural tone generation)
- [x] Mute/unmute controls
- [x] Async voice playback (threading)

### Networking
- [x] UDP server skeleton
- [x] UDP client skeleton
- [x] Command-line argument parsing for --host, --join, --2v2
- [x] Basic message protocol (JSON over UDP)

### Multiplayer
- [x] 2v2 mode flag
- [x] Game server initialization
- [x] Client connection logic

### Graphics
- [x] Arena-specific color palettes and lighting
- [x] Spell visual indicators (color system)
- [x] Particle system framework

### Performance
- [x] Object pooling design
- [x] Spatial hashing structure
- [x] Asset lazy-loading

### UI/UX
- [x] Menu system stubs
- [x] Audio toggle (M key)
- [x] Voice test trigger (V key)

## 📋 Next Session Priorities

### Tier 1: Core Gameplay
- [ ] Integrate expanded card roster into game simulation
- [ ] Implement spell casting system (target selection, AoE)
- [ ] Wire new sigils into combat logic (swiftdive, inksplash, knock, etc.)
- [ ] Add new arena hazards (currents, blizzard, decay)

### Tier 2: Multiplayer
- [ ] Full server/client message sync
- [ ] State replication (unit positions, HP, attacks)
- [ ] Client-side prediction with rollback
- [ ] Latency compensation

### Tier 3: Voice & SFX
- [ ] Context-triggered voice lines (boss taunts, unit flavor, victory)
- [ ] Recorded audio fallback (if TTS unavailable)
- [ ] Sound effect library expansion
- [ ] Background music per arena

### Tier 4: 2v2 Specific
- [ ] Dual-lane board layout
- [ ] Shared team elixir pool
- [ ] Princess tower placement
- [ ] Spectator mode

### Tier 5: Polish
- [ ] Card unlock progression
- [ ] Deck save/load to disk
- [ ] Match replays
- [ ] Leaderboards
- [ ] Settings menu (graphics, audio, keybinds)

## 🎯 Success Metrics

| Goal | Status | Notes |
|------|--------|-------|
| 30 cards playable | ⏳ In progress | Need sigil implementation |
| 9 arenas selectable | ⏳ In progress | Rendering works, hazards pending |
| Spells castable | ⏳ In progress | System designed, needs wiring |
| Voice audible | ⏳ In progress | TTS works, needs integration |
| SFX functional | ⏳ In progress | Generation works, needs triggers |
| LAN matches playable | 🔴 Not started | Protocol designed, needs testing |
| 2v2 matches | 🔴 Not started | Layout designed, needs implementation |

## 📦 Deliverables

- [x] Expanded game_mp.py skeleton
- [x] Documentation (EXPANSION_SUMMARY.md, this file)
- [x] Original single-player game.py (unchanged, 1717 lines)
- [ ] Full multiplayer implementation (target: ~3500 lines total)
- [ ] Voice integration (pyttsx3 wiring)
- [ ] SFX integration (trigger system)
- [ ] Spell system (complete)
- [ ] 2v2 framework (complete)

## 🔧 Technical Debt

- Voice fallback for missing pyttsx3
- Audio device detection (handle no audio gracefully)
- Network error handling (timeouts, dropped packets)
- Thread safety for shared game state
- Performance profiling on large matches (200+ units)

## 📝 Documentation

- [x] EXPANSION_SUMMARY.md (7 sections, 150+ lines)
- [x] EXPANSION_PLAN.md (design spec)
- [x] This checklist (tracking)
- [ ] Inline code documentation
- [ ] Tutorial/getting started guide
- [ ] Modding guide (custom cards/arenas)

---

**Progress: 40% of expansion complete**

Focus: Integrate voice and SFX fully, then wire spells and 2v2 framework.
