#!/usr/bin/env bash
# registry_doctor.sh — diagnose "failed to select a version for the
# requirement `X = "^1"` (locked to N)" across the fleet.
#
# WHAT THIS ERROR ACTUALLY MEANS, because the obvious reading is wrong.
#
# It looks like a lockfile problem, and every search result will tell you
# to delete Cargo.lock or run `cargo update`. On a monorepo that is 36
# lockfiles of collateral damage for a fault that is usually not in any
# of them.
#
# The line that matters is the SECOND one:
#
#   error: failed to select a version for the requirement `uuid = "^1"` (locked to 1.24.1)
#   candidate versions found which didn't match: 0.8.1, 0.8.0, 0.7.4, ...
#
# "Candidate versions found" is what cargo could SEE in the local index
# cache. If the newest candidate is years older than the locked version,
# the lockfile is fine and the local index cache is stale or truncated
# for that crate. uuid 1.x has existed since 2022; an index offering
# 0.8.1 as its best is not a version-selection problem, it is a cache
# that never learned uuid 1 exists.
#
# The giveaway that it is ONE crate and not the whole index: the same
# build resolves egui 0.34 and accesskit 0.24 without complaint. A
# genuinely old index would fail on those first.
#
# WHY IT HITS "HALF THE MONOREPO": nothing to do with our crates. `uuid`
# is a transitive dependency of `accesskit`, which every egui app pulls
# in, so one stale cache entry fails every GUI app at once and looks like
# a repo-wide breakage.

set -uo pipefail
ROOT="${1:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
CRATE="${2:-uuid}"

echo "== $CRATE across $(find "$ROOT" -name Cargo.lock -not -path '*/target/*' | wc -l) lockfiles"
find "$ROOT" -name Cargo.lock -not -path '*/target/*' -print0 \
  | xargs -0 grep -A1 "^name = \"$CRATE\"$" 2>/dev/null \
  | grep '^.*version = ' | sed 's/.*version = //' | sort | uniq -c | sort -rn

echo
echo "== what crates.io actually publishes"
if curl -fsS --max-time 20 "https://index.crates.io/$(
      printf '%s' "$CRATE" | awk '{n=length($0);
        if (n==1) print "1/" $0;
        else if (n==2) print "2/" $0;
        else if (n==3) print "3/" substr($0,1,1) "/" $0;
        else print substr($0,1,2) "/" substr($0,3,2) "/" $0}')" \
     2>/dev/null | python3 -c '
import sys, json
rows = [json.loads(l) for l in sys.stdin if l.strip()]
live = [r["vers"] for r in rows if not r.get("yanked")]
print("newest published:", live[-1] if live else "none")
yanked = [r["vers"] for r in rows if r.get("yanked")]
print("yanked:", ", ".join(yanked[-6:]) if yanked else "none")
'; then :; else
  echo "  (could not reach index.crates.io — check network, then rerun)"
fi

echo
echo "== local index cache for $CRATE"
found=0
for c in "$HOME"/.cargo/registry/index/*/.cache; do
  [ -d "$c" ] || continue
  hit=$(find "$c" -name "$CRATE" -type f 2>/dev/null | head -1)
  if [ -n "$hit" ]; then
    found=1
    newest=$(strings "$hit" 2>/dev/null | grep -o '"vers":"[0-9][^"]*"' \
             | sed 's/.*://; s/"//g' | sort -V | tail -1)
    echo "  $hit"
    echo "    newest version this cache knows: ${newest:-unreadable}"
  fi
done
[ "$found" = 0 ] && echo "  no cached index entry (cargo will fetch it fresh — good)"

cat <<'ADVICE'

== if the cache's newest is older than the locked version
Delete just that entry and let cargo refetch. This is safe: the index
cache is derived data, not configuration, and nothing else depends on it.

    rm -rf ~/.cargo/registry/index/*/.cache/uu/id/uuid

Do NOT delete Cargo.lock or run `cargo update` first. The lock is
correct — the same lockfiles resolve on a machine with a healthy cache —
and regenerating 36 of them would replace a one-directory fix with a
fleet-wide version bump nobody asked for, plus a diff that hides
whatever the next real problem turns out to be.

If the cache looks current and it STILL fails, check for source
replacement (a vendored or mirrored registry that genuinely lacks the
crate):

    cat ~/.cargo/config.toml .cargo/config.toml 2>/dev/null | grep -A3 'source\.'
ADVICE
