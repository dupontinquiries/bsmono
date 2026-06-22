# Test sessions

Structured manual-test walkthroughs for web2qml. Each session
targets a specific persona or feature area; unlike unit tests
(which run in CI and pin specific behaviors), these are
human-driven and cover the UI / voice surfaces that automated
tests can't exercise well.

## Sessions

- **[power-user/](power-user/)** — exercises the v0.1.349–v0.1.358
  power-user feature set: scrollback search, git status badge,
  smart git command parsing, voice intents, chord overlay,
  token accounting, destructive-op signaling.

## Adding a new session

Conventions:
1. Make a new subfolder named for the persona/focus
   (e.g. `accessibility/`, `newcomer/`, `mobile/`).
2. Inside: a `README.md` with the test plan, optional
   `scripts/` for setup helpers, and `templates/` for
   failure-note templates if useful.
3. The README should have a table at the top mapping each
   feature/ship to its section number so readers can skip to
   what they need.
4. Setup scripts should be idempotent or abort-if-exists —
   never silently corrupt state.

## When to run

- Before a release: run all sessions, file failures.
- After landing big work: run the session that covers the
  affected feature area.
- Onboarding: walk a new contributor through the power-user
  session so they see the toolkit's intended use.
