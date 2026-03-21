// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Shared `no_proxy` / `no-proxy` matching logic used by all vigil binaries.
//!
//! Each entry in the comma-separated list is matched against the request host
//! (port stripped before comparison) using these rules, in order:
//!
//! 1. **CIDR** (`192.168.0.0/16`, `2001:db8::/32`) — the host is parsed as an
//!    IP address and tested against the network prefix.
//! 2. **Exact hostname** (`internal.corp`) — case-insensitive equality.
//! 3. **Domain suffix** (`internal.corp` also matches `api.internal.corp`).
//!
//! A leading `.` on an entry is ignored (`.local.com` ≡ `local.com`).

use std::net::IpAddr;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a comma-separated `no_proxy` string into a list of lowercase entries.
pub fn parse_no_proxy(s: Option<&str>) -> Vec<String> {
    match s {
        None => vec![],
        Some(s) => s
            .split(',')
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect(),
    }
}

/// Returns `true` if `host` should bypass the proxy.
///
/// `host` may include a port (`host:8080`); the port is stripped before
/// matching. See module-level docs for matching rules.
pub fn no_proxy_matches(host: &str, entries: &[String]) -> bool {
    if entries.is_empty() {
        return false;
    }

    // Strip port if present (e.g. "host:8080" → "host", "[::1]:443" → "::1").
    let bare = strip_port(host).to_ascii_lowercase();

    // Try to parse as an IP address for CIDR matching.
    let host_ip: Option<IpAddr> = bare.parse().ok();

    entries.iter().any(|entry| {
        let e = entry.strip_prefix('.').unwrap_or(entry.as_str());

        // 1. CIDR match (only when the host is an IP address).
        if let Some(ip) = host_ip {
            if let Some(matched) = cidr_matches(ip, e) {
                return matched;
            }
        }

        // 2. Exact hostname / domain suffix match.
        bare == e
            || (bare.len() > e.len()
                && bare.as_bytes()[bare.len() - e.len() - 1] == b'.'
                && bare.ends_with(e))
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Strip a trailing `:port` from `host`, handling IPv6 bracket notation.
fn strip_port(host: &str) -> &str {
    // IPv6 bracketed: "[::1]:443" → "::1"
    if let Some(rest) = host.strip_prefix('[') {
        if let Some(close) = rest.find(']') {
            return &rest[..close];
        }
    }
    // Bare IPv6 address (multiple colons) — no port to strip.
    if host.chars().filter(|&c| c == ':').count() > 1 {
        return host;
    }
    // Plain host:port — only strip if the part after ':' is all digits.
    host.rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
        .map(|(h, _)| h)
        .unwrap_or(host)
}

/// Test whether `ip` falls inside the CIDR range described by `entry`.
///
/// Returns `Some(true/false)` if `entry` is valid CIDR, `None` if it is not.
fn cidr_matches(ip: IpAddr, entry: &str) -> Option<bool> {
    let (net_str, prefix_len_str) = entry.split_once('/')?;
    let prefix_len: u32 = prefix_len_str.parse().ok()?;
    let net_ip: IpAddr = net_str.parse().ok()?;

    Some(match (ip, net_ip) {
        (IpAddr::V4(host_v4), IpAddr::V4(net_v4)) => {
            if prefix_len > 32 {
                return Some(false);
            }
            if prefix_len == 0 {
                return Some(true); // /0 matches all IPv4
            }
            let shift = 32 - prefix_len;
            (u32::from(host_v4) >> shift) == (u32::from(net_v4) >> shift)
        }
        (IpAddr::V6(host_v6), IpAddr::V6(net_v6)) => {
            if prefix_len > 128 {
                return Some(false);
            }
            if prefix_len == 0 {
                return Some(true); // /0 matches all IPv6
            }
            let host_bits = u128::from(host_v6);
            let net_bits = u128::from(net_v6);
            let shift = 128 - prefix_len;
            (host_bits >> shift) == (net_bits >> shift)
        }
        // Mixed address families never match.
        _ => false,
    })
}

#[cfg(test)]
mod tests;
