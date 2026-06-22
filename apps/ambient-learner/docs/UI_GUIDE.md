# Ambient Learner v0.2.0 - UI Guide

## Application Layout

```
┌─────────────────────────────────────┐
│     ☰ Local Weather           ⋮    │  ← Header Bar
├─────────────────────────────────────┤
│                                     │
│                                     │
│                                     │
│           [Live View]               │  ← Content Area
│            Network                  │     (View Stack)
│            History                  │
│                                     │
│                                     │
│                                     │
├─────────────────────────────────────┤
│   Live    Network    History        │  ← View Switcher Bar
└─────────────────────────────────────┘
```

## Tab 1: Live Temperature

```
╔═════════════════════════════════════╗
║                                     ║
║                                     ║
║            23.4°C                   ║  ← Large temperature display
║                                     ║     (color-coded: blue/green/orange)
║         Confidence: 87%             ║  ← Confidence level
║                                     ║
║  Active sensors: 8 | Calibrations: 3║  ← Status info
║                                     ║
║      ● Sensors Locked               ║  ← Sensor lock status
║                                     ║     (Green = OK, Red = Error)
║                                     ║
╚═════════════════════════════════════╝
```

**Features**:
- Real-time temperature updates (200ms refresh)
- Color-coded by temperature range
- Confidence score based on sensor reliability
- Active sensor count and calibration count
- Sensor lock status indicator

## Tab 2: Neural Network

```
╔═════════════════════════════════════╗
║                                     ║
║         ┌─────────────┐             ║
║      ┌──○ Sensor 0    │             ║  ← Network visualization
║      │  ○ Sensor 1    │             ║    (Circles = sensors)
║    ┌─┴─○ Sensor 2    │             ║    (Size = weight)
║    │   ○ Sensor 3    ├──→ [Temp]   ║    (Color = reliability)
║    └───○ Sensor 4    │             ║
║      └─○ Sensor 5    │             ║
║        └─────────────┘             ║
║                                     ║
║    ┌─────────────────────┐          ║
║    │  Calibrate Model    │          ║  ← Calibration button
║    └─────────────────────┘          ║
║                                     ║
╚═════════════════════════════════════╝
```

**Features**:
- Dynamic network visualization
- Pulsing animation shows active prediction
- Circle size indicates sensor weight
- Color intensity shows reliability
- One-click calibration

## Tab 3: History (NEW IMPROVED UI)

```
╔═════════════════════════════════════╗
║                                     ║
║       ╭─────────────────────╮       ║
║       │                     │       ║
║       │   Temperature       │       ║  ← SVG graph showing
║       │   Graph             │       ║    daily averages
║       │      ╱╲    ╱        │       ║
║       │     ╱  ╲  ╱  ╲      │       ║
║       │────╱────╲╱────╲─────│       ║
║       │                     │       ║
║       ╰─────────────────────╯       ║
║                                     ║
║           This Week                 ║  ← Week title
║        Mar 1 to Mar 7               ║  ← Date range
║                                     ║
║      [◀]  [   Today   ]  [▶]       ║  ← Navigation controls
║                                     ║    ◀ = Previous week
║                                     ║    Today = Jump to current
║                                     ║    ▶ = Next week (disabled at current)
║                                     ║
║    ┌─────────────────────┐          ║
║    │  Export as CSV      │          ║  ← CSV export button
║    └─────────────────────┘          ║
║                                     ║
╚═════════════════════════════════════╝
```

**New Features**:
- ✨ Improved layout with visual hierarchy
- ✨ Week title at top with date range
- ✨ Circular navigation buttons with tooltips
- ✨ "Today" button for quick return to current week
- ✨ CSV export button integrated into view
- ✨ Touch gesture support (swipe left/right)
- ✨ Disabled state for "Next Week" when at current week

### Touch Gestures

```
     Swipe Right →     ┌──────────────┐     ← Swipe Left
   (Older/Previous)    │   History    │    (Newer/Next)
                       │    Graph     │
                       └──────────────┘
```

**Gesture Behavior**:
- **Swipe Left** (velocity < -200 px/s): Go to next week (more recent)
- **Swipe Right** (velocity > 200 px/s): Go to previous week (older)
- Works on any touchscreen device
- Provides natural, mobile-like navigation

## Calibration Dialog

```
┌─────────────────────────────────────┐
│         Calibrate Model             │
├─────────────────────────────────────┤
│                                     │
│  Enter the known ambient            │
│  temperature:                       │
│                                     │
│  Temperature (°C)                   │
│  ┌─────────────────────────────┐   │
│  │ 23.5                        │   │
│  └─────────────────────────────┘   │
│                                     │
│              [Cancel] [Calibrate]   │
└─────────────────────────────────────┘
```

## CSV Export Success Dialog

```
┌─────────────────────────────────────┐
│        Export Successful            │
├─────────────────────────────────────┤
│                                     │
│  History exported to:               │
│  ~/.local/share/ambient-learner/    │
│  history_export_20260304_143022.csv │
│                                     │
│                        [OK]         │
└─────────────────────────────────────┘
```

## UI/UX Improvements Summary

| Feature | Before | After |
|---------|--------|-------|
| Week Navigation | Text buttons | Circular icon buttons |
| Week Info | Single label | Title + date range |
| Jump to Current | Not available | "Today" button |
| CSV Export | Not available | Integrated button |
| Touch Support | None | Swipe gestures |
| Button States | Always enabled | Smart disable logic |
| Visual Hierarchy | Flat | Clear information layers |
| Spacing | Compact | Comfortable, modern |

## Color Scheme

| Element | Color | Meaning |
|---------|-------|---------|
| Temperature < 40°C | Blue (#4A90E2) | Cool |
| Temperature 40-55°C | Green (#7ED321) | Normal |
| Temperature > 55°C | Orange (#F5A623) | Warm |
| Sensors Locked | Green (#7ED321) | OK |
| Sensors Error | Red (#E24A4A) | Error |
| UI Accent | libadwaita blue | Primary actions |

## Responsive Design

The UI adapts to window sizes:
- **Minimum**: 400x700 pixels
- **Maximum**: Unlimited (scales gracefully)
- **Orientation**: Vertical preferred
- **Touch targets**: All buttons ≥44x44 pixels

## Accessibility

- ✅ High contrast color scheme
- ✅ Tooltips on all icon buttons
- ✅ Keyboard navigation support
- ✅ Screen reader compatible labels
- ✅ Touch-friendly button sizes
- ✅ Clear status indicators
