//! dialer-core — the headless half of the phone app.
//!
//! Layering (protocol before transport):
//!
//! ```text
//!   UI shell (dialer-egui)
//!      │  Link { tx: Command, rx: Event }        ← link.rs, the seam
//!      ▼
//!   backend thread: mock.rs  OR  mm.rs (feature `mm`)
//! ```
//!
//! The UI never awaits anything: it sends [`link::Command`]s and drains
//! [`link::Event`]s once per frame, feeding them to [`session::Session`],
//! which owns every decision about call state and produces
//! [`history::HistoryEntry`]s when calls end. Both backends speak the same
//! channel pair, so swapping a simulated modem for ModemManager is a
//! constructor choice, not an architecture change.
//!
//! State/direction enums mirror ModemManager's numeric values
//! (`MMCallState`, `MMCallDirection`) so the mm backend is a cast, not a
//! mapping table that can drift.

pub mod history;
pub mod link;
pub mod matrix;
pub mod mock;
pub mod number;
pub mod providers;
pub mod session;
pub mod types;

#[cfg(feature = "mm")]
pub mod mm;

pub use link::{Command, Event, Link};
pub use session::Session;
pub use types::{CallDirection, CallInfo, CallState};
