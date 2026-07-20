# bsgames integration

Skydribble already speaks the games centre's whole protocol:

- **Handoff env** — `BSGAMES_HANDOFF_ROLE` (`host`/`join`),
  `BSGAMES_HANDOFF_ME` (`<id>|<name>`), `BSGAMES_HANDOFF_PEER` (join only)
  are read at boot by `src/net.rs::from_handoff`, which hosts or joins the
  named friend automatically with your centre display name.
- **`--quick-match`** — the centre's quick-match flag skips the splash and
  drops straight into the sandbox (and combines with the handoff env).

The one change needed on the bsgames side is a roster entry, shipped here as
`bsgames-skydribble.patch`. From the monorepo root:

    git apply games/skydribble/integration/bsgames-skydribble.patch

(or `patch -p1 < ...`), after placing this repo at `games/skydribble/`. The
entry uses `LaunchKind::Cargo`, so the centre launches it exactly like
clawbound/soldier/unendlich: `cargo run --release --manifest-path
games/skydribble/Cargo.toml`, appending `--quick-match` for quick-match.
