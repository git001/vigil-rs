// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::Override;
use super::alert::{AlertConfig, AlertsBlock, merge_alert};
use super::check::{CheckConfig, merge_check};
use super::service::{ServiceConfig, merge_service};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Layer {
    #[serde(skip)]
    pub order: u32,
    #[serde(skip)]
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub services: IndexMap<String, ServiceConfig>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub checks: IndexMap<String, CheckConfig>,
    #[serde(default)]
    pub alerts: AlertsBlock,
}

/// The merged, resolved plan built from all layers in order.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub layers: Vec<Layer>,
    pub services: IndexMap<String, ServiceConfig>,
    pub checks: IndexMap<String, CheckConfig>,
    pub alerts: IndexMap<String, AlertConfig>,
    /// Merged value of `alerts.max-queue-depth` across all layers (last wins).
    /// `None` means "use the built-in default".
    pub alert_queue_depth: Option<usize>,
    /// Merged value of `alerts.max-queue-time` across all layers (last wins).
    /// `None` means "use the built-in default".
    pub alert_max_queue_time: Option<String>,
}

impl Plan {
    /// Merge a list of layers (in order) into a resolved Plan.
    pub fn from_layers(layers: Vec<Layer>) -> Self {
        let mut services: IndexMap<String, ServiceConfig> = IndexMap::new();
        let mut checks: IndexMap<String, CheckConfig> = IndexMap::new();
        let mut alerts: IndexMap<String, AlertConfig> = IndexMap::new();
        let mut alert_queue_depth: Option<usize> = None;
        let mut alert_max_queue_time: Option<String> = None;

        for layer in &layers {
            for (name, svc) in &layer.services {
                match svc.override_mode {
                    Override::Replace => {
                        services.insert(name.clone(), svc.clone());
                    }
                    Override::Merge => match services.get_mut(name) {
                        None => {
                            services.insert(name.clone(), svc.clone());
                        }
                        Some(existing) => merge_service(existing, svc),
                    },
                }
            }

            for (name, chk) in &layer.checks {
                match chk.override_mode {
                    Override::Replace => {
                        checks.insert(name.clone(), chk.clone());
                    }
                    Override::Merge => match checks.get_mut(name) {
                        None => {
                            checks.insert(name.clone(), chk.clone());
                        }
                        Some(existing) => merge_check(existing, chk),
                    },
                }
            }

            // Global queue settings: last layer wins.
            if let Some(d) = layer.alerts.max_queue_depth {
                alert_queue_depth = Some(d);
            }
            if let Some(t) = &layer.alerts.max_queue_time {
                alert_max_queue_time = Some(t.clone());
            }

            for (name, alert) in &layer.alerts.entries {
                match alert.override_mode {
                    Override::Replace => {
                        alerts.insert(name.clone(), alert.clone());
                    }
                    Override::Merge => match alerts.get_mut(name) {
                        None => {
                            alerts.insert(name.clone(), alert.clone());
                        }
                        Some(existing) => merge_alert(existing, alert),
                    },
                }
            }
        }

        Plan {
            layers,
            services,
            checks,
            alerts,
            alert_queue_depth,
            alert_max_queue_time,
        }
    }
}
