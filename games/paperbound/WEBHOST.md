# Paperbound Web Host

Strap the game to your LAN so anyone on the same Wi-Fi can spectate matches in
3D (three.js) from a browser and an organiser can run tournaments from a phone.

## Run
    python webhost.py            # serves on 0.0.0.0:8080, prints the LAN URL
    python webhost.py --demo     # also runs a self-playing match to watch
    python webhost.py --port 9000

On start it prints the address other devices should open, e.g.
`http://192.168.1.42:8080/`.

## Pages
- `/`        — landing page with links
- `/play`    — live 3D board viewer (three.js, Canvas-2D fallback if offline).
               Tap a card, then tap the lower half of the board to deploy.
- `/control` — tournament control: build a bracket (single/double elim, round
               robin, swiss), seat players, tap a name to report the winner,
               rounds advance automatically.

## Features
- **Folded-paper 3D**: units render as low-poly folded-paper silhouettes chosen
  by card family (birds get swept wings, cats/dogs/weasels each their own shape,
  apes stand upright, wisps float and glow, etc.), tinted by each creature's
  colour. Towers and wounded units show health rings/pips.
- **Live animation**: unit meshes persist across snapshots and their positions
  are tweened at 60fps between the ~10Hz updates, so they walk smoothly rather
  than teleport. A measured-speed walk bob and a fire-triggered attack lunge give
  the same feel as the native client. Both the three.js renderer AND the Canvas-2D
  fallback interpolate and animate.
- **Death crush**: dying units crumple — flattening to the page, tipping over and
  fading — in both the 3D and 2D views.
- **Head-to-head**: two browsers can each *Claim a seat* (bottom vs top) and play
  against each other with no AI — each spends its own ink and deploys to its own
  half. Extra viewers spectate.
- **Cast from the bracket**: on the control page, ready matches have a *▶ Play
  live* button. It launches a live match for that pairing; when the game ends the
  winner is **auto-reported back to the bracket** and the round advances.
- **Live status**: the landing page shows whether a match is running, its title,
  unit count, and whether a tournament is active (polled every 2s).
- **Spectator count**: the board shows a live 👁 count of how many browsers are
  currently watching (pruned a few seconds after a tab closes).
- **Browser sound**: synthesized Web Audio cues for deploys, deaths, and win/lose
  (no asset files; a 🔊/🔇 toggle, remembered per device). Detected by diffing
  snapshots client-side.
- **Embedded live view**: casting a match from the control page opens the live
  board *inline* on the control page (an embedded panel) rather than a new tab, so
  the organiser watches and manages from one screen.

## How it works / scope
The match simulation runs authoritatively on the host device; browsers show a
live view of the same sim by polling `/state` (~10 Hz), interpolating to 60fps,
and deploy via `/api`. This is a spectate + organise + head-to-head layer — the
desktop/mobile app is still the highest-fidelity way to play. Stdlib only (no pip
installs); three.js loads from a CDN with a built-in Canvas-2D fallback (also
animated) for pure-offline LANs.
