# av1tuner-m1: RESOLVED (c11) — merged here, fork retired

This file previously declared `apps/av1tuner-m1` the live fork. That was true
when written and then went stale in the §5 way: two later commits fixed bugs
in THIS copy only (95a2b66d, 96e35966), so both sides ended up ahead of the
other. The c11 merge took the union into this tree, under this name:

* from m1: concat.rs (mkvmerge VVenC muxing — raw H.266 concat has no
  per-packet timestamps, which is the encode-failure mechanism), preflight's
  mkvmerge check, app.rs (cache hits verified DECODABLE before acceptance —
  m1's app.rs was a strict superset of this copy's cache handling),
  cleanup.rs intermediates, prediction_stats' remainder assertion, scene.rs.
* from this copy: job.rs — the real derive_state_dir fix
  (`parent().filter(|p| !p.as_os_str().is_empty())`), which m1 lacked.

Verified at merge: 97 passed / 0 failed across the workspace, matching m1's
own record. Which binary failed the field AV1 encode was NOT determined —
the merged tree carries every known fix from both, and the strongest
mechanism-level candidate is the cache-decodability check (a byte-intact,
undecodable cached file completes an "encode" with broken output). If AV1
encodes still fail here, that is a NEW bug, not the duplicate's.

The fork's full history remains in git under `apps/av1tuner-m1/`.
