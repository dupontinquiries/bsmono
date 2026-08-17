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

CAPTURE_LOG="${CAPTURE_LOG:-${TMPDIR:-/tmp}/session-capture-$$.jsonl}"
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
_capture_flush() {
  local rc=$1
  [ -n "${_CAP_CMD:-}" ] || return 0
  case "$_CAP_CMD" in
    # The script's own machinery, including the `return 0` that ends the
    # sourcing — it was appearing as the first "command" of every session.
    _capture_*|trap*|*CAPTURE_LOG*|"return 0 2> /dev/null"|return*|set\ -T|source*|.\ */session_capture.sh*)
      _CAP_CMD=""; return 0 ;;
  esac
  local now dur cmd
  now=$(date +%s%3N 2>/dev/null || date +%s000)
  dur=$(( now - ${_CAP_START:-$now} ))
  cmd=$(printf '%s' "$_CAP_CMD" | _redact | _json_escape)
  printf '{"ts":"%s","layer":"shell","cwd":"%s","exit":%d,"ms":%d,"cmd":%s}\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$PWD" "$rc" "$dur" "$cmd" >> "$CAPTURE_LOG"
  _CAP_CMD=""
}

_capture_pre() {
  local rc=$?
  _capture_flush "$rc"
  _CAP_START=$(date +%s%3N 2>/dev/null || date +%s000)
  _CAP_CMD="$BASH_COMMAND"
}

if [ "${BASH_SOURCE[0]}" != "${0}" ]; then
  # Sourced: arm the trap and leave. set -T so it survives into functions.
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
[ -n "${CAPTURE_LOG:-}" ] || exit 0
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
  *)
    echo "usage: source $0            # record"
    echo "       $0 --install         # add the post-commit hook"
    echo "       $0 --show [rev]      # read a commit's captured commands"
    ;;
esac
