//! board-gestures — rusty-board's swipe-typing core, lifted out of the GTK app so BOTH
//! frontends run the same recognizer.
//!
//! This is the same move `board-layouts` already made for the layout schema, for the same
//! reason: the GTK `src/ui` tree and the egui frontend were about to grow two swipe
//! implementations that would drift apart, and a swipe decoder is exactly the kind of
//! heavily-tuned code (velocity smoothing windows, pivot separation floors, turn-angle
//! thresholds) where a second implementation is guaranteed to be a worse one.
//!
//! The modules here are the GTK app's `gestures::swipe` and `gestures::recognizer` moved
//! VERBATIM — same thresholds, same comments, same tests — with exactly one change: the
//! hitbox used to hold the GTK crate's `keyboard::Key`, but the algorithm only ever reads
//! `label` and `swipe_target` off it, so [`swipe::KeyHitbox`] now carries those two fields
//! directly. That is the whole decoupling; nothing about the recognition changed, and the
//! ported tests are the evidence.
//!
//! The GTK app has NOT yet been switched over to this crate — it cannot be built or tested
//! in the porting environment (no GTK4/libadwaita), and swapping a hot path blind is how you
//! ship a regression. The adoption there is mechanical and small: add this crate as a
//! dependency, delete `src/gestures/{swipe,recognizer}.rs`, and re-export
//! (`pub use board_gestures::{swipe, recognizer};`), then fix the ~4 `KeyHitbox { key: .. }`
//! construction sites to pass `label`/`swipe_target`. Until then this crate is the egui
//! frontend's recognizer and the reference copy.

pub mod recognizer;
pub mod swipe;

pub use recognizer::{GestureRecognizer, WordCandidate};
pub use swipe::{decode_swipe_path, KeyHitbox, SwipePoint, SwipeTracker};
