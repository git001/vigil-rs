// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Log entries from a byte stream: lines, joined by a multi-line rule.

use std::io;

use tokio::io::AsyncBufRead;
use tokio::time::Instant;

use super::{Line, LineReader, Multiline};

/// Reads entries: without a [`Multiline`] each line is one, with one the
/// lines are joined by its rule. An entry waits at most the rule's timeout
/// for a further line, so the last one of a burst is not held back until
/// the service writes again.
pub struct EntryReader<R> {
    lines: LineReader<R>,
    multiline: Option<Multiline>,
    /// A read error that came while an entry was pending; returned after it.
    error: Option<io::Error>,
    eof: bool,
}

impl<R: AsyncBufRead + Unpin> EntryReader<R> {
    pub fn new(lines: LineReader<R>, multiline: Option<Multiline>) -> Self {
        EntryReader {
            lines,
            multiline,
            error: None,
            eof: false,
        }
    }

    /// The next entry, or `None` at the end of the stream. An entry pending
    /// at the end or before a read error is returned first.
    pub async fn next_entry(&mut self) -> io::Result<Option<Line>> {
        let Some(m) = &mut self.multiline else {
            return self.lines.next_line().await;
        };
        loop {
            if let Some(e) = self.error.take() {
                return Err(e);
            }
            if self.eof {
                return Ok(m.flush());
            }
            let read = match m.deadline() {
                Some(deadline) => tokio::select! {
                    r = self.lines.next_line() => r,
                    // No further line in time: the entry is complete.
                    () = tokio::time::sleep_until(deadline) => return Ok(m.flush()),
                },
                None => self.lines.next_line().await,
            };
            match read {
                Ok(Some(line)) => {
                    if let Some(done) = m.push(line, Instant::now()) {
                        return Ok(Some(done));
                    }
                }
                Ok(None) => self.eof = true,
                Err(e) => match m.flush() {
                    Some(pending) => {
                        self.error = Some(e);
                        return Ok(Some(pending));
                    }
                    None => return Err(e),
                },
            }
        }
    }
}
