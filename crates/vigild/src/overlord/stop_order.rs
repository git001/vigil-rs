// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The order in which vigild stops its services when it shuts down: the
//! start order reversed. A service stops once every service that depends on
//! it (`after`, `requires`, or its own `before`) has stopped; services
//! without such a relation stop at the same time.

use std::collections::{HashMap, HashSet};

use vigil_types::plan::Plan;

use super::plan::dependencies::ordering_deps;

/// Which of `names` each of them depends on, by the plan: `after`,
/// `requires` and the `before` of the others. Names not in the plan have no
/// dependencies.
pub(super) fn dependencies(plan: &Plan, names: &[String]) -> Vec<(String, Vec<String>)> {
    let known: HashSet<&str> = names.iter().map(String::as_str).collect();
    names
        .iter()
        .map(|name| {
            let mut deps: Vec<String> = Vec::new();
            if let Some(svc) = plan.services.get(name) {
                for dep in ordering_deps(plan, name, svc).chain(&svc.requires) {
                    if dep != name && known.contains(dep.as_str()) && !deps.contains(dep) {
                        deps.push(dep.clone());
                    }
                }
            }
            (name.clone(), deps)
        })
        .collect()
}

/// Tracks which services may stop now.
pub(super) struct StopOrder {
    /// Per service: the services it depends on.
    deps: HashMap<String, Vec<String>>,
    /// Per service: how many services depending on it still run.
    dependents_left: HashMap<String, usize>,
}

impl StopOrder {
    /// The order for `deps` (from [`dependencies`]) and the services that
    /// may stop at once: those nothing depends on.
    pub(super) fn new(deps: Vec<(String, Vec<String>)>) -> (Self, Vec<String>) {
        let mut dependents_left: HashMap<String, usize> =
            deps.iter().map(|(name, _)| (name.clone(), 0)).collect();
        for (_, ds) in &deps {
            for d in ds {
                *dependents_left.entry(d.clone()).or_default() += 1;
            }
        }
        // In the order given, so the log is stable.
        let first = deps
            .iter()
            .filter(|(name, _)| dependents_left[name] == 0)
            .map(|(name, _)| name.clone())
            .collect();
        let order = StopOrder {
            deps: deps.into_iter().collect(),
            dependents_left,
        };
        (order, first)
    }

    /// All services not let go yet, for when the order cannot be kept.
    pub(super) fn rest(&mut self) -> Vec<String> {
        let rest: Vec<String> = self
            .dependents_left
            .iter()
            .filter(|(_, left)| **left > 0)
            .map(|(name, _)| name.clone())
            .collect();
        for name in &rest {
            self.dependents_left.insert(name.clone(), 0);
        }
        rest
    }

    /// `name` has stopped; returns the services that may stop now.
    pub(super) fn done(&mut self, name: &str) -> Vec<String> {
        let mut next = Vec::new();
        for dep in self.deps.get(name).into_iter().flatten() {
            if let Some(left) = self.dependents_left.get_mut(dep) {
                if *left == 0 {
                    continue; // already let go by `rest`
                }
                *left -= 1;
                if *left == 0 {
                    next.push(dep.clone());
                }
            }
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(deps: &[(&str, &[&str])]) -> (StopOrder, Vec<String>) {
        StopOrder::new(
            deps.iter()
                .map(|(n, ds)| (n.to_string(), ds.iter().map(|d| d.to_string()).collect()))
                .collect(),
        )
    }

    #[test]
    fn dependents_stop_first() {
        // app needs db and cache; proxy is in front of app; log is alone.
        let (mut o, first) = order(&[
            ("db", &[]),
            ("cache", &[]),
            ("app", &["db", "cache"]),
            ("proxy", &["app"]),
            ("log", &[]),
        ]);
        assert_eq!(first, ["proxy", "log"]);
        assert!(o.done("log").is_empty());
        assert_eq!(o.done("proxy"), ["app"]);
        let mut next = o.done("app");
        next.sort();
        assert_eq!(next, ["cache", "db"]);
        assert!(o.done("db").is_empty());
        assert!(o.done("cache").is_empty());
    }

    #[test]
    fn a_shared_dependency_waits_for_all_its_dependents() {
        let (mut o, first) = order(&[("db", &[]), ("a", &["db"]), ("b", &["db"])]);
        assert_eq!(first, ["a", "b"]);
        assert!(o.done("a").is_empty(), "b still runs");
        assert_eq!(o.done("b"), ["db"]);
    }

    #[test]
    fn rest_lets_everything_go_once() {
        let (mut o, first) = order(&[("db", &[]), ("app", &["db"])]);
        assert_eq!(first, ["app"]);
        assert_eq!(o.rest(), ["db"]);
        assert!(o.done("app").is_empty(), "db was let go already");
        assert!(o.rest().is_empty());
    }

    #[test]
    fn without_dependencies_all_stop_at_once() {
        let (_, first) = order(&[("a", &[]), ("b", &[]), ("c", &[])]);
        assert_eq!(first, ["a", "b", "c"]);
    }

    #[test]
    fn dependencies_from_after_requires_and_before() {
        let plan = Plan::from_layers(vec![
            serde_yaml::from_str(
                "services:\n\
             \x20 db: {command: x}\n\
             \x20 app: {command: x, requires: [db], after: [migrate]}\n\
             \x20 migrate: {command: x, type: oneshot, after: [db]}\n\
             \x20 proxy: {command: x, before: [app]}\n\
             \x20 gone: {command: x, after: [db]}\n",
            )
            .unwrap(),
        ]);
        // "gone" is not among the services to stop.
        let names: Vec<String> = ["db", "app", "migrate", "proxy"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let deps: HashMap<_, _> = dependencies(&plan, &names).into_iter().collect();
        assert!(deps["db"].is_empty());
        assert_eq!(deps["app"], ["migrate", "proxy", "db"]);
        assert_eq!(deps["migrate"], ["db"]);
        assert!(deps["proxy"].is_empty());
    }
}
