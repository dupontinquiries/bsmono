# Icon Animations

Eight animation styles, configurable per-launcher. All animations respect the global speed setting and can be turned off completely.

## Styles

| # | Name | When it plays | Best for |
|---|------|---------------|----------|
| 0 | None | Never | Battery saving, accessibility |
| 1 | Bounce | On hover, one-shot | General desktop use (default) |
| 2 | Pulse | While hovered, continuous | Drawing attention to focus |
| 3 | Wiggle | On hover/press, one-shot | Playful, touch devices |
| 4 | Spin | On hover, one-shot | Special-occasion themes |
| 5 | Lift | While hovered, hold | Subtle, professional |
| 6 | Squish | On press only | Tactile feedback |
| 7 | Shimmer | While hovered, continuous | Premium feel |

## Configuration

**Configure Launcher → Appearance → Animations**

- **Enable animations** — master toggle
- **Animation speed** — Instant (0ms), Fast (100ms), Normal (200ms), Slow (400ms), Very Slow (600ms)
- **Icon animation** — pick one of the 8 styles
- **Animate on hover** — disable to keep the style but only trigger on press
- **Animate on press** — always uses the squish effect (independent of selected style)
- **Animate when launcher opens** — staggered cascade as the launcher appears

A live preview at the bottom of the page shows three icons doing the selected animation.

## What Each Animation Does

### Bounce (default)
Icon pops up to 115% scale, dips to 95%, settles back to 100% with a bounce ease. One-shot on hover, ~200ms.

### Pulse
Continuous gentle scale oscillation between 100% and 108%, 600ms per cycle. Looks like a heartbeat — good for drawing attention without being distracting.

### Wiggle
Rotation jitter: -8°, +8°, -5°, +3°, 0°. Touch-friendly because it confirms the tap registered even before the app opens.

### Spin
Single full 360° rotation. Eye-catching but use sparingly — gets old fast.

### Lift
Icon translates up 4 pixels and scales to 105% while hovered, returns when not. The most subtle option, doesn't compete with content.

### Squish
Compress to 85% then overshoot back to 100% with a 1.5x overshoot ease. Only triggers on press. Best for touchscreens — feels like pressing a physical button.

### Shimmer
Uses `MultiEffect.brightness` to sweep brightness up and down (0.0 → 0.4 → 0.0) on a 1.6s cycle. Most expensive performance-wise.

## Cascade-on-Open

Independent of the per-icon style: when the launcher opens, each icon fades in from `opacity:0` with a 20px upward translate. They don't all do it simultaneously — items appear in grid order so the eye follows top-left to bottom-right naturally.

The system favorites row (bottom icons) has its own staggered cascade with a 40ms delay per item, scaling up from 60% to 100%.

Turn off with **Animate when launcher opens**.

## Press Feedback

Independent of the per-icon style. Every press gets a quick 80ms compress-to-90% + 120ms overshoot-back. This is the "tap confirmation" that makes touchscreens feel responsive.

Disable with **Animate on press**.

## Performance Notes

| Style | GPU cost | CPU cost | Battery impact |
|-------|----------|----------|----------------|
| None | none | none | none |
| Bounce | low | low | negligible |
| Pulse | low | medium (always running) | low |
| Wiggle | low | low | negligible |
| Spin | low | low | negligible |
| Lift | low | low | negligible |
| Squish | low | low | negligible |
| Shimmer | **high** | medium | noticeable |

Pulse and Shimmer run continuously while an icon is hovered. On a phone you usually don't have "hover" so they only kick in during keyboard navigation — they're still fine for desktop.

Shimmer uses GPU shaders (via `MultiEffect`) and is the only style that costs serious GPU time. Skip it on the Fairphone if you're on battery.

## Hover Detection on Touchscreens

Touch devices don't have hover. The launcher treats the *focused* item as hovered. So:

- Tap an icon: it gets focus → animation plays → tap fires the launch
- Arrow-key navigate: focus moves → animation plays on the newly focused item
- Drag your finger across the grid: focus follows → each crossed icon animates briefly

This means Wiggle/Bounce work great on touch — they confirm the tap. Pulse/Shimmer are mostly invisible on touch (no sustained hover) so they only really shine with a mouse.

## Per-Style Recommendations

**Fairphone / touch primary:** Wiggle or Squish — both confirm taps clearly.

**Desktop with mouse:** Bounce (default) or Lift — feel polished without being distracting.

**Public / shared workstations:** Pulse — keeps the focused item obvious for the next user.

**Battery anxious:** None or Lift — Lift is hold-state only so it doesn't burn cycles unless you're actively hovering.

**Theming / showcasing:** Spin or Shimmer — they're showy enough to demo the launcher's polish.

## Customizing Beyond the Built-ins

Each animation is a `SequentialAnimation` or `ParallelAnimation` inside `ItemGridDelegate.qml`. To add a new one:

1. Add a new entry to the `iconAnimationStyle` ComboBox in `ConfigAppearance.qml`
2. Add a corresponding animation block in `ItemGridDelegate.qml`
3. Add a case to the `onIsHoveredChanged` / `onIsPressedChanged` switch
4. (Optional) Add a preview branch in the config preview

The selected style is just an integer in `Plasmoid.configuration.iconAnimationStyle`, so adding more is just adding cases.

## Accessibility

Some users get motion sickness from animations. Set **Enable animations** to off — every animation becomes a no-op, transitions become instant assignments. The launcher otherwise works identically.

Plasma's global "Reduce motion" accessibility setting is not automatically respected (this is a TODO). For now, the per-applet toggle is what to use.

## Debugging

If animations stutter:

```bash
# Check if QML is hitting binding loops or warnings
journalctl --user -u plasma-plasmashell.service -f | grep -i "qml\|animation"

# Lower the speed to Fast or Instant
# Configure Launcher → Appearance → Animation speed → Fast

# Disable Shimmer specifically (the only GPU-heavy one)
# Configure Launcher → Appearance → Icon animation → anything except Shimmer
```

If animations don't play at all:

1. Check **Enable animations** is on
2. Check speed isn't set to Instant (which is 0ms — effectively off)
3. Check **Icon animation** isn't set to None
4. Restart plasmashell: `kquitapp5 plasmashell && kstart5 plasmashell`
