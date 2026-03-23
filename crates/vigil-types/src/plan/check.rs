// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::Override;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckLevel {
    #[default]
    Alive,
    Ready,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct HttpCheck {
    pub url: String,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, String>,
    /// Do not follow HTTP redirects (default: true).
    /// Set to false to follow redirects transparently.
    #[serde(default = "default_true", skip_serializing_if = "std::ops::Not::not")]
    pub no_follow_redirects: bool,
    /// Skip TLS certificate verification (useful for self-signed certs).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure: bool,
    /// Skip TLS certificate verification for the HTTPS proxy itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure_proxy: bool,
    /// PEM file with a CA certificate (or chain) to verify the server's TLS.
    /// Supports chain files with multiple concatenated PEM blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<std::path::PathBuf>,
    /// HTTP status codes that count as success. Default (empty): 200–299.
    /// Example: [200, 204, 301]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_statuses: Vec<u16>,
}

impl Default for HttpCheck {
    fn default() -> Self {
        Self {
            url: String::new(),
            headers: IndexMap::new(),
            no_follow_redirects: true,
            insecure: false,
            insecure_proxy: false,
            ca: None,
            success_statuses: vec![],
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TcpCheck {
    pub host: Option<String>,
    pub port: u16,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ExecCheck {
    pub command: String,
    /// Inherit env/user/group/working-dir from this service; check settings override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_context: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub environment: IndexMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CheckConfig {
    #[serde(default)]
    pub override_mode: Override,
    #[serde(default)]
    pub level: CheckLevel,
    #[serde(default)]
    pub startup: Startup,
    /// Initial delay before the first check is performed (vigil extension).
    /// Useful to avoid false failures while a service is still starting up.
    /// Example: "5s", "500ms". Default: no delay (first check runs immediately).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tcp: Option<TcpCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec: Option<ExecCheck>,
}

use super::service::Startup;

pub(super) fn merge_check(base: &mut CheckConfig, overlay: &CheckConfig) {
    base.level = overlay.level;
    base.startup = overlay.startup;

    macro_rules! opt {
        ($f:ident) => {
            if overlay.$f.is_some() {
                base.$f = overlay.$f.clone();
            }
        };
    }

    opt!(delay);
    opt!(period);
    opt!(timeout);
    opt!(threshold);
    opt!(http);
    opt!(tcp);
    opt!(exec);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> CheckConfig {
        CheckConfig {
            period: Some("5s".to_string()),
            timeout: Some("2s".to_string()),
            threshold: Some(5),
            exec: Some(ExecCheck {
                command: "old".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn merge_check_level_and_startup_always_overwritten() {
        let mut b = base();
        b.level = CheckLevel::Alive;
        let overlay = CheckConfig {
            level: CheckLevel::Ready,
            startup: Startup::Disabled,
            ..Default::default()
        };
        merge_check(&mut b, &overlay);
        assert_eq!(b.level, CheckLevel::Ready);
        assert_eq!(b.startup, Startup::Disabled);
    }

    #[test]
    fn merge_check_optional_fields_overwritten_when_set() {
        let mut b = base();
        let overlay = CheckConfig {
            period: Some("30s".to_string()),
            timeout: Some("10s".to_string()),
            threshold: Some(1),
            exec: Some(ExecCheck {
                command: "new".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        merge_check(&mut b, &overlay);
        assert_eq!(b.period.as_deref(), Some("30s"));
        assert_eq!(b.timeout.as_deref(), Some("10s"));
        assert_eq!(b.threshold, Some(1));
        assert_eq!(b.exec.as_ref().unwrap().command, "new");
    }

    #[test]
    fn merge_check_none_overlay_preserves_base() {
        let mut b = base();
        let overlay = CheckConfig::default(); // all optionals are None
        merge_check(&mut b, &overlay);
        assert_eq!(b.period.as_deref(), Some("5s"));
        assert_eq!(b.timeout.as_deref(), Some("2s"));
        assert_eq!(b.threshold, Some(5));
        assert_eq!(b.exec.as_ref().unwrap().command, "old");
    }

    #[test]
    fn merge_check_http_overlay_replaces_exec() {
        let mut b = base(); // has exec
        let overlay = CheckConfig {
            http: Some(HttpCheck {
                url: "http://x/".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        merge_check(&mut b, &overlay);
        // http is set in overlay → merged into base
        assert!(b.http.is_some());
        // exec was not in overlay → preserved from base
        assert!(b.exec.is_some());
    }

    #[test]
    fn check_level_serde_roundtrip() {
        assert_eq!(
            serde_json::from_str::<CheckLevel>("\"alive\"").unwrap(),
            CheckLevel::Alive,
        );
        assert_eq!(
            serde_json::from_str::<CheckLevel>("\"ready\"").unwrap(),
            CheckLevel::Ready,
        );
    }

    #[test]
    fn check_config_serde_skips_none_fields() {
        let cfg = CheckConfig {
            period: Some("10s".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"period\""));
        assert!(!json.contains("\"timeout\""));
        assert!(!json.contains("\"http\""));
    }
}
