# Providers

A provider is any executable that reads `<system>\0<user>` on stdin and
writes a plan to stdout. Exit 0 with the plan; exit non-zero and your
stderr becomes the message the player sees.

This is aperture's `CommandProvider` contract
(`apps/aperture/crates/aperture-complete/src/providers.rs`), used for
the same reason: the game does not grow an HTTP client, a TLS stack, or
an opinion about which server you run.

`VOXEL_MODEL` is set to the path of the model chosen with `/model`.

- `fixture.sh` — returns a canned plan. No model needed; use it to
  prove the whole loop works before touching weights.
- `ollama.sh` — talks to a local ollama over curl.
- `llama-cli.sh` — runs llama.cpp's CLI directly on `$VOXEL_MODEL`.

Point the game at one:

    /provider providers/ollama.sh
