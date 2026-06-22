# Unified bottom bar — expandable multi-button tabs / workspaces / quick-launch

Status: **DESIGN + pure model core (v0.1.507).** Composes the EXISTING
gesture primitives (GestureHost + SlidePopup) rather than inventing new
input handling. The pure dynamic model is built + tested; the QML
assembly is staged (live surface).

## The feature (user)

A unified bottom bar where each button supports the SAME tap /
tap-and-hold-drag-to-a-multi-button gesture as the terminal tab's
pipe (`|⋯`) and brace (`{}`) toolbar buttons. The bar is EXPANDABLE
into: tabs, workspaces, quick-launch commands (recent cards, etc.),
and is DYNAMIC. It should reuse the previously-written buttons to
abstract away the gesture quirks.

## What already exists (the primitives to compose — confirmed)

- **GestureHost.qml** — input-owning wrapper. `GestureHost { popup;
  gestureContext }` around arbitrary visuals; owns the press → drag →
  release → commit/cancel lifecycle; drives a SlidePopup. Solves the
  quirks the user means: coordinate handoff (host-local → hostRoot
  via mapToItem), single event source (no PointHandler reparenting
  race — the v0.1.143 bug), parent-Flickable steal prevention. New
  hosts are ~5 lines.
- **SlidePopup.qml** — passive visualizer. `model: [{label, id, ...}]`,
  fans out a grid/radial of options, tracks the drag pointer, emits
  `selected(optionId, variant, context)` + `cancelled()`. One popup
  instance can serve MANY hosts (disambiguated by the opaque
  `gestureContext` echoed back in selected()).
- The toolbar pipe/brace buttons are the proof: GestureHost +
  pipeMenuModel (3×2 shell operators) + a shared SlidePopup.

So the bottom bar is architecturally: **a Row of GestureHost buttons,
each with its own model, all reusing the SlidePopup machinery.** Tap =
primary action; tap-hold-drag = fan out that button's menu. The
gesture quirks are ALREADY abstracted — we just supply models +
visuals + a selected() router.

## Architecture

### The dynamic model (THIS ship: pure Rust core)
A `BottomBar` model the backend builds + exposes, so the bar is
DYNAMIC (buttons appear/reorder/relabel by state). Each button is a
`BarButton`:
  - id, label/icon, a `kind` (Tab / Workspace / QuickLaunch /
    RecentCards / Custom),
  - a `primary` action id (what a tap does),
  - an `expand` model: Vec of `BarOption {id, label}` — what the
    tap-hold-drag fan-out shows (the SlidePopup model for that
    button).
The backend assembles the button list from live state: open tabs,
defined workspaces, recent cards (from note_cards / session), recent
commands. Reordering, badges, enable/disable are model edits — the
QML Row just renders whatever the model says.

### The QML assembly (staged — live surface)
A Row in the bottom bar binding to the model: one GestureHost per
BarButton, its SlidePopup model = the button's expand options, a
single `onSelected` router that maps (buttonId, optionId) → a backend
action call. Reuses GestureHost/SlidePopup verbatim — no new gesture
code. Needs the running surface to lay out + tune sizing/animation
(like the pipe/brace 38×32 vs 32×28 tuning the toolbar already went
through).

### Selection routing
SlidePopup already echoes `gestureContext` in `selected()`. Set each
host's context to its button id; the router becomes
`onSelected(optionId, variant, context) → backend.bar_invoke(context,
optionId)`. One handler for the whole bar.

## What v0.1.507 ships (pure model core)

`src/bottom_bar.rs`: BarButton / BarOption / BarButtonKind, a
`BottomBar` that holds the ordered buttons, and pure builders/queries:
add/reorder/find, `options_for(button_id)`, and a `default_bar()`
assembling Tabs + Workspaces + QuickLaunch from supplied state. Pure,
no QML, no IO. Fully tested. The backend later feeds it live tab/
workspace/card lists; QML renders it via GestureHost+SlidePopup.

NOT yet: the QML Row, the backend qt_methods exposing the model to
QML, the selected() router. Those need the live surface (layout,
animation tuning) + the real tab/workspace/session sources wired.

## Context / principles honored

- Reuses GestureHost + SlidePopup (the user's "previously written
  buttons" / "abstract away the quirks") — ZERO new gesture code.
- Model in Rust, QML renders — the no-JS-overhead split.
- Dynamic = a Rust-built model the QML Row mirrors (same pattern as
  pipeMenuModel, generalized).

## Open questions for the user

- Default button set + order (Tabs, Workspaces, QuickLaunch, Recent
  Cards — which, in what order)?
- Tap vs tap-hold semantics per kind (e.g. tap a Tab button = switch
  to last tab? hold = fan out all tabs?).
- Should the bar auto-condense like the terminal toolbar does, or
  scroll horizontally when crowded?
