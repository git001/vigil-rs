// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `logs-multiline:` of a service.

use serde::{Deserialize, Serialize};

/// Which lines of a service's output belong to the entry before them, so
/// that a stack trace is one log entry instead of one per line. Exactly
/// one of `preset`, `start` and `continue` is set.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LogsMultiline {
    /// Built-in rules for a kind of output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<MultilinePreset>,
    /// Regex: a line that matches starts a new entry; every other line
    /// belongs to the entry before it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// Regex: a line that matches belongs to the entry before it; every
    /// other line starts a new entry.
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub continue_: Option<String>,
    /// How long to wait for a further line before an entry is complete.
    /// Default `1s`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
}

/// Built-in multi-line rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MultilinePreset {
    /// Java (and Kotlin, Scala) stack traces: the exception line after a
    /// log line, the indented `at …` frames, `Caused by:` and `... N more`.
    Java,
    /// Python tracebacks: `Traceback (most recent call last):`, the
    /// indented frames, the exception line, and the lines between chained
    /// exceptions.
    Python,
    /// Every indented line (space or tab) belongs to the entry before it.
    Indent,
}
