// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! An alert as it is sent: `env:VAR` values resolved, HTTP client built.

use std::collections::BTreeMap;
use std::ffi::OsString;

use anyhow::Context;
use indexmap::IndexMap;
use reqwest::Client;
use vigil_types::plan::{AlertConfig, ProxyConfig};

use crate::process_util::EnvPolicy;

/// vigild's environment (`std::env::vars_os()`, read once per load).
type Parent = [(OsString, OsString)];

/// What an alert sees of vigild's environment: the same
/// [`EnvPolicy::build`] a service gets (base variables plus what
/// `pass-env*` selects), no user switch, no own `environment`.
struct AlertEnv<'a> {
    visible: BTreeMap<String, String>,
    parent: &'a Parent,
}

impl<'a> AlertEnv<'a> {
    fn new(policy: &EnvPolicy, parent: &'a Parent) -> Self {
        let built = policy.build(parent.to_vec(), None, &IndexMap::new());
        Self {
            visible: crate::template::vars_of(&built),
            parent,
        }
    }

    fn get(&self, var: &str) -> Option<&str> {
        self.visible
            .get(var)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// Set (non-empty) in vigild's environment, visible or not.
    fn is_set(&self, var: &str) -> bool {
        self.parent
            .iter()
            .any(|(k, v)| k.to_str() == Some(var) && !v.is_empty())
    }
}

/// The value of `val`, or of vigild's environment variable it names as
/// `env:VAR`, as far as the alert's `pass-env*` policy allows. Errors name
/// `field` and say whether the variable is unset or not passed.
fn resolve(field: &str, val: &str, env: &AlertEnv<'_>) -> anyhow::Result<String> {
    let Some(var) = val.strip_prefix("env:") else {
        return Ok(val.to_owned());
    };
    if let Some(v) = env.get(var) {
        return Ok(v.to_owned());
    }
    if env.is_set(var) {
        anyhow::bail!(
            "{field}: environment variable {var} is not passed to the alert; \
             allow it with pass-env-glob or pass-env-regex (from {val:?})"
        )
    }
    anyhow::bail!("{field}: environment variable {var} is not set (from {val:?})")
}

/// Proxy variables used when the alert has no `proxy:` block (first one
/// set wins; same order as the vigil CLI), and the no-proxy list.
const PROXY_VARS: &[&str] = &[
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "HTTP_PROXY",
    "http_proxy",
];
const NO_PROXY_VARS: &[&str] = &["NO_PROXY", "no_proxy"];

/// The first of `vars` the alert sees.
fn passed_var(vars: &[&str], env: &AlertEnv<'_>) -> Option<String> {
    vars.iter().find_map(|v| env.get(v)).map(str::to_owned)
}

/// `cfg` as it is sent: every `env:VAR` value replaced (`url`, `headers`,
/// `labels`, `send-info-fields`, `proxy.url`, `proxy.no-proxy`) and,
/// without a `proxy:` block, the proxy variables the policy passes.
fn resolve_all(cfg: &AlertConfig, parent: &Parent) -> anyhow::Result<AlertConfig> {
    let env = AlertEnv::new(&EnvPolicy::for_alert(cfg)?, parent);
    let env = &env;
    let mut out = cfg.clone();
    out.url = resolve("url", &cfg.url, env)?;
    for (field, map) in [
        ("headers", &mut out.headers),
        ("labels", &mut out.labels),
        ("send-info-fields", &mut out.send_info_fields),
    ] {
        for (k, v) in map.iter_mut() {
            *v = resolve(&format!("{field}.{k}"), v, env)?;
        }
    }
    out.proxy = match &cfg.proxy {
        Some(p) => Some(ProxyConfig {
            url: resolve("proxy.url", &p.url, env)?,
            no_proxy: p
                .no_proxy
                .as_deref()
                .map(|v| resolve("proxy.no-proxy", v, env))
                .transpose()?,
        }),
        None => passed_var(PROXY_VARS, env).map(|url| ProxyConfig {
            url,
            no_proxy: passed_var(NO_PROXY_VARS, env),
        }),
    };
    Ok(out)
}

/// [`resolve_all`] plus the HTTP client, done once when the layers are
/// loaded: a missing variable, an unreadable `tls.ca` or a bad proxy URL
/// is reported there, never at send time.
pub(crate) fn prepare(
    cfg: &AlertConfig,
    env: &crate::alert::LoadEnv,
) -> anyhow::Result<(AlertConfig, Client)> {
    let out = resolve_all(cfg, &env.parent)?;
    let client = http_client(&out)?;
    Ok((out, client))
}

/// The HTTP client of a resolved alert (reads the `tls:` files).
fn http_client(out: &AlertConfig) -> anyhow::Result<Client> {
    crate::tls::build_http_client(
        reqwest::Client::builder(),
        out.tls.as_ref(),
        out.proxy.as_ref(),
    )
    .context("HTTP client (tls, proxy)")
}
