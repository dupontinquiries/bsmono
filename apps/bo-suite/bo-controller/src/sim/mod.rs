//! Controller state-machine simulator.
//!
//! What this is for
//! ----------------
//!
//! The real `QtState` in main.rs has ~80 properties, a fan-out to a
//! background worker, and depends on a Qt event loop. That's
//! impossible to unit-test against scenarios like:
//!
//!   "Two devices on the network (Stage @ .12 + Emerge @ .14). Three
//!    BT speakers paired (two B&O Emerges + one no-name unit that
//!    sometimes fails to connect). User does:
//!      → Select Stage
//!      → Bind multi-room of {Emerge01, Emerge02}
//!      → Play track A (opus)
//!      → Switch to Emerge device
//!      → Pick the no-name speaker (fails)
//!      → Play track B (flac)
//!      → Disconnect Emerge01's BT mid-playback
//!      → Switch back to Stage
//!    Did any of these steps leave the controller in an incoherent
//!    state? Did volume routing go to the wrong sink? Did the BT
//!    respect ledger end up willing to disconnect speakers we didn't
//!    cause to connect?"
//!
//! This simulator lets us answer that. It models the controller's
//! pure decision logic — routing, driver-kind transitions, BT-respect
//! bookkeeping, sink-binding persistence — without Qt or async, then
//! exposes an `apply` function that takes a state + action + environment
//! and produces a new state. Pure functions all the way down.
//!
//! Then `fuzz.rs` drives randomized action sequences against the model
//! and checks `invariants.rs` after every step. When an invariant
//! fails the test prints the action trace deterministically (PRNG
//! seeded), so any failure can be reproduced for debugging.
//!
//! The module's public API is consumed only by `cargo test` for now —
//! a future refactor can pull the routing decisions out of main.rs
//! and reuse `apply_action` directly from the production path. Until
//! that lands, the symbols here look "dead" to the production binary,
//! so we silence dead_code at the module boundary rather than
//! sprinkling allows everywhere.

#![allow(dead_code)]

pub mod state;
pub mod env;
pub mod action;
pub mod transitions;
pub mod invariants;
pub mod fuzz;
pub mod scenarios;
