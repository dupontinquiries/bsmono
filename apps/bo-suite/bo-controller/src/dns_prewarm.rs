// src/dns_prewarm.rs
#![allow(dead_code)]
//
// DNS pre-resolution.
//
// Most of our device URLs are IP-based (e.g. http://192.168.2.12), so
// DNS isn't a hot spot. But mDNS-discovered devices appear as
// `<name>.local` hostnames, and reqwest does a DNS lookup on every
// connection setup unless the OS cache is warm. The first lookup
// after the cache expires (typically ~5 min on Linux) costs 5-30 ms
// before the request even begins.
//
// This module:
//   1. Walks known device URLs at startup.
//   2. Extracts the hostname from each.
//   3. For hostnames (not literal IPs), kicks off a getaddrinfo()
//      call to populate the OS resolver cache.
//
// We don't replace reqwest's resolver — that would lose mDNS-driven
// updates when a device's IP changes. We just warm the cache so the
// FIRST request after startup pays "cache hit" latency rather than
// "cache miss". Subsequent requests reuse the cached entry until it
// expires.

use std::collections::HashSet;
use std::net::ToSocketAddrs;
use std::time::Duration;

/// Pull the hostname portion out of an http(s) URL. Returns None if
/// the URL is malformed or already an IP literal (no warming needed).
pub fn hostname_from_url(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map(|(_, r)| r)?;
    // Handle bracketed IPv6: http://[::1]/foo
    if after_scheme.starts_with('[') {
        // Bracketed IPv6 — always an IP literal, no DNS needed.
        return None;
    }
    let host_part = after_scheme.split(|c: char| c == ':' || c == '/').next()?;
    if host_part.is_empty() { return None; }
    // Skip IP literals — they don't need DNS.
    if host_part.parse::<std::net::IpAddr>().is_ok() { return None; }
    Some(host_part.to_string())
}

/// Resolve a list of URLs in the background. Returns immediately;
/// resolutions happen on a spawned thread so startup isn't blocked.
/// Each unique hostname gets one getaddrinfo() call.
pub fn warm_urls(urls: &[String]) {
    let hosts: HashSet<String> = urls.iter()
        .filter_map(|u| hostname_from_url(u))
        .collect();
    if hosts.is_empty() { return; }
    std::thread::spawn(move || {
        for host in hosts {
            // ToSocketAddrs on `host:0` triggers a DNS lookup and
            // returns whatever IPs the resolver finds. We don't care
            // about the result; the side effect is what we want
            // (cache population).
            //
            // Note: getaddrinfo blocks. We're on a dedicated thread so
            // that's fine, but we cap the patience: if the resolver
            // wedges, we move on rather than blocking forever. Linux
            // doesn't expose a timeout for getaddrinfo directly, so
            // this is best-effort — typical resolutions are <30ms.
            let _ = (host.as_str(), 0).to_socket_addrs().map(|iter| {
                let count = iter.count();
                eprintln!("dns_prewarm: resolved {} → {} address(es)", host, count);
            });
            // Tiny gap so we don't hammer the resolver with parallel
            // queries (most home routers serialize DNS anyway).
            std::thread::sleep(Duration::from_millis(20));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_from_url_extracts_hostname() {
        assert_eq!(hostname_from_url("http://device.local"), Some("device.local".to_string()));
        assert_eq!(hostname_from_url("http://device.local:8080"), Some("device.local".to_string()));
        assert_eq!(hostname_from_url("http://device.local/path"), Some("device.local".to_string()));
        assert_eq!(hostname_from_url("https://my-stage.lan/api"), Some("my-stage.lan".to_string()));
    }

    #[test]
    fn hostname_from_url_skips_ip_literals() {
        assert_eq!(hostname_from_url("http://192.168.1.10"), None);
        assert_eq!(hostname_from_url("http://10.0.0.1:8080"), None);
        assert_eq!(hostname_from_url("http://[::1]"), None);   // bracketed IPv6
        // The IPv6 bracketed form needs no DNS even though our parser
        // returns "[" for the host_part — neither way costs a DNS hit.
    }

    #[test]
    fn hostname_from_url_handles_malformed() {
        assert_eq!(hostname_from_url(""), None);
        assert_eq!(hostname_from_url("not-a-url"), None);
        assert_eq!(hostname_from_url("http://"), None);
    }

    #[test]
    fn warm_urls_handles_empty_list() {
        warm_urls(&[]);   // shouldn't panic
    }

    #[test]
    fn warm_urls_deduplicates_hostnames() {
        // Same hostname twice = one lookup. We can't assert on the
        // thread's behavior directly without making warm_urls return
        // a handle, but the HashSet path is exercised here.
        let urls = vec![
            "http://device.local/a".to_string(),
            "http://device.local/b".to_string(),
            "http://device.local:8080".to_string(),
        ];
        let hosts: HashSet<String> = urls.iter()
            .filter_map(|u| hostname_from_url(u))
            .collect();
        assert_eq!(hosts.len(), 1);
        assert!(hosts.contains("device.local"));
    }

    #[test]
    fn warm_urls_mixes_ip_and_hostname() {
        let urls = vec![
            "http://192.168.1.10".to_string(),
            "http://stage.local".to_string(),
            "http://10.0.0.1".to_string(),
            "http://emerge.local:8080".to_string(),
        ];
        let hosts: HashSet<String> = urls.iter()
            .filter_map(|u| hostname_from_url(u))
            .collect();
        assert_eq!(hosts.len(), 2);
        assert!(hosts.contains("stage.local"));
        assert!(hosts.contains("emerge.local"));
    }
}
