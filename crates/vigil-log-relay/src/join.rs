//! Joining the lines of a stack trace into one record, for every source
//! mode (see `--multiline`).

use std::time::Duration;

use tokio::time::Instant;
use vigil_types::lines::{Line, Multiline, Rule};

/// The multi-line rule of the relay (`--multiline*`) and the longest
/// joined message (`--max-line-bytes`).
#[derive(Clone, Debug)]
pub struct JoinConfig {
    pub max: usize,
    pub multiline: Option<(Rule, Duration)>,
}

impl Default for JoinConfig {
    /// The default line limit, no multi-line rule.
    fn default() -> Self {
        JoinConfig {
            max: vigil_types::lines::DEFAULT_MAX_LINE,
            multiline: None,
        }
    }
}

impl JoinConfig {
    /// A joiner for one stream of entries.
    pub fn joiner<M>(&self) -> Joiner<M> {
        Joiner {
            multiline: self
                .multiline
                .as_ref()
                .map(|(rule, timeout)| Multiline::new(rule.clone(), self.max, *timeout)),
            meta: None,
        }
    }
}

/// Joins the messages of one stream into entries (with a [`Multiline`])
/// and keeps `M` of each entry's first message: its timestamp in pod logs,
/// the other fields of its record in a stream from vigild.
pub struct Joiner<M> {
    multiline: Option<Multiline>,
    /// `M` of the pending entry.
    meta: Option<M>,
}

impl<M> Joiner<M> {
    /// Take a message; returns an entry that is complete now. Without a
    /// rule every message is an entry at once.
    pub fn push(&mut self, meta: M, msg: Line, now: Instant) -> Option<(M, Line)> {
        let Some(m) = &mut self.multiline else {
            return Some((meta, msg));
        };
        match m.push(msg, now) {
            // `msg` started a new entry: the pending one is complete.
            Some(done) => self.meta.replace(meta).map(|first| (first, done)),
            None => {
                self.meta.get_or_insert(meta);
                None
            }
        }
    }

    /// The pending entry, complete now (timeout or end of the stream).
    pub fn flush(&mut self) -> Option<(M, Line)> {
        let entry = self.multiline.as_mut()?.flush()?;
        Some((self.meta.take()?, entry))
    }

    /// When the pending entry is complete if no message comes before.
    pub fn deadline(&self) -> Option<Instant> {
        self.multiline.as_ref()?.deadline()
    }
}

#[cfg(test)]
mod tests {
    use vigil_types::plan::MultilinePreset;

    use super::*;

    fn line(text: &str) -> Line {
        Line {
            text: text.to_string(),
            continued: false,
        }
    }

    fn java() -> JoinConfig {
        JoinConfig {
            max: 1024,
            multiline: Some((Rule::Preset(MultilinePreset::Java), Duration::from_secs(10))),
        }
    }

    #[test]
    fn a_joined_entry_keeps_the_meta_of_its_first_message() {
        let mut j = java().joiner();
        let now = Instant::now();
        assert_eq!(j.push("T1", line("ERROR failed"), now), None);
        assert_eq!(
            j.push("T2", line("java.lang.IllegalStateException: boom"), now),
            None
        );
        assert_eq!(j.push("T3", line("\tat a.B.c(B.java:1)"), now), None);
        let (meta, entry) = j.push("T4", line("next"), now).unwrap();
        assert_eq!(meta, "T1");
        assert_eq!(
            entry.text,
            "ERROR failed\njava.lang.IllegalStateException: boom\n\tat a.B.c(B.java:1)"
        );
        assert!(j.deadline().is_some());
        let (meta, entry) = j.flush().unwrap();
        assert_eq!((meta, entry.text.as_str()), ("T4", "next"));
        assert_eq!(j.flush(), None);
        assert_eq!(j.deadline(), None);
    }

    #[test]
    fn without_a_rule_every_message_is_an_entry() {
        let mut j = JoinConfig {
            max: 1024,
            multiline: None,
        }
        .joiner();
        let (meta, entry) = j.push("T1", line("  indented"), Instant::now()).unwrap();
        assert_eq!((meta, entry.text.as_str()), ("T1", "  indented"));
        assert_eq!(j.flush(), None);
        assert_eq!(j.deadline(), None);
    }
}
