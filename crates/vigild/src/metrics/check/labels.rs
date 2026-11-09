// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The labels and histogram buckets of the check metrics.

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::Histogram;
use vigil_types::plan::CheckLevel;

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(super) struct CheckLabel {
    pub(super) check: String,
    /// `alive` or `ready`.
    pub(super) level: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(super) struct CheckResultLabel {
    pub(super) check: String,
    /// `alive` or `ready`.
    pub(super) level: String,
    /// `success` or `error`.
    pub(super) result: String,
}

/// Buckets of the check duration histograms: 1 ms to 60 s.
const DURATION_BUCKETS: [f64; 14] = [
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
];

pub(super) type DurationFamily = Family<CheckResultLabel, Histogram, fn() -> Histogram>;

pub(super) fn duration_family() -> DurationFamily {
    Family::new_with_constructor(|| Histogram::new(DURATION_BUCKETS.into_iter()))
}

pub(super) fn result_label(check: &str, level: CheckLevel, success: bool) -> CheckResultLabel {
    CheckResultLabel {
        check: check.to_owned(),
        level: level_str(level).to_owned(),
        result: if success { "success" } else { "error" }.to_owned(),
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(super) struct LevelLabel {
    /// `alive` or `ready`.
    pub(super) level: String,
}

pub(super) fn level_str(level: CheckLevel) -> &'static str {
    match level {
        CheckLevel::Alive => "alive",
        CheckLevel::Ready => "ready",
    }
}

pub(super) fn label(check: &str, level: CheckLevel) -> CheckLabel {
    CheckLabel {
        check: check.to_owned(),
        level: level_str(level).to_owned(),
    }
}
