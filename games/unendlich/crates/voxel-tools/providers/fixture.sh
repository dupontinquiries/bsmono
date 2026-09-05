#!/usr/bin/env sh
# A provider that needs no model. Proves the loop end to end.
# Reads and discards the prompt, emits a plan relative to the focus.
cat >/dev/null
cat <<'PLAN'
# fixture tower
terraform @-5,-5 @5,5 smooth block=stone
cylinder @0,0 r=3 h=11 block=stone_bricks hollow
sphere @3,0 r=1 block=air
path @3,0 @16,9 w=2 block=cobblestone
PLAN
