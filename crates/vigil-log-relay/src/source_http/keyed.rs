//! Joining the lines of a stack trace in a stream from vigild (or another
//! HTTP source): one joiner per service and stream, so that the output of
//! different services, which the stream interleaves, is not mixed.

use std::collections::HashMap;

use indexmap::IndexMap;
use serde_json::value::RawValue;
use tokio::time::Instant;
use vigil_types::lines::Line;

use crate::join::{JoinConfig, Joiner};

/// The fields of a record, in their order and as written.
type Fields = IndexMap<String, Box<RawValue>>;

/// What a joined entry keeps of its first line.
enum Meta {
    /// A line that is no record: the entry is text.
    Text,
    /// A record with a `message` string: the entry is this record with the
    /// joined message.
    Record(Fields),
}

/// `service` and `stream` of a record; empty for lines that are no record.
type Key = (String, String);

/// Joiners by service and stream.
pub(super) struct Keyed {
    config: JoinConfig,
    joiners: HashMap<Key, Joiner<Meta>>,
}

/// A complete entry: the payload to send, and whether it is a later part
/// of a split one.
pub(super) type Out = (String, bool);

impl Keyed {
    pub(super) fn new(config: JoinConfig) -> Self {
        Keyed {
            config,
            joiners: HashMap::new(),
        }
    }

    /// Take a payload; returns an entry that is complete now. Without a
    /// multi-line rule every payload is one, unchanged.
    pub(super) fn push(&mut self, payload: String, continued: bool, now: Instant) -> Option<Out> {
        if self.config.multiline.is_none() {
            return Some((payload, continued));
        }
        let (key, meta, text) = match (!continued).then(|| record(&payload)).flatten() {
            Some((key, fields, message)) => (key, Meta::Record(fields), message),
            None => (Key::default(), Meta::Text, payload),
        };
        let line = Line { text, continued };
        let config = &self.config;
        self.joiners
            .entry(key)
            .or_insert_with(|| config.joiner())
            .push(meta, line, now)
            .map(render)
    }

    /// When the first pending entry is complete if no line comes before.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.joiners.values().filter_map(Joiner::deadline).min()
    }

    /// The pending entries whose time is up at `now` (all with `None`).
    pub(super) fn flush(&mut self, now: Option<Instant>) -> Vec<Out> {
        let mut out = Vec::new();
        self.joiners.retain(|_, j| {
            if now.is_none_or(|now| j.deadline().is_some_and(|d| d <= now)) {
                out.extend(j.flush().map(render));
                return false;
            }
            true
        });
        out
    }
}

/// A record with a `message` string: its key, its fields and the message.
fn record(payload: &str) -> Option<(Key, Fields, String)> {
    if !payload.starts_with('{') {
        return None;
    }
    let fields: Fields = serde_json::from_str(payload).ok()?;
    let message: String = serde_json::from_str(fields.get("message")?.get()).ok()?;
    let field = |name: &str| {
        fields
            .get(name)
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
            .unwrap_or_default()
    };
    Some(((field("service"), field("stream")), fields, message))
}

/// The payload of a complete entry.
fn render((meta, line): (Meta, Line)) -> Out {
    let text = match meta {
        Meta::Text => line.text,
        Meta::Record(mut fields) => {
            let message = serde_json::to_string(&line.text)
                .ok()
                .and_then(|s| RawValue::from_string(s).ok());
            match message {
                Some(m) => {
                    fields.insert("message".into(), m);
                    serde_json::to_string(&fields).unwrap_or(line.text)
                }
                None => line.text,
            }
        }
    };
    (text, line.continued)
}

#[cfg(test)]
mod tests;
