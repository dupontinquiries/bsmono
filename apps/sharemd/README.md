# sharemd

The cross-app data-share daemon: binds the `"share"` Unix socket and serves
`datashare-core`'s namespaced key/value + pub-sub Hub to every app. Apps try
this socket first (`ipc-kit`) and fall back to private state if it's down.

## Verification (2026-08-19)

Had no build-status record anywhere. Ran it for the first time: `cargo
test` is 1/1 — two clients sharing data see each other's pub-sub updates.
Compiles clean, no display or heavy dependency tree needed.
