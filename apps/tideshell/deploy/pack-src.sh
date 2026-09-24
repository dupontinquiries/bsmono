#!/bin/sh
# Pack exactly the sources tideshell's phone build needs (tideshell and
# tideshell-settings; 13 in-repo crates,
# no workspace root) from a clone of the bundle, into tideshell-src.tar.gz.
# Run in the clone, on any machine with git:
#   sh apps/tideshell/deploy/pack-src.sh [ref]     (default: HEAD)
# The list was computed with `cargo metadata --no-default-features
# --features layer,ei` on the tideshell branch; if cargo is available here,
# the script re-derives it and refuses to pack a stale list.
set -eu
REF=${1:-HEAD}
PATHS="apps/hatch/crates/hatch-core
apps/tideshell/crates/tideshell-core
apps/tideshell/deploy
apps/tideshell/egui-frontend
apps/tideshell/settings
shared/ambient-palette
shared/config-kit
shared/cosmic-levers
shared/math-kit
shared/settings-kit
shared/speaker-pose
shared/systemcompkit
shared/touchkit
shared/uikit/crates/snapclone-theme
shared/uikit/crates/uikit-egui
shared/uikit/crates/uikit-gesture
system/comfortgov"
if command -v cargo >/dev/null 2>&1; then
    rel='import json,sys,os; r=os.path.realpath("../../.."); m=json.load(sys.stdin); print("\n".join(sorted({os.path.relpath(os.path.dirname(p["manifest_path"]), r) for p in m["packages"] if p["source"] is None})))'
    derived=$( (cd apps/tideshell/egui-frontend && cargo metadata --format-version 1 --no-default-features --features layer,ei 2>/dev/null | python3 -c "$rel"; \
                cd ../settings && cargo metadata --format-version 1 2>/dev/null | python3 -c "$rel") | sort -u)
    for d in $derived; do
        echo "$PATHS" | grep -qx "$d" || { echo "pack-src: $d is a build dependency but not in the list; update PATHS" >&2; exit 1; }
    done
fi
git archive --format=tar.gz -o tideshell-src.tar.gz "$REF" $PATHS
echo "wrote tideshell-src.tar.gz ($(du -h tideshell-src.tar.gz | cut -f1)) from $(git rev-parse --short "$REF")"
