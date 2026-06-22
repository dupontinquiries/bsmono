//! Run all checks in parallel and collect results.

use crate::{checks, result::{DiagReport, CheckResult}};
use chrono::Utc;
use tracing::info;

/// Run all network diagnostics and return a complete report.
/// Checks run concurrently — total time ≈ slowest single check (≤5s).
pub async fn run_all(ws_port: u16, gossip_port: u16) -> DiagReport {
    info!("Running network diagnostics...");

    // Run all checks concurrently
    let (multicast, tcp, public_ip, stun) = tokio::join!(
        checks::check_udp_multicast(gossip_port),
        checks::check_tcp_listen(ws_port),
        checks::check_public_ip(),
        checks::check_stun_udp(),
    );

    let results: Vec<CheckResult> = vec![multicast, tcp, public_ip, stun];

    let pass_count = results.iter().filter(|r| r.status == crate::result::CheckStatus::Pass).count();
    info!("Diagnostics complete: {}/{} checks passed", pass_count, results.len());

    DiagReport { results, timestamp_ms: Utc::now().timestamp_millis() }
}
