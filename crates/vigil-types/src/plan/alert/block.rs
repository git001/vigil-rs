// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The `alerts:` block: queue settings and alerts share one map.

use std::fmt;

use indexmap::IndexMap;
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use super::AlertConfig;

/// The `alerts:` block in a layer file.
///
/// Global queue settings (`max-queue-depth`, `max-queue-time`) are optional
/// top-level keys in this block and apply to all alerts. Individual alert
/// configurations are keyed by their name.
///
/// ```yaml
/// alerts:
///   max-queue-depth: 256   # optional — max pending delivery jobs
///   max-queue-time:  60s   # optional — discard stale jobs after this age
///
///   my-webhook:
///     url: https://hooks.example.com/alert
///     format: webhook
///     on-check: [website-alive]
/// ```
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AlertsBlock {
    /// Maximum number of delivery jobs waiting in the queue. New jobs are
    /// dropped (with a warning) when the queue is full. Default: 256.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_queue_depth: Option<usize>,
    /// Maximum time a delivery job may wait before being discarded (e.g. "60s",
    /// "2m"). Prevents a flood of stale alerts after an endpoint comes back
    /// online. Default: "60s".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_queue_time: Option<String>,
    /// Named alert configurations.
    #[serde(flatten)]
    pub entries: IndexMap<String, AlertConfig>,
}

const SETTINGS: &[&str] = &["max-queue-depth", "max-queue-time"];

/// Deserialized by hand: with `#[serde(flatten)]` every unknown key would be
/// taken as an alert, so a typo like `max-queue-depht: 5` failed with
/// "expected struct AlertConfig". Here a key that is not a setting must hold
/// a mapping (an alert), and the error says so.
impl<'de> Deserialize<'de> for AlertsBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(BlockVisitor)
    }
}

struct BlockVisitor;

impl<'de> Visitor<'de> for BlockVisitor {
    type Value = AlertsBlock;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a mapping of queue settings and alerts")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<AlertsBlock, A::Error> {
        let mut block = AlertsBlock::default();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "max-queue-depth" if block.max_queue_depth.is_none() => {
                    block.max_queue_depth = map.next_value()?;
                }
                "max-queue-time" if block.max_queue_time.is_none() => {
                    block.max_queue_time = map.next_value()?;
                }
                _ if block.entries.contains_key(&key) || SETTINGS.contains(&key.as_str()) => {
                    return Err(de::Error::custom(format!("duplicate key {key:?}")));
                }
                _ => {
                    let alert = map.next_value_seed(AlertSeed(&key))?;
                    block.entries.insert(key, alert);
                }
            }
        }
        Ok(block)
    }
}

/// One alert; the name goes into the message for a non-mapping value.
struct AlertSeed<'k>(&'k str);

impl<'de> DeserializeSeed<'de> for AlertSeed<'_> {
    type Value = AlertConfig;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<AlertConfig, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for AlertSeed<'_> {
    type Value = AlertConfig;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "an alert (a mapping); {:?} is not a setting either ({})",
            self.0,
            SETTINGS.join(", ")
        )
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<AlertConfig, A::Error> {
        // serde_yaml puts the key path and position in front of errors.
        AlertConfig::deserialize(de::value::MapAccessDeserializer::new(map))
    }
}
