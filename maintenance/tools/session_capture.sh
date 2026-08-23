#!/usr/bin/env bash
# session_capture.sh — record what was actually run, and attach it to the commit.
#
# Every corpus record this repo generates carries `observed_calls: null`,
# because the commands an author ran are not in git and reconstructing them
# from a finished diff produces fluent fiction. The fix was always "capture
# going forward" — said repeatedly and, until now, not built.
#
# WHAT THIS CAPTURES AND WHAT IT CANNOT
#
# It captures the SHELL: every command, its exit code, duration and working
# directory. That is most of the observable work — builds, tests, greps, git.
#
# It cannot capture an agent's internal tool calls, which happen in a harness
# above the shell and never reach a terminal. So a record produced here is
# honest about being partial: `layer: "shell"`. A later harness that logs its
# own calls can add `layer: "agent"` records against the same commit, and the
# two compose. Nothing here pretends to be the whole trace.
#
# WHY GIT NOTES
#
# The log attaches to the commit as a note under `refs/notes/toolcalls`, not
# as message text. Notes are out-of-band, so a message stays readable, and they
# can be rewritten or dropped without touching history. The catch, stated
# because it will bite: notes do NOT transfer on clone or fetch by default.
# Push and fetch them explicitly:
#
#     git push  origin refs/notes/toolcalls
#     git fetch origin refs/notes/toolcalls:refs/notes/toolcalls
#
# SECRETS
#
# A command log is a keylogger pointed at your own shell, and this repo's own
# workflow types a device password. Redaction runs before anything is written,
# never after — a secret that reaches the log has already been written to disk,
# and scrubbing it later leaves it in the reflog. Patterns are conservative and
# the list is easy to extend; the failure mode is an over-redacted log, which
# is the right direction to fail in.
#
# Usage:
#   source maintenance/tools/session_capture.sh      # start recording
#   ./maintenance/tools/session_capture.sh --install # add the post-commit hook
#   ./maintenance/tools/session_capture.sh --show HEAD

set -uo pipefail

# The default must be stable across SHELLS, not unique per shell. The first
# version used $$, and an agent harness runs every command in a fresh shell —
# so each invocation opened a new log, the hook's same-log test never matched,
# and each commit got at most its own invocation's commands (the two-line
# notes in this repo's own history, e.g. fbcfd6a7). Key the default by
# (user, repo) instead: any shell in the same repo lands on the same log,
# which is what "session" means to the hook that consumes it. Two terminals
# in one repo now interleave into one log; for a corpus that is more honest
# than each losing the other's commands. Set CAPTURE_LOG to override.
if [ -z "${CAPTURE_LOG:-}" ]; then
  _cap_root=$(git rev-parse --show-toplevel 2>/dev/null) || _cap_root=$PWD
  CAPTURE_LOG="${TMPDIR:-/tmp}/session-capture-$(id -u)-$(printf '%s' "$_cap_root" | cksum | cut -d' ' -f1).jsonl"
  unset _cap_root
fi
NOTES_REF="refs/notes/toolcalls"

# ── redaction ────────────────────────────────────────────────────────────────
# Applied to the command string before it is written. Anything that looks like
# a credential becomes a marker; the SHAPE of the command survives, which is
# what a corpus wants, and the value does not.
_redact() {
  sed -E \
    -e 's/(-p|--password|--pass)[= ][^ ]+/\1 <redacted>/g' \
    -e 's/(sshpass[^|]*-p)[= ]?[^ ]+/\1 <redacted>/g' \
    -e 's/(PASSWORD|PASSWD|TOKEN|SECRET|API_KEY)=[^ ]+/\1=<redacted>/g' \
    -e 's/(Authorization:[[:space:]]*Bearer)[[:space:]]+[^ "]+/\1 <redacted>/g' \
    -e 's/\b[0-9]{6,}\b/<digits>/g' \
    -e 's#(https?://)[^:/@ ]+:[^@ ]+@#\1<redacted>@#g'
}

_json_escape() {
  python3 -c 'import json,sys; sys.stdout.write(json.dumps(sys.stdin.read().rstrip("\n")))' 2>/dev/null \
    || sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/^/"/' -e 's/$/"/'
}

# ── recording (source this file) ─────────────────────────────────────────────
#
# One DEBUG trap, flushing the PREVIOUS command. The first version used
# PROMPT_COMMAND to flush, which only runs in an INTERACTIVE shell — so it
# recorded nothing at all under `bash -c` or any script, which is exactly how
# an agent runs commands. The trap sees `$?` from the command that just
# finished, so exit codes survive without a prompt hook.
#
# The final-command problem, and why there is a sidecar file. Flush-on-next
# means a shell's FINAL command is never flushed — the next command never
# comes. In a one-command-per-fresh-shell harness the final command is the
# only command, so three such shells recorded nothing at all (--selftest
# reproduces this against the old code). An EXIT-trap flush was tried and
# does not survive contact with bash: with `set -T`, several DEBUG firings
# land inside the EXIT handler before any disarm can run, each seeing a
# BASH_COMMAND frozen at the interrupted command — the pending command was
# written once per firing (instrumented: one marker, three writes, one pid,
# no subshell). So completion is not observable at exit, and the fix stops
# pretending it is: ARMING writes the pending command to a per-pid sidecar
# (`$CAPTURE_LOG.pending.$$`), a normal flush finalizes it with the real
# exit code and removes it, and a sidecar left by a dead shell is swept into
# the log at the next source (or by the hook) with `"exit":null` — null
# meaning "not recorded", the same contract the corpus already uses. A
# side effect worth having: a shell killed mid-command leaves its command
# on disk instead of in a lost trap.
_CAP_PENDING=""   # path of this shell's sidecar; set when armed

_capture_flush() {
  local rc=$1
  [ -n "${_CAP_CMD:-}" ] || return 0
  local now dur
  now=$(date +%s%3N 2>/dev/null || date +%s000)
  dur=$(( now - ${_CAP_START:-$now} ))
  # The sidecar is ground truth. If it is gone, a sweeper already finalized
  # this command (as exit:null); writing again would duplicate the record.
  if [ -n "$_CAP_PENDING" ] && [ -f "$_CAP_PENDING" ]; then
    printf '%s"exit":%d,"ms":%d,"cmd":%s}\n' \
      "$(sed -n 1p "$_CAP_PENDING")" "$rc" "$dur" "$(sed -n 2p "$_CAP_PENDING")" >> "$CAPTURE_LOG"
    rm -f "$_CAP_PENDING"
  fi
  _CAP_CMD=""; _CAP_PENDING=""
}

_capture_pre() {
  local rc=$?
  _capture_flush "$rc"
  case "$BASH_COMMAND" in
    # The script's own machinery, including the `return 0` that ends the
    # sourcing — it was appearing as the first "command" of every session.
    # Filtered at ARM time so a machinery line can never sit in a sidecar
    # and be swept into the log as a dead shell's last command.
    _capture_*|trap*|*CAPTURE_LOG*|"return 0 2> /dev/null"|return*|set\ -T|source*|.\ */session_capture.sh*)
      return 0 ;;
  esac
  _CAP_START=$(date +%s%3N 2>/dev/null || date +%s000)
  _CAP_CMD="$BASH_COMMAND"
  _CAP_PENDING="$CAPTURE_LOG.pending.$$"
  # Line 1: the record up to the fields only completion knows. Line 2: the
  # already-redacted, already-escaped command, so the sweeper needs no logic.
  {
    printf '{"ts":"%s","layer":"shell","cwd":"%s",\n' \
      "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$PWD"
    printf '%s' "$_CAP_CMD" | _redact | _json_escape
    printf '\n'
  } > "$_CAP_PENDING"
}

# Sweep sidecars whose shells are gone: their final commands, finalized with
# exit:null ("not recorded" — the status died with the shell). Live pids are
# left alone; their own flush will finalize with the real code.
_capture_sweep() {
  local p pid cmdjson
  for p in "$CAPTURE_LOG".pending.*; do
    [ -f "$p" ] || continue
    pid="${p##*.}"
    case "$pid" in *[!0-9]*|'') continue ;; esac
    kill -0 "$pid" 2>/dev/null && continue
    cmdjson=$(sed -n 2p "$p")
    # Ghost filter. When the dying shell had its own EXIT trap, the DEBUG
    # firing inside it finalized the real record (with the real exit code)
    # and then re-armed a sidecar from a frozen BASH_COMMAND — a ghost of
    # the command already on the log's last line. Deliberate trade: this
    # also drops a dying command byte-identical to the immediately
    # preceding record, which loses almost nothing, while keeping ghosts
    # would add a spurious record per exit.
    if [ -f "$CAPTURE_LOG" ] && tail -n 1 "$CAPTURE_LOG" | grep -qF "\"cmd\":$cmdjson}"; then
      rm -f "$p"; continue
    fi
    printf '%s"exit":null,"ms":null,"cmd":%s}\n' \
      "$(sed -n 1p "$p")" "$cmdjson" >> "$CAPTURE_LOG"
    rm -f "$p"
  done
}

if [ "${BASH_SOURCE[0]}" != "${0}" ]; then
  # Sourced: sweep what previous shells left, then arm. set -T so the DEBUG
  # trap survives into functions. Armed LAST so the lines above it are never
  # themselves recorded.
  _capture_sweep
  set -T
  trap '_capture_pre' DEBUG
  export CAPTURE_LOG
  echo "capturing to $CAPTURE_LOG"
  return 0 2>/dev/null || true
fi

# ── subcommands (executed) ───────────────────────────────────────────────────
case "${1:-}" in
  --install)
    root="$(git rev-parse --show-toplevel)" || exit 1
    hooks="$root/.githooks"; mkdir -p "$hooks"
    cat > "$hooks/post-commit" <<'HOOK'
#!/bin/sh
# Attach the commands run since the previous commit to this one, as a note.
# Silent and non-fatal by design: a capture failure must never break a commit.
# Compute the same (user, repo)-keyed default as session_capture.sh — keep
# the two in lockstep. A commit made from a shell that never sourced the
# recorder (an agent invocation that only commits) still finds the shared log
# this way. The -f guard below keeps "capture never used" silent as before.
if [ -z "${CAPTURE_LOG:-}" ]; then
  root=$(git rev-parse --show-toplevel 2>/dev/null) || root=$PWD
  CAPTURE_LOG="${TMPDIR:-/tmp}/session-capture-$(id -u)-$(printf '%s' "$root" | cksum | cut -d' ' -f1).jsonl"
fi
# Sweep sidecars left by dead shells (their unflushed final commands) into
# the log before slicing, finalized with exit:null. Keep in lockstep with
# _capture_sweep in session_capture.sh. The committing shell's own sidecar
# (this very `git commit`) has a live pid and is left for its own flush.
for p in "$CAPTURE_LOG".pending.*; do
  [ -f "$p" ] || continue
  pid="${p##*.}"
  case "$pid" in *[!0-9]*|'') continue ;; esac
  kill -0 "$pid" 2>/dev/null && continue
  cmdjson=$(sed -n 2p "$p")
  # Ghost filter — see _capture_sweep in session_capture.sh.
  if [ -f "$CAPTURE_LOG" ] && tail -n 1 "$CAPTURE_LOG" | grep -qF "\"cmd\":$cmdjson}"; then
    rm -f "$p"; continue
  fi
  printf '%s"exit":null,"ms":null,"cmd":%s}\n' \
    "$(sed -n 1p "$p")" "$cmdjson" >> "$CAPTURE_LOG"
  rm -f "$p"
done
[ -f "$CAPTURE_LOG" ] || exit 0
# The mark records BOTH the log it refers to and the position in it. Storing
# only the position was wrong: each session opens a new log starting at line
# 1, so a stale mark from a previous session pointed past the end and the
# commit silently got no note. Same log -> resume; different log -> start over.
mark="$(git rev-parse --git-dir)/capture-mark"
start=1
if [ -f "$mark" ]; then
  prev_log=$(sed -n 1p "$mark")
  [ "$prev_log" = "$CAPTURE_LOG" ] && start=$(sed -n 2p "$mark")
fi
total=$(wc -l < "$CAPTURE_LOG")
[ "$total" -ge "$start" ] || exit 0
sed -n "${start},\$p" "$CAPTURE_LOG" > "$(git rev-parse --git-dir)/capture-slice"
git notes --ref=refs/notes/toolcalls add -F "$(git rev-parse --git-dir)/capture-slice" -f HEAD 2>/dev/null
printf '%s\n%s\n' "$CAPTURE_LOG" "$((total + 1))" > "$mark"
exit 0
HOOK
    chmod +x "$hooks/post-commit"
    git config core.hooksPath .githooks
    echo "installed $hooks/post-commit and set core.hooksPath"
    echo "now: source maintenance/tools/session_capture.sh"
    ;;
  --show)
    git notes --ref="$NOTES_REF" show "${2:-HEAD}" 2>/dev/null \
      || echo "no captured commands for ${2:-HEAD}"
    ;;
  --selftest)
    # Simulate the environment that broke the first version: an agent harness
    # running EVERY command in a fresh shell, none exporting CAPTURE_LOG, and
    # each shell's useful command being its FINAL command. Against the old
    # $$-default this recorded nothing (three markers lost, one machinery
    # line attached); the current code must attach all of them.
    self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
    S=$(mktemp -d) || exit 1
    export TMPDIR="$S/tmp"; mkdir -p "$TMPDIR"
    R="$S/repo"; mkdir -p "$R"
    git -C "$R" init -q
    git -C "$R" config user.name selftest
    git -C "$R" config user.email selftest@localhost
    cp "$self" "$R/sc.sh"
    ( cd "$R" && ./sc.sh --install >/dev/null )
    fail=0
    say() { echo "selftest: $*"; }

    # 1+2: three fresh shells record; a fourth commits. All markers must land.
    for i in 1 2 3; do
      env -u CAPTURE_LOG bash -c "cd '$R' && . ./sc.sh >/dev/null 2>&1 && echo marker_$i > f$i"
    done
    env -u CAPTURE_LOG bash -c "cd '$R' && . ./sc.sh >/dev/null 2>&1 && git add -A && git commit -qm one"
    note=$(git -C "$R" notes --ref=toolcalls show HEAD 2>/dev/null)
    [ -n "$note" ] || { say "FAIL: first commit carries no note at all"; fail=1; }
    for i in 1 2 3; do
      echo "$note" | grep -q "marker_$i" || { say "FAIL: marker_$i missing (fresh-shell loss)"; fail=1; }
    done
    # A range, because both directions are failures: too few lines is loss,
    # too many is the EXIT flush re-firing DEBUG and writing machinery or
    # duplicates. Expected content is 3 markers + shell 4's `git add`.
    n=$(echo "$note" | grep -c .)
    { [ "$n" -ge 4 ] && [ "$n" -le 6 ]; } \
      || { say "FAIL: note is $n lines, expected 4..6 (leak or loss)"; fail=1; }
    echo "$note" | grep -qE 'sc\.sh|_capture|local rc|trap -- ' \
      && { say "FAIL: machinery line leaked into note"; fail=1; }
    dup=$(echo "$note" | grep -c "marker_1")
    [ "$dup" = 1 ] || { say "FAIL: marker_1 written $dup times (subshell re-flush)"; fail=1; }

    # 3: a shell that records but a LATER shell that commits WITHOUT sourcing —
    # the hook's own default must find the shared log.
    env -u CAPTURE_LOG bash -c "cd '$R' && . ./sc.sh >/dev/null 2>&1 && echo marker_4 > f4"
    env -u CAPTURE_LOG bash -c "cd '$R' && git add -A && git commit -qm two"
    note=$(git -C "$R" notes --ref=toolcalls show HEAD 2>/dev/null)
    echo "$note" | grep -q "marker_4" || { say "FAIL: unsourced-commit hook fallback missed marker_4"; fail=1; }

    # 4: an explicit CAPTURE_LOG must still win over the computed default.
    env CAPTURE_LOG="$S/custom.jsonl" bash -c "cd '$R' && . ./sc.sh >/dev/null 2>&1 && echo marker_5 > f5"
    env CAPTURE_LOG="$S/custom.jsonl" bash -c "cd '$R' && . ./sc.sh >/dev/null 2>&1 && true"
    grep -q "marker_5" "$S/custom.jsonl" 2>/dev/null \
      || { say "FAIL: explicit CAPTURE_LOG override not honoured"; fail=1; }

    # 5: a swept final command records existence but not status: exit must be
    # null ("not recorded"), never a fabricated code.
    echo "$note" | grep "marker_4" | grep -q '"exit":null' \
      || { say "FAIL: swept final command should carry exit:null"; fail=1; }

    # 6: a shell with its OWN EXIT trap: the DEBUG firing inside it finalizes
    # the real record with a real exit code; the frozen-BASH_COMMAND ghost it
    # re-arms must be deduped by the sweep, and the user's trap must still run.
    env -u CAPTURE_LOG bash -c "cd '$R' && trap 'echo t > trapran' EXIT && . ./sc.sh >/dev/null 2>&1 && echo marker_6 > f6"
    env -u CAPTURE_LOG bash -c "cd '$R' && . ./sc.sh >/dev/null 2>&1 && true"
    log="$TMPDIR/$(ls "$TMPDIR" | grep 'session-capture.*\.jsonl$' | head -1)"
    m6=$(grep -c "marker_6" "$log" 2>/dev/null)
    [ "$m6" = 1 ] || { say "FAIL: marker_6 in log $m6 times, want 1 (ghost not deduped)"; fail=1; }
    grep "marker_6" "$log" 2>/dev/null | grep -q '"exit":0' \
      || { say "FAIL: marker_6 should carry its real exit code from the trap-context flush"; fail=1; }
    [ -f "$R/trapran" ] || { say "FAIL: user's own EXIT trap did not run"; fail=1; }

    rm -rf "$S"
    if [ "$fail" = 0 ]; then say "PASS (fresh-shell notes, hook fallback, override)"; exit 0
    else exit 1; fi
    ;;
  *)
    echo "usage: source $0            # record"
    echo "       $0 --install         # add the post-commit hook"
    echo "       $0 --show [rev]      # read a commit's captured commands"
    echo "       $0 --selftest        # prove capture survives fresh-shell harnesses"
    ;;
esac
