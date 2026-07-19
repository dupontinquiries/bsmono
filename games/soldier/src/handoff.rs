//! handoff — reads the `BSGAMES_HANDOFF_*` env vars bsgames' "join" button sets (see
//! `bsgames::centre::handoff_env`) so Soldier can skip its own main menu and dial straight into a
//! friend's match. This file was the ORIGINAL parser; it moved verbatim (tests included) to
//! `netcode::handoff` once every fleet game started reading the same protocol — re-exported here
//! so soldier's ~4 call sites keep their `crate::handoff::` paths unchanged.

pub use netcode::handoff::{from_env, Handoff};
