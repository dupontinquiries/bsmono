# Phase 1 Implementation Guide - Card Sigil Integration

## Overview
This guide walks through adding 8 new card sigils to game.py. Each sigil is integrated in 4 places:
1. CARDS dictionary (stat balancing)
2. SIGIL_COLORS dictionary (visual color)
3. SIGIL_DESC dictionary (tooltip text)
4. Unit.__init__ () (initialization flags)
5. Game.update() (runtime mechanics)
6. origami_animal() (visual rendering)

## The 8 New Sigils to Add

### 1. swiftdive (Raven Crows - cost 3)
**Mechanic**: Flocking behavior - nearby allies attack same target for bonus damage

**Implementation**:
```python
# In CARDS dict:
"ravencrows": dict(name="Raven Crows", cost=3, hp=120, dmg=40, spd=86, rng=24,
                   size=13, flying=True, paper=(30, 28, 36), sigil="swiftdive", count=4),

# In SIGIL_COLORS:
"swiftdive": (60, 60, 120),

# In SIGIL_DESC:
"swiftdive": "Flocking dive-bombs deal bonus damage.",

# In Unit.__init__:
self.flock_bonus = 1.0 if self.sigil == "swiftdive" else 0.0

# In Game.update() - when unit attacks:
if u.sigil == "swiftdive":
    nearby = [x for x in self.units if x.team == u.team and 
              abs(x.x - u.x) < 80 and x != u and x.alive]
    if nearby:
        u.dmg *= (1.0 + 0.3 * len(nearby))  # +30% per ally

# Rendering: Small curved lines on units, flocking pattern
```

**Effort**: 1.5 hours

---

### 2. inksplash (Ink Golem - cost 6)
**Mechanic**: On hit, leaves hazardous ink puddle that damages nearby units

**Implementation**:
```python
# In CARDS dict:
"inkgolem": dict(name="Ink Golem", cost=6, hp=1600, dmg=85, spd=20, rng=22,
                 size=36, flying=False, paper=(24, 22, 28), sigil="inksplash"),

# In SIGIL_COLORS:
"inksplash": (80, 20, 80),

# In SIGIL_DESC:
"inksplash": "Splashes ink puddles on impact.",

# Add to Game.__init__:
self.ink_puddles = []  # list of {x, y, timer, team}

# In Game.update() - after unit hits:
if u.sigil == "inksplash" and tgt_hit:
    self.ink_puddles.append(dict(x=tgt.x, y=tgt.y, timer=3.0, team=u.team))

# In Game.update() - in main loop, update puddles:
for puddle in self.ink_puddles[:]:
    puddle["timer"] -= DT
    for unit in self.units:
        if unit.team != puddle["team"] and abs(unit.x - puddle["x"]) < 40:
            unit.hp -= 5 * DT  # 5 HP/sec damage
    if puddle["timer"] <= 0:
        self.ink_puddles.remove(puddle)

# Rendering: Dark purple circles with glow
```

**Effort**: 2 hours

---

### 3. knock (Tidal Wave - cost 7)
**Mechanic**: Large AoE knockback - pushes entire lane of enemies away

**Implementation**:
```python
# In CARDS dict:
"tidalwave": dict(name="Tidal Wave", cost=7, hp=2000, dmg=120, spd=18, rng=200,
                  size=40, flying=False, paper=(70, 130, 180), sigil="knock"),

# In SIGIL_COLORS:
"knock": (100, 180, 220),

# In SIGIL_DESC:
"knock": "Knocks back whole lanes.",

# In Unit.__init__:
self.knock_timer = 0.0

# In Game.update() - when unit attacks:
if u.sigil == "knock":
    for target in self.units:
        if target.team != u.team:
            # Push perpendicular to attack direction
            dist = abs(target.y - u.y)
            if dist < 100:  # Only nearby units
                target.x += 100 * (1 if target.x > u.x else -1)  # Push away
                target.knock_timer = 0.5  # Stun for 0.5s
                self.texts.append(dict(x=target.x, y=target.y-50, txt="KNOCK!", 
                                      c=(100, 180, 220), t=0.6))

# Rendering: Large wave emanation from unit
```

**Effort**: 1.5 hours

---

### 4. charm (Piper - cost 4, building)
**Mechanic**: Building aura - nearby enemies forget targets and stand idle

**Implementation**:
```python
# In CARDS dict:
"piper": dict(name="Pipe Piper", cost=4, hp=380, dmg=0, spd=60, rng=0,
              size=22, flying=False, paper=(160, 130, 90), sigil="charm", building=True),

# In SIGIL_COLORS:
"charm": (200, 150, 100),

# In SIGIL_DESC:
"charm": "Enemies near it forget targets.",

# In Game.update() - in passive auras section:
for u in self.units:
    if u.sigil == "charm":
        for other in self.units:
            if other.team != u.team and abs(other.x - u.x) < 150:
                other.target = None  # Forget target
                other.spd = 0  # Stop moving

# Rendering: Musical notes floating around unit
```

**Effort**: 1.5 hours

---

### 5. steal (Void Critter - cost 5)
**Mechanic**: On hit, takes 2 mana from enemy team pool

**Implementation**:
```python
# In CARDS dict:
"voidcritter": dict(name="Void Critter", cost=5, hp=500, dmg=60, spd=65, rng=18,
                    size=24, flying=False, paper=(40, 20, 60), sigil="steal"),

# In SIGIL_COLORS:
"steal": (100, 20, 100),

# In SIGIL_DESC:
"steal": "Steals enemy mana.",

# In Game.update() - when unit hits:
if u.sigil == "steal" and tgt_hit:
    self.ai_ink = max(0, self.ai_ink - 2)  # Steal 2 mana
    VOICE.say("Stolen!")
    self.texts.append(dict(x=u.x, y=u.y-50, txt="STEAL!", c=(100, 20, 100), t=0.6))

# Rendering: Dark tendrils from unit to target
```

**Effort**: 1 hour

---

### 6. freeze (Frost Mage - cost 4)
**Mechanic**: On hit, stuns target for 1.5 seconds

**Implementation**:
```python
# In CARDS dict:
"frostmage": dict(name="Frost Mage", cost=4, hp=350, dmg=50, spd=55, rng=30,
                  size=20, flying=False, paper=(180, 210, 240), sigil="freeze"),

# In SIGIL_COLORS:
"freeze": (150, 200, 240),

# In SIGIL_DESC:
"freeze": "Slows and can stun.",

# In Unit.__init__:
self.stunned = 0.0  # stun duration timer

# In Game.update() - when unit hits:
if u.sigil == "freeze" and tgt_hit:
    tgt.stunned = 1.5  # Stun for 1.5s
    # Create freeze particle effect
    for _ in range(5):
        self.parts.append(dict(x=tgt.x, y=tgt.y, vx=random.uniform(-50,50),
                              vy=random.uniform(-100,-20), t=0.5, 
                              c=(180, 210, 240), typ="freeze"))

# In Game.update() - reduce movement for stunned units:
if u.stunned > 0:
    u.stunned -= DT
    u.spd = 0  # Can't move while stunned

# Rendering: Blue ice crystals around stunned unit
```

**Effort**: 2 hours

---

### 7. forge (Iron Smith - cost 5, building)
**Mechanic**: Building aura - nearby allies get +20% armor, +30% damage

**Implementation**:
```python
# In CARDS dict:
"ironsmith": dict(name="Iron Smith", cost=5, hp=900, dmg=70, spd=32, rng=20,
                  size=28, flying=False, paper=(140, 140, 150), sigil="forge", building=True),

# In SIGIL_COLORS:
"forge": (200, 160, 100),

# In SIGIL_DESC:
"forge": "Nearby armor+damage.",

# In Unit.__init__:
self.forge_bonus = 1.0  # damage multiplier from forge aura

# In Game.update() - in passive auras section:
for u in self.units:
    if u.sigil == "forge":
        for other in self.units:
            if other.team == u.team and other != u and abs(other.x - u.x) < 140:
                other.dmg *= 1.3  # +30% damage
                # Armor is represented as reduced incoming damage
                other.forge_bonus = 0.8  # Take 20% less damage

# When calculating damage taken:
incoming_dmg *= target.forge_bonus

# Rendering: Golden aura with hammers around unit
```

**Effort**: 1.5 hours

---

### 8. stealth (Shadow Cat - cost 3)
**Mechanic**: Hidden until first attack, gets 50% damage boost on first hit

**Implementation**:
```python
# In CARDS dict:
"shadowcat": dict(name="Shadow Cat", cost=3, hp=280, dmg=65, spd=75, rng=18,
                  size=18, flying=False, paper=(50, 48, 56), sigil="stealth"),

# In SIGIL_COLORS:
"stealth": (100, 100, 120),

# In SIGIL_DESC:
"stealth": "Hides until it attacks.",

# In Unit.__init__:
self.stealthed = True if self.sigil == "stealth" else False
self.stealth_dmg = 1.5 if self.sigil == "stealth" else 1.0

# In Game.update() - when unit attacks:
if u.stealthed and u.sigil == "stealth":
    dmg *= u.stealth_dmg  # 1.5x damage on first hit
    u.stealthed = False  # Reveal after attack
    VOICE.say("Assassinate!")

# In rendering, skip rendering stealthed units OR render semi-transparent

# Rendering: Invisible/translucent until attacking
```

**Effort**: 1.5 hours

---

## Integration Checklist

### Step 1: Update CARDS Dictionary (15 minutes)
- [ ] Add all 8 new cards to CARDS dict with stats
- [ ] Verify costs are in CARDS (game.py lines ~130-170)
- [ ] Double-check sigil names match exactly

### Step 2: Update SIGIL_COLORS (10 minutes)
- [ ] Add color for each new sigil (game.py line ~176)
- [ ] Use visually distinct colors

### Step 3: Update SIGIL_DESC (10 minutes)
- [ ] Add description for each new sigil (game.py line ~184)
- [ ] Keep descriptions short (max 40 chars)

### Step 4: Update Unit.__init__ (30 minutes)
- [ ] Add initialization for each sigil's special properties
- [ ] Examples: stealthed, stunned, puddles, etc.

### Step 5: Implement Mechanics in Game.update() (4 hours)
- [ ] swiftdive: Check nearby allies, boost damage
- [ ] inksplash: Create puddles on hit, damage nearby units
- [ ] knock: Knockback enemies in lane
- [ ] charm: Enemies forget targets in aura
- [ ] steal: Remove enemy mana on hit
- [ ] freeze: Stun target, prevent movement
- [ ] forge: Boost nearby allies' damage & defense
- [ ] stealth: Hide until first attack, 1.5x first hit

### Step 6: Update origami_animal() Rendering (2 hours)
- [ ] Add visual for each new unit
- [ ] Use procedural drawing (triangles, circles, prisms)
- [ ] Make them distinct from originals

### Step 7: Test in Game (2 hours)
- [ ] Deploy each card via deck builder
- [ ] Verify no crashes
- [ ] Check each sigil triggers visually
- [ ] Balance adjustments if needed

---

## Total Effort: 12-16 hours

This matches Phase 1 estimate in ROADMAP_DETAILED.md

---

## Key Tips

1. **Copy existing sigil code**: Look at `pounce`, `armored`, `chill`, etc. as templates
2. **Test incrementally**: Add 1-2 sigils, test, then continue
3. **Use VOICE.say()** for feedback (helps verify mechanics work)
4. **Keep it simple**: Start with basic implementation, optimize later
5. **Balance**: Play matches with new cards, adjust HP/dmg if overpowered

---

## Example: Full swiftdive Implementation

```python
# Line ~165 in CARDS dict:
"ravencrows": dict(name="Raven Crows", cost=3, hp=120, dmg=40, spd=86, rng=24,
                   size=13, flying=True, paper=(30, 28, 36), sigil="swiftdive", count=4),

# Line ~180 in SIGIL_COLORS:
"swiftdive": (60, 60, 120),

# Line ~195 in SIGIL_DESC:
"swiftdive": "Flocking dive-bombs deal bonus damage.",

# Line ~335 in Unit.__init__:
self.flock_count = 0  # Number of allies attacking same target

# Line ~540 in Game.update() combat section:
if u.sigil == "swiftdive" and u.target and u.target in self.units:
    nearby = [x for x in self.units if x.team == u.team and 
              abs(x.x - u.x) < 80 and x != u and x.target == u.target and x.alive]
    if nearby:
        dmg *= (1.0 + 0.3 * len(nearby))
        u.cool = max(u.cool, u.dmg / 100.0)

# In origami_animal() function, add case:
elif u.key == "ravencrows":
    # Small curved wings on each unit
    for i in range(u.count):
        offset_x = (i - u.count//2) * 8
        draw arc from (u.x-8+offset_x, u.y-4) to (u.x+8+offset_x, u.y+4)
```

---

## Next Steps After Phase 1

Once all 8 sigils work:
1. Test all 30 cards in deck builder
2. Play full matches with new cards
3. Balance adjustments (HP, damage, costs)
4. Move to Phase 2: Spell System

Good luck! 🎮
