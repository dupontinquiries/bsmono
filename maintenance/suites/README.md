# App suites

A **suite** is a named bundle of apps that installs together. Installing a suite
builds every app for whatever device you run it on and drops a `.desktop` launcher
for each, so they show up in the Plasma app grid (mobile and desktop).

- `all.json` — a reference bundle of every binary crate in this repo, with sane
  defaults (native build, default features, no launch flags). Paths are relative to
  the repo root.

Runtime suites live in `~/.config/gitmind/suites/`. To generate one for your machine
(with absolute paths), use the GUI (Apps → suites → "create all") or:

    gitmind fleet suite create all --root ~/Code
    gitmind fleet suite install all

Other commands: `gitmind fleet suite list | show <name> | rm <name> [<app>]`.
Each app entry: `{ name, path, features[], no_default, target ("" = native), run_flags }`.
