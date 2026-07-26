//! Pure-logic engine subsystems, ported from C.
//!
//! SCOPE, deliberately narrow. Of the engine's ~30k lines:
//!
//!     game.c            11,932 lines, 745 static globals, 171 SDL calls
//!     exengine (logic)   6,123 lines, no SDL, no GL, no Vulkan
//!     exengine (gfx/os)    628 lines
//!
//! The 6,123 pure-logic lines are portable AND verifiable: they take data in and
//! return data out, so a port can be differentially tested against the original.
//! game.c is not — 745 interdependent globals driving SDL and a Vulkan backend is
//! an architectural rewrite, not a translation, and none of it can be checked
//! without a display.
//!
//! Every module here must pass a differential test against the C before it counts
//! as ported. combat.rs did: 200 random ray casts through a shared xorshift stream,
//! 400 lines of output, byte-identical.

pub mod combat;
