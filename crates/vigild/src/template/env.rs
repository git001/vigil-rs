// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! A child's environment: the base set, env files and rendered values.

use std::collections::BTreeMap;
use std::ffi::OsString;

use indexmap::IndexMap;

use super::{Renderer, env_file};

/// Explicit variables of a service (or exec check): `env-file` contents,
/// then the rendered `environment` values.
pub struct EnvLayer<'a> {
    pub env_file: &'a [String],
    pub environment: &'a IndexMap<String, String>,
}

/// Build a child's environment: what `policy` lets through from vigild's
/// environment, then each layer's env files and `environment` values.
///
/// Templates in a layer's `environment` see everything before them (base
/// set, pass-env, env files, earlier layers), but not the other entries of
/// the same `environment`. vigild's whole environment is never visible.
pub fn child_env(
    renderer: &Renderer,
    policy: &crate::process_util::EnvPolicy,
    creds: Option<&crate::process_util::Credentials>,
    layers: &[EnvLayer<'_>],
) -> anyhow::Result<Vec<(OsString, OsString)>> {
    // Read once so every build uses the same snapshot.
    let parent: Vec<_> = std::env::vars_os().collect();
    let mut explicit: IndexMap<String, String> = IndexMap::new();
    for layer in layers {
        env_file::load(layer.env_file, &mut explicit, &renderer.reads)?;
        let vars = vars_of(&policy.build(parent.clone(), creds, &explicit));
        for (k, v) in layer.environment {
            let v = renderer
                .render(v, &vars)
                .map_err(|e| anyhow::anyhow!("environment {k:?}: {e}"))?;
            explicit.insert(k.clone(), v);
        }
    }
    Ok(policy.build(parent, creds, &explicit))
}

/// Environment entries with UTF-8 names and values, as template variables.
pub fn vars_of(env: &[(OsString, OsString)]) -> BTreeMap<String, String> {
    env.iter()
        .filter_map(|(k, v)| Some((k.to_str()?.to_owned(), v.to_str()?.to_owned())))
        .collect()
}
