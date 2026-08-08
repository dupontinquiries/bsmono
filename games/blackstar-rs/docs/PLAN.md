# PLAN — from a library to a game

## The thing to read first

You said the game plays like it did before the changes, and you are right.
I measured it:

```
module      references from main.rs
weapons     6      chord      18      xform      8
anim        5      content     3      lift       2
ui          0      menu        0      patrol     0
ship        0      drop        0      clinch     0
interior    0      airlock     0      bigbody    0
terrain     0
```

**Nine of sixteen modules have zero call sites in the game loop.** The
compiler agrees: 147 "never used", 39 "never constructed", 21 "never read".

So the honest position is that roughly 60% of what I built over these
sessions is a *library sitting beside the game*, not in it. It compiles, it
is tested, and it does nothing when you press a key, because nothing calls
it. The commits are real and the behaviour is not. That is on me — I kept
choosing "build the next thing, tested" over "wire the last thing, unclear
whether it feels right", because the first is verifiable without a display
and the second is not. Verifiable is not the same as valuable.

Everything below is written around fixing that, not around adding more.

---

## Phase 0 — WIRING DEBT (do this before anything new)

No new modules. Each item is "make an existing, tested thing reachable".

| what | where it plugs in | why first |
|---|---|---|
| `menu` + `ui` | replace one `Mode` screen end to end | the whole menu rewrite depends on this path working |
| `bigbody` | `physics::step_car` consults it for ground | ships, moving platforms and wrecks are all one hook |
| `clinch` | air-to-air contact check in `sim` | already tested, needs a caller and a HUD prompt |
| `patrol` | a game mode that calls `update` | the loop exists; nothing ticks it |
| `ship` + `interior` + `airlock` + `drop` | one "leave the planet" flow | they were designed as one chain and should land as one |
| `terrain` | `World::generate` takes a `Ground` | turns 13 route rows into 13 places |

Success test for the phase: the dead-code warning count goes to near zero.
That number is the honest progress bar, not the commit count.

---

## Phase 1 — THE FRONT DOOR

Right now the game starts on a map. It should start with a person.

### 1a. Character creation wizard

A linear wizard, not a settings screen: one decision per page, back always
works, nothing is modal.

1. **Faction** — the four from the script: Coalition, Corrupted, Hijacks,
   Blackstar's clans. Sets the element pool and who shoots at you.
2. **Element** — the seven from `content::ELEMENTS`. This is the real
   choice: it drives damage, range, cooldown, movement AND the
   rock-paper-scissors matchups, all already implemented.
3. **Chassis** — from `content::CHASSIS`, which already carries `xform_dur`
   and proportions, so the pick visibly changes how your fold looks.
4. **Signature** — the back-mounted piece, `content::SIGNATURES`, ten of
   them, every one a trade.
5. **Name and paint** — the only cosmetic page. Last, because it is the
   least important and people linger on it.

### 1b. Spaceship selection wizard

Follows immediately, same shape.

1. **Hull** — `ship::HULLS`. Length drives fold time, berths, and whether it
   can land at all. A carrier cannot set down; the wizard should say so on
   the card rather than in an error later.
2. **Home berth** — which room you spawn in; ties to `interior`.
3. **First heading** — a `patrol` destination, which doubles as the tutorial
   flight.

### 1c. Persistence

One `Profile` struct: faction, element, chassis, signature, name, paint,
hull, berth, progress. Saved through the existing `save.rs`. Everything
downstream reads the profile instead of hard-coded defaults.

**This is the shift you described**: the map stops being where the game
starts and becomes somewhere the profile sends you.

### 1d. Where multiplayer goes

The wizard flow becomes the single-player entry. Multiplayer moves behind
its own item on the title screen and keeps today's straight-to-map
behaviour, because that is what it wants. The campaign — eight ported
missions, six films, nineteen raids in `docs/RAIDS.md` — hangs off the
profile.

---

## Phase 2 — MENUS WITH CHARACTER

### 2a. The problems, named

- **Font too big.** `T_BODY` is sized for a HUD readout, not a list.
- **Rows too tight.** `menu_cmds` uses `size * 2.4` for row height, which is
  barely more than the glyphs. A list needs air.
- **No character.** It is white text on a dim quad. Nothing about it says
  what this game is.

### 2b. Typography

`src/font.rs` is a 5x7 bitmap: 112 lines, one weight, no kerning. The C
engine ships **OpenSans-Regular.ttf** and **SourceSansPro-Regular.otf** under
the SIL Open Font License in `engine_src/data/fonts`, plus a VGA bitmap in
`exengine/render/vga_font.h`.

Plan: keep the bitmap for the HUD, where it is legible and cheap, and add a
real typeface for menus. Two weights: a light for body, a heavy for
headings. The size drops and the leading goes up — the fix for "too big and
too tight" is the same fix.

### 2c. Theme

Darker and more elegant, concretely:

- near-black ground rather than dim grey, so the panel reads as a surface
- one accent, taken from the player's ELEMENT, so the menu is *theirs*
- rules and dividers instead of boxes
- selection by a filled bar and a shifted indent, not a bright rectangle
- generous margins; `ui::Screen::safe` already exists for this

### 2d. Menus that transform

The one that earns the theme. A menu is rows; a transform is a strip of
poses; `ui::Cmd` already carries rectangles. So a page change can be a
FOLD — rows rotate out on their long axis and the next set rotates in,
staggered by index. It is the game's own verb applied to its interface, and
it needs an animation layer on `ui` rather than anything new underneath.

### 2e. ESC

Settings reachable from anywhere via ESC, pushed onto `menu::Stack` over
whatever is running, popped back to it. `Host` already returns `Close` only
at the bottom of the stack, which is exactly this.

---

## Phase 3 — CONTROLS

The controls are a mess and the mess is legible: bindings accumulated one
feature at a time, and three passes moved keys around without ever laying
the whole thing out.

- One table, all 25 actions, grouped by context: driving / flying / robot /
  camera / menu.
- `chord::resolve` already makes space contextual; the same treatment for
  the other overloaded keys instead of moving them.
- Show CONTEXT in the Binds menu — "SPACE: jump (robot), next target
  (flyer)" — because a key that does two things needs to say so.
- A printable default sheet, so the scheme can be argued with on one page.

---

## Phase 4 — ANIMATION AND FIDELITY

Only after the above.

1. **Gait strips.** The gait model is ported and correct (Froude cadence,
   counter-phase arms, from `robot_parts.c`). What is missing is limb poses:
   foldview generating walk/run cycles from its fitted skeleton the way it
   generates fold strips. Deferred three times now; it should go first here.
2. **Transform quality.** The morph blends and the personality profiles
   work. Next is secondary motion — panels settling after the body stops —
   and the fold audio.
3. **Character models.** `src/rig.rs` can already evaluate real skinned
   glTF; the reference Optimus rig is 83 joints and does what you want the
   bots to do. The gap is a runtime skinning path in `render.rs`, which is
   the same gap as gait strips: pose data reaching the GPU.

---

## Order, and why

Phase 0 first because everything else is built on modules that currently do
nothing. Phase 1 next because it changes what the game *is*. Phase 2 and 3
together, since they are both "the interface has no design". Phase 4 last
because it is the most expensive and the least blocking.

The one thing I would ask: Phase 0 items each need someone to press a key
and say whether it feels right. I can wire and test them, but I cannot tell
you whether the clinch window is too tight or the menus are too dark, and
three sessions of guessing about the roster orientation is the argument for
not guessing about feel.
