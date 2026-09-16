//! `util::` — the engine paths the extracted files reach through.
//!
//! `sim::climate` and `sim::terrain_variation` call
//! `crate::util::mathx::{saturate, lerp}` inline. Those two lines were missed
//! when this crate was extracted, because the scan looked for `use crate::`
//! and these are inline paths — the same flaw that later broke the voiceterm
//! port, found there and not retro-applied here.
//!
//! Reproducing the path rather than editing the callers is the point: every
//! file in `sim/` stays byte-identical to `unendlich-engine`, which is what
//! lets the two be diffed mechanically.

pub mod mathx;
