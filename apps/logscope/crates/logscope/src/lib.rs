// lib.rs — exports for logscope-filter binary and any future external crates.
// The GUI binary (main.rs) declares its own mod tree independently.
// This lib only needs to expose the headless-safe modules.

pub mod analyzer;
pub mod charts;
pub mod pipeline;
pub mod probe;
pub mod rotation;
pub mod silence;
pub mod shorten;
#[cfg(feature = "convo")]
pub mod conversation_db;
pub mod model_selector;
pub mod providers;
pub mod llm_backend;
#[cfg(feature = "bloc")]
pub mod bloc_engine;
#[allow(dead_code)] pub mod specdecode;
#[allow(dead_code)] pub mod multimodal;
pub mod markdown_core;
#[allow(dead_code)] pub mod batching;
#[allow(dead_code)] pub mod kvcache;
#[allow(dead_code)] pub mod deploy;
#[allow(dead_code)] pub mod memplan;
#[allow(dead_code)] pub mod arch;
#[allow(dead_code)] pub mod gemmplan;
#[allow(dead_code)] pub mod dedup;
#[allow(dead_code)] pub mod moe;
#[allow(dead_code)] pub mod mla;
#[allow(dead_code)] pub mod rope;
#[allow(dead_code)] pub mod quantfmt;
