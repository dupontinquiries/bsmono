//! gdb over MI3. `translate` is the protocol half (pure, replayable from
//! transcripts); `process` is the transport half (child process, pty, threads).

pub mod process;
pub mod translate;

pub use process::{Debuginfod, GdbBackend, GdbConfig};
