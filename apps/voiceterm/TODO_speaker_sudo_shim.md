# TODO: speaker sudo-shim — tier-gate + point-of-elevation attribution

## What this is ("the fake user stuff")
Your design (quoted in docs/todo.md ~4695): instead of creating real Linux
users for guest speakers, web2qml keeps APP-LEVEL identity, runs everything
as the actual logged-in OS user, and "passes fake users through to valid
system users for sudo … with some echo logging." Framed correctly in the
doc as ATTRIBUTION, not impersonation: track WHO ASKED while the action
RUNS as the real OS user; the app permission TIER governs WHETHER a
privileged attempt is allowed.

## What's ALREADY DONE (shipped — don't rebuild)
The attribution half is fully wired:
- **App-level identity + allow model**: speakers.rs has `allowed` (+ muted
  tiering). An un-allowed speaker's voice is discarded/backgrounded, never
  drives commands. No real OS users are created — exactly the conservative
  middle ground the doc recommends.
- **Per-command attribution**: voice identifies a speaker →
  handle_identified_speaker() (main.rs ~15405) calls
  audit_log::set_pending_attribution(id, name, confidence, "voice").
  term_blocks submit_command (~416) consumes it via
  take_pending_attribution() and stamps the block with speaker_id/name/
  confidence/attestation. Single-shot + 30s stale-timeout so a later typed
  command isn't falsely attributed. Typed commands record attestation
  "typed".
- **Audit log**: audit_log.rs — hash-chained, append-only, fsync-per-entry,
  XDG_STATE_HOME path, session tracking. Real, tested.

So "track who asked, run as the real OS user, log it" = DONE.

## The actual GAP (still TODO)
There is NO privileged-action TIER GATE. Two missing pieces:
1. **Tier check before elevation**: when an identified speaker's intent
   requires sudo / privilege, check their tier (owner vs guest) and decide
   allow/deny BEFORE the command runs. Today sudo is only DETECTED (the
   password-prompt state at main.rs ~2090), not gated by speaker tier.
   speakers.rs notes (line ~32-34) explicitly flag "privilege levels" as
   not-yet-built.
2. **Point-of-elevation audit entry**: a distinct audit record at the
   moment a privileged action is attempted/executed (speaker, tier
   decision, command), separate from the normal block attribution. The doc
   wants the attempt logged "regardless" of whether it was allowed.

## Build plan (pure-first, privileged path is build-gated)
- PURE + testable NOW: a `sudo_shim::decision(speaker_tier, intent_needs_priv,
  is_owner) -> {Allow, Deny, AllowWithLog}` plus a "does this command need
  privilege" classifier (sudo/doas/pkexec prefix, known privileged verbs).
  Both are pure string/enum logic — build + test in-sandbox like wake_gate.
- Then a Tier concept on Speaker (owner | trusted | guest | observer) — a
  pure addition to speakers.rs with serialization; the default for the
  single existing speaker = owner (no behavior change).
- BUILD-GATED wiring: gate the privileged-command path on the decision +
  write the point-of-elevation audit entry. This touches the command-exec
  path near sudo detection — do NOT blind-edit; do the pure decision first,
  document the exact gate site, verify on device.

## Why it wasn't findable
Lived only in the big docs/todo.md narrative, never in a TODO_*.md. The
attribution infra got built (v0.1.253-254) but the tier-gate half was
never filed as its own task. Captured here so it survives.

## Security note
This is the one area to be MOST careful: a tier-gate bug that lets a guest
elevate, or that creates a false mental model of isolated OS users, is a
real footgun. The doc's framing (attribution not impersonation, never real
OS users, OS user's own consent at execution) is the correct guardrail —
keep it.
