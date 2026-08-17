#!/bin/sh
# counter-scope-check.sh — catch ratios that mix a per-reply counter with a lifetime one.
#
# This class has now produced three wrong numbers:
#   c4-810  the roofline line never printed, because it read counters c4-774 had already consumed
#   c4-816  vv was a LIFETIME total while hv was per-reply, so the offset-bearing hit rate shrank
#           toward zero the longer a session ran — introduced in the commit meant to fix a denominator
#   and the 265600% cache rate before both of them
#
# The shape is always the same: a global counter read at reply start to form a baseline, and a
# sibling that is not. So flag any `let X0 = 0` sitting next to real baselines, and any counter both
# reset with swap/store(0) AND read with a baseline subtraction — those cannot both be right.
set -e
cd "$(dirname "$0")/.."
fail=0

echo "== hardcoded-zero baselines beside real ones =="
if grep -rn 'let [a-z_0-9]*0 = 0u64;' crates/*/src/*.rs | grep -v test; then
  echo "  ^ a baseline of literal zero makes its counter a LIFETIME total. If a sibling in the same"
  echo "    ratio is baselined at reply start, the two are on different clocks."
  fail=1
else
  echo "  none"
fi

echo "== counters both reset and baselined =="
grep -rhoE '[A-Z_]{4,}\.(swap|store)\(0' crates/*/src/*.rs | sed 's/\..*//' | sort -u > /tmp/.csc_reset
grep -rhoE '[A-Z_]{4,}\.load\([A-Za-z:]*Relaxed\)\.saturating_sub' crates/*/src/*.rs | sed 's/\..*//' | sort -u > /tmp/.csc_based
both=$(comm -12 /tmp/.csc_reset /tmp/.csc_based)
if [ -n "$both" ]; then
  echo "$both" | sed 's/^/  /'
  echo "  ^ reset elsewhere AND read with a baseline. The subtraction saturates to zero after a"
  echo "    reset, so the reader silently sees nothing — which is how the roofline line never printed."
  fail=1
else
  echo "  none"
fi

[ "$fail" = 0 ] && echo "counter scopes look consistent" || echo "REVIEW the findings above"
exit 0
