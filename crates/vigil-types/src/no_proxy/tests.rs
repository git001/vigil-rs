// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use super::{no_proxy_matches, parse_no_proxy};

fn entries(s: &str) -> Vec<String> {
    parse_no_proxy(Some(s))
}

// --- hostname matching ---

#[test]
fn exact_match() {
    assert!(no_proxy_matches("local.com", &entries("local.com")));
}

#[test]
fn port_stripped() {
    assert!(no_proxy_matches("local.com:80", &entries("local.com")));
    assert!(no_proxy_matches("local.com:8443", &entries("local.com")));
}

#[test]
fn subdomain() {
    assert!(no_proxy_matches("www.local.com", &entries("local.com")));
    assert!(no_proxy_matches("a.b.local.com", &entries("local.com")));
}

#[test]
fn no_false_suffix() {
    assert!(!no_proxy_matches("www.notlocal.com", &entries("local.com")));
    assert!(!no_proxy_matches("notlocal.com", &entries("local.com")));
}

#[test]
fn leading_dot_entry() {
    assert!(no_proxy_matches("local.com", &entries(".local.com")));
    assert!(no_proxy_matches("www.local.com", &entries(".local.com")));
    assert!(!no_proxy_matches("www.notlocal.com", &entries(".local.com")));
}

#[test]
fn multiple_entries() {
    let e = entries("internal.corp, .dev.local, 127.0.0.1");
    assert!(no_proxy_matches("internal.corp", &e));
    assert!(no_proxy_matches("api.dev.local", &e));
    assert!(no_proxy_matches("127.0.0.1", &e));
    assert!(!no_proxy_matches("external.corp", &e));
}

#[test]
fn empty_list() {
    assert!(!no_proxy_matches("anything.com", &[]));
}

#[test]
fn case_insensitive() {
    assert!(no_proxy_matches("LOCAL.COM", &entries("local.com")));
    assert!(no_proxy_matches("WWW.Local.Com", &entries("local.com")));
}

// --- IPv4 CIDR ---

#[test]
fn ipv4_cidr_match() {
    assert!(no_proxy_matches("10.0.1.5", &entries("10.0.0.0/8")));
    assert!(no_proxy_matches("192.168.1.100", &entries("192.168.0.0/16")));
    assert!(no_proxy_matches("169.254.1.2", &entries("169.254.0.0/16")));
}

#[test]
fn ipv4_cidr_no_match() {
    assert!(!no_proxy_matches("10.0.1.5", &entries("192.168.0.0/16")));
    assert!(!no_proxy_matches("172.16.0.1", &entries("10.0.0.0/8")));
}

#[test]
fn ipv4_cidr_host_route() {
    assert!(no_proxy_matches("10.0.0.1", &entries("10.0.0.1/32")));
    assert!(!no_proxy_matches("10.0.0.2", &entries("10.0.0.1/32")));
}

#[test]
fn ipv4_cidr_with_port() {
    assert!(no_proxy_matches("10.0.1.5:8080", &entries("10.0.0.0/8")));
}

#[test]
fn ipv4_cidr_default_route() {
    assert!(no_proxy_matches("1.2.3.4", &entries("0.0.0.0/0")));
}

// --- IPv6 CIDR ---

#[test]
fn ipv6_cidr_match() {
    assert!(no_proxy_matches("2001:db8::1", &entries("2001:db8::/32")));
    assert!(no_proxy_matches("fd00::1", &entries("fd00::/8")));
}

#[test]
fn ipv6_cidr_no_match() {
    assert!(!no_proxy_matches("2001:db9::1", &entries("2001:db8::/32")));
}

#[test]
fn ipv6_with_port() {
    assert!(no_proxy_matches("[2001:db8::1]:443", &entries("2001:db8::/32")));
}

// --- mixed entries ---

#[test]
fn mixed_cidr_and_hostname() {
    let e = entries("localhost,192.168.0.0/16,internal.corp");
    assert!(no_proxy_matches("localhost", &e));
    assert!(no_proxy_matches("192.168.5.5", &e));
    assert!(no_proxy_matches("api.internal.corp", &e));
    assert!(!no_proxy_matches("8.8.8.8", &e));
    assert!(!no_proxy_matches("external.io", &e));
}
