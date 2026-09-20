//! Headless core. Layering, bottom up:
//!
//!   stoat-mi          MI wire format (pure)
//!   gdb::translate    MI records <-> backend-neutral Request/Event (pure)
//!   gdb::process      child process + pty + threads (the only I/O in the backend)
//!   backend           the seam: `trait Backend` + Request/Event
//!   session           sans-IO state machine over Events
//!   driver            session + backend + vitals glue, for the thread that receives events
//!   diag              structured diagnostics
//!   command           one vocabulary for typed, palette and spoken input
//!   discover/history/build/cache/vitals/procs   independent services a shell composes
//!
//! Shells (CLI, GTK) own the event loop and glue these together; nothing in here
//! spawns a main loop or assumes one.

pub mod backend;
pub mod build;
pub mod cache;
pub mod command;
pub mod diag;
pub mod discover;
pub mod driver;
pub mod gdb;
pub mod history;
pub mod model;
pub mod procs;
pub mod session;
pub mod vitals;
