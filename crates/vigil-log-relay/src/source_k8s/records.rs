//! From the lines of a pod's log stream to the records the relay sends:
//! skipping repeated lines and filtering (joining: [`crate::join`]).

use chrono::{DateTime, FixedOffset};
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::warn;
use vigil_types::lines::Line;

use crate::LineFilter;

/// Which lines of a pod's log stream are forwarded, with which timestamp.
///
/// K8s can deliver the same line twice near stream start when tailLines +
/// follow=true are combined (known K8s quirk): a line whose timestamp is
/// not later than the last forwarded one is skipped. The timestamps are
/// compared as points in time: the kubelet writes them as RFC 3339 with
/// trailing zeros of the fraction cut (`…46.12345Z` is before
/// `…46.123456789Z`, although it sorts after it as text). A line whose
/// timestamp cannot be read is forwarded. The later parts of a split line
/// have no timestamp of their own; they get the first part's, and its
/// decision.
#[derive(Default)]
pub struct Dedup {
    last_ts: String,
    last: Option<DateTime<FixedOffset>>,
    /// The parts of the current split line are forwarded.
    forward_parts: bool,
}

impl Dedup {
    /// `(timestamp, message)` of `line` ("2026-03-18T11:23:45.123Z
    /// <message>"), or `None` to skip it.
    pub fn accept(&mut self, line: Line) -> Option<(String, Line)> {
        if line.continued {
            return self.forward_parts.then(|| (self.last_ts.clone(), line));
        }
        let (ts, msg) = line.text.split_once(' ').unwrap_or((&line.text, ""));
        let time = DateTime::parse_from_rfc3339(ts).ok();
        self.forward_parts = match (time, self.last) {
            (Some(t), Some(last)) => t > last,
            _ => true,
        };
        if !self.forward_parts {
            return None;
        }
        if time.is_some() {
            self.last = time;
        }
        self.last_ts = ts.to_owned();
        let msg = Line {
            text: msg.to_owned(),
            continued: false,
        };
        Some((self.last_ts.clone(), msg))
    }
}

/// ndjson record produced in --kubernetes mode.
#[derive(Serialize)]
struct LogEntry<'a> {
    timestamp: &'a str,
    namespace: &'a str,
    pod: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    container: Option<&'a str>,
    stream: &'static str,
    message: &'a str,
}

/// Sends the entries of one stream as ndjson records, after the filter.
pub struct Records<'a> {
    pub namespace: &'a str,
    pub pod: &'a str,
    pub container: Option<&'a str>,
    pub stream: &'static str,
    pub filter: &'a LineFilter,
    pub tx: &'a mpsc::Sender<String>,
    /// The filter's decision for the entry whose later parts follow.
    pub allow_parts: bool,
}

impl Records<'_> {
    /// Filter and send one entry. The later parts of a split entry follow
    /// the decision on its first part.
    pub fn send(&mut self, ts: &str, entry: &Line) {
        let allow = if entry.continued {
            self.allow_parts
        } else {
            self.filter.allow(&entry.text)
        };
        self.allow_parts = allow;
        if !allow {
            return;
        }
        let record = LogEntry {
            timestamp: ts,
            namespace: self.namespace,
            pod: self.pod,
            container: self.container,
            stream: self.stream,
            message: &entry.text,
        };
        if let Ok(mut json) = serde_json::to_string(&record) {
            json.push('\n');
            if self.tx.try_send(json).is_err() {
                warn!(pod = %self.pod, stream = self.stream, "send buffer full — dropping log line");
            }
        }
    }
}

#[cfg(test)]
mod tests;
