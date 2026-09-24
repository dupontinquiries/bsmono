//! logscope-hive — what a session looks like when more than one box is in it.
//!
//! The rules this crate encodes (see docs/HIVE.md for where each came from):
//!   * a box is an endpoint with roles (worker / verifier / integrator) and measured capability
//!   * tasks form a DAG; independence is declared, not inferred; independent tasks fan out
//!   * every task declares what it forwards; the runtime forwards only that (summary contract)
//!   * premise checks run before a task commits (pre-flight verifier), and their outcome is recorded
//!   * the ledger is append-only and has a slot for absence: looked_for / did_not_find / command / scope
//!   * the frontier model (Claude via Claude Code) is one endpoint kind among others, never special-cased
pub mod endpoint;
pub mod matrix;
pub mod ledger;
pub mod plan;
pub mod dispatch;
pub mod counter;
pub mod directive;
pub mod from_spec;   // a fan WorkerSpec → a hive Task (contract → task, not contract → box)
pub mod status;
