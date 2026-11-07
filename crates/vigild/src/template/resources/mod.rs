// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `mem(expr)` and `cpu([expr])`: size arguments from the container's limits.
//!
//! - Memory: the smallest cgroup limit (v2 `memory.max` of the own cgroup and
//!   its ancestors, else v1 `memory.limit_in_bytes`), capped by `MemTotal`.
//!   In Kubernetes/OpenShift this is the container's `limits.memory`.
//! - CPUs: the CPUs the process may run on (`sched_getaffinity`), capped by
//!   the cgroup CPU quota rounded up (v2 `cpu.max`, else v1
//!   `cpu.cfs_quota_us`). In Kubernetes this is `limits.cpu`.
//!
//! Without a limit the whole node is used and a warning is logged once.

use std::path::Path;

use minijinja::{Error, ErrorKind};

mod cgroup;
use cgroup::{affinity_cpus, detect_cpus, detect_memory};

const MIB: u64 = 1024 * 1024;

fn fail(msg: String) -> Error {
    Error::new(ErrorKind::InvalidOperation, msg)
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// `N%`, `N% - SIZE` or `SIZE` (memory); the subtrahend has a unit for
/// memory and is a plain count for CPUs.
#[derive(Debug, PartialEq)]
enum Expr {
    Percent { pct: f64, minus: f64 },
    Fixed(f64),
}

fn parse_expr(s: &str, parse_amount: impl Fn(&str) -> Option<f64>) -> Option<Expr> {
    let s = s.trim();
    let (head, minus) = match s.split_once('-') {
        Some((h, m)) => (h.trim(), Some(parse_amount(m.trim())?)),
        None => (s, None),
    };
    if let Some(p) = head.strip_suffix('%') {
        let pct: f64 = p.trim().parse().ok()?;
        (pct > 0.0 && pct.is_finite()).then_some(Expr::Percent {
            pct,
            minus: minus.unwrap_or(0.0),
        })
    } else if minus.is_none() {
        parse_amount(head).map(Expr::Fixed)
    } else {
        None // `SIZE - SIZE` is not supported
    }
}

/// `64MiB`, `1GiB`, `500MB`, `1024KiB`, `100B` → MiB.
fn parse_size_mib(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let bytes_per_unit = match unit.trim() {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => MIB as f64,
        "GiB" => (1024 * MIB) as f64,
        "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        _ => return None,
    };
    (n >= 0.0).then_some(n * bytes_per_unit / MIB as f64)
}

fn parse_count(s: &str) -> Option<f64> {
    s.trim().parse::<u32>().ok().map(f64::from)
}

/// Memory budget in MiB for `expr`, given the detected memory in bytes.
fn eval_mem(expr: &str, total_bytes: u64) -> Result<u64, String> {
    let e = parse_expr(expr, parse_size_mib).ok_or_else(|| {
        format!("mem({expr:?}): expected e.g. \"75%\", \"90% - 64MiB\" or \"512MiB\"")
    })?;
    let mib = match e {
        Expr::Fixed(m) => m,
        Expr::Percent { pct, minus } => (total_bytes as f64 / MIB as f64) * pct / 100.0 - minus,
    };
    let mib = mib.floor();
    if mib < 1.0 {
        return Err(format!(
            "mem({expr:?}): result is {mib} MiB with {} MiB detected",
            total_bytes / MIB
        ));
    }
    Ok(mib as u64)
}

/// CPU count for `expr`, given the detected CPUs. At least 1.
fn eval_cpu(expr: Option<&str>, cpus: u64) -> Result<u64, String> {
    let Some(expr) = expr else {
        return Ok(cpus.max(1));
    };
    match parse_expr(expr, parse_count) {
        Some(Expr::Percent { pct, minus }) => {
            Ok(((cpus as f64 * pct / 100.0).round() - minus).max(1.0) as u64)
        }
        _ => Err(format!(
            "cpu({expr:?}): expected e.g. \"50%\" or \"100% - 1\""
        )),
    }
}

// ---------------------------------------------------------------------------
// Template functions
// ---------------------------------------------------------------------------

pub(super) fn mem(expr: String) -> Result<u64, Error> {
    let total = detect_memory(Path::new("/")).map_err(fail)?;
    eval_mem(&expr, total).map_err(fail)
}

pub(super) fn cpu(expr: Option<String>) -> Result<u64, Error> {
    let cpus = detect_cpus(Path::new("/"), affinity_cpus());
    eval_cpu(expr.as_deref(), cpus).map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * MIB;

    #[test]
    fn mem_expressions() {
        assert_eq!(eval_mem("75%", 2 * GIB), Ok(1536));
        assert_eq!(eval_mem("90% - 64MiB", 2 * GIB), Ok(1779));
        assert_eq!(eval_mem("100% - 1GiB", 2 * GIB), Ok(1024));
        assert_eq!(eval_mem("512MiB", 2 * GIB), Ok(512));
        assert_eq!(eval_mem(" 50 % ", 2 * GIB), Ok(1024));
        assert_eq!(eval_mem("1GB", 0), Ok(953));
    }

    #[test]
    fn mem_rejects_bad_or_empty_results() {
        for bad in [
            "",
            "75",
            "x%",
            "-5%",
            "0%",
            "1GiB - 1MiB",
            "50% - 10",
            "5TB",
        ] {
            assert!(eval_mem(bad, 2 * GIB).is_err(), "{bad:?}");
        }
        let err = eval_mem("100% - 64MiB", 64 * MIB).unwrap_err();
        assert!(err.contains("result is"), "{err}");
    }

    #[test]
    fn cpu_expressions() {
        assert_eq!(eval_cpu(None, 4), Ok(4));
        assert_eq!(eval_cpu(None, 0), Ok(1));
        assert_eq!(eval_cpu(Some("50%"), 4), Ok(2));
        assert_eq!(eval_cpu(Some("50%"), 3), Ok(2)); // 1.5 rounds up
        assert_eq!(eval_cpu(Some("100% - 1"), 8), Ok(7));
        assert_eq!(eval_cpu(Some("100% - 1"), 1), Ok(1)); // clamped
        assert_eq!(eval_cpu(Some("200%"), 2), Ok(4));
        for bad in ["4", "4 - 1", "50% - 1GiB", "abc"] {
            assert!(eval_cpu(Some(bad), 4).is_err(), "{bad:?}");
        }
    }
}
