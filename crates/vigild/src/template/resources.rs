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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use minijinja::{Error, ErrorKind};
use tracing::warn;

const MIB: u64 = 1024 * 1024;
/// cgroup v1 reports "no limit" as a huge page-aligned number.
const V1_UNLIMITED: u64 = 1 << 60;

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
// Detection
// ---------------------------------------------------------------------------

/// Paths of the own cgroup (v2) and its ancestors, innermost first, under
/// `root`.
fn cgroup_v2_dirs(root: &Path) -> Vec<PathBuf> {
    let base = root.join("sys/fs/cgroup");
    let own = std::fs::read_to_string(root.join("proc/self/cgroup"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("0::").map(|p| p.trim().to_owned()))
        })
        .unwrap_or_default();
    let mut dirs = Vec::new();
    let mut rel = PathBuf::from(own.trim_start_matches('/'));
    loop {
        let dir = base.join(&rel);
        if dir.is_dir() {
            dirs.push(dir);
        }
        if !rel.pop() {
            break;
        }
    }
    if dirs.is_empty() {
        dirs.push(base);
    }
    dirs
}

fn read_trimmed(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

/// Smallest cgroup memory limit in bytes, `None` if unlimited.
fn cgroup_memory_limit(root: &Path) -> Option<u64> {
    let v2 = cgroup_v2_dirs(root)
        .iter()
        .filter_map(|d| read_trimmed(&d.join("memory.max")))
        .filter_map(|v| v.parse::<u64>().ok()) // "max" = unlimited
        .min();
    v2.or_else(|| {
        read_trimmed(&root.join("sys/fs/cgroup/memory/memory.limit_in_bytes"))
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&v| v < V1_UNLIMITED)
    })
}

fn mem_total(root: &Path) -> Option<u64> {
    let s = std::fs::read_to_string(root.join("proc/meminfo")).ok()?;
    let line = s.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// Smallest cgroup CPU quota in CPUs (rounded up), `None` if unlimited.
fn cgroup_cpu_quota(root: &Path) -> Option<u64> {
    let ceil_div = |q: u64, p: u64| (p > 0).then(|| q.div_ceil(p).max(1));
    let v2 = cgroup_v2_dirs(root)
        .iter()
        .filter_map(|d| read_trimmed(&d.join("cpu.max")))
        .filter_map(|v| {
            let mut it = v.split_whitespace();
            let q = it.next()?.parse::<u64>().ok()?; // "max" = unlimited
            let p = it.next()?.parse::<u64>().ok()?;
            ceil_div(q, p)
        })
        .min();
    v2.or_else(|| {
        let q = read_trimmed(&root.join("sys/fs/cgroup/cpu/cpu.cfs_quota_us"))?
            .parse::<i64>()
            .ok()?;
        let p = read_trimmed(&root.join("sys/fs/cgroup/cpu/cpu.cfs_period_us"))?
            .parse::<u64>()
            .ok()?;
        u64::try_from(q).ok().and_then(|q| ceil_div(q, p)) // -1 = unlimited
    })
}

fn affinity_cpus() -> u64 {
    use nix::sched::{CpuSet, sched_getaffinity};
    use nix::unistd::Pid;
    match sched_getaffinity(Pid::from_raw(0)) {
        Ok(set) => (0..CpuSet::count())
            .filter(|&i| set.is_set(i).unwrap_or(false))
            .count() as u64,
        Err(_) => std::thread::available_parallelism().map_or(1, |n| n.get() as u64),
    }
}

static WARNED_MEM: AtomicBool = AtomicBool::new(false);
static WARNED_CPU: AtomicBool = AtomicBool::new(false);

fn warn_once(flag: &AtomicBool, what: &str) {
    if !flag.swap(true, Ordering::Relaxed) {
        warn!("no cgroup {what} limit found; mem()/cpu() templates use the whole node");
    }
}

fn detect_memory(root: &Path) -> Result<u64, String> {
    let total = mem_total(root);
    match (cgroup_memory_limit(root), total) {
        (Some(l), Some(t)) => Ok(l.min(t)),
        (Some(l), None) => Ok(l),
        (None, Some(t)) => {
            warn_once(&WARNED_MEM, "memory");
            Ok(t)
        }
        (None, None) => Err("cannot detect the available memory".into()),
    }
}

fn detect_cpus(root: &Path, affinity: u64) -> u64 {
    match cgroup_cpu_quota(root) {
        Some(q) => q.min(affinity).max(1),
        None => {
            warn_once(&WARNED_CPU, "CPU");
            affinity.max(1)
        }
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

    fn fake_root(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (p, content) in files {
            let path = dir.path().join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        dir
    }

    const MEMINFO: &str = "MemTotal:       16384000 kB\nMemFree: 1 kB\n";

    #[test]
    fn container_with_cgroupns_uses_root_limits() {
        // Kubernetes: limits.memory 2Gi, limits.cpu 1500m.
        let r = fake_root(&[
            ("proc/self/cgroup", "0::/\n"),
            ("proc/meminfo", MEMINFO),
            ("sys/fs/cgroup/memory.max", "2147483648\n"),
            ("sys/fs/cgroup/cpu.max", "150000 100000\n"),
        ]);
        assert_eq!(detect_memory(r.path()), Ok(2 * GIB));
        assert_eq!(detect_cpus(r.path(), 16), 2);
        assert_eq!(detect_cpus(r.path(), 1), 1); // cpuset smaller than quota
    }

    #[test]
    fn nested_cgroup_takes_the_smallest_ancestor_limit() {
        let r = fake_root(&[
            ("proc/self/cgroup", "0::/kubepods/pod1/ctr\n"),
            ("proc/meminfo", MEMINFO),
            ("sys/fs/cgroup/kubepods/pod1/memory.max", "1073741824\n"),
            ("sys/fs/cgroup/kubepods/pod1/ctr/memory.max", "max\n"),
            ("sys/fs/cgroup/kubepods/pod1/ctr/cpu.max", "max 100000\n"),
            ("sys/fs/cgroup/kubepods/cpu.max", "400000 100000\n"),
        ]);
        assert_eq!(detect_memory(r.path()), Ok(GIB));
        assert_eq!(detect_cpus(r.path(), 64), 4);
    }

    #[test]
    fn no_limit_falls_back_to_the_node() {
        let r = fake_root(&[
            ("proc/self/cgroup", "0::/\n"),
            ("proc/meminfo", MEMINFO),
            ("sys/fs/cgroup/memory.max", "max\n"),
            ("sys/fs/cgroup/cpu.max", "max 100000\n"),
        ]);
        assert_eq!(detect_memory(r.path()), Ok(16384000 * 1024));
        assert_eq!(detect_cpus(r.path(), 8), 8);
    }

    #[test]
    fn cgroup_v1_limits() {
        let r = fake_root(&[
            ("proc/self/cgroup", "4:memory:/docker/abc\n"),
            ("proc/meminfo", MEMINFO),
            ("sys/fs/cgroup/memory/memory.limit_in_bytes", "536870912\n"),
            ("sys/fs/cgroup/cpu/cpu.cfs_quota_us", "50000\n"),
            ("sys/fs/cgroup/cpu/cpu.cfs_period_us", "100000\n"),
        ]);
        assert_eq!(detect_memory(r.path()), Ok(512 * MIB));
        assert_eq!(detect_cpus(r.path(), 8), 1);

        let unlimited = fake_root(&[
            ("proc/meminfo", MEMINFO),
            (
                "sys/fs/cgroup/memory/memory.limit_in_bytes",
                "9223372036854771712\n",
            ),
            ("sys/fs/cgroup/cpu/cpu.cfs_quota_us", "-1\n"),
            ("sys/fs/cgroup/cpu/cpu.cfs_period_us", "100000\n"),
        ]);
        assert_eq!(detect_memory(unlimited.path()), Ok(16384000 * 1024));
        assert_eq!(detect_cpus(unlimited.path(), 8), 8);
    }

    #[test]
    fn memory_limit_above_ram_is_capped() {
        let r = fake_root(&[
            ("proc/meminfo", MEMINFO),
            ("sys/fs/cgroup/memory.max", "99999999999999\n"),
        ]);
        assert_eq!(detect_memory(r.path()), Ok(16384000 * 1024));
    }

    #[test]
    fn memory_without_cgroup_and_meminfo_is_an_error() {
        let r = fake_root(&[]);
        assert_eq!(
            detect_memory(r.path()),
            Err("cannot detect the available memory".to_string())
        );
    }
}
