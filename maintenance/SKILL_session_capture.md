# Skill: shell-layer capture

**Trigger this whenever you are about to do work in this repo that ends in a
commit.** One command at the start of a session; nothing to remember after.

```sh
source maintenance/tools/session_capture.sh
```

That is the whole obligation. If `--install` has never been run on this clone,
run it once first:

```sh
./maintenance/tools/session_capture.sh --install
```

## What this is for

Every commit in this repo can carry the commands that produced it, attached as
a git note under `refs/notes/toolcalls`. `maintenance/tools/commit_corpus.py`
reads them into each record's `observed_calls`.

The reason it matters is narrow and worth stating, because the obvious
alternative looks equally good and is not. **Tool calls cannot be recovered
after the fact.** A trace generated later from a finished diff walks straight
to the answer: the greps that returned nothing, the build that failed, the
assumption held for twenty minutes are all absent, and any wrong turn staged
into it is one whose resolution was already known. It reads exactly like a
recording and it is fiction. See `maintenance/tools/rollout.py` for the only
defensible way to generate traces — predict forward from repo state, then score
against what the commit really did.

So capture is cheap and reconstruction is impossible. That asymmetry is the
whole argument.

The exit codes are the valuable part. A recorded `exit: 127` or a failing test
run is a genuine failure at the moment it happened — the signal that no
after-the-fact process can produce.

## What it records, and what it cannot

Records the **shell**: command, exit code, duration, working directory. That is
most of the observable work here — builds, tests, greps, git.

It cannot see an agent's internal tool calls, which happen above the shell and
never reach a terminal. Every record is stamped `layer: "shell"` so a harness
that logs its own calls can add `layer: "agent"` against the same commit and
the two compose. Nothing pretends to be the whole trace.

A commit made without capture running keeps `observed_calls: null`, and null
means **not recorded** — never "no commands were run". Do not fill it in later.

## Three ways this silently does nothing

Each of these was hit while building it, and none surfaces as an error.

**Not sourced.** The hook alone captures nothing; it only attaches what the
shell recorded. `--install` sets up the hook, `source` starts the recording, and
the two are separate steps. A session that only ran `--install` produces empty
notes.

**A new shell.** The trap lives in one shell process. A subshell, a new
terminal, or a `bash -c` that does not source the script records nothing for
its own commands. When an agent spawns commands in fresh shells, source it
inside the same invocation that commits.

**Notes do not travel.** They are not fetched or pushed by default:

```sh
git push  origin refs/notes/toolcalls
git fetch origin refs/notes/toolcalls:refs/notes/toolcalls
```

A clone without that refspec shows every `observed_calls` as null and looks
exactly like a repo where capture was never enabled.

## Secrets

Redaction runs **before** anything is written — passwords, tokens, bearer
headers, credentials in URLs, and long digit runs. This matters here
specifically: the fleet workflow types a device password, and a log is a
keylogger pointed at your own shell.

Scrubbing after the fact is not equivalent. A secret that reached the file is
on disk, and once it is in a note it is in the object store. If you add a
command shape that carries a credential, extend `_redact` in
`session_capture.sh` in the same commit that introduces it. Patterns are
deliberately broad; an over-redacted log is the correct direction to fail.

## Checking it works

```sh
./maintenance/tools/session_capture.sh --show HEAD
```

Prints the commands attached to a commit, or says there are none. If a commit
you just made shows none, you hit one of the three cases above — most likely
the second.
