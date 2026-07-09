# Paperbound

An origami lane-battler on a candlelit desk. Fold paper beasts, cross wax-ribbon
bridges over an ink river, and topple the enemy's king tower. Clash-style
mechanics with an Inscryption hush.

## Run
```bash
pip install pygame-ce          # required (NOT vanilla pygame)
python game.py                 # play
python game.py --shot          # render screenshots headlessly
```
Python 3.10–3.14. On 3.14 you must use `pygame-ce` (vanilla pygame's font module
is broken there).

## What's in this build

### Single-player
30 cards across 30 sigils, 9 arenas with weather/hazards/POIs, and the Angler
boss in the opening chapter.

### Multiplayer (LAN)  ·  `netplay.py`
Two players on one network, delayed-input lockstep over UDP. Menu →
**Multiplayer (LAN)** → Host or Join. Deterministic sim means only card
deployments cross the wire (a few bytes each), no full-state streaming.
See `MULTIPLAYER_GUIDE.md`.

### Expanded deck builder
Filter (All/Troops/Buildings/Flying) + sort rail, a live card-detail panel with
full stats and sigil text, clickable deck slots, average-cost read-out, and a
cost-curve histogram.

### Graphics presets  ·  Options page
- **Low** — the original fast renderer.
- **Photorealistic** — flame-bloom lightmap, 3-layer soft-penumbra shadows +
  contact AO, a full-screen bloom pass, film grain, and a warm cinematic
  vignette. Toggle any time; takes effect instantly.

## Controls
- Menus: click.
- Battle: click a hand card, then click your half of the board to deploy.
- Multiplayer IP entry: click the field and type.

## Files
- `game.py` — everything: menus, battle, rendering, net integration.
- `netplay.py` — standalone UDP lockstep networking (`python netplay.py` self-tests).
- `MULTIPLAYER_GUIDE.md` — deep dive on the three latest features.
