//! Latest-frame broadcast slots for the composed modes.
//!
//! `run_rear` and `run_front` capture loops push their most-recent frame
//! into these `ArcSwap` slots. Composed-mode loops (`run_sidebyside`,
//! `run_stacked`, `run_pip`) read whatever's currently in the slots and
//! mix them into a composed output.
//!
//! Why ArcSwap, not Mutex<Option<Frame>>:
//!   • Producers never block. Capture loops are tight inner loops at
//!     30fps+; a contended Mutex acquire would introduce timing jitter
//!     into the actual camera output, not just the composed output.
//!   • Consumers also never block. They just snapshot whatever the
//!     current p