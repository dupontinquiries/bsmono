//! Diagnostic result types — matches DiagnosticResult in C++.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus { Pass, Warn, Fail }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name:    String,
    pub status:  CheckStatus,
    pub detail:  String,
    pub hint:    Option<String>,
}

impl CheckResult {
    pub fn pass(name: &str, detail: &str) -> Self {
        Self { name: name.to_owned(), status: CheckStatus::Pass,
               detail: detail.to_owned(), hint: None }
    }
    pub fn warn(name: &str, detail: &str, hint: &str) -> Self {
        Self { name: name.to_owned(), status: CheckStatus::Warn,
               detail: detail.to_owned(), hint: Some(hint.to_owned()) }
    }
    pub fn fail(name: &str, detail: &str, hint: &str) -> Self {
        Self { name: name.to_owned(), status: CheckStatus::Fail,
               detail: detail.to_owned(), hint: Some(hint.to_owned()) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagReport {
    pub results: Vec<CheckResult>,
    pub timestamp_ms: i64,
}

impl DiagReport {
    pub fn passed(&self) -> bool { self.results.iter().all(|r| r.status == CheckStatus::Pass) }
    pub fn any_fail(&self) -> bool { self.results.iter().any(|r| r.status == CheckStatus::Fail) }
}
