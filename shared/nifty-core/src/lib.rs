//! nifty-core — voiceterm's pure diagram/graph engines, shared by the Qt app and the egui
//! front-end (no UI in here). `niftyplot` parses a diagram source into a `GraphDoc`;
//! `graph_layout` positions nodes force-directed; `mermaid` parses mermaid flowcharts.
pub mod graph_layout;
pub mod mermaid;
pub mod niftyplot;
