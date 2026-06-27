//! Query AST
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Query {
    Range { path: PathBuf, start: usize, end: usize },
    Bucket { path: PathBuf, bucket: usize, width: usize },
    Symbol { path: Option<PathBuf>, symbol: String },
}
