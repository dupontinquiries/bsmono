# Command Surface (Palette Mode) — Design Doc

Status: **design / pre-prototype**
Working name: **Palette mode** (the surface is the "command surface")
Filed: this session, alongside the micro-mode / clipboard ideation.

---

## 1. The idea in one paragraph

The keyboard already owns the bottom slab of the screen and already talks
to the compositor. Instead of *only* emitting characters, that slab can
become a **compact, gesture-rich command surface**: a touch-friendly,
suggestive list of actions — `File ▸ Open / Save / Recent…`, `View ▸ Zoom /
Layout…`, edit commands, app menus — rendered as a dense, scrollable,
reorderable palette with KDE-Plasma-applet-style context. It is a command
palette and a menu bar and a widget host, fused, living where your thumbs
already are. Think: the macOS menu bar + VS Code command palette + a Plasma
panel, but thumb-shaped and gesture-driven.

This is **not** another letter layout. It's a parallel surface the user
swaps to (like steno/cardinal are modes, but this one shows *actions*, not
*keys*). It sits in the same `experimental_mode` family as a new value
`"palette"`, OR — better — as its own toggle reachable from any layout,
because you want it available *while typing* (open it, fire a command, fall
back to the keyboard).

---

## 2. Why a keyboard is uniquely good at this

- **It already owns screen real estate** at the bottom — no new window,
  no focus-stealing.
- **It already has compositor access** (input-method + virtual-keyboard
  protocols) — so commands can do real things: commit text, send keysyms
  (Ctrl+S, Ctrl+O), and — with more protocol work — talk to KWin /
  plasma-window-management.
- **It already intercepts the keystroke stream** — so it knows context
  (what app, what field type via `content_purpose`, recent commits) and
  can suggest relevant commands.
- **It already has the microterm + LLM** — so a command can be "ask the
  model to do X" and the result flows back through the same pipe.

A standalone command-palette app can't do most of this without becoming a
keyboard. We already *are* the keyboard.

---

## 3. What lives on the surface

Three content classes, in priority order:

### 3a. Commands (the core)
Discrete actions rendered as **chips / list rows**. Each command has:
- a glyph/icon + short label,
- an action (see §5),
- optional context predicate (only show in app X / field type Y / when
  text is selected),
- optional submenu (File ▸ … expands a child list).

Examples grouped by source:
- **App menu mirror**: `File`, `Edit`, `View`, … expanding to the focused
  app's actual menu items where the app exposes them (DBusMenu / global
  menu — KDE has this via `org.kde.plasma` global-menu / `com.canonical.
  AppMenu`). Where unavailable, fall back to standard keysym shortcuts
  (`Ctrl+S` for Save, `Ctrl+O` for Open).
- **Edit commands**: cut/copy/paste, undo/redo, select-all — sent as
  keysyms or via the clipboard ring.
- **Text transforms**: the LLM rewrite/shorten/fix actions (reuses the
  existing LlmTask pipeline) — shown when text is selected.
- **Window/workspace** (later, protocol-gated): tile, maximize, move to
  desktop, switch desktop.

### 3b. Clipboard ring (the "superpower")
A horizontally-scrolling strip of recent clips as **draggable, reorderable
chips**. Long-press to pick up, drag to reorder, tap to paste-commit,
swipe-up to pin, swipe-off to delete. Captured from every `TextCommitted`
and from selection via surrounding-text. Bounded ring (~20), pinned items
exempt from eviction. This is self-contained and could ship before the full
palette.

### 3c. Widgets (the ambitious part)
KDE-Plasma-applet-style mini-widgets embedded in the surface: clock,
battery, media controls, a calculator strip, a unit converter. **Third-party
widget support** is the stretch: a small declarative widget manifest
(label + data source + tap action) that third parties can drop into a
config dir, OR — more Plasma-native — actually host Plasma applets. Hosting
real Plasma applets (QML `Plasmoid`s) from a GTK process is hard (different
toolkits); the realistic v1 is *our own* widget trait with a simple plugin
manifest, and "Plasma-style" is an aesthetic + interaction goal, not literal
plasmoid hosting.

---

## 4. Interaction model (gesture-rich + compact)

The whole point is density without clutter. Borrowed grammar from the rest
of the keyboard so it feels native:

- **Tap** a chip → fire its primary action (or expand a submenu).
- **Long-press** a chip → secondary action / pick-up-to-drag (clipboard) /
  reveal context menu (Plasma-applet style).
- **Swipe a chip up** → pin / promote.
- **Swipe a chip off (down)** → dismiss / delete.
- **Horizontal drag on a chip** → reorder within its row (clipboard, custom
  command order).
- **Swipe the whole surface left/right** → page between command groups
  (File-group ▸ Edit-group ▸ Clipboard ▸ Widgets).
- **Swipe down on the surface header** → collapse back to the keyboard.
- **Search-to-filter**: a thin entry at the top filters the command list as
  you type (command-palette behavior); the same typed text can fall through
  to the document if no command matches (so you don't *have* to switch back
  to the keyboard for a quick word).

Layout: a **header strip** (title + search + collapse handle) over a
**scrollable body** of grouped rows. Each row is a horizontal scroller of
chips (so it's 2D: vertical groups, horizontal items). Compact = chips are
~1.2 line-heights tall, icon + truncated label, density like a Plasma
system-tray flyout.

---

## 5. Action types (how a command *does* something)

A `PaletteAction` enum, dispatched through machinery we mostly already have:

1. **Keysym(s)** — send a shortcut chord (Ctrl+S, Ctrl+Shift+Z). Uses the
   virtual-keyboard backend's `press_keysym` (needs modifier-chord support;
   today it presses single keysyms — small extension).
2. **CommitText(String)** — paste a snippet / clipboard entry via the
   existing commit pipeline.
3. **LlmTask** — run a rewrite/continue/freeform on the surrounding text;
   result commits back. Reuses `src/llm`.
4. **AppMenuItem(id)** — trigger a global-menu item over DBusMenu (later).
5. **WindowCmd** — tile/move/switch via plasma-window-management or KWin
   D-Bus (later, protocol-gated).
6. **OpenSubmenu(group_id)** — navigate to a child command group.
7. **Builtin** — internal (collapse surface, open settings, toggle a
   widget).

Most of v1 is achievable with #1, #2, #3, #7 — all of which ride existing
infrastructure. #4 and #5 are the "talks to Plasma" stretch items.

---

## 6. Intelligent surfacing (ties to the auto-switch setting)

The user asked for intelligent style selection with: an on/off setting, a
small button to see all options, and an **"off for next 30 minutes"**
escape. That generalizes beyond palette mode into a single
**Context Engine**:

- Inputs: focused-app id, `content_purpose` (from text-input-v3, currently
  dropped on the floor at wayland_backend.rs:892 — easy to start storing),
  whether text is selected, recent command usage.
- Output: a ranked, filtered command list + a *suggested surface* (full
  keyboard vs numpad micro vs palette vs correction bar).
- Setting model:
  - `auto_surface: bool` — master on/off.
  - A small **"surfaces" button** on the keyboard that opens a chooser
    showing every available surface (QWERTY, Dvorak, Cardinal, Steno,
    Numpad, URL pad, Palette, Clipboard) so the user can pick manually.
  - **"Pause auto for 30 min"** — a timestamp in config; while now <
    paused_until, the Context Engine is bypassed and the last manual
    choice sticks. (Implementation: `AtomicU64 paused_until_epoch`; the
    resolver checks it before consulting the engine.)

The 30-minute pause is a genuinely nice UX primitive — it acknowledges that
auto-switching is great until it fights you, and gives a low-commitment
"leave me alone" without making the user hunt through settings to disable
it permanently.

---

## 7. Architecture sketch

```
src/palette/
  mod.rs          // PaletteSurface, PaletteAction, dispatch
  command.rs      // Command struct, groups, context predicates
  registry.rs     // built-in command set + manifest loader (3rd-party)
  clipboard.rs    // bounded clip ring, pin/evict, capture hooks
  widgets/        // builtin mini-widgets (clock, battery, calc…)
    mod.rs
src/context/
  engine.rs       // ContextEngine: inputs → ranked commands + surface hint
  purpose.rs      // content_purpose plumbing from wayland_backend
```

- The palette surface is a sibling of the keyboard view, swapped into (or
  overlaid on) `keyboard_box` like the settings panel is today.
- Commands are data; the registry seeds built-ins and merges a user/3rd-
  party manifest dir (`$XDG_CONFIG_HOME/rusty-board/commands.d/*.toml`).
- The clipboard ring hooks `EngineEvent::TextCommitted` (already broadcast)
  to capture, and uses surrounding-text for selection capture.
- The Context Engine is consulted by the layout resolver; gated by
  `auto_surface` and the 30-min pause.

---

## 8. Risk / effort ladder (what to build in what order)

1. **Clipboard ring mode** — self-contained, high delight, reuses commit
   pipeline + GTK drag/drop. No protocol risk. *Ship first.*
2. **Command palette (keysym + LLM + builtin actions)** — the core surface
   with a search filter and grouped chips. Actions limited to keysym
   chords, snippets, LLM tasks, and builtins. No Plasma protocol needed.
3. **Context Engine + content_purpose plumbing** — store the dropped
   ContentType event; add the auto_surface setting, surfaces chooser, and
   30-min pause. Wire numpad/URL micro-layouts as the first auto targets.
4. **Widgets (builtin)** — clock/battery/calc as our own widget trait.
5. **Third-party widget manifest** — declarative TOML plugin format.
6. **App-menu mirror (DBusMenu)** — talk to the global menu. Protocol work.
7. **Window/workspace commands** — plasma-window-management / KWin D-Bus.
   Highest protocol risk; do last.

---

## 9. Open questions

- **Overlay vs replace**: should the palette overlay the top of the
  keyboard (keys still visible/usable below) or fully replace it? Overlay is
  more powerful (type + command simultaneously) but harder to lay out on a
  short phone screen. Lean: replace on phone, overlay on tablet/landscape.
- **Search fall-through**: if a typed query matches no command, do we
  silently commit it to the document, or require an explicit "send as text"?
  Fall-through is magical but can surprise; maybe a setting.
- **3rd-party trust**: a manifest that can send arbitrary keysym chords is a
  security surface. Sandbox to a keysym allowlist? Require explicit enable
  per manifest?
- **Widget data sources**: builtin widgets need battery (upower DBus),
  media (MPRIS DBus), clock (trivial). Each DBus dependency is a small risk;
  start with clock (zero deps) to prove the widget trait.

---

## 10. What the visualizer should test

Like the steno visualizer, build a standalone HTML harness to feel out the
*interaction*, not the plumbing:
- The 2D chip grid (vertical groups × horizontal scrollers).
- Tap / long-press / swipe-up-pin / swipe-off-delete / drag-reorder on
  chips.
- Page-swipe between groups.
- The search-filter behavior + fall-through.
- Density: how many chips fit legibly on a 360px-wide phone slab.
- The "surfaces chooser" popover + the 30-min pause affordance.

The thing we most need to learn: **is a 2D scrollable chip palette actually
thumb-usable at phone density, or does it need to be 1D (single scrolling
list) to avoid fat-finger misfires?** That's the question the visualizer
answers before any Rust gets written.
