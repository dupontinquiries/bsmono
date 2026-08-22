# Tile Geometry — Slide-in & Restack

Two related but distinct animation systems control how tiles enter the grid and how they reflow when content changes.

## The Two Systems

**Slide-in (entrance)**
Plays once per delegate when it's created — typically when the launcher opens or when the search clears. The tile arrives from some off-grid position (below, off-edge, scaled-down, etc.) and settles into its slot.

**Restack (in-place reflow)**
Plays on the GridView itself when items are added, removed, or moved within an existing populated grid. Other tiles slide to their new positions. This is what makes search-as-you-type feel fluid instead of teleporting.

Each can be tuned independently.

---

## Slide-In Patterns

Set under **Configure Launcher → Appearance → Tile Geometry → Slide-in pattern**.

### 0. None
Tiles appear in place with no entrance animation. Fastest, least visual interest. Good for accessibility or very low-end hardware.

### 1. From below
Every tile rises from ~60–100px below its slot. Tiles farther down the grid travel slightly farther, giving the effect of a rising swell rather than a flat sheet.

Stagger: left-to-right, top-to-bottom (`itemIndex * 15ms`).

Best for: general-purpose, the default. Feels like the app is unfolding.

### 2. From edges
Left half of grid slides in from the left, right half from the right. Center column arrives last.

Stagger: outside columns first, inside last (`|col - cols/2| * 25ms + row * 30ms`).

Best for: focusing attention on the center where search results will appear.

### 3. Cascade
Diagonal sweep from upper-left. Each tile arrives from up-and-to-the-left of its slot, with offsets compounding by row+col.

Stagger: diagonal wavefront (`(row + col) * 25ms`).

Best for: showcasing — demos really well, makes the launcher look polished.

### 4. Ripple
Tiles emanate from the geometric center of the grid. Center tile arrives first, corners last. They start scaled down to 30% and grow into place.

Stagger: Euclidean distance from center (`hypot(col - cx, row - cy) * 35ms`).

Best for: dramatic, eye-catching. Good for launcher-as-experience.

### 5. Scatter
Tiles arrive from pseudo-random positions, with random rotation and scale. Seeded by `itemIndex` so the same tile always comes from the same direction (deterministic, not jittery).

Stagger: pseudo-random delays up to 800ms (`(itemIndex * 7919) % 800`).

Best for: playful, casual. Looks great on phones — feels like dumping a bag of icons onto the screen.

### 6. Wave
Vertical entry from below, but with a sinusoidal column offset so the wave undulates.

Stagger: per-column with sine-wave timing.

Best for: smooth, hypnotic. Good background motion if you stare at it.

---

## Restack Behavior

Set under **Configure Launcher → Appearance → Tile Geometry**.

### Restack Speed (`restackDuration`)

How long tiles take to reflow when content changes. Default: **350ms**.

| Value | Feel |
|-------|------|
| 0 | Instant — no animation, tiles snap to new positions |
| 100–200 | Snappy — search filters feel fast |
| 300–400 | Balanced — clearly visible motion without delay (recommended) |
| 500–700 | Smooth — luxurious, slightly slow |
| 800–1500 | Languid — only for demos or accessibility |

### Restack Stagger (`restackStagger`)

Per-tile delay during reflow. Default: **12ms**.

| Value | Effect |
|-------|--------|
| 0 | All affected tiles start moving simultaneously |
| 5–10 | Subtle wave — you sense the motion is propagating |
| 12–20 | Clear cascade — the wave is visible and intentional |
| 25–50 | Strong ripple — pronounced sequential motion |

A stagger of 0 with a duration of 200ms feels efficient. A stagger of 20 with 500ms feels expressive but might frustrate someone typing quickly. The defaults (12/350) split the difference.

---

## When Each Animation Fires

| Action | Trigger | Animations |
|--------|---------|------------|
| Open launcher | `Component.onCompleted` on each delegate | Slide-in (per tile) + GridView `populate` transition |
| Close launcher | (no animation — the view just goes away) | — |
| Start typing in search | Model swaps to runner results | `populate` on new tiles |
| Continue typing | Items added/removed from model | `add`, `remove`, `displaced` |
| Clear search | Model swaps back to all apps | `populate` again |
| Drag-and-drop reorder | Favorites model receives a `moveRow` | `move`, `moveDisplaced` |
| Add favorite | New row in favorites model | `add`, `addDisplaced` |
| Remove favorite | Row removed | `remove`, `displaced` |

The key one to understand is `displaced`: when one tile leaves, every tile after it gets pushed to a new slot. The `displaced` transition is what animates that push. Without it, tiles teleport. With it, they flow.

---

## Performance

Tile geometry animations are **transform-only** — they don't trigger layout recalculation, just GPU compositing. So:

- **60+ tiles animating simultaneously**: trivial on modern hardware
- **Long animations + frequent restacks** (typing in search): can stack up if `restackDuration` is high and the user types fast

If search feels laggy during typing, lower `restackDuration` to 150–200ms. The animations still play but they finish before the next keypress triggers the next restack.

### What's expensive vs. cheap

| Operation | Cost |
|-----------|------|
| Translate (slide) | Cheap — single GPU matrix |
| Scale | Cheap — same matrix |
| Rotation | Cheap — same matrix |
| Opacity fade | Cheap — single uniform |
| MultiEffect (Shimmer) | Expensive — full-screen shader pass |
| Restack with `cacheBuffer: 400` | Moderate — more delegates realized |

If you're hitting frame drops, the first thing to try is lowering `cacheBuffer` in `ItemGridView.qml` from 400 to 100, then disabling Shimmer.

---

## Mixing With Icon Animations

Slide-in and per-icon hover animations don't conflict. The slide-in plays once on creation, then the icon hover/press animations take over for the lifetime of the delegate.

A tile can:
1. Slide in (Ripple from center) — once
2. Bounce on hover — every time you mouse over
3. Squish on press — every tap
4. Get displaced to a new position when search filters the list — many times

All four use different transform layers so they don't fight each other.

---

## Touch-Specific Considerations

On a touchscreen with no hover state, the slide-in is the most visible animation. Restack also matters a lot — when you tap a letter, you want to see the unmatched apps slide away and the matched ones reorder.

**Recommended for Fairphone / Plasma Mobile:**
```
Slide-in pattern:   From below (1) or Scatter (5)
Restack speed:      200–300 ms
Restack stagger:    8–15 ms
Animation speed:    Normal (200ms)
```

From-below is the most legible: things rise into place. Scatter is more fun but can look chaotic if you have 100+ apps.

Avoid Spin and Shimmer on touch — they need hover to show.

---

## Recommended Combinations

**Snappy desktop:**
- Slide-in: From below
- Restack duration: 200ms
- Restack stagger: 8ms
- Icon animation: Bounce
- Animation speed: Fast

**Showcase / demo:**
- Slide-in: Ripple or Cascade
- Restack duration: 600ms
- Restack stagger: 25ms
- Icon animation: Lift or Shimmer
- Animation speed: Slow

**Touch / mobile:**
- Slide-in: From below
- Restack duration: 250ms
- Restack stagger: 10ms
- Icon animation: Wiggle
- Animation speed: Normal

**Minimal / battery:**
- Slide-in: None
- Restack duration: 100ms (or 0)
- Restack stagger: 0
- Icon animation: None
- Animation speed: Instant

---

## Debugging

If tiles never slide in:

```bash
# Check enableAnimations is on
kreadconfig5 --file plasma-org.kde.plasma.desktop-appletsrc \
    --group "Containments[1][Applets][N][Configuration][General]" \
    --key "enableAnimations"

# Check tileSlideStyle isn't 0
kreadconfig5 ... --key "tileSlideStyle"

# Restart cleanly
kquitapp5 plasmashell && kstart5 plasmashell
```

If restack jumps instead of animating:

- `restackDuration` is set to 0
- Or `enableAnimations` is off
- Or you're hitting a binding loop somewhere — check journal:
  ```bash
  journalctl --user -u plasma-plasmashell.service -f | grep -i "binding loop"
  ```

If animations stutter during typing:

- Lower `restackDuration` (350 → 200)
- Lower `cacheBuffer` in `ItemGridView.qml` (400 → 200)
- Disable per-icon Shimmer

---

## Architecture Notes

The slide-in lives in `ItemGridDelegate.qml` because each tile knows its own grid position and can compute its own arrival vector. The restack lives in `ItemGridView.qml`'s `GridView` transitions because it's about how the view handles its own model changes — the view sees the deltas, the delegate doesn't.

The two systems are intentionally decoupled. You can have wild slide-in (Scatter) with instant restack (duration 0), or no slide-in with luxurious restack (duration 800), or both off, or both on.

Five transform stages on each delegate:
1. `Translate` (slide) — drives the slide-in
2. `Scale` — slide-in + icon hover
3. `Rotation` — slide-in (Scatter only) + icon Wiggle/Spin
4. The view's own `x`/`y` — drives the restack
5. Inner `iconContainer.scale`/`rotation` — drives the per-icon animations

Each layer has its own target so animations can run in parallel without overwriting each other.
