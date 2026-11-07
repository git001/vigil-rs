// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `after`, `before` and `requires`: names, cycles, and what autostart
//! does with them.

use vigil_types::plan::{Plan, ServiceConfig};

/// Every name in `after`, `before` and `requires` is a service, and not the
/// service itself (a typo would otherwise be ignored or wait forever).
pub(super) fn validate_dependency_names(
    plan: &Plan,
    name: &str,
    svc: &ServiceConfig,
) -> anyhow::Result<()> {
    for (key, deps) in [
        ("after", &svc.after),
        ("before", &svc.before),
        ("requires", &svc.requires),
    ] {
        for dep in deps {
            anyhow::ensure!(dep != name, "{key}: a service cannot depend on itself");
            anyhow::ensure!(
                plan.services.contains_key(dep),
                "{key}: {dep:?} is not a service"
            );
        }
    }
    Ok(())
}

/// The start order must have no cycle: `a after b` and `b after a` would
/// make both wait forever. `after` and `requires` point from a service to
/// its dependency, `before` the other way. Reports the first cycle found.
pub(super) fn check_dependency_cycles(plan: &Plan) -> anyhow::Result<()> {
    let names: Vec<&String> = plan.services.keys().collect();
    let index = |n: &str| names.iter().position(|x| x.as_str() == n);
    // deps[i]: the services service i waits for.
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); names.len()];
    for (i, svc) in plan.services.values().enumerate() {
        for dep in svc.after.iter().chain(&svc.requires) {
            if let Some(j) = index(dep) {
                deps[i].push(j);
            }
        }
        for later in &svc.before {
            if let Some(j) = index(later) {
                deps[j].push(i);
            }
        }
    }

    // Depth-first search; 0 = unvisited, 1 = on the current path, 2 = done.
    fn visit(
        i: usize,
        deps: &[Vec<usize>],
        mark: &mut [u8],
        path: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        mark[i] = 1;
        path.push(i);
        for &j in &deps[i] {
            match mark[j] {
                1 => {
                    let start = path.iter().position(|&p| p == j).unwrap_or(0);
                    let mut cycle = path[start..].to_vec();
                    cycle.push(j);
                    return Some(cycle);
                }
                0 => {
                    if let Some(c) = visit(j, deps, mark, path) {
                        return Some(c);
                    }
                }
                _ => {}
            }
        }
        path.pop();
        mark[i] = 2;
        None
    }

    let mut mark = vec![0u8; names.len()];
    for i in 0..names.len() {
        if mark[i] == 0
            && let Some(cycle) = visit(i, &deps, &mut mark, &mut Vec::new())
        {
            let chain: Vec<&str> = cycle.iter().map(|&k| names[k].as_str()).collect();
            anyhow::bail!(
                "services wait for each other (after, before, requires): {}",
                chain.join(" → ")
            );
        }
    }
    Ok(())
}

/// Why autostart does not start `svc`, judged from its config alone (the
/// overlord adds the runtime reasons, see `Overlord::autostart_skip_reason`).
pub(in crate::overlord) fn autostart_skip_reason(svc: &ServiceConfig) -> Option<&'static str> {
    if svc.startup() != vigil_types::plan::Startup::Enabled {
        Some("startup is not enabled")
    } else if svc.schedule.is_some() {
        // Scheduled jobs are started by their schedule, not at startup.
        Some("scheduled; its schedule starts it")
    } else {
        None
    }
}

/// The services `name` starts after (`after` and `requires` aside): its
/// `after`, then every other service that lists `name` in `before`.
pub(in crate::overlord) fn ordering_deps<'a>(
    plan: &'a Plan,
    name: &'a str,
    svc: &'a ServiceConfig,
) -> impl Iterator<Item = &'a String> {
    let reverse_before = plan
        .services
        .iter()
        .filter(move |(other, c)| *other != name && c.before.iter().any(|b| b == name))
        .map(|(other, _)| other);
    svc.after.iter().chain(reverse_before)
}

/// Warnings for dependencies of an autostarted service that autostart does
/// not start: an `after`/`before` ordering on them is ignored, a `requires`
/// makes the service wait until they are started.
pub(super) fn dependency_warnings(plan: &Plan, name: &str, svc: &ServiceConfig) -> Vec<String> {
    if autostart_skip_reason(svc).is_some() {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    let mut seen: Vec<&String> = Vec::new();
    for dep in ordering_deps(plan, name, svc) {
        if let Some(why) = plan.services.get(dep).and_then(autostart_skip_reason)
            && !seen.contains(&dep)
        {
            seen.push(dep);
            warnings.push(format!(
                "starts after {dep:?}, but autostart does not start {dep:?} ({why}): \
                 the ordering is ignored"
            ));
        }
    }
    for dep in &svc.requires {
        if let Some(why) = plan.services.get(dep).and_then(autostart_skip_reason) {
            warnings.push(format!(
                "requires {dep:?}, but autostart does not start {dep:?} ({why}): \
                 {name:?} waits until {dep:?} is started"
            ));
        }
    }
    warnings
}
