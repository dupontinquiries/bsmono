//! shellkit — game-shell modules shared between skydribble and blackstar-rs.
//!
//! Both games carried byte-identical copies of every module here (confirmed by
//! content hash before this crate existed) with no `crate::` dependency on
//! anything else in either game's own source — platform detection, touch/
//! gyro/analog/gamepad input handling, net/stream transport helpers built on
//! the shared `netcode`/`touchkit` crates, and academy/trials/replay
//! progression tracking. Pulled out once rather than kept in sync by hand
//! twice; see maintenance/C13_TODO.md for how this was found.
//!
//! `fx.rs` was NOT moved here even though it was also identical at the time:
//! it depends on each game's own `crate::render`/`crate::world` types, so
//! moving it would mean moving those too, or genericizing over them — a
//! bigger, riskier change than this crate's other modules needed.
//!
//! `gyro.rs` and `platform.rs` were also identical between the two games
//! but each references `crate::config::{GyroCfg, Config}` — each game's OWN
//! config struct, not a shared type — so they stayed put too, same reasoning
//! as `fx.rs`.

pub mod academy;
pub mod analog;
pub mod custom;
pub mod gamepad;
pub mod net;
pub mod partdata;
pub mod replay;
pub mod stream;
pub mod style;
pub mod touch;
pub mod trials;
