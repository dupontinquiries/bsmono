//! Gesture recognition: swipe typing, spacebar cursor, shift swipe, etc.

pub mod adaptive_dictionary;
pub mod ai_swipe;
pub mod boundary;
pub mod cardinal;
pub mod hitbox_nudger;
pub mod llm_swipe;
pub mod ngram;
pub mod recognizer;
pub mod swipe;

pub use recognizer::GestureRecognizer;
pub use swipe::{SwipePoint, SwipeTracker};
