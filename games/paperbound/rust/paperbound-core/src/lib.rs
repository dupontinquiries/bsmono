//! paperbound-core — the Foldworks RTS logic, duplicated to Rust.
//!
//! Strategy: port the game bottom-up in verifiable slices, each slice reading
//! the SAME data files the Python game ships and reproducing facts the Python
//! behavioural suites already proved. Slice 1 (this crate today) is the tech
//! & economy layer: the faction book, supply, training gates, research flow,
//! and upgrade application. Rendering, pathing and combat come later, once
//! this foundation is pinned by parity tests.

pub mod data;
pub mod sim;
