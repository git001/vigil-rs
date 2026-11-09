// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Joining the lines of a multi-line entry, such as a stack trace.

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use tokio::time::Instant;

use super::Line;
use crate::duration::parse_duration;
use crate::plan::{LogsMultiline, MultilinePreset};

/// How long an entry waits for a further line by default.
pub const DEFAULT_MULTILINE_TIMEOUT: Duration = Duration::from_millis(200);
/// Shortest allowed multi-line timeout.
pub const MIN_MULTILINE_TIMEOUT: Duration = Duration::from_millis(10);
/// Longest allowed multi-line timeout: every line of a service waits this
/// long before it shows, when no further one follows.
pub const MAX_MULTILINE_TIMEOUT: Duration = Duration::from_secs(10);

/// Parse a multi-line timeout (`200ms`, `1s`) and require it to be within
/// [`MIN_MULTILINE_TIMEOUT`] and [`MAX_MULTILINE_TIMEOUT`]. The error
/// names the value, the caller adds the field.
pub fn parse_timeout(s: &str) -> Result<Duration, String> {
    let d = parse_duration(s).map_err(|e| format!("{s:?} is not a duration: {e}"))?;
    if !(MIN_MULTILINE_TIMEOUT..=MAX_MULTILINE_TIMEOUT).contains(&d) {
        return Err(format!("{s:?} must be between 10ms and 10s"));
    }
    Ok(d)
}

/// Which lines belong to the entry before them.
#[derive(Debug, Clone)]
pub enum Rule {
    Preset(MultilinePreset),
    /// A matching line starts a new entry.
    Start(Regex),
    /// A matching line continues the entry before it.
    Continue(Regex),
}

/// A line that begins with a space or a tab.
static INDENTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[ \t]").unwrap());

/// Java: `Caused by: …`, and the line that names the exception after the
/// log line (a qualified class name ending in Exception, Error or
/// Throwable, as `printStackTrace` writes it); frames, `Suppressed:` and
/// `... N more` are indented.
static JAVA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(Caused by: |([A-Za-z_$][\w$]*\.)+[\w$]*(Exception|Error|Throwable)(: |$))")
        .unwrap()
});

/// Python: the traceback header, the exception line at its end, the
/// sentences between chained exceptions and the empty lines around them;
/// frames and code lines are indented.
static PYTHON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(Traceback \(most recent call last\):$",
        r"|([A-Za-z_]\w*\.)*[A-Za-z_]\w*(Error|Exception|Warning|Interrupt|Exit)(: |$)",
        r"|The above exception was the direct cause of the following exception:$",
        r"|During handling of the above exception, another exception occurred:$",
        r"|$)",
    ))
    .unwrap()
});

impl Rule {
    /// The rule of a `logs-multiline:` block. The error names the bad
    /// field; a block that sets not exactly one rule is an error too.
    pub fn from_config(c: &LogsMultiline) -> Result<Rule, String> {
        let regex = |field: &str, re: &str| {
            Regex::new(re).map_err(|e| format!("logs-multiline: {field}: invalid regex: {e}"))
        };
        match (&c.preset, &c.start, &c.continue_) {
            (Some(p), None, None) => Ok(Rule::Preset(*p)),
            (None, Some(re), None) => Ok(Rule::Start(regex("start", re)?)),
            (None, None, Some(re)) => Ok(Rule::Continue(regex("continue", re)?)),
            _ => Err("logs-multiline: set exactly one of preset, start and continue".into()),
        }
    }

    /// Whether `line` belongs to the entry before it.
    pub fn continues(&self, line: &str) -> bool {
        match self {
            Rule::Preset(MultilinePreset::Indent) => INDENTED.is_match(line),
            Rule::Preset(MultilinePreset::Java) => INDENTED.is_match(line) || JAVA.is_match(line),
            Rule::Preset(MultilinePreset::Python) => {
                INDENTED.is_match(line) || PYTHON.is_match(line)
            }
            Rule::Start(re) => !re.is_match(line),
            Rule::Continue(re) => re.is_match(line),
        }
    }
}

/// Joins lines into entries by a [`Rule`]. An entry is complete when a
/// line starts the next one, when it would grow past `max` bytes, or when
/// no further line came for `timeout` ([`Multiline::deadline`]).
pub struct Multiline {
    rule: Rule,
    max: usize,
    timeout: Duration,
    entry: Option<Line>,
    deadline: Option<Instant>,
}

impl Multiline {
    /// The joiner of a `logs-multiline:` block, entries of at most `max`
    /// bytes. The error names the bad field.
    pub fn from_config(c: &LogsMultiline, max: usize) -> Result<Multiline, String> {
        let rule = Rule::from_config(c)?;
        let timeout = match &c.timeout {
            Some(s) => parse_timeout(s).map_err(|e| format!("logs-multiline: timeout {e}"))?,
            None => DEFAULT_MULTILINE_TIMEOUT,
        };
        Ok(Multiline::new(rule, max, timeout))
    }

    pub fn new(rule: Rule, max: usize, timeout: Duration) -> Self {
        Multiline {
            rule,
            max,
            timeout,
            entry: None,
            deadline: None,
        }
    }

    /// Take `line`; returns the entry before it if `line` starts a new one.
    ///
    /// The later parts of a split line ([`Line::continued`]) always belong
    /// to the entry before them, without a newline in between; but an
    /// entry never grows past `max`, so such a part starts its own entry
    /// unless the limit is not reached yet. A line that belongs to the
    /// entry but no longer fits starts its next part: an entry with
    /// [`Line::continued`] set, so a reader treats it like the later part
    /// of a split line (the relay's filter decides on the first part only).
    pub fn push(&mut self, mut line: Line, now: Instant) -> Option<Line> {
        self.deadline = Some(now + self.timeout);
        let Some(entry) = &mut self.entry else {
            self.entry = Some(line);
            return None;
        };
        let sep = usize::from(!line.continued);
        let joins = line.continued || self.rule.continues(&line.text);
        if joins && entry.text.len() + sep + line.text.len() <= self.max {
            if sep == 1 {
                entry.text.push('\n');
            }
            entry.text.push_str(&line.text);
            return None;
        }
        line.continued |= joins;
        self.entry.replace(line)
    }

    /// The entry so far, complete now (timeout or end of the stream).
    pub fn flush(&mut self) -> Option<Line> {
        self.deadline = None;
        self.entry.take()
    }

    /// When the entry so far is complete if no line comes before.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline.filter(|_| self.entry.is_some())
    }

    /// No entry is pending.
    pub fn is_empty(&self) -> bool {
        self.entry.is_none()
    }
}
