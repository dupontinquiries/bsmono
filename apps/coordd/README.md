# coordd

The coordination daemon: binds the `"coord"` Unix socket and serves
`coord-core`'s Arbiter to every app. Apps claim/release shared resources
(mic, call audio, attention); coordd recomputes and PUSHES each app's
Directive (Own/Yield/Duck/Share) whenever it changes — so a snapclone call
mutes when voiceterm ducks, and resumes when it releases, with no app
knowing about the others.

## Verification (2026-08-19)

Had no build-status record anywhere. Ran it for the first time: `cargo
test` is 1/1 — a call, a duck, and a resume correctly push the right
directive sequence to affected apps. Compiles clean, no display or heavy
dependency tree needed.
