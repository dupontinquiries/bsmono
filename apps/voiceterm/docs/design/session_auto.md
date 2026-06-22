# Session auto-management (v0.1.658)

## Motivation (from the Hector corpus)
Hector works LINEARLY in one terminal — manually juggling tracer scripts,
reboots, and edits. A clean engineering workflow would split that into
per-task sessions/tabs, but that costs keyboard throughput he's already
maxed on and thought-stream keepup he has no voice for. session_auto detects
context shifts and PROPOSES the structure he'd make if he had spare hands —
"new tab for foo.py, helix, in this workspace" — never auto-applying.

## Pure core (src/session_auto.rs, 10 tests)
- segment(activity_stream) → named Segments, split at repo-change + reboot.
- propose(...) → NewTabForFile / NewSessionForRepo / ReentryAfterReboot.
- name_segment(...) → "repo · dominant-file/lang".

## Validated on real data
Derived a 103-event stream from Hector's transcript (6 reboots). Output: 7
named sessions split at the reboots; 6 reboot re-entry proposals + a new-tab
proposal on a script switch. Matches his documented #1 pain (post-reboot
context loss). Repro: see scripts/hector_stream.py technique (corpus →
ordered Activity stream).

## Honest scope
The pure core is validated on a DERIVED stream (transcript markers in order),
not a live pty feed — segmentation is right in shape but coarse (tiny inter-
reboot gaps; needs a min-session merge). Wiring (real events in, assist-
bubble out, accept-actions) is build-gated. NEVER auto-applies — proposals
are transient, dismissable bubbles.
