// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Secrets masked out of the plan for `GET /v1/plan`.
//!
//! - `environment` values (services, exec checks): all of them.
//! - `headers` values (HTTP checks, alerts): all but a reference `env:NAME`.
//! - URLs (HTTP checks, alerts, proxies): the password (or a user name
//!   alone, which may be a token) and the query values; for alerts also
//!   the path, which carries the secret of many webhooks. A reference
//!   `env:NAME` stays.
//! - `password-hash` of identities.
//!
//! Everything else stays as written: `command`, `body-template`, `labels`,
//! `send-info-fields` and file paths. The docs say not to put secrets there.

use indexmap::IndexMap;
use vigil_types::plan::{Layer, ProxyConfig};

/// What a masked value reads.
pub(crate) const MASK: &str = "***";

/// Mask the secrets of `layer` (from [`vigil_types::plan::Plan::to_layer`]).
pub(crate) fn mask(layer: &mut Layer) {
    for svc in layer.services.values_mut() {
        mask_all(&mut svc.environment);
    }
    for check in layer.checks.values_mut() {
        if let Some(exec) = &mut check.exec {
            mask_all(&mut exec.environment);
        }
        if let Some(http) = &mut check.http {
            http.url = mask_url(&http.url, Path::Keep);
            mask_headers(&mut http.headers);
            mask_proxy(&mut http.proxy);
        }
    }
    for alert in layer.alerts.entries.values_mut() {
        alert.url = mask_url(&alert.url, Path::Mask);
        mask_headers(&mut alert.headers);
        mask_proxy(&mut alert.proxy);
    }
    for id in layer.identities.values_mut() {
        if let Some(hash) = id.basic.as_mut().and_then(|b| b.password_hash.as_mut()) {
            *hash = MASK.to_owned();
        }
    }
}

fn mask_all(values: &mut IndexMap<String, String>) {
    for v in values.values_mut() {
        *v = MASK.to_owned();
    }
}

fn mask_headers(headers: &mut IndexMap<String, String>) {
    for v in headers.values_mut() {
        if !is_env_ref(v) {
            *v = MASK.to_owned();
        }
    }
}

fn mask_proxy(proxy: &mut Option<ProxyConfig>) {
    if let Some(p) = proxy {
        p.url = mask_url(&p.url, Path::Keep);
    }
}

/// `env:NAME`: a variable vigild resolves, not a secret itself.
fn is_env_ref(value: &str) -> bool {
    value.strip_prefix("env:").is_some_and(|name| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether the path of a URL stays.
#[derive(Clone, Copy, PartialEq)]
enum Path {
    Keep,
    Mask,
}

/// `url` with its secrets masked: user info, query values, a fragment and,
/// with [`Path::Mask`], the path. A value that is no `scheme://` URL is
/// masked whole.
fn mask_url(url: &str, path: Path) -> String {
    if is_env_ref(url) {
        return url.to_owned();
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return MASK.to_owned();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, rest) = rest.split_at(end);
    let host = match authority.rsplit_once('@') {
        Some((userinfo, host)) => match userinfo.split_once(':') {
            Some((user, _)) => format!("{user}:{MASK}@{host}"),
            None => format!("{MASK}@{host}"),
        },
        None => authority.to_owned(),
    };
    let (rest, fragment) = match rest.split_once('#') {
        Some((r, _)) => (r, format!("#{MASK}")),
        None => (rest, String::new()),
    };
    let (url_path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, format!("?{}", mask_query(q))),
        None => (rest, String::new()),
    };
    let url_path = if path == Path::Mask && url_path.len() > 1 {
        format!("/{MASK}")
    } else {
        url_path.to_owned()
    };
    format!("{scheme}://{host}{url_path}{query}{fragment}")
}

/// `a=1&b` → `a=***&***`.
fn mask_query(query: &str) -> String {
    query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) => format!("{key}={MASK}"),
            None if pair.is_empty() => String::new(),
            None => MASK.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let cases = [
            (
                "http://localhost:8080/healthz",
                Path::Keep,
                "http://localhost:8080/healthz",
            ),
            (
                "https://u:pw@host/x?a=1&b&c=",
                Path::Keep,
                "https://u:***@host/x?a=***&***&c=***",
            ),
            ("https://token@host", Path::Keep, "https://***@host"),
            ("https://host/a#frag", Path::Keep, "https://host/a#***"),
            (
                "https://hooks.slack.com/services/T0/B0/XYZ",
                Path::Mask,
                "https://hooks.slack.com/***",
            ),
            ("https://host/", Path::Mask, "https://host/"),
            ("https://host?key=s", Path::Mask, "https://host?key=***"),
            ("env:WEBHOOK_URL", Path::Mask, "env:WEBHOOK_URL"),
            ("not a url", Path::Keep, "***"),
        ];
        for (url, path, want) in cases {
            assert_eq!(mask_url(url, path), want, "{url}");
        }
    }

    #[test]
    fn plan_secrets_are_masked_and_the_rest_stays() {
        let mut layer: Layer = serde_yaml::from_str(
            "services:\n  web:\n    command: app --port 80\n    environment: {DB_PASSWORD: s3cret}\n\
             checks:\n  up:\n    http:\n      url: http://localhost/healthz\n      \
             headers: {Authorization: Bearer x, X-Token: 'env:TOKEN'}\n      \
             proxy: {url: 'http://u:pw@proxy:3128'}\n  \
             run:\n    exec: {command: 'true', environment: {K: v}}\n\
             alerts:\n  ops:\n    url: https://hooks.example.com/T0/secret\n    \
             headers: {Authorization: Basic x}\n    labels: {team: web}\n\
             identities:\n  relay:\n    access: read\n    basic: {password-hash: $6$x$y}\n",
        )
        .unwrap();
        mask(&mut layer);
        let yaml = serde_yaml::to_string(&layer).unwrap();
        for secret in [
            "s3cret",
            "Bearer",
            ":pw@",
            "/T0/secret",
            "Basic x",
            "$6$x$y",
            ": v\n",
        ] {
            assert!(!yaml.contains(secret), "{secret} in {yaml}");
        }
        for kept in [
            "app --port 80",
            "DB_PASSWORD: '***'",
            "http://localhost/healthz",
            "env:TOKEN",
            "http://u:***@proxy:3128",
            "https://hooks.example.com/***",
            "team: web",
        ] {
            assert!(yaml.contains(kept), "{kept} not in {yaml}");
        }
    }
}
