#!/usr/bin/env bash
# build/flatpak.sh — Flatpak packaging stage (PLAN-printing stub).
#
# Runs on a host with flatpak + flatpak-builder + flatpak-cargo-generator.
# For each project with flatpak != false in manifest/projects.toml:
#   1. flatpak-cargo-generator.py Cargo.lock -o cargo-sources.json   (offline deps)
#   2. assemble a manifest using the project's `runtime` (kde|gnome|freedesktop)
#   3. flatpak-builder --repo=<self-hosted repo> build-dir <app>.yml
#
# Not wired in-sandbox (no flatpak toolchain here). Prints the plan it would run.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
echo "flatpak stage — PLAN ONLY (needs flatpak-builder + flatpak-cargo-generator)"
python3 - "$ROOT" <<'PY'
import sys, os, tomllib
P = tomllib.load(open(os.path.join(sys.argv[1],"manifest","projects.toml"),"rb"))
for grp, items in P.items():
    if not isinstance(items, dict): continue
    for name, m in items.items():
        if not isinstance(m, dict): continue
        fp = str(m.get("flatpak", "false")).lower()
        if fp == "false": continue
        rt = m.get("runtime", "tbd")
        print(f"  {name:16} flatpak={fp:8} runtime={rt:12} -> cargo-sources.json + {name}.yml")
PY
echo
echo "notes:"
echo "  - 'partial' (bo-suite): only the app component ships; the daemon is system-level."
echo "  - runtime 'tbd' (av1tuner, gitmind): confirm the GUI toolkit before writing the manifest."
echo "  - alpine/musl + server services (logscope, snapclone daemon) ship as packages, not flatpaks."
