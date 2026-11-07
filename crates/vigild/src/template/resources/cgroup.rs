// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Detect the memory and CPUs a container may use: cgroup v2 (own cgroup
//! and ancestors) or v1 limits, `MemTotal`, `sched_getaffinity`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::warn;

/// cgroup v1 reports "no limit" as a huge page-aligned number.
const V1_UNLIMITED: u64 = 1 << 60;

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

pub(super) fn affinity_cpus() -> u64 {
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

pub(super) fn detect_memory(root: &Path) -> Result<u64, String> {
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

pub(super) fn detect_cpus(root: &Path, affinity: u64) -> u64 {
    match cgroup_cpu_quota(root) {
        Some(q) => q.min(affinity).max(1),
        None => {
            warn_once(&WARNED_CPU, "CPU");
            affinity.max(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;

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
