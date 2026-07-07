# r63-07 — unendlich-game bin (compile-on-your-end, UNVERIFIED)

The new menu→gameplay binary + the `run_world` seam. NOT wired into the engine
lib/Cargo here: it needs render+window (can't compile in-sandbox) and a `run_world`
extraction from `unendlich_world.rs`'s `fn main`. The worker (honestly) did NOT perform
the ~4k-line transplant — these files are the seam + recipe, authored against the merged
r63-01..06 sibling APIs. Integrate per WALKTHROUGH.md:
- add `src/app/mod.rs` (`#![cfg(feature="window")] pub mod world_session; pub use ...`)
  + `src/app/world_session.rs` (this file), `pub mod app;` in lib.rs;
- add `[[bin]] name="unendlich-game" required-features=["render","window"]`;
- move the `unendlich_world.rs` main body into `run_world(&WorldSpec,&Settings)->ExitReason`
  and make `unendlich-world` a thin caller (keeps it working);
- `unendlich_game.rs` drives game_flow + menu_screen_layout + the catalog/new-world/settings
  models, then calls `run_world`.
Verify: `cargo run --release --features render,window --bin unendlich-game`. My r63 edits to
`unendlich_world.rs` (giant creatures/flock removed, block palette) are preserved — do the
extraction on top of those.
