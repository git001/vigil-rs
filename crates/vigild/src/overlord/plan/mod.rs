// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Loading the layers: read, merge and validate them into a plan.

mod alert;
mod check;
pub(super) mod dependencies;
mod service;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::Context;
use vigil_types::plan::{Layer, Plan, ReadyMode};

use crate::process_util::{REDUNDANT_LISTS, redundant_lists};

use alert::validate_alert;
use check::validate_check;
use dependencies::check_dependency_cycles;
use service::validate_service;

/// Largest layer file read.
const MAX_LAYER_FILE: u64 = 4 * 1024 * 1024;

/// Whether a directory entry is a layer file: `*.yaml` or `*.yml`, but not
/// the kubelet's own entries of a ConfigMap volume (`..data`,
/// `..2026_11_06_…`), which start with `..`.
pub(crate) fn is_layer_file(name: &str) -> bool {
    !name.starts_with("..") && (name.ends_with(".yaml") || name.ends_with(".yml"))
}

/// A validated plan, its alerts ready to send, the clients of its HTTP
/// checks and its identities.
pub struct Loaded {
    pub plan: Plan,
    pub alerts: crate::alert::PreparedAlerts,
    pub checks: crate::check::PreparedChecks,
    pub identities: Vec<vigil_types::identity::Identity>,
    /// The files the layers refer to and that were read with them (`tls:`
    /// of checks, alerts and identities, `password-hash-file`):
    /// `--watch-layers` hashes them like the layer files.
    pub files: Vec<std::path::PathBuf>,
}

/// Read, merge and validate the layers in `dir`, prepare the alerts
/// against vigild's environment, build the HTTP check clients and load the
/// identities. `notify_dir` is where `ready: notify` sockets go. Reads
/// files: call it off the async runtime (see `off_runtime`).
pub fn load(dir: &Path, notify_dir: &Path) -> anyhow::Result<Loaded> {
    load_with(dir, notify_dir, &crate::alert::LoadEnv::process())
}

fn load_with(dir: &Path, notify_dir: &Path, env: &crate::alert::LoadEnv) -> anyhow::Result<Loaded> {
    let plan = read_plan(dir)?;
    validate(&plan, notify_dir)?;
    let alerts = crate::alert::prepare_alerts(&plan.alerts, env)?;
    let checks = crate::check::prepare_checks(&plan.checks)?;
    let identities = crate::identity::load_layer_identities(&plan.identities)?;
    let files = referenced_files(&plan, identities.files);
    Ok(Loaded {
        plan,
        alerts,
        checks,
        identities: identities.identities,
        files,
    })
}

/// The files of the identities followed by those of the `tls:` blocks of
/// HTTP checks and alerts, each once.
fn referenced_files(plan: &Plan, identity_files: Vec<PathBuf>) -> Vec<PathBuf> {
    let tls = plan
        .checks
        .values()
        .filter_map(|c| c.http.as_ref()?.tls.as_ref())
        .chain(plan.alerts.values().filter_map(|a| a.tls.as_ref()));
    let mut files = identity_files;
    for t in tls {
        files.extend(t.ca.iter().chain(&t.cert).chain(&t.key).cloned());
    }
    let mut seen = std::collections::HashSet::new();
    files.retain(|p| seen.insert(p.clone()));
    files
}

fn read_plan(dir: &Path) -> anyhow::Result<Plan> {
    if !dir.exists() {
        return Ok(Plan::default());
    }

    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading layers dir {dir:?}"))?
        .filter_map(|e| e.ok())
        .filter(|e| is_layer_file(&e.file_name().to_string_lossy()))
        .collect();

    entries.sort_by_key(|e| e.file_name());

    let mut layers = Vec::new();
    for (order, entry) in entries.iter().enumerate() {
        let path = entry.path();
        let label = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let content = vigil_files::read_text(&path, MAX_LAYER_FILE, vigil_files::PathRule::Any)
            .map_err(|e| anyhow::anyhow!("reading {e}"))?
            .with_context(|| format!("reading {path:?}: removed while loading"))?;
        let mut layer: Layer =
            serde_yaml::from_str(&content).with_context(|| format!("parsing {path:?}"))?;
        layer.order = order as u32;
        layer.label = label;
        layers.push(layer);
    }

    Ok(Plan::from_layers(layers))
}

/// Checks that need more than deserialization. Runs on startup and on every
/// replan, so a bad layer is rejected before anything is (re)started.
fn validate(plan: &Plan, notify_dir: &Path) -> anyhow::Result<()> {
    for (name, svc) in &plan.services {
        validate_service(plan, name, svc).with_context(|| format!("service {name:?}"))?;
    }
    if let Some(name) = plan
        .services
        .iter()
        .find_map(|(name, svc)| (svc.ready == Some(ReadyMode::Notify)).then_some(name))
    {
        crate::service::notify::check_dir(notify_dir)
            .with_context(|| format!("service {name:?}: ready: notify"))?;
    }
    check_dependency_cycles(plan)?;
    for (name, check) in &plan.checks {
        validate_check(plan, name, check).with_context(|| format!("check {name:?}"))?;
    }
    crate::alert::validate_queue_limits(plan)?;
    for (name, alert) in &plan.alerts {
        if redundant_lists(alert.pass_env, &alert.pass_env_glob, &alert.pass_env_regex) {
            tracing::warn!(alert = %name, "{REDUNDANT_LISTS}");
        }
        validate_alert(alert).with_context(|| format!("alert {name:?}"))?;
    }
    Ok(())
}
