# hatch

Fullscreen tiling app launcher, twice: a Plasma 6 plasmoid (QML, the
kickerdash fork, touch-corrected) and an egui shell over a headless Rust
core. Same interaction contract on both — tap launches, empty-space taps do
nothing, dismissal is always an explicit gesture.

```
qml/            the plasmoid (org.hatch.dash)
crates/
  hatch-core/   headless launcher model: .desktop scan, Exec spawn,
                icon-theme resolve, fuzzy rank, favorites/recents (config-kit)
egui-frontend/  hatch-egui: the grid shell over hatch-core (uikit-egui)
packaging/      org.hatch.Hatch.desktop
```

## Plasmoid

```sh
kpackagetool6 --type Plasma/Applet --install qml     # or --upgrade
plasmashell --replace &                               # or log out/in
```

Then add "Hatch Dash" to a panel. The button toggles the dash; so does
`qdbus6 org.kde.plasmashell /PlasmaShell activateLauncherMenu` if it is set
as the launcher, which is what you bind Meta to.

Touch contract (the point of the rewrite): grid flicks natively; tap
launches; tap on empty grid space does nothing; long-press opens the
context menu (favorites live there); pull past the top of the grid or
swipe down on the backdrop to close (both honor the Touch config section);
tap on empty backdrop closes; typing anywhere searches; Enter launches the
top hit; the system row hides while maliit is up.

The plasmoid's pure logic is plain JavaScript under `contents/ui/code/`
precisely so it can be run without Plasma:

```sh
node --test qml/tests/*.mjs
```

That covers `tools.js` (the context-menu builder, written from scratch
because the fork's drop didn't include kicker's) and `animation.js` (the
seven slide-in patterns, extracted from the delegate for this reason —
the arithmetic is invisible in review and obvious on screen).

## hatch-egui

```sh
cd egui-frontend
cargo build --release            # bin: target/release/hatch-egui
cargo test                       # core + shell logic, no display needed
```

Launcher lifecycle: one instance; a second launch (or `hatch-egui
--toggle`) pokes the running one to show/hide — bind a global shortcut to
exactly that. Dismissal hides rather than quits so the next summon is
instant (`--quit-on-hide` for one-shot). Focus loss hides (`--stay` while
debugging on a desktop). Config in `~/.config/hatch/config.ini`
(columns/icon_px/opacity/icon_theme/system_actions/terminal_wrap),
favorites and recents next to it, all plain text.

## Favorites across the two shells

The plasmoid uses kicker's favorites model (stored in Plasma's
`plasma-org.kde.plasma.desktop-appletsrc`); hatch-egui uses
`~/.config/hatch/favorites`. On first run — and only while hatch has no
favorites of its own — hatch reads the Plasma list and adopts it, so a
curated dash carries over instead of greeting you with an alphabetical
wall.

It is a seed, not a sync, and deliberately one-directional. plasmashell
holds its config open and rewrites it wholesale on its own schedule, so an
outside writer loses races it cannot see: reading is safe, writing is not.
After the seed the two lists can diverge, which is at least honest and
both are plain text. `cargo run --example probe` prints what each side
currently thinks.

## Verified / not verified

Verified here: `cargo test` green for hatch-core and hatch-egui
(x86_64 host); `cargo build` of the full app feature; qmllint syntax pass
on the QML (unresolved org.kde.* imports expected off-device); `node --test qml/tests/*.mjs` green for the plasmoid's pure JS.

Not verified here, needs the FP5: anything rendered (no display in the
build env) — maliit summon/avoidance, KWin resize behavior when the
keyboard shows, Adreno/zink glow performance, plasmoid against real
Plasma 6 libs, aarch64-musl cross build.
